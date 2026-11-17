<#
.SYNOPSIS
Measures what unflub-cli.exe costs while it sits in the background.

.DESCRIPTION
Starts unflub-cli.exe with `--dry-run --pill off` (hook installed, engine loaded, nothing ever
injected or swallowed), waits until the hook is installed plus a warm-up, then samples it for
a while: peak private bytes, peak working set, CPU time used, peak thread and handle counts.
Prints one JSON report on stdout (logs go to stderr) and exits 1 if a limit in the given
section of perf/limits.toml is exceeded, 2 if the run itself failed.

This script sends no input of any kind. It stops only the processes it started, always.

If the hook can't be installed (some CI runners), unflub-cli's dry run keeps the engine loaded and
the report says "hook unavailable on runner": the numbers are then the engine-only footprint.

.EXAMPLE
pwsh perf/idle.ps1
pwsh perf/idle.ps1 -Launcher target/release/examples/jobrun.exe -Section lowend.idle
#>
[CmdletBinding()]
param(
    [string]$Exe,       # default: target\release\unflub-cli.exe
    [string]$Limits,    # default: perf\limits.toml
    [string]$Section = 'idle',
    [string]$Name = 'idle footprint',
    [int]$WarmupSeconds = 3,
    [int]$SampleSeconds = 20,
    [int]$ReadyTimeoutSeconds = 60,
    # Optional wrapper that runs unflub-cli.exe for us (perf/lowend.ps1 passes jobrun.exe). It must
    # print "jobrun: pid <n>" on stderr.
    [string]$Launcher,
    [string[]]$LauncherArgs = @()
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
# Defaults set here: Windows PowerShell 5.1 can leave $PSScriptRoot empty in param defaults.
if (-not $Exe) { $Exe = Join-Path $PSScriptRoot '..\target\release\unflub-cli.exe' }
if (-not $Limits) { $Limits = Join-Path $PSScriptRoot 'limits.toml' }

# Start-Process joins arguments with spaces in Windows PowerShell 5.1; quote the ones that need it.
function Join-Arguments([string[]]$Arguments) {
    ($Arguments | ForEach-Object { if ($_ -match '[\s"]') { '"' + $_.Replace('"', '\"') + '"' } else { $_ } }) -join ' '
}

function Read-Shared([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return '' }
    $stream = [IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite')
    try { (New-Object IO.StreamReader($stream)).ReadToEnd() } finally { $stream.Dispose() }
}

# Never echo dry-run "skip" lines: they contain words someone typed on this machine.
function Get-SafeLog([string]$Out, [string]$Err) {
    $lines = @((Read-Shared $Out) -split "`r?`n") + @((Read-Shared $Err) -split "`r?`n")
    ($lines | Where-Object { $_ -and $_ -notmatch '^skip ' } | Select-Object -Last 20) -join "`n"
}

$limitsTable = Read-Limits $Limits $Section
$exePath = (Resolve-Path -LiteralPath $Exe).Path
$logDir = Join-Path ([IO.Path]::GetTempPath()) ('unflub-perf-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $logDir | Out-Null
$outLog = Join-Path $logDir 'stdout.txt'
$errLog = Join-Path $logDir 'stderr.txt'

# Every process this script started, as Process objects (they hold a handle, so a recycled
# pid can never make us stop someone else's process).
$started = New-Object System.Collections.Generic.List[System.Diagnostics.Process]
$exitCode = 2
try {
    $unflubArgs = @('--dry-run', '--pill', 'off')
    $clock = [Diagnostics.Stopwatch]::StartNew()
    if ($Launcher) {
        $launcherPath = (Resolve-Path -LiteralPath $Launcher).Path
        $argLine = Join-Arguments (@($LauncherArgs) + @('--', $exePath) + $unflubArgs)
        $launched = Start-Process -FilePath $launcherPath -ArgumentList $argLine -PassThru `
            -WindowStyle Hidden -RedirectStandardOutput $outLog -RedirectStandardError $errLog
        $started.Add($launched)
        $target = $null
        while (-not $target) {
            if ((Read-Shared $errLog) -match 'jobrun: pid (\d+)') {
                $target = Get-Process -Id ([int]$Matches[1])
                $null = $target.Handle
                $started.Insert(0, $target)
                if ($target.Path -ne $exePath) { throw "launcher started $($target.Path), expected $exePath" }
            } elseif ($launched.HasExited) {
                throw "launcher exited ($($launched.ExitCode)):`n$(Get-SafeLog $outLog $errLog)"
            } elseif ($clock.Elapsed.TotalSeconds -gt $ReadyTimeoutSeconds) {
                throw "launcher didn't report a pid within $ReadyTimeoutSeconds s"
            } else {
                Start-Sleep -Milliseconds 20
            }
        }
    } else {
        $target = Start-Process -FilePath $exePath -ArgumentList (Join-Arguments $unflubArgs) -PassThru `
            -WindowStyle Hidden -RedirectStandardOutput $outLog -RedirectStandardError $errLog
        $null = $target.Handle
        $started.Add($target)
    }
    Write-Log "started unflub-cli.exe (pid $($target.Id)) with $($unflubArgs -join ' ')"

    # Ready when the hook is installed, or when the dry run says it carries on without one.
    $hook = $null
    while ($null -eq $hook) {
        $text = Read-Shared $outLog
        if ($text -match '(?m)^hooked\.') {
            $hook = 'installed'
        } elseif ($text -match '(?m)^hook unavailable') {
            $hook = 'unavailable'
        } elseif ($target.HasExited) {
            throw "unflub-cli.exe exited ($($target.ExitCode)) before it was ready:`n$(Get-SafeLog $outLog $errLog)"
        } elseif ($clock.Elapsed.TotalSeconds -gt $ReadyTimeoutSeconds) {
            throw "unflub-cli.exe wasn't ready within $ReadyTimeoutSeconds s:`n$(Get-SafeLog $outLog $errLog)"
        } else {
            Start-Sleep -Milliseconds 10
        }
    }
    $readyMs = $clock.Elapsed.TotalMilliseconds
    Write-Log ("hook {0} after {1:N0} ms; warming up {2} s, then sampling {3} s" -f $hook, $readyMs, $WarmupSeconds, $SampleSeconds)

    Start-Sleep -Seconds $WarmupSeconds
    $target.Refresh()
    $cpuStart = $target.TotalProcessorTime
    $peak = @{ private = 0L; ws = 0L; threads = 0; handles = 0 }
    $samples = 0
    $sampleClock = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $target.Refresh()
        if ($target.HasExited) { throw "unflub-cli.exe exited ($($target.ExitCode)) while idle:`n$(Get-SafeLog $outLog $errLog)" }
        $peak.private = [Math]::Max($peak.private, $target.PrivateMemorySize64)
        $peak.ws = [Math]::Max($peak.ws, $target.WorkingSet64)
        $peak.threads = [Math]::Max($peak.threads, $target.Threads.Count)
        $peak.handles = [Math]::Max($peak.handles, $target.HandleCount)
        $samples++
        if ($sampleClock.Elapsed.TotalSeconds -ge $SampleSeconds) { break }
        Start-Sleep -Milliseconds 1000
    }
    $target.Refresh()
    $cpu = ($target.TotalProcessorTime - $cpuStart).TotalSeconds
    $elapsed = $sampleClock.Elapsed.TotalSeconds

    $metrics = [ordered]@{
        ready_ms          = [Math]::Round($readyMs, 1)
        private_bytes     = $peak.private
        working_set_bytes = $peak.ws
        cpu_seconds       = [Math]::Round($cpu, 3)
        threads           = $peak.threads
        handles           = $peak.handles
    }
    $notes = @("dry run, pill off; $samples samples over $([Math]::Round($elapsed, 1)) s after a $WarmupSeconds s warm-up")
    if ($hook -eq 'unavailable') {
        $notes += 'hook unavailable on runner: engine-only footprint (no hook thread, no UI Automation worker)'
    }
    if ($Launcher) {
        $jobLine = ((Read-Shared $errLog) -split "`r?`n" | Where-Object { $_ -match '^jobrun: pid' } | Select-Object -First 1)
        if ($jobLine) { $notes += ($jobLine -replace '^jobrun: pid \d+ ', 'job: ') }
    }
    $checks = Compare-Limits $limitsTable $metrics
    $report = New-Report $Name $Section $metrics $checks $notes
    $report['hook'] = $hook
    $report | ConvertTo-Json -Depth 6 -Compress
    $exitCode = if ($report.pass) { 0 } else { 1 }
} catch {
    Write-Log "idle check failed: $_"
    $exitCode = 2
} finally {
    foreach ($p in $started) {
        try {
            if (-not $p.HasExited) {
                $p.Kill()
                $null = $p.WaitForExit(5000)
            }
        } catch {
            Write-Log "could not stop pid $($p.Id): $_"
        }
    }
    Remove-Item -LiteralPath $logDir -Recurse -Force -ErrorAction SilentlyContinue
}
exit $exitCode
