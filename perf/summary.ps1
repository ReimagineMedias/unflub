<#
.SYNOPSIS
Turns perf JSON reports into a markdown table and GitHub annotations.

.DESCRIPTION
Reads each file (one JSON report per line, as printed by the bench example, idle.ps1 and
lowend.ps1). Writes a markdown table per report with a result per metric to
$GITHUB_STEP_SUMMARY when set (and always to stdout), and emits an `::error` annotation on the
matching line of perf/limits.toml for every exceeded limit. A missing or empty file means the
step failed before it reported, which is an error too. Always exits 0; the measuring steps
carry the pass/fail.

.EXAMPLE
pwsh perf/summary.ps1 -Path .perf/bench.json, .perf/idle.json -Title 'Perf (windows-latest)'
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string[]]$Path,
    [string]$Limits,    # default: perf/limits.toml
    [string]$Title = 'Performance'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
# Default set here: Windows PowerShell 5.1 can leave $PSScriptRoot empty in param defaults.
if (-not $Limits) { $Limits = Join-Path $PSScriptRoot 'limits.toml' }

$ok = [char]::ConvertFromUtf32(0x2705)
$bad = [char]::ConvertFromUtf32(0x274C)
$micro = [char]0xB5

function Format-Value([string]$Metric, $Value) {
    if ($null -eq $Value) { return 'n/a' }
    $v = [double]$Value
    $inv = [Globalization.CultureInfo]::InvariantCulture
    switch -Regex ($Metric) {
        '_bytes$' { return ($v / 1MB).ToString('0.0', $inv) + ' MB' }
        '_ms$' { return $v.ToString('0', $inv) + ' ms' }
        '_us$' { return $v.ToString('0.0', $inv) + " ${micro}s" }
        '_seconds$' { return $v.ToString('0.000', $inv) + ' s' }
        '_pct$' { return $v.ToString('0.00', $inv) + ' %' }
        '_per_10k$' { return $v.ToString('0.00', $inv) }
        default { return $v.ToString('0', $inv) }
    }
}

# Line of `metric =` inside [section] of limits.toml, for the annotation.
function Find-LimitLine([string]$Section, [string]$Metric) {
    $current = ''
    $n = 0
    foreach ($raw in Get-Content -LiteralPath $Limits) {
        $n++
        $line = ($raw -split '#', 2)[0].Trim()
        if ($line -match '^\[(.+)\]$') { $current = $Matches[1].Trim(); continue }
        if ($current -eq $Section -and $line -match "^$([regex]::Escape($Metric))\s*=") { return $n }
    }
    1
}

# Annotations need the path relative to the repository root (the working directory in CI).
$limitsFile = ((Resolve-Path -LiteralPath $Limits -Relative) -replace '^\.[\\/]', '') -replace '\\', '/'

$md = New-Object System.Collections.Generic.List[string]
$md.Add("## $Title")
$md.Add('')
foreach ($file in $Path) {
    $lines = @()
    if (Test-Path -LiteralPath $file) {
        $lines = @(Get-Content -LiteralPath $file | Where-Object { $_ -like '{*' })
    }
    if ($lines.Count -eq 0) {
        $name = [IO.Path]::GetFileNameWithoutExtension($file)
        $md.Add("$bad **$name**: no report; the step failed before measuring (see its log).")
        $md.Add('')
        Write-Output "::error title=perf $name::no report from $file; the step failed before measuring"
        continue
    }
    foreach ($json in $lines) {
        $r = $json | ConvertFrom-Json
        $verdict = if ($r.pass) { $ok } else { $bad }
        $md.Add("### $verdict $($r.name) ($($r.os))")
        $md.Add('')
        foreach ($note in @($r.notes)) {
            if ($note -match 'hook unavailable') {
                $md.Add('> [!WARNING]')
                $md.Add("> $note. The keyboard hook could not be installed here, so these numbers leave out the hook thread.")
                $md.Add('')
            }
        }
        $md.Add('| Metric | Value | Limit | Result |')
        $md.Add('|---|---:|---:|:---:|')
        $checks = @{}
        foreach ($c in @($r.checks)) { $checks[$c.metric] = $c }
        foreach ($p in $r.metrics.PSObject.Properties) {
            $value = Format-Value $p.Name $p.Value
            if ($checks.ContainsKey($p.Name)) {
                $c = $checks[$p.Name]
                $limit = Format-Value $p.Name $c.limit
                $result = if ($null -eq $c.pass) { 'n/a' } elseif ($c.pass) { $ok } else { $bad }
                if ($c.pass -eq $false) {
                    $line = Find-LimitLine $r.section $p.Name
                    Write-Output ("::error file={6},line={0},title=perf {1}::{2} is {3}, outside the [{4}] limit of {5}" -f `
                            $line, $r.name, $p.Name, $value, $r.section, $limit, $limitsFile)
                }
            } else {
                $limit = ''
                $result = ''
            }
            $md.Add("| ``$($p.Name)`` | $value | $limit | $result |")
        }
        $other = @(@($r.notes) | Where-Object { $_ -notmatch 'hook unavailable' })
        if ($other.Count -gt 0) {
            $md.Add('')
            $md.Add('<sub>' + ($other -join '; ') + '</sub>')
        }
        $md.Add('')
    }
}

$text = $md -join "`n"
Write-Output $text
if ($env:GITHUB_STEP_SUMMARY) {
    [IO.File]::AppendAllText($env:GITHUB_STEP_SUMMARY, $text + "`n", (New-Object Text.UTF8Encoding($false)))
}
exit 0
