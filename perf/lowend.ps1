<#
.SYNOPSIS
Runs the core bench and the idle check as if on a slow machine.

.DESCRIPTION
Both run inside a Windows Job Object (target/release/examples/jobrun.exe) with:
  - a hard CPU cap of 25 % of ONE logical core (JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP; jobrun
    converts it to the 1/100 % of the whole machine that Windows expects),
  - affinity limited to one core,
  - a 128 MB commit limit per process (allocations past it fail).
Checks against the [lowend.core] and [lowend.idle] sections of perf/limits.toml. Prints one
JSON report per line on stdout and exits 1 if a limit is exceeded, 2 if a run failed.

Build first: cargo build --release -p unflub-core --example bench
             cargo build --release -p unflub-windows --bin unflub --example jobrun
#>
[CmdletBinding()]
param(
    [string]$Limits,    # default: perf\limits.toml
    [string]$Target,    # default: target\release
    [double]$CorePercent = 25,
    [int]$Cores = 1,
    [int]$MemoryMB = 128,
    [int]$SampleSeconds = 20
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
# Defaults set here: Windows PowerShell 5.1 can leave $PSScriptRoot empty in param defaults.
if (-not $Limits) { $Limits = Join-Path $PSScriptRoot 'limits.toml' }
if (-not $Target) { $Target = Join-Path $PSScriptRoot '..\target\release' }

$jobrun = (Resolve-Path -LiteralPath (Join-Path $Target 'examples\jobrun.exe')).Path
$bench = (Resolve-Path -LiteralPath (Join-Path $Target 'examples\bench.exe')).Path
$exe = (Resolve-Path -LiteralPath (Join-Path $Target 'unflub.exe')).Path
$limitsPath = (Resolve-Path -LiteralPath $Limits).Path
$jobArgs = @('--core-percent', "$CorePercent", '--cores', "$Cores", '--memory-mb', "$MemoryMB")
$worst = 0

# Core bench. jobrun waits for it, so nothing is left running when this returns.
$errFile = [IO.Path]::GetTempFileName()
try {
    # Windows PowerShell 5.1 turns a native command's stderr into errors under 'Stop'.
    $ErrorActionPreference = 'Continue'
    $out = & $jobrun @jobArgs -- $bench --json --check $limitsPath --section lowend.core 2>$errFile
    $code = $LASTEXITCODE
    $ErrorActionPreference = 'Stop'
    $jobLines = @(Get-Content -LiteralPath $errFile)
} finally {
    Remove-Item -LiteralPath $errFile -Force -ErrorAction SilentlyContinue
}
$json = @($out | Where-Object { $_ -like '{*' }) | Select-Object -Last 1
if ($code -ge 2 -or -not $json) {
    Write-Log "low-end bench failed ($code):`n$(($jobLines + @($out)) -join "`n")"
    $worst = 2
} else {
    $report = $json | ConvertFrom-Json
    $report.name = 'core bench (low-end sim)'
    $report.notes = @($jobLines | Where-Object { $_ -match '^jobrun: ' } |
        ForEach-Object { $_ -replace '^jobrun: pid \d+ ', 'job: ' -replace '^jobrun: ', '' })
    $report | ConvertTo-Json -Depth 6 -Compress
    $worst = [Math]::Max($worst, $code)
}

# Idle footprint, with unflub.exe started through jobrun. idle.ps1 stops what it started.
$idle = & (Join-Path $PSScriptRoot 'idle.ps1') -Exe $exe -Limits $limitsPath -Section 'lowend.idle' `
    -Name 'idle (low-end sim)' -Launcher $jobrun -LauncherArgs $jobArgs -SampleSeconds $SampleSeconds
$worst = [Math]::Max($worst, $LASTEXITCODE)
$idle

exit $worst
