# Patchbay installer for Windows — one command to get started.
#
# Install CLI (default): connects to patchbay.aspectlylabs.com
#   irm https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.ps1 | iex
#
# Self-host: starts a local Patchbay server + installs CLI + configures
#   $env:PATCHBAY_MODE="local"; irm https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.ps1 | iex
#

$ErrorActionPreference = "Stop"

if (-not $env:PATCHBAY_INSTALL_DIR -and $env:CORDY_INSTALL_DIR) { $env:PATCHBAY_INSTALL_DIR = $env:CORDY_INSTALL_DIR } # legacy-brand-compat
if (-not $env:PATCHBAY_MODE -and $env:CORDY_MODE) { $env:PATCHBAY_MODE = $env:CORDY_MODE } # legacy-brand-compat

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
$RepoUrl       = "https://github.com/alexj11324/Cordy.git"
$RepoWebUrl    = "https://github.com/alexj11324/Cordy"
$DefaultInstallDir = Join-Path $env:USERPROFILE ".patchbay\server"
$PatchbayHome      = Join-Path $env:USERPROFILE ".patchbay"
$LegacyPatchbayHome = Join-Path $env:USERPROFILE ".cordy" # legacy-brand-compat
$InstallDir    = if ($env:PATCHBAY_INSTALL_DIR) { $env:PATCHBAY_INSTALL_DIR } else { $DefaultInstallDir }

# Host ports Compose reported after `up -d`; set by Setup-Server and reused by
# the summary so the health check and the printed URLs cannot diverge.
$script:SelfHostBackendPort  = $null
$script:SelfHostFrontendPort = $null

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
function Write-Info  { param([string]$Msg) Write-Host "==> $Msg" -ForegroundColor Cyan }
function Write-Ok    { param([string]$Msg) Write-Host "[OK] $Msg" -ForegroundColor Green }
function Write-Warn  { param([string]$Msg) Write-Warning $Msg }
function Write-Fail  { param([string]$Msg) Write-Host "[ERROR] $Msg" -ForegroundColor Red; exit 1 }

function Test-CommandExists {
    param([string]$Name)
    $null -ne (Get-Command $Name -ErrorAction SilentlyContinue)
}

function Move-LegacyPatchbayHome {
    if (-not $env:PATCHBAY_INSTALL_DIR -and -not (Test-Path $PatchbayHome) -and (Test-Path $LegacyPatchbayHome -PathType Container)) {
        Move-Item -Path $LegacyPatchbayHome -Destination $PatchbayHome
        Write-Ok "Migrated the existing Patchbay home directory to $PatchbayHome"
    }
}

function New-RandomHex {
    param([int]$ByteCount)

    $bytes = New-Object byte[] $ByteCount
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $rng.GetBytes($bytes)
    } finally {
        $rng.Dispose()
    }
    return -join ($bytes | ForEach-Object { "{0:x2}" -f $_ })
}

# Host port Docker Compose actually published for a service.
#
# This is the only authority. Compose's interpolation gives the calling process
# environment precedence over .env, so an ambient PORT / BACKEND_PORT / API_PORT
# / SERVER_PORT / FRONTEND_PORT moves the published port without touching the
# file. Re-deriving the port from .env alone made the installer probe and print
# a port the stack was never published on (#6145). Must be called from the
# installation directory, after `up -d`.
function Get-ComposePublishedPort {
    param(
        [Parameter(Mandatory = $true)][string]$Service,
        [Parameter(Mandatory = $true)][int]$ContainerPort
    )

    $output = $null
    try {
        $output = docker compose -f docker-compose.selfhost.yml port $Service $ContainerPort 2>$null
    } catch {
        return $null
    }
    if ($LASTEXITCODE -ne 0) {
        return $null
    }

    $line = @($output | Where-Object { $_ }) | Select-Object -Last 1
    if (-not $line) {
        return $null
    }

    $published = ($line -split ":")[-1].Trim()
    if ($published -notmatch '^[0-9]+$') {
        return $null
    }
    return $published
}

function Get-LatestVersion {
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/alexj11324/Cordy/releases/latest" -ErrorAction Stop
        return $release.tag_name
    } catch {
        return $null
    }
}

function Get-SelfHostRef {
    if ($env:PATCHBAY_SELFHOST_REF) {
        return $env:PATCHBAY_SELFHOST_REF
    }

    $latest = Get-LatestVersion
    if ($latest) {
        return $latest
    }

    return "main"
}

function Checkout-ServerRef {
    param([string]$Ref)

    git fetch origin $Ref --depth 1
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "Could not fetch self-host ref '$Ref'."
    }
    git checkout --force --detach FETCH_HEAD
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "Could not check out self-host ref '$Ref'."
    }
}

function Update-LegacySelfHostImageRepositories {
    param([Parameter(Mandatory = $true)][string]$EnvPath)

    $canonicalBackend = "PATCHBAY_BACKEND_IMAGE=ghcr.io/alexj11324/patchbay-backend"
    $canonicalWeb = "PATCHBAY_WEB_IMAGE=ghcr.io/alexj11324/patchbay-web"
    $legacyImages = @{
        "PATCHBAY_BACKEND_IMAGE=ghcr.io/cordy-ai/cordy-backend" = $canonicalBackend # legacy-brand-compat
        "PATCHBAY_BACKEND_IMAGE=ghcr.io/alexj11324/cordy-backend" = $canonicalBackend # legacy-brand-compat
        "PATCHBAY_WEB_IMAGE=ghcr.io/cordy-ai/cordy-web" = $canonicalWeb # legacy-brand-compat
        "PATCHBAY_WEB_IMAGE=ghcr.io/alexj11324/cordy-web" = $canonicalWeb # legacy-brand-compat
    }
    $changed = $false
    $content = @(Get-Content $EnvPath) | ForEach-Object {
        $line = $_
        if ($line -cmatch '^CORDY_') { # legacy-brand-compat
            $changed = $true
            $line = $line -creplace '^CORDY_', 'PATCHBAY_' # legacy-brand-compat
        }
        if ($legacyImages.ContainsKey($line)) {
            $changed = $true
            $legacyImages[$line]
        } else {
            $line
        }
    }
    if ($changed) {
        $content | Set-Content $EnvPath
        Write-Ok "Migrated the existing self-host configuration to Patchbay identifiers"
    }
}

function Set-SelfHostImageTag {
    param([string]$Ref)

    $imageTag = if ($Ref -eq "main") { "latest" } else { $Ref }
    if ($imageTag -notmatch '^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$') {
        Write-Fail "Self-host ref '$Ref' is not a valid container image tag. Use a release tag such as v0.4.10 or main."
    }

    $envPath = Join-Path $InstallDir ".env"
    $content = @(Get-Content $envPath)
    if ($content -match '^PATCHBAY_IMAGE_TAG=') {
        $content = $content | ForEach-Object {
            if ($_ -match '^PATCHBAY_IMAGE_TAG=') { "PATCHBAY_IMAGE_TAG=$imageTag" } else { $_ }
        }
        $content | Set-Content $envPath
    } else {
        Add-Content -Path $envPath -Value "`nPATCHBAY_IMAGE_TAG=$imageTag"
    }

    # Compose gives the process environment precedence over .env. Pin both so
    # an ambient value cannot silently defeat an explicit rollback ref.
    $env:PATCHBAY_IMAGE_TAG = $imageTag
    Write-Ok "Pinned backend and web images to $imageTag"
}

function Pull-OfficialSelfHostImages {
    docker compose -f docker-compose.selfhost.yml pull
    if ($LASTEXITCODE -eq 0) {
        return
    }

    Write-Host ""
    Write-Warn "Official images for the selected self-host channel are not published yet."
    Write-Host "This can happen before the first GHCR release is available."
    Write-Host "From $InstallDir, build from source instead:"
    Write-Host "  docker compose -f docker-compose.selfhost.yml -f docker-compose.selfhost.build.yml up -d --build"
    exit 1
}

function Convert-ToCliArch {
    param([object]$Value)

    if ($null -eq $Value) {
        return $null
    }

    $normalized = "$Value".Trim().ToUpperInvariant()
    switch ($normalized) {
        "9"      { return "amd64" }
        "AMD64"  { return "amd64" }
        "X64"    { return "amd64" }
        "X86_64" { return "amd64" }
        "12"     { return "arm64" }
        "ARM64"  { return "arm64" }
        "AARCH64" { return "arm64" }
        default  { return $null }
    }
}

function Get-WindowsCliArch {
    $signals = @()
    $nativeArchSignalFound = $false

    # Prefer the native processor architecture over the current PowerShell
    # process architecture. This keeps Windows on ARM from being misdetected
    # when PowerShell is running through x64/x86 emulation.
    try {
        if (Get-Command Get-CimInstance -ErrorAction SilentlyContinue) {
            $processorArch = Get-CimInstance -ClassName Win32_Processor -ErrorAction Stop |
                Select-Object -First 1 -ExpandProperty Architecture
            $signals += [pscustomobject]@{ Source = "Win32_Processor.Architecture"; Value = $processorArch }
            $nativeArchSignalFound = $true
        }
    } catch {}

    try {
        if (-not $nativeArchSignalFound -and (Get-Command Get-WmiObject -ErrorAction SilentlyContinue)) {
            $processorArch = Get-WmiObject -Class Win32_Processor -ErrorAction Stop |
                Select-Object -First 1 -ExpandProperty Architecture
            $signals += [pscustomobject]@{ Source = "Win32_Processor.Architecture"; Value = $processorArch }
            $nativeArchSignalFound = $true
        }
    } catch {}

    try {
        $signals += [pscustomobject]@{
            Source = "RuntimeInformation.OSArchitecture"
            Value = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
        }
    } catch {}

    $signals += [pscustomobject]@{ Source = "PROCESSOR_ARCHITEW6432"; Value = $env:PROCESSOR_ARCHITEW6432 }
    $signals += [pscustomobject]@{ Source = "PROCESSOR_ARCHITECTURE"; Value = $env:PROCESSOR_ARCHITECTURE }

    foreach ($signal in $signals) {
        $arch = Convert-ToCliArch $signal.Value
        if ($arch) {
            return $arch
        }
    }

    $details = ($signals |
        Where-Object { $null -ne $_.Value -and "$($_.Value)".Trim() -ne "" } |
        ForEach-Object { "$($_.Source)=$($_.Value)" }) -join ", "
    if (-not $details) {
        $details = "no architecture signals available"
    }

    Write-Fail "Unsupported Windows architecture ($details). Only x64 and ARM64 are supported."
}

function Get-InstalledCliVersion {
    try {
        $firstLine = patchbay version 2>$null | Select-Object -First 1
        if ("$firstLine" -match '\b(v?\d+(?:\.\d+)+)\b') {
            $version = $Matches[1]
            if ($version -notlike 'v*') {
                $version = "v$version"
            }
            return $version
        }
    } catch {}

    return $null
}

# ---------------------------------------------------------------------------
# CLI Installation
# ---------------------------------------------------------------------------
function Install-CliBinary {
    Write-Info "Installing Patchbay CLI from GitHub Releases..."

    if (-not [Environment]::Is64BitOperatingSystem) {
        Write-Fail "Patchbay requires a 64-bit Windows installation."
    }

    $arch = Get-WindowsCliArch

    $latest = Get-LatestVersion
    if (-not $latest) {
        Write-Fail "Could not determine latest release. Check your network connection."
    }

    $version = $latest.TrimStart('v')
    $url = "https://github.com/alexj11324/Cordy/releases/download/$latest/patchbay-cli-$version-windows-$arch.zip"
    $tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "patchbay-install"

    if (Test-Path $tmpDir) { Remove-Item $tmpDir -Recurse -Force }
    New-Item -ItemType Directory -Path $tmpDir | Out-Null

    Write-Info "Downloading $url ..."
    try {
        Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmpDir "patchbay.zip") -UseBasicParsing
    } catch {
        Remove-Item $tmpDir -Recurse -Force
        Write-Fail "Failed to download CLI binary: $_"
    }

    # Verify SHA256 checksum. A missing, malformed, or unavailable manifest is
    # fatal: the release workflow publishes one for every CLI archive, and
    # installing without it would silently remove the download integrity gate.
    $checksumUrl = "https://github.com/alexj11324/Cordy/releases/download/$latest/checksums.txt"
    try {
        $checksums = Invoke-WebRequest -Uri $checksumUrl -UseBasicParsing -ErrorAction Stop
        $checksumContent = if ($checksums.Content -is [byte[]]) {
            [System.Text.Encoding]::UTF8.GetString($checksums.Content)
        } else {
            [string]$checksums.Content
        }
        $zipFile = Join-Path $tmpDir "patchbay.zip"
        $actualHash = (Get-FileHash -Path $zipFile -Algorithm SHA256).Hash.ToLower()
        $releaseAsset = "patchbay-cli-$version-windows-$arch.zip"
        $expectedHashes = @(
            $checksumContent -split "`r?`n" | ForEach-Object {
                if ($_ -match '^(?<hash>[0-9a-fA-F]{64})\s+\*?(?<name>\S+)\s*$' -and $Matches.name -eq $releaseAsset) {
                    $Matches.hash.ToLowerInvariant()
                }
            }
        )
        if ($expectedHashes.Count -ne 1) {
            Remove-Item $tmpDir -Recurse -Force
            Write-Fail "Checksum manifest has no unique valid entry for $releaseAsset."
        }
        $expectedHash = $expectedHashes[0]
        if ($actualHash -ne $expectedHash) {
            Remove-Item $tmpDir -Recurse -Force
            Write-Fail "Checksum verification failed. Expected: $expectedHash, Got: $actualHash"
        }
        Write-Ok "Checksum verified"
    } catch {
        Remove-Item $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
        Write-Fail "Could not verify the CLI checksum: $_"
    }

    Expand-Archive -Path (Join-Path $tmpDir "patchbay.zip") -DestinationPath $tmpDir -Force

    $binDir = Join-Path $env:USERPROFILE ".patchbay\bin"
    if (-not (Test-Path $binDir)) {
        New-Item -ItemType Directory -Path $binDir -Force | Out-Null
    }

    $exeSrc = Join-Path $tmpDir "patchbay.exe"
    if (-not (Test-Path $exeSrc)) {
        $exeSrc = Get-ChildItem -Path $tmpDir -Filter "patchbay.exe" -Recurse | Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not $exeSrc -or -not (Test-Path $exeSrc)) {
        Remove-Item $tmpDir -Recurse -Force
        Write-Fail "patchbay.exe not found in downloaded archive."
    }

    Copy-Item $exeSrc (Join-Path $binDir "patchbay.exe") -Force
    Remove-Item $tmpDir -Recurse -Force

    Add-ToUserPath $binDir
    Write-Ok "Patchbay CLI installed to $binDir\patchbay.exe"
}

function Add-ToUserPath {
    param([string]$Dir)
    $currentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($currentPath -and $currentPath.Split(";") -contains $Dir) {
        return
    }
    $newPath = if ($currentPath) { "$currentPath;$Dir" } else { $Dir }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    # Also update current session
    if ($env:Path -notlike "*$Dir*") {
        $env:Path = "$Dir;$env:Path"
    }
    Write-Info "Added $Dir to user PATH (restart your terminal for other sessions to pick it up)."
}

function Install-Cli {
    if (Test-CommandExists "patchbay") {
        $currentVer = Get-InstalledCliVersion
        $latestVer = Get-LatestVersion

        $currentCmp = if ($currentVer) { $currentVer -replace '^v','' } else { $null }
        $latestCmp = if ($latestVer) { $latestVer -replace '^v','' } else { $null }

        $isUpToDate = $currentCmp -and -not $latestCmp
        if (-not $isUpToDate) {
            try {
                $isUpToDate = $currentCmp -and $latestCmp -and ([System.Version]$currentCmp -ge [System.Version]$latestCmp)
            } catch {
                $isUpToDate = $currentCmp -and $latestCmp -and ($currentCmp -eq $latestCmp)
            }
        }

        if ($isUpToDate) {
            Write-Ok "Patchbay CLI is up to date ($currentVer)"
            return
        }

        Write-Info "Patchbay CLI $currentVer installed, latest is $latestVer - upgrading..."
        Install-CliBinary

        $newVer = Get-InstalledCliVersion
        Write-Ok "Patchbay CLI upgraded ($currentVer -> $newVer)"
        return
    }

    Install-CliBinary

    if (-not (Test-CommandExists "patchbay")) {
        Write-Fail "CLI installed but 'patchbay' not found on PATH. Restart your terminal and try again."
    }
}

# ---------------------------------------------------------------------------
# Docker check
# ---------------------------------------------------------------------------
function Test-Docker {
    if (-not (Test-CommandExists "docker")) {
        Write-Fail @"
Docker is not installed. Patchbay self-hosting requires Docker and Docker Compose.

Install Docker Desktop for Windows:
  https://docs.docker.com/desktop/install/windows-install/

After installing Docker, re-run this script with `$env:PATCHBAY_MODE="local"`.
"@
    }

    try {
        docker info 2>$null | Out-Null
    } catch {
        Write-Fail "Docker is installed but not running. Please start Docker Desktop and re-run this script."
    }

    Write-Ok "Docker is available"
}

# ---------------------------------------------------------------------------
# Server setup (self-host / local)
# ---------------------------------------------------------------------------
function Install-Server {
    Write-Info "Setting up Patchbay server..."
    $serverRef = Get-SelfHostRef
    Write-Info "Using self-host assets from $serverRef..."

    if (Test-Path (Join-Path $InstallDir ".git")) {
        Write-Info "Updating existing installation at $InstallDir..."
        Write-Warn "Any local changes in $InstallDir will be overwritten."
    } else {
        Write-Info "Cloning Patchbay repository..."
        if (-not (Test-CommandExists "git")) {
            Write-Fail "Git is not installed. Please install git and re-run."
        }
        if (Test-Path $InstallDir) {
            Write-Warn "Removing incomplete installation at $InstallDir..."
            Remove-Item $InstallDir -Recurse -Force
        }
        $parentDir = Split-Path $InstallDir -Parent
        if (-not (Test-Path $parentDir)) {
            New-Item -ItemType Directory -Path $parentDir -Force | Out-Null
        }
        git clone --depth 1 $RepoUrl $InstallDir
    }

    Push-Location $InstallDir
    Checkout-ServerRef $serverRef
    Write-Ok "Repository ready at $InstallDir ($serverRef)"

    if (-not (Test-Path ".env")) {
        Write-Info "Creating .env with random secrets..."
        Copy-Item ".env.example" ".env"
        $jwt = New-RandomHex 32
        $pgpass = New-RandomHex 24
        $content = Get-Content ".env"
        $content = $content -replace '^JWT_SECRET=.*', "JWT_SECRET=$jwt"
        $content = $content -replace '^POSTGRES_PASSWORD=.*', "POSTGRES_PASSWORD=$pgpass"
        $content = $content -replace '^(DATABASE_URL=postgres://[^:]+:)[^@]*(@.*)', "`${1}$pgpass`${2}"
        $content | Set-Content ".env"
        Write-Ok "Generated .env with random JWT_SECRET and POSTGRES_PASSWORD"
    } else {
        Write-Ok "Using existing .env"
    }

    Update-LegacySelfHostImageRepositories -EnvPath (Join-Path $InstallDir ".env")
    Set-SelfHostImageTag -Ref $serverRef

    Write-Info "Pulling official Patchbay images..."
    Pull-OfficialSelfHostImages
    Write-Info "Starting Patchbay services (this may take a few minutes on first run)..."
    docker compose -f docker-compose.selfhost.yml up -d

    # Read the ports Compose actually published, once, and reuse them for both
    # the health check and the summary so the two can never disagree.
    $script:SelfHostBackendPort = Get-ComposePublishedPort -Service "backend" -ContainerPort 8080
    if (-not $script:SelfHostBackendPort) {
        Write-Fail "Started the stack but could not read the backend host port from Docker Compose.`n  Check it with: cd $InstallDir; docker compose -f docker-compose.selfhost.yml ps"
    }
    $script:SelfHostFrontendPort = Get-ComposePublishedPort -Service "frontend" -ContainerPort 3000
    if (-not $script:SelfHostFrontendPort) {
        Write-Fail "Started the stack but could not read the frontend host port from Docker Compose.`n  Check it with: cd $InstallDir; docker compose -f docker-compose.selfhost.yml ps"
    }

    Write-Info "Waiting for backend to be ready..."
    $ready = $false
    for ($i = 1; $i -le 45; $i++) {
        try {
            $null = Invoke-WebRequest -Uri "http://localhost:$($script:SelfHostBackendPort)/health" -UseBasicParsing -TimeoutSec 2
            $ready = $true
            break
        } catch {
            Start-Sleep -Seconds 2
        }
    }

    if ($ready) {
        Write-Ok "Patchbay server is running"
    } else {
        Write-Warn "Server is still starting. Check logs with:"
        Write-Host "  cd $InstallDir; docker compose -f docker-compose.selfhost.yml logs"
    }

    Pop-Location
}


# ---------------------------------------------------------------------------
# Main: Default mode (cloud)
# ---------------------------------------------------------------------------
function Start-DefaultInstall {
    Write-Host ""
    Write-Host "  Patchbay - Installer" -ForegroundColor White
    Write-Host ""

    Install-Cli

    Write-Host ""
    Write-Host "  ============================================" -ForegroundColor Green
    Write-Host "  [OK] Patchbay CLI is ready!" -ForegroundColor Green
    Write-Host "  ============================================" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Next: configure your environment"
    Write-Host ""
    Write-Host "     patchbay setup               " -NoNewline; Write-Host "# Connect to Patchbay Cloud (patchbay.aspectlylabs.com)" -ForegroundColor DarkGray
    Write-Host "     patchbay setup self-host      " -NoNewline; Write-Host "# Connect to a self-hosted server" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "  Self-hosting? Install the server first:"
    Write-Host '     $env:PATCHBAY_MODE="with-server"; irm https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.ps1 | iex'
    Write-Host ""
}

# ---------------------------------------------------------------------------
# Main: Local mode (self-host)
# ---------------------------------------------------------------------------
function Start-LocalInstall {
    Write-Host ""
    Write-Host "  Patchbay - Self-Host Installer" -ForegroundColor White
    Write-Host "  Provisioning server infrastructure + installing CLI"
    Write-Host ""

    Test-Docker
    Install-Server
    Install-Cli

    Write-Host ""
    Write-Host "  ============================================" -ForegroundColor Green
    Write-Host "  [OK] Patchbay server is running and CLI is ready!" -ForegroundColor Green
    Write-Host "  ============================================" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Frontend:  http://localhost:$($script:SelfHostFrontendPort)"
    Write-Host "  Backend:   http://localhost:$($script:SelfHostBackendPort)"
    Write-Host "  Server at: $InstallDir"
    Write-Host ""
    Write-Host "  Next: configure your CLI to connect"
    Write-Host ""
    Write-Host "     patchbay setup self-host  " -NoNewline; Write-Host "# Configure + authenticate + start daemon" -ForegroundColor DarkGray
    Write-Host ""
    Write-Host "  Login: configure RESEND_API_KEY in .env for email codes,"
    Write-Host "  or read the generated code from backend logs when Resend is unset."
    Write-Host ""
    Write-Host "  To stop all services:"
    Write-Host '     $env:PATCHBAY_MODE="stop"; irm https://raw.githubusercontent.com/alexj11324/Cordy/main/scripts/install.ps1 | iex'
    Write-Host ""
}

# ---------------------------------------------------------------------------
# Stop: shut down a self-hosted installation
# ---------------------------------------------------------------------------
function Start-Stop {
    Write-Host ""
    Write-Info "Stopping Patchbay services..."

    if (Test-Path $InstallDir) {
        Push-Location $InstallDir
        if (Test-Path "docker-compose.selfhost.yml") {
            docker compose -f docker-compose.selfhost.yml down
            Write-Ok "Docker services stopped"
        } else {
            Write-Warn "No docker-compose.selfhost.yml found at $InstallDir"
        }
        Pop-Location
    } else {
        Write-Warn "No Patchbay installation found at $InstallDir"
    }

    if (Test-CommandExists "patchbay") {
        try {
            patchbay daemon stop 2>$null
            Write-Ok "Daemon stopped"
        } catch {}
    }

    Write-Host ""
}

# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
Move-LegacyPatchbayHome
$mode = if ($env:PATCHBAY_MODE) { $env:PATCHBAY_MODE.ToLower() } else { "default" }

switch ($mode) {
    "with-server" { Start-LocalInstall }
    "local"       { Start-LocalInstall }  # backwards compat alias
    "stop"        { Start-Stop }
    default       { Start-DefaultInstall }
}
