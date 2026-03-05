#Requires -Version 5.1
<#
.SYNOPSIS
    Loads the Visual Studio C++ build environment (VsDevCmd) into the current
    PowerShell session and verifies that cl.exe and link.exe are available.
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ── 1. Locate vswhere ────────────────────────────────────────────────────────
$vswhere = "${Env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) {
    Write-Error "vswhere.exe not found at: $vswhere`nIs Visual Studio installed?"
}

# ── 2. Ask vswhere for the latest VS install with VC tools ───────────────────
$installPath = & $vswhere `
    -latest `
    -products '*' `
    -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    -property installationPath `
    2>$null

if (-not $installPath) {
    Write-Error "No Visual Studio installation with VC tools found."
}
$installPath = $installPath.Trim()
Write-Host "VS install path : $installPath"

# ── 3. Build the path to VsDevCmd.bat ────────────────────────────────────────
$vsDevCmd = Join-Path $installPath 'Common7\Tools\VsDevCmd.bat'
if (-not (Test-Path $vsDevCmd)) {
    Write-Error "VsDevCmd.bat not found at: $vsDevCmd"
}
Write-Host "VsDevCmd.bat    : $vsDevCmd"

# ── 4. Run VsDevCmd and import the resulting environment variables ────────────
Write-Host "`nLoading VS build environment..."

$rawEnv = cmd /c "`"$vsDevCmd`" -no_logo -arch=amd64 && set" 2>&1

foreach ($line in $rawEnv) {
    # Only process lines that look like NAME=VALUE
    if ($line -match '^([^=]+)=(.*)$') {
        [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
    }
}

# ── 5. Success message ───────────────────────────────────────────────────────
Write-Host ""
Write-Host "VS build environment loaded successfully." -ForegroundColor Green

# ── 6. Verify cl.exe and link.exe are on the PATH ────────────────────────────
Write-Host ""
Write-Host "Verifying tools..." -ForegroundColor Cyan

foreach ($tool in @('cl', 'link')) {
    $found = Get-Command $tool -ErrorAction SilentlyContinue
    if ($found) {
        Write-Host "  [OK] $tool -> $($found.Source)" -ForegroundColor Green
    } else {
        Write-Warning "  [MISSING] $tool was not found on PATH"
    }
}
