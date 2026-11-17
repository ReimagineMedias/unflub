<#
.SYNOPSIS
Measures what unflub costs while it sits in the background.

.DESCRIPTION
Two modes, chosen by -Exe:

  unflub.exe (default): the shipping pair. Starts `unflub.exe --perf-idle <temp dir>`: the tray
  app with settings and data in a temp folder, every network feature off, no settings window,
  and its engine process `unflub-engine.exe --dry-run` (hook installed, engine loaded, nothing
  ever injected or swallowed). Measures each process and the two together, against
  [<Section>.engine], [<Section>.app] and [<Section>.total] in perf/limits.toml.

  any other exe (unflub-cli.exe): one process, `--dry-run --pill off`, against [<Section>].
  perf/lowend.ps1 uses this.

Waits until the hook is installed plus a warm-up, then samples for a while: peak private bytes,
peak working set, CPU time used, peak thread and handle counts (for the pair, the total is the
peak of the per-sample sums). Prints one JSON report per line on stdout (logs go to stderr) and
exits 1 if a limit is exceeded, 2 if the run itself failed.

This script sends no input of any kind. It stops only the processes it started (the app, and
the engine the app started), always.

If the hook can't be installed (some CI runners), the dry run keeps the engine loaded and the
report says "hook unavailable on runner": the numbers are then the engine-only footprint.

.EXAMPLE
pwsh perf/idle.ps1
pwsh perf/idle.ps1 -Exe target/release/unflub-cli.exe -Launcher target/release/examples/jobrun.exe -Section lowend.idle
#>
[CmdletBinding()]
param(
    [string]$Exe,       # default: target\release\unflub.exe
    [string]$Limits,    # default: perf\limits.toml
    [string]$Section = 'idle',
    [string]$Name = 'idle footprint',
    [int]$WarmupSeconds = 3,
    [int]$SampleSeconds = 20,
    [int]$ReadyTimeoutSeconds = 60,
    # Optional wrapper that runs the exe for us (perf/lowend.ps1 passes jobrun.exe). It must
    # print "jobrun: pid <n>" on stderr.
    [string]$Launcher,
    [string[]]$LauncherArgs = @()
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
# Defaults set here: Windows PowerShell 5.1 can leave $PSScriptRoot empty in param defaults.
if (-not $Exe) { $Exe = Join-Path $PSScriptRoot '..\target\release\unflub.exe' }
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

# One process being sampled: peaks and the CPU time at the start of sampling.
function New-Probe([string]$Label, [Diagnostics.Process]$Process) {
    @{ label = $Label; process = $Process; cpuStart = $null; private = 0L; ws = 0L; threads = 0; handles = 0 }
}

function Get-Metrics($Probe, [double]$ReadyMs) {
    [ordered]@{
        ready_ms          = [Math]::Round($ReadyMs, 1)
        private_bytes     = $Probe.private
        working_set_bytes = $Probe.ws
        cpu_seconds       = [Math]::Round($Probe.cpu, 3)
        threads           = $Probe.threads
        handles           = $Probe.handles
    }
}

$exePath = (Resolve-Path -LiteralPath $Exe).Path
$pair = [IO.Path]::GetFileName($exePath) -ieq 'unflub.exe'
$sections = if ($pair) { [ordered]@{ engine = "$Section.engine"; app = "$Section.app"; total = "$Section.total" } } else { [ordered]@{ single = $Section } }
$limitTables = @{}
foreach ($k in $sections.Keys) { $limitTables[$k] = Read-Limits $Limits $sections[$k] }

$logDir = Join-Path ([IO.Path]::GetTempPath()) ('unflub-perf-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $logDir | Out-Null
$outLog = Join-Path $logDir 'stdout.txt'
$errLog = Join-Path $logDir 'stderr.txt'
# The app's settings and data for this run only.
$dataDir = Join-Path $logDir 'data'

# Every process this script started, as Process objects (they hold a handle, so a recycled
# pid can never make us stop someone else's process).
$started = New-Object System.Collections.Generic.List[System.Diagnostics.Process]
$exitCode = 2
try {
    $exeArgs = if ($pair) { @('--perf-idle', $dataDir) } else { @('--dry-run', '--pill', 'off') }
    $exeName = [IO.Path]::GetFileName($exePath)
    $clock = [Diagnostics.Stopwatch]::StartNew()
    if ($Launcher) {
        $launcherPath = (Resolve-Path -LiteralPath $Launcher).Path
        $argLine = Join-Arguments (@($LauncherArgs) + @('--', $exePath) + $exeArgs)
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
        $target = Start-Process -FilePath $exePath -ArgumentList (Join-Arguments $exeArgs) -PassThru `
            -WindowStyle Hidden -RedirectStandardOutput $outLog -RedirectStandardError $errLog
        $null = $target.Handle
        $started.Add($target)
    }
    Write-Log "started $exeName (pid $($target.Id)) with $($exeArgs -join ' ')"

    # Ready when the hook is installed, or when the dry run says it carries on without one. In
    # the pair, the engine prints this to the stdout it inherited from the app.
    $hook = $null
    while ($null -eq $hook) {
        $text = Read-Shared $outLog
        if ($text -match '(?m)^hooked\.') {
            $hook = 'installed'
        } elseif ($text -match '(?m)^hook unavailable') {
            $hook = 'unavailable'
        } elseif ($target.HasExited) {
            throw "$exeName exited ($($target.ExitCode)) before it was ready:`n$(Get-SafeLog $outLog $errLog)"
        } elseif ($clock.Elapsed.TotalSeconds -gt $ReadyTimeoutSeconds) {
            throw "$exeName wasn't ready within $ReadyTimeoutSeconds s:`n$(Get-SafeLog $outLog $errLog)"
        } else {
            Start-Sleep -Milliseconds 10
        }
    }
    $readyMs = $clock.Elapsed.TotalMilliseconds

    $probes = New-Object System.Collections.Generic.List[object]
    if ($pair) {
        $expected = Join-Path (Split-Path -Parent $exePath) 'unflub-engine.exe'
        $children = @(Get-CimInstance Win32_Process -Filter "ParentProcessId = $($target.Id)" |
                Where-Object { $_.Name -ieq 'unflub-engine.exe' })
        if ($children.Count -ne 1) { throw "expected one unflub-engine.exe under pid $($target.Id), found $($children.Count)" }
        $engine = Get-Process -Id ([int]$children[0].ProcessId)
        $null = $engine.Handle
        # Stopped after the app: the app's Job Object ends it first, so the app never sees a
        # crash to restart.
        $started.Add($engine)
        if ($engine.Path -ne $expected) { throw "the app started $($engine.Path), expected $expected" }
        $probes.Add((New-Probe 'engine' $engine))
        $probes.Add((New-Probe 'app' $target))
        Write-Log "engine is pid $($engine.Id)"
    } else {
        $probes.Add((New-Probe 'single' $target))
    }
    Write-Log ("hook {0} after {1:N0} ms; warming up {2} s, then sampling {3} s" -f $hook, $readyMs, $WarmupSeconds, $SampleSeconds)

    Start-Sleep -Seconds $WarmupSeconds
    foreach ($p in $probes) { $p.process.Refresh(); $p.cpuStart = $p.process.TotalProcessorTime }
    $total = @{ private = 0L; ws = 0L; threads = 0; handles = 0 }
    $samples = 0
    $sampleClock = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $sum = @{ private = 0L; ws = 0L; threads = 0; handles = 0 }
        foreach ($p in $probes) {
            $proc = $p.process
            $proc.Refresh()
            if ($proc.HasExited) { throw "$($p.label) exited ($($proc.ExitCode)) while idle:`n$(Get-SafeLog $outLog $errLog)" }
            $p.private = [Math]::Max($p.private, $proc.PrivateMemorySize64)
            $p.ws = [Math]::Max($p.ws, $proc.WorkingSet64)
            $p.threads = [Math]::Max($p.threads, $proc.Threads.Count)
            $p.handles = [Math]::Max($p.handles, $proc.HandleCount)
            $sum.private += $proc.PrivateMemorySize64
            $sum.ws += $proc.WorkingSet64
            $sum.threads += $proc.Threads.Count
            $sum.handles += $proc.HandleCount
        }
        foreach ($k in @('private', 'ws', 'threads', 'handles')) { $total[$k] = [Math]::Max($total[$k], $sum[$k]) }
        $samples++
        if ($sampleClock.Elapsed.TotalSeconds -ge $SampleSeconds) { break }
        Start-Sleep -Milliseconds 1000
    }
    foreach ($p in $probes) {
        $p.process.Refresh()
        $p.cpu = ($p.process.TotalProcessorTime - $p.cpuStart).TotalSeconds
    }
    $elapsed = $sampleClock.Elapsed.TotalSeconds

    $common = @("dry run; $samples samples over $([Math]::Round($elapsed, 1)) s after a $WarmupSeconds s warm-up")
    if ($hook -eq 'unavailable') {
        $common += 'hook unavailable on runner: engine-only footprint (no hook thread, no UI Automation worker)'
    }
    if ($Launcher) {
        $jobLine = ((Read-Shared $errLog) -split "`r?`n" | Where-Object { $_ -match '^jobrun: pid' } | Select-Object -First 1)
        if ($jobLine) { $common += ($jobLine -replace '^jobrun: pid \d+ ', 'job: ') }
    }

    $reports = @()
    if ($pair) {
        $named = @{ engine = 'unflub-engine.exe (hook, engine, pill)'; app = 'unflub.exe (tray, updates, stats)' }
        foreach ($p in $probes) {
            $metrics = Get-Metrics $p $readyMs
            $checks = Compare-Limits $limitTables[$p.label] $metrics
            $notes = @($common) + @("$($named[$p.label]), settings window closed")
            $r = New-Report "${Name}: $($p.label)" $sections[$p.label] $metrics $checks $notes
            $r['hook'] = $hook
            $reports += $r
        }
        $metrics = [ordered]@{
            ready_ms          = [Math]::Round($readyMs, 1)
            private_bytes     = $total.private
            working_set_bytes = $total.ws
            cpu_seconds       = [Math]::Round((($probes | ForEach-Object { $_.cpu }) | Measure-Object -Sum).Sum, 3)
            threads           = $total.threads
            handles           = $total.handles
        }
        $checks = Compare-Limits $limitTables['total'] $metrics
        $r = New-Report "${Name}: both processes" $sections['total'] $metrics $checks (@($common) + @('peak of the per-sample sums; the working sets share system DLLs, so their sum overstates RAM'))
        $r['hook'] = $hook
        $reports += $r
    } else {
        $metrics = Get-Metrics $probes[0] $readyMs
        $checks = Compare-Limits $limitTables['single'] $metrics
        $r = New-Report $Name $Section $metrics $checks (@($common) + @('pill off'))
        $r['hook'] = $hook
        $reports += $r
    }
    foreach ($r in $reports) { $r | ConvertTo-Json -Depth 6 -Compress }
    $exitCode = if (@($reports | Where-Object { -not $_.pass }).Count -eq 0) { 0 } else { 1 }
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
