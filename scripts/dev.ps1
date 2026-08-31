#!/usr/bin/env pwsh

param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ElectronArgs
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path (Split-Path $PSCommandPath -Parent) -Parent
Set-Location $RepoRoot

foreach ($key in @(
        "NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY",
        "CLERK_PUBLISHABLE_KEY",
        "CLERK_SECRET_KEY",
        "CLERK_JWT_KEY",
        "CLERK_ISSUER",
        "CLERK_AUTHORIZED_PARTIES",
        "PATCHBAY_DEV_AUTH_READY")) {
    Remove-Item "Env:$key" -ErrorAction SilentlyContinue
}

foreach ($command in @("node", "pnpm")) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Missing prerequisite: $command. Install Node.js 22 and pnpm 10.28.2."
    }
}
$NodeMajor = (& node -p 'process.versions.node.split(".")[0]').Trim()
$PnpmVersion = (& pnpm --version).Trim()
if ($NodeMajor -ne "22") {
    throw "Patchbay development requires Node.js 22 (found $(& node --version)). Run through pnpm's pinned dev runtime or activate .nvmrc."
}
if ($PnpmVersion -ne "10.28.2") {
    throw "Patchbay development requires pnpm 10.28.2 (found $PnpmVersion). Run: corepack prepare pnpm@10.28.2 --activate"
}
if (-not $env:ENV_FILE) {
    throw "The complete Node launcher did not provide ENV_FILE. Run 'pnpm dev' instead of invoking scripts/dev.ps1 directly."
}

$env:SCCACHE_CACHE_SIZE = if ($env:SCCACHE_CACHE_SIZE) { $env:SCCACHE_CACHE_SIZE } else { "10G" }

Write-Host "==> Verifying dependencies..."
& pnpm install
if ($LASTEXITCODE -ne 0) { throw "pnpm install failed with exit code $LASTEXITCODE" }

& node apps/desktop/scripts/prepare-dev-runtime.mjs
if ($LASTEXITCODE -ne 0) { throw "Development runtime preparation failed with exit code $LASTEXITCODE" }

$DevBackend = Join-Path $RepoRoot ".patchbay-dev/bin/patchbay-server.exe"
$DevMigrate = Join-Path $RepoRoot ".patchbay-dev/bin/patchbay-migrate.exe"

$DevMode = if ($env:PATCHBAY_DEV_MODE) { $env:PATCHBAY_DEV_MODE } else { "local" }
if ($ElectronArgs -contains "--hosted") { $DevMode = "hosted" }
if ($DevMode -notin @("local", "hosted")) {
    throw "Unsupported development runtime mode: $DevMode"
}
$env:PATCHBAY_DEV_MODE = $DevMode

if ($DevMode -eq "hosted") {
    # Keep the hosted OAuth/API tuple immutable. This mode deliberately skips
    # the local database, Rust server, and Next login origin.
    $env:PATCHBAY_DEV_API_URL = "https://api.aspectlylabs.com"
    $env:PATCHBAY_DEV_WS_URL = "wss://api.aspectlylabs.com/ws"
    $env:PATCHBAY_DEV_APP_URL = "https://patchbay.aspectlylabs.com"
    $env:PATCHBAY_DEV_ACCOUNTS_URL = "https://accounts.aspectlylabs.com"
    $env:PATCHBAY_PUBLIC_URL = $env:PATCHBAY_DEV_API_URL
    $env:PATCHBAY_SERVER_URL = $env:PATCHBAY_DEV_WS_URL
    $env:PATCHBAY_APP_URL = $env:PATCHBAY_DEV_APP_URL
    $env:VITE_API_URL = $env:PATCHBAY_DEV_API_URL
    $env:VITE_WS_URL = $env:PATCHBAY_DEV_WS_URL
    $env:VITE_APP_URL = $env:PATCHBAY_DEV_APP_URL
    $env:VITE_ACCOUNTS_URL = $env:PATCHBAY_DEV_ACCOUNTS_URL
    $env:NEXT_PUBLIC_API_URL = $env:VITE_API_URL
    $env:NEXT_PUBLIC_WS_URL = $env:VITE_WS_URL
    $env:PATCHBAY_REQUIRE_SOURCE_CLI = "1"
    $env:PATCHBAY_DEV_ENV_FILE = $env:ENV_FILE
    Write-Host ""
    Write-Host "✓ Hosted Desktop development environment"
    Write-Host "  OAuth:    $($env:PATCHBAY_DEV_ACCOUNTS_URL)"
    Write-Host "  API:      $($env:PATCHBAY_DEV_API_URL)"
    Write-Host "  Renderer: local Electron/Vite hot reload"
    Write-Host ""
    & node apps/desktop/scripts/dev.mjs @ElectronArgs
    if ($LASTEXITCODE -ne 0) { throw "Electron development process exited with code $LASTEXITCODE" }
    exit 0
}

function Get-PostgresCommand {
    param([string]$Name)
    foreach ($candidate in @("$Name.exe", $Name)) {
        $resolved = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($resolved) { return $resolved.Source }
    }
    return $null
}

function Get-PositiveTimeoutSeconds {
    param([string]$Name, [int]$Default = 120)
    $value = [Environment]::GetEnvironmentVariable($Name)
    if (-not $value) { return $Default }
    $parsed = 0
    if (-not [int]::TryParse($value, [ref]$parsed) -or $parsed -le 0) {
        throw "$Name must be a positive number of seconds."
    }
    return $parsed
}

function Test-TcpPort {
    param([string]$HostName, [int]$Port)
    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $task = $client.ConnectAsync($HostName, $Port)
        return $task.Wait(500) -and $client.Connected
    } catch {
        return $false
    } finally {
        $client.Dispose()
    }
}

function Test-DockerPostgresAvailable {
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) { return $false }
    & docker compose version *> $null
    if ($LASTEXITCODE -ne 0) { return $false }
    & docker info *> $null
    return $LASTEXITCODE -eq 0
}

function Stop-TrackedProcessTree {
    param([object]$Process)
    if (-not $Process -or $Process.HasExited) { return }
    & taskkill.exe /PID $Process.Id /T /F *> $null
    $Process.WaitForExit()
}

function ConvertTo-WindowsCommandLineArgument {
    param([AllowEmptyString()][string]$Argument)
    if ($Argument.Length -gt 0 -and $Argument -notmatch '[\s"]') {
        return $Argument
    }
    $escaped = [regex]::Replace($Argument, '(\\*)"', '$1$1\"')
    $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
    return '"' + $escaped + '"'
}

$DatabaseUri = [Uri]$env:DATABASE_URL
$DatabaseHost = $DatabaseUri.Host
$DatabasePort = if ($DatabaseUri.Port -gt 0) { $DatabaseUri.Port } else { [int]$env:POSTGRES_PORT }
$DatabaseName = if ($env:POSTGRES_DB) { $env:POSTGRES_DB } else { $DatabaseUri.AbsolutePath.TrimStart("/") }
$PostgresUser = if ($env:POSTGRES_USER) { $env:POSTGRES_USER } else { "patchbay" }
$env:PGPASSWORD = if ($env:POSTGRES_PASSWORD) { $env:POSTGRES_PASSWORD } else { "" }
$RuntimeMode = if ($env:PATCHBAY_POSTGRES_RUNTIME) { $env:PATCHBAY_POSTGRES_RUNTIME } else { "auto" }
$IsLocal = $DatabaseHost -in @("localhost", "127.0.0.1", "::1")
$ComposeEndpoint = $IsLocal -and $DatabasePort -eq 5432

if ($RuntimeMode -notin @("auto", "docker", "native")) {
    throw "PATCHBAY_POSTGRES_RUNTIME must be auto, docker, or native (received '$RuntimeMode')."
}
if ($IsLocal -and $DatabaseName -notmatch '^[A-Za-z_][A-Za-z0-9_]*$') {
    throw "Unsafe local database name: $DatabaseName"
}
$DockerAvailable = $false
if ($RuntimeMode -eq "docker" -or ($RuntimeMode -eq "auto" -and $ComposeEndpoint)) {
    $DockerAvailable = Test-DockerPostgresAvailable
}
if ($RuntimeMode -eq "docker" -and -not $ComposeEndpoint) {
    throw "Docker PostgreSQL is published only at localhost:5432; the configured endpoint is $DatabaseHost`:$DatabasePort."
}
if ($RuntimeMode -eq "docker" -and -not $DockerAvailable) {
    throw "PATCHBAY_POSTGRES_RUNTIME=docker but Docker Compose or its daemon is unavailable."
}
$UseDocker = $IsLocal -and (($RuntimeMode -eq "docker") -or ($RuntimeMode -eq "auto" -and $ComposeEndpoint -and $DockerAvailable))
$DatabaseTimeoutSeconds = Get-PositiveTimeoutSeconds "PATCHBAY_DEV_DB_TIMEOUT_SECONDS"
$DatabaseDeadline = [DateTime]::UtcNow.AddSeconds($DatabaseTimeoutSeconds)

if ($UseDocker) {
    Write-Host "==> Ensuring shared PostgreSQL container is running on localhost:5432..."
    & docker compose up -d postgres
    if ($LASTEXITCODE -ne 0) { throw "docker compose up failed with exit code $LASTEXITCODE" }
    do {
        & docker compose exec -T postgres pg_isready -U $PostgresUser -d postgres *> $null
        if ([DateTime]::UtcNow -ge $DatabaseDeadline) {
            & docker compose ps postgres
            throw "PostgreSQL did not become ready within $DatabaseTimeoutSeconds seconds. Inspect: docker compose logs postgres"
        }
        if ($LASTEXITCODE -ne 0) { Start-Sleep -Seconds 1 }
    } while ($LASTEXITCODE -ne 0)
    $exists = & docker compose exec -T postgres psql -U $PostgresUser -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname = '$DatabaseName'"
    if (($exists | Select-Object -First 1) -ne "1") {
        & docker compose exec -T postgres psql -U $PostgresUser -d postgres -v ON_ERROR_STOP=1 -c "CREATE DATABASE `"$DatabaseName`"" *> $null
        if ($LASTEXITCODE -ne 0) { throw "Could not create PostgreSQL database '$DatabaseName'." }
    }
} elseif ($IsLocal) {
    $PgIsReady = Get-PostgresCommand "pg_isready"
    $Psql = Get-PostgresCommand "psql"
    $CreateDb = Get-PostgresCommand "createdb"
    if (-not $PgIsReady -or -not $Psql -or -not $CreateDb) {
        throw "Native PostgreSQL tools are required for $DatabaseHost`:$DatabasePort, or set PATCHBAY_POSTGRES_RUNTIME=docker with localhost:5432."
    }
    do {
        & $PgIsReady -h $DatabaseHost -p $DatabasePort -U $PostgresUser -d postgres *> $null
        if ([DateTime]::UtcNow -ge $DatabaseDeadline) {
            throw "PostgreSQL did not become ready at $DatabaseHost`:$DatabasePort within $DatabaseTimeoutSeconds seconds. Verify the native service and DATABASE_URL."
        }
        if ($LASTEXITCODE -ne 0) { Start-Sleep -Seconds 1 }
    } while ($LASTEXITCODE -ne 0)
    $exists = & $Psql -h $DatabaseHost -p $DatabasePort -U $PostgresUser -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname = '$DatabaseName'"
    if (($exists | Select-Object -First 1) -ne "1") {
        & $CreateDb -h $DatabaseHost -p $DatabasePort -U $PostgresUser --owner $PostgresUser $DatabaseName
        if ($LASTEXITCODE -ne 0) { throw "Could not create PostgreSQL database '$DatabaseName'." }
    }
} else {
    $PgIsReady = Get-PostgresCommand "pg_isready"
    if ($PgIsReady) {
        do {
            & $PgIsReady -d $env:DATABASE_URL *> $null
            if ([DateTime]::UtcNow -ge $DatabaseDeadline) {
                throw "PostgreSQL did not become ready at $DatabaseHost`:$DatabasePort within $DatabaseTimeoutSeconds seconds. Verify DATABASE_URL and network access."
            }
            if ($LASTEXITCODE -ne 0) { Start-Sleep -Seconds 1 }
        } while ($LASTEXITCODE -ne 0)
    }
}

Write-Host "==> Running migrations..."
Push-Location (Join-Path $RepoRoot "server-rs")
try {
    & $DevMigrate up
    if ($LASTEXITCODE -ne 0) { throw "Database migrations failed with exit code $LASTEXITCODE" }
} finally {
    Pop-Location
}

$BackendPort = if ($env:PORT) { $env:PORT } else { "8080" }
$BackendReadyUrl = "http://127.0.0.1:$BackendPort/healthz"
if (Test-TcpPort "127.0.0.1" ([int]$BackendPort)) {
    throw "Backend port $BackendPort is occupied. Stop its listener or regenerate this checkout's isolated environment with FORCE=1 make worktree-env."
}

$FrontendPort = if ($env:FRONTEND_PORT) { $env:FRONTEND_PORT } else { "3000" }
$FrontendOrigin = "http://localhost:$FrontendPort"
$env:FRONTEND_ORIGIN = $FrontendOrigin
$env:PATCHBAY_APP_URL = $FrontendOrigin

$BackendProcess = $null
$WebProcess = $null
$LogDir = Join-Path $RepoRoot ".patchbay-dev/logs"
New-Item -ItemType Directory -Force -Path $LogDir *> $null
$BackendStdoutLog = Join-Path $LogDir "backend.stdout.log"
$BackendStderrLog = Join-Path $LogDir "backend.stderr.log"
$FrontendStdoutLog = Join-Path $LogDir "frontend.stdout.log"
$FrontendStderrLog = Join-Path $LogDir "frontend.stderr.log"
try {
    $BackendArguments = @(
        (ConvertTo-WindowsCommandLineArgument "../scripts/dev-auth-command.mjs"),
        (ConvertTo-WindowsCommandLineArgument "backend"),
        (ConvertTo-WindowsCommandLineArgument $DevBackend)
    )
    $BackendProcess = Start-Process -FilePath "node" -ArgumentList $BackendArguments -WorkingDirectory (Join-Path $RepoRoot "server-rs") -PassThru -RedirectStandardOutput $BackendStdoutLog -RedirectStandardError $BackendStderrLog
    $BackendTimeoutSeconds = Get-PositiveTimeoutSeconds "PATCHBAY_DEV_BACKEND_TIMEOUT_SECONDS"
    $Deadline = [DateTime]::UtcNow.AddSeconds($BackendTimeoutSeconds)
    while ($true) {
        $BackendProcess.Refresh()
        if ($BackendProcess.HasExited) {
            Get-Content -Tail 80 -ErrorAction SilentlyContinue $BackendStdoutLog, $BackendStderrLog | Write-Error
            throw "Backend exited before its database readiness check passed (exit $($BackendProcess.ExitCode))."
        }
        $Ready = $false
        try {
            $health = Invoke-RestMethod -Uri $BackendReadyUrl -TimeoutSec 2
            if ($health.status -eq "ready") {
                $BackendProcess.Refresh()
                if ($BackendProcess.HasExited) { throw "Spawned backend exited during readiness verification." }
                $Ready = $true
            }
        } catch { }
        if ($Ready) { break }
        if ([DateTime]::UtcNow -ge $Deadline) {
            Get-Content -Tail 80 -ErrorAction SilentlyContinue $BackendStdoutLog, $BackendStderrLog | Write-Error
            throw "Backend did not become ready within $BackendTimeoutSeconds seconds: $BackendReadyUrl"
        }
        Start-Sleep -Seconds 1
    }

    $env:PATCHBAY_DEV_API_URL = "http://127.0.0.1:$BackendPort"
    $env:PATCHBAY_DEV_WS_URL = "ws://127.0.0.1:$BackendPort/ws"
    $env:PATCHBAY_DEV_APP_URL = $FrontendOrigin
    $env:PATCHBAY_DEV_ACCOUNTS_URL = $FrontendOrigin
    $env:VITE_API_URL = $env:PATCHBAY_DEV_API_URL
    $env:VITE_WS_URL = $env:PATCHBAY_DEV_WS_URL
    $env:VITE_APP_URL = $env:PATCHBAY_DEV_APP_URL
    $env:VITE_ACCOUNTS_URL = $env:PATCHBAY_DEV_ACCOUNTS_URL
    $env:NEXT_PUBLIC_API_URL = $env:VITE_API_URL
    $env:NEXT_PUBLIC_WS_URL = $env:VITE_WS_URL

    $FrontendReadyUrl = "$FrontendOrigin/"
    if (Test-TcpPort "127.0.0.1" ([int]$FrontendPort)) {
        throw "Frontend port $FrontendPort is occupied. Stop its listener or regenerate this checkout's isolated environment with FORCE=1 make worktree-env."
    }

    Write-Host "==> Starting the browser/share/login origin at $FrontendOrigin..."
    $WebProcess = Start-Process `
        -FilePath "node" `
        -ArgumentList @("../../scripts/dev-auth-command.mjs", "web", "node", "node_modules/next/dist/bin/next", "dev", "--webpack", "--port", $FrontendPort) `
        -WorkingDirectory (Join-Path $RepoRoot "apps/web") `
        -RedirectStandardOutput $FrontendStdoutLog `
        -RedirectStandardError $FrontendStderrLog `
        -PassThru
    $FrontendTimeoutSeconds = Get-PositiveTimeoutSeconds "PATCHBAY_DEV_FRONTEND_TIMEOUT_SECONDS"
    $FrontendDeadline = [DateTime]::UtcNow.AddSeconds($FrontendTimeoutSeconds)
    while ($true) {
        $WebProcess.Refresh()
        if ($WebProcess.HasExited) {
            Get-Content -Tail 80 -ErrorAction SilentlyContinue $FrontendStdoutLog, $FrontendStderrLog | Write-Error
            throw "Frontend exited before its browser-link health check passed (exit $($WebProcess.ExitCode))."
        }
        $FrontendReady = $false
        try {
            $frontendHealth = Invoke-WebRequest -Uri $FrontendReadyUrl -TimeoutSec 2 -UseBasicParsing
            if ($frontendHealth.StatusCode -ge 200 -and $frontendHealth.StatusCode -lt 400) { $FrontendReady = $true }
        } catch { }
        if ($FrontendReady) { break }
        if ([DateTime]::UtcNow -ge $FrontendDeadline) {
            Get-Content -Tail 80 -ErrorAction SilentlyContinue $FrontendStdoutLog, $FrontendStderrLog | Write-Error
            throw "Frontend did not become reachable within $FrontendTimeoutSeconds seconds: $FrontendReadyUrl"
        }
        Start-Sleep -Seconds 1
    }

    $env:PATCHBAY_REQUIRE_SOURCE_CLI = "1"
    $env:PATCHBAY_DEV_ENV_FILE = $env:ENV_FILE
    & node apps/desktop/scripts/dev.mjs @ElectronArgs
    if ($LASTEXITCODE -ne 0) { throw "Electron development process exited with code $LASTEXITCODE" }
} finally {
    Stop-TrackedProcessTree $WebProcess
    Stop-TrackedProcessTree $BackendProcess
}
