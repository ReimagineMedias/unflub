# Shared helpers for the perf scripts. Dot-source it: . "$PSScriptRoot/common.ps1"
# Works in Windows PowerShell 5.1 and PowerShell 7.

Set-StrictMode -Version 3

# Reads one [section] of perf/limits.toml: `key = number` lines, `#` comments. The same subset
# the bench example parses. Returns an ordered dictionary of key -> double.
function Read-Limits([string]$Path, [string]$Section) {
    $limits = [ordered]@{}
    $current = ''
    $found = $false
    $n = 0
    foreach ($raw in Get-Content -LiteralPath $Path) {
        $n++
        $line = ($raw -split '#', 2)[0].Trim()
        if ($line -eq '') { continue }
        if ($line -match '^\[(.+)\]$') {
            $current = $Matches[1].Trim()
            if ($current -eq $Section) { $found = $true }
            continue
        }
        if ($line -notmatch '^([^=]+)=(.+)$') { throw "${Path}:${n}: expected 'key = number'" }
        $key = $Matches[1].Trim()
        $value = 0.0
        $text = $Matches[2].Trim().Replace('_', '')
        if (-not [double]::TryParse($text, [Globalization.NumberStyles]::Float,
                [Globalization.CultureInfo]::InvariantCulture, [ref]$value)) {
            throw "${Path}:${n}: '$text' is not a number"
        }
        if ($current -eq $Section) { $limits[$key] = $value }
    }
    if (-not $found) { throw "${Path}: no [$Section] section" }
    $limits
}

# One check per limit; a limit naming no metric is an error so a typo can't disable a check.
# A metric whose value is $null (not measurable here) gets pass = $null (skipped).
function Compare-Limits($Limits, $Metrics) {
    foreach ($key in $Limits.Keys) {
        if (-not $Metrics.Contains($key)) { throw "unknown metric '$key' in limits" }
        $value = $Metrics[$key]
        $pass = if ($null -eq $value) { $null } else { [bool]($value -le $Limits[$key]) }
        [ordered]@{ metric = $key; value = $value; limit = $Limits[$key]; pass = $pass }
    }
}

# The report shape every perf producer prints (the bench example prints the same).
function New-Report([string]$Name, [string]$Section, $Metrics, $Checks, [string[]]$Notes) {
    $checks = @($Checks)
    $failed = @($checks | Where-Object { $_.pass -eq $false })
    [ordered]@{
        name    = $Name
        os      = 'windows'
        section = $Section
        metrics = $Metrics
        checks  = $checks
        notes   = @($Notes | Where-Object { $_ })
        pass    = ($failed.Count -eq 0)
    }
}

function Write-Log([string]$Message) {
    [Console]::Error.WriteLine($Message)
}
