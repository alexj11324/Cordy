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
        "PATCHBAY_POSTGRES_RUNTIME",
        "PGPASSWORD",
        "patchbay-migrate.exe",
        "patchbay-server.exe",
        "Backend port",
        "apps/web",
        "VITE_APP_URL",
        "VITE_ACCOUNTS_URL",
        "PATCHBAY_REQUIRE_SOURCE_CLI",
        "apps/desktop/scripts/dev.mjs @ElectronArgs")) {
    if ($source -notmatch [regex]::Escape($required)) {
        throw "scripts/dev.ps1 is missing the complete development contract: $required"
    }
}

$missingKeysFunction = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq "Get-MissingEnvironmentKeys"
    }, $true)
if (-not $missingKeysFunction) {
    throw "scripts/dev.ps1 is missing Get-MissingEnvironmentKeys"
}
Invoke-Expression $missingKeysFunction.Extent.Text

$clerkKeys = @(
    "NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY",
    "CLERK_SECRET_KEY",
    "CLERK_JWT_KEY",
    "CLERK_ISSUER",
    "CLERK_AUTHORIZED_PARTIES"
)
$originalClerkValues = @{}
try {
    foreach ($key in $clerkKeys) {
        $originalClerkValues[$key] = [Environment]::GetEnvironmentVariable($key)
        [Environment]::SetEnvironmentVariable($key, "configured-for-test")
    }
    $missing = @(Get-MissingEnvironmentKeys -Names $clerkKeys)
    if ($missing.Count -ne 0) {
        throw "configured Clerk variables must produce an empty missing-key array"
    }
    [Environment]::SetEnvironmentVariable("CLERK_JWT_KEY", $null)
    $missing = @(Get-MissingEnvironmentKeys -Names $clerkKeys)
    if ($missing.Count -ne 1 -or $missing[0] -ne "CLERK_JWT_KEY") {
        throw "missing Clerk variables must be reported exactly"
    }
} finally {
    foreach ($key in $clerkKeys) {
        [Environment]::SetEnvironmentVariable($key, $originalClerkValues[$key])
    }
}

Write-Host "Windows complete development launcher contract passed"
