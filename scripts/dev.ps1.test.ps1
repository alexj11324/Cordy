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
        "Stop-TrackedProcessTree `$WebProcess",
        "Stop-TrackedProcessTree `$BackendProcess",
        "apps/desktop/scripts/dev.mjs @ElectronArgs")) {
    if ($source -notmatch [regex]::Escape($required)) {
        throw "scripts/dev.ps1 is missing the complete development contract: $required"
    }
}

$stopFunctionAst = $ast.Find({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
        $node.Name -eq "Stop-TrackedProcessTree"
    }, $true)
if (-not $stopFunctionAst) {
    throw "scripts/dev.ps1 must define Stop-TrackedProcessTree"
}

Invoke-Expression $stopFunctionAst.Extent.Text
$global:PatchbayTaskkillArgs = $null
function global:taskkill.exe {
    $global:PatchbayTaskkillArgs = @($args)
}
$fakeProcess = [pscustomobject]@{
    HasExited = $false
    Id = 4242
    Waited = $false
}
$fakeProcess | Add-Member -MemberType ScriptMethod -Name WaitForExit -Value {
    $this.Waited = $true
}
try {
    Stop-TrackedProcessTree $fakeProcess
    $taskkillCommand = $global:PatchbayTaskkillArgs -join " "
    if ($taskkillCommand -ne "/PID 4242 /T /F") {
        throw "Stop-TrackedProcessTree did not terminate the full process tree: $taskkillCommand"
    }
    if (-not $fakeProcess.Waited) {
        throw "Stop-TrackedProcessTree did not wait for process-tree termination"
    }
} finally {
    Remove-Item Function:\global:taskkill.exe -ErrorAction SilentlyContinue
    Remove-Variable PatchbayTaskkillArgs -Scope Global -ErrorAction SilentlyContinue
}

Write-Host "Windows complete development launcher contract passed"
