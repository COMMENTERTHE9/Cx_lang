# perf-rider: wall-time benchmark runner.
#
# Usage (from repo root or bench/):
#   pwsh bench/run_bench.ps1                 # table to stdout
#   pwsh bench/run_bench.ps1 -Markdown      # markdown rows (for BASELINE.md)
#   pwsh bench/run_bench.ps1 -Runs 11       # more samples
#
# Methodology (locked by the perf-rider dispatch):
# - Release binary only (debug timings are noise about the wrong thing).
# - PowerShell Measure-Command per run; bash on this platform mangles exit
#   codes (D1.0 lesson) and its timing is no more trustworthy.
# - 1 warmup run per program x backend, DISCARDED; then -Runs timed runs
#   (default 7); the MEDIAN is reported (resists Windows scheduling noise
#   better than the mean). Min..max spread is reported alongside — that
#   observed spread is the regression threshold, not an invented number.
# - The warmup run doubles as the correctness screen: both backends' stdout
#   must agree (when the JIT lowers the program), and any exit code other
#   than 0 or 127 (JIT_SKIP_EXIT_CODE) is a crash -> hard stop.
# - JIT support is detected per program (exit 127 => interpreter-only).
#   Interpreter-only programs are still regression-tracked on one backend
#   and flip to two-backend automatically when their surface starts lowering.

param(
    [int]$Runs = 7,
    [string]$Binary = "",
    [switch]$Markdown
)

$ErrorActionPreference = "Stop"
$benchDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $benchDir
if ($Binary -eq "") { $Binary = Join-Path $repoRoot "target\release\Cx_0V.exe" }
if (-not (Test-Path $Binary)) {
    Write-Error "release binary not found at $Binary - run: cargo build --release --features jit"
}

function Get-Median([double[]]$xs) {
    $s = $xs | Sort-Object
    $n = $s.Count
    if ($n % 2 -eq 1) { return $s[[int][Math]::Floor($n / 2)] }
    return ($s[$n / 2 - 1] + $s[$n / 2]) / 2.0
}

function Time-Runs([string]$bin, [string[]]$argv, [int]$count) {
    $times = @()
    for ($r = 0; $r -lt $count; $r++) {
        $t = Measure-Command { & $bin @argv 2>$null | Out-Null }
        $times += $t.TotalMilliseconds
    }
    return $times
}

$programs = Get-ChildItem (Join-Path $benchDir "bench_*.cx") | Sort-Object Name
$rows = @()

foreach ($p in $programs) {
    $name = $p.BaseName

    # Warmup + correctness screen, interpreter.
    $iout = (& $Binary $p.FullName 2>&1 | Out-String).Trim()
    $irc = $LASTEXITCODE
    if ($irc -ne 0) {
        Write-Error "CRASH SCREEN: $name interpreter exited $irc - benchmarks must not crash. STOP."
    }

    # Warmup + correctness screen, JIT (exit 127 = clean SKIP sentinel).
    $jout = (& $Binary --backend=cranelift $p.FullName 2>$null | Out-String).Trim()
    $jrc = $LASTEXITCODE
    $jitSupported = $false
    if ($jrc -eq 0) {
        if ($jout -ne $iout) {
            Write-Error "PARITY SCREEN: $name backends disagree (interp='$iout' jit='$jout'). STOP."
        }
        $jitSupported = $true
    } elseif ($jrc -ne 127) {
        Write-Error "CRASH SCREEN: $name JIT exited $jrc (not 0, not SKIP-127). STOP."
    }

    $it = Time-Runs $Binary @($p.FullName) $Runs
    $imed = Get-Median $it
    $ispread = "{0:N0}..{1:N0}" -f ($it | Measure-Object -Minimum).Minimum, ($it | Measure-Object -Maximum).Maximum

    if ($jitSupported) {
        $jt = Time-Runs $Binary @("--backend=cranelift", $p.FullName) $Runs
        $jmed = "{0:N0}" -f (Get-Median $jt)
        $jspread = "{0:N0}..{1:N0}" -f ($jt | Measure-Object -Minimum).Minimum, ($jt | Measure-Object -Maximum).Maximum
    } else {
        $jmed = "SKIP"
        $jspread = "-"
    }

    $rows += [pscustomobject]@{
        Program       = $name
        InterpMedianMs = [math]::Round($imed)
        InterpSpread  = $ispread
        JitMedianMs   = $jmed
        JitSpread     = $jspread
    }
}

if ($Markdown) {
    "| program | interp median (ms) | interp spread | jit median (ms) | jit spread |"
    "|---|---|---|---|---|"
    foreach ($r in $rows) {
        "| {0} | {1} | {2} | {3} | {4} |" -f $r.Program, $r.InterpMedianMs, $r.InterpSpread, $r.JitMedianMs, $r.JitSpread
    }
} else {
    $rows | Format-Table -AutoSize
}
