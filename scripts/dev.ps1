#!/usr/bin/env pwsh

param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$ElectronArgs
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path (Split-Path $PSCommandPath -Parent) -Parent
Set-Location $RepoRoot

foreach ($command in @("node", "pnpm")) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Missing prerequisite: $command. Install Node.js 22 and pnpm 10.28.2."
    }
}
if (-not $env:ENV_FILE) {
    throw "The complete Node launcher did not provide ENV_FILE. Run 'pnpm dev' instead of invoking scripts/dev.ps1 directly."
}

$env:SCCACHE_CACHE_SIZE = if ($env:SCCACHE_CACHE_SIZE) { $env:SCCACHE_CACHE_SIZE } else { "10G" }

if (-not (Test-Path (Join-Path $RepoRoot "node_modules"))) {
    Write-Host "==> Installing dependencies..."
    & pnpm install
    if ($LASTEXITCODE -ne 0) { throw "pnpm install failed with exit code $LASTEXITCODE" }
}

& node apps/desktop/scripts/prepare-dev-runtime.mjs
if ($LASTEXITCODE -ne 0) { throw "Development runtime preparation failed with exit code $LASTEXITCODE" }

$DevBackend = Join-Path $RepoRoot ".patchbay-dev/bin/patchbay-server.exe"
$DevMigrate = Join-Path $RepoRoot ".patchbay-dev/bin/patchbay-migrate.exe"

function Get-PostgresCommand {
    param([string]$Name)
    foreach ($candidate in @("$Name.exe", $Name)) {
        $resolved = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($resolved) { return $resolved.Source }
    }
    return $null
}

function Get-MissingEnvironmentKeys {
    param([string[]]$Names)
    @($Names | Where-Object { -not [Environment]::GetEnvironmentVariable($_) })
}

function Test-DockerPostgresAvailable {
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) { return $false }
    & docker compose version *> $null
    if ($LASTEXITCODE -ne 0) { return $false }
    & docker info *> $null
    return $LASTEXITCODE -eq 0
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

if ($UseDocker) {
    Write-Host "==> Ensuring shared PostgreSQL container is running on localhost:5432..."
    & docker compose up -d postgres
    if ($LASTEXITCODE -ne 0) { throw "docker compose up failed with exit code $LASTEXITCODE" }
    do {
        & docker compose exec -T postgres pg_isready -U $PostgresUser -d postgres *> $null
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
$PortOccupied = $false
try {
    $existing = Invoke-RestMethod -Uri $BackendReadyUrl -TimeoutSec 1
    if ($existing.status) { $PortOccupied = $true }
} catch { }
if ($PortOccupied) {
    throw "Backend port $BackendPort is already serving another Patchbay instance. Stop it or use this checkout's isolated PORT."
}

$FrontendPort = if ($env:FRONTEND_PORT) { $env:FRONTEND_PORT } else { "3000" }
$FrontendOrigin = "http://localhost:$FrontendPort"
$env:FRONTEND_ORIGIN = $FrontendOrigin
$env:PATCHBAY_APP_URL = $FrontendOrigin

$BackendProcess = $null
$WebProcess = $null
try {
    $BackendProcess = Start-Process -FilePath $DevBackend -WorkingDirectory (Join-Path $RepoRoot "server-rs") -NoNewWindow -PassThru
    $Deadline = [DateTime]::UtcNow.AddMinutes(30)
    while ($true) {
        $BackendProcess.Refresh()
        if ($BackendProcess.HasExited) {
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
            throw "Backend did not become ready within 30 minutes: $BackendReadyUrl"
        }
        Start-Sleep -Seconds 1
    }

    $env:VITE_API_URL = "http://127.0.0.1:$BackendPort"
    $env:VITE_WS_URL = "ws://127.0.0.1:$BackendPort/ws"
    $env:VITE_APP_URL = $FrontendOrigin
    $env:VITE_ACCOUNTS_URL = $FrontendOrigin
    $env:NEXT_PUBLIC_API_URL = $env:VITE_API_URL
    $env:NEXT_PUBLIC_WS_URL = $env:VITE_WS_URL

    $FrontendReadyUrl = "$FrontendOrigin/"
    $FrontendOccupied = $false
    try {
        $existingFrontend = Invoke-WebRequest -Uri $FrontendReadyUrl -TimeoutSec 1 -UseBasicParsing
        if ($existingFrontend.StatusCode -ge 200 -and $existingFrontend.StatusCode -lt 400) { $FrontendOccupied = $true }
    } catch { }
    if ($FrontendOccupied) {
        throw "Frontend port $FrontendPort is already serving another Patchbay instance. Stop it or use this checkout's isolated FRONTEND_PORT."
    }

    Write-Host "==> Starting the browser/share/login origin at $FrontendOrigin..."
    $WebProcess = Start-Process `
        -FilePath "node" `
        -ArgumentList @("node_modules/next/dist/bin/next", "dev", "--webpack", "--port", $FrontendPort) `
        -WorkingDirectory (Join-Path $RepoRoot "apps/web") `
        -NoNewWindow `
        -PassThru
    $FrontendDeadline = [DateTime]::UtcNow.AddMinutes(2)
    while ($true) {
        $WebProcess.Refresh()
        if ($WebProcess.HasExited) {
            throw "Frontend exited before its browser-link health check passed (exit $($WebProcess.ExitCode))."
        }
        $FrontendReady = $false
        try {
            $frontendHealth = Invoke-WebRequest -Uri $FrontendReadyUrl -TimeoutSec 2 -UseBasicParsing
            if ($frontendHealth.StatusCode -ge 200 -and $frontendHealth.StatusCode -lt 400) { $FrontendReady = $true }
        } catch { }
        if ($FrontendReady) { break }
        if ([DateTime]::UtcNow -ge $FrontendDeadline) {
            throw "Frontend did not become reachable within 2 minutes: $FrontendReadyUrl"
        }
        Start-Sleep -Seconds 1
    }

    $MissingGoogleKeys = @(Get-MissingEnvironmentKeys -Names @(
            "NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY",
            "CLERK_SECRET_KEY",
            "CLERK_JWT_KEY",
            "CLERK_ISSUER",
            "CLERK_AUTHORIZED_PARTIES"
        ))
    if ($MissingGoogleKeys.Count -gt 0) {
        Write-Host "! Google sign-in is unavailable; add these values to $($env:ENV_FILE): $($MissingGoogleKeys -join ', ')"
    }

    $env:PATCHBAY_REQUIRE_SOURCE_CLI = "1"
    $env:PATCHBAY_DEV_ENV_FILE = $env:ENV_FILE
    & node apps/desktop/scripts/dev.mjs @ElectronArgs
    if ($LASTEXITCODE -ne 0) { throw "Electron development process exited with code $LASTEXITCODE" }
} finally {
    if ($WebProcess -and -not $WebProcess.HasExited) {
        & taskkill.exe /PID $WebProcess.Id /T /F *> $null
        $WebProcess.WaitForExit()
    }
    if ($BackendProcess -and -not $BackendProcess.HasExited) {
        Stop-Process -Id $BackendProcess.Id -ErrorAction SilentlyContinue
        $BackendProcess.WaitForExit()
    }
}
