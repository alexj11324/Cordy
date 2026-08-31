#!/usr/bin/env pwsh

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path (Split-Path $PSCommandPath -Parent) -Parent
$DevScript = Join-Path $RepoRoot "scripts/dev.ps1"
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($DevScript, [ref]$null, [ref]$parseErrors)
if ($parseErrors) {
    $parseErrors | ForEach-Object { Write-Host "  $($_.Message) (line $($_.Extent.StartLineNumber))" }
    throw "scripts/dev.ps1 has parse errors"
}

$source = Get-Content -Raw -Path $DevScript
foreach ($required in @(
        "prepare-dev-runtime.mjs",
        "dev-auth-command.mjs",
        "PATCHBAY_POSTGRES_RUNTIME",
        "PGPASSWORD",
        "patchbay-migrate.exe",
        "patchbay-server.exe",
        "Backend port",
        "apps/web",
        "VITE_APP_URL",
        "VITE_ACCOUNTS_URL",
        "PATCHBAY_DEV_MODE",
        "https://accounts.aspectlylabs.com",
        'if ($DevMode -eq "hosted")',
        "PATCHBAY_REQUIRE_SOURCE_CLI",
        "apps/desktop/scripts/dev.mjs @ElectronArgs")) {
    if ($source -notmatch [regex]::Escape($required)) {
        throw "scripts/dev.ps1 is missing the complete development contract: $required"
    }
}

Write-Host "Windows complete development launcher contract passed"
