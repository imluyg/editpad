# Checks that the local working ledger (HANDOFF.md) still agrees with git and
# with its own declared numbers. It exists because the ledger has repeatedly gone
# quietly stale: HEAD rows left a dozen commits behind, hand-added test totals
# that did not sum up, "not pushed" notes written after the push, and one
# "watch this flaky test" row whose root cause had already been fixed.
#
# Usage (repo root, Windows PowerShell 5.1+):
#   powershell -ExecutionPolicy Bypass -File .\tools\ledger-check.ps1
#   powershell -File .\tools\ledger-check.ps1 -Ledger ..\somewhere\HANDOFF.md -MaxAgeDays 30
#
# HANDOFF.md is gitignored on purpose (it is a local working ledger, not a
# shipped document). When it is absent this script reports SKIP and exits 0, so
# a fresh clone and CI are unaffected. Exit code 1 = at least one FAIL.
#
# NOTE: keep this file ASCII-only. The harness shell reads .ps1 files without a
# BOM as ANSI, so non-ASCII literals here would arrive mangled (same reason
# gen-bench-log.ps1 stays ASCII). The ledger itself is UTF-8 and is read with an
# explicit encoding below -- Windows PowerShell 5.1 would otherwise decode
# BOM-less UTF-8 as ANSI and every match below would silently miss.

[CmdletBinding()]
param(
    # Path to the working ledger, relative to the repo root.
    [string] $Ledger = 'HANDOFF.md',
    # Machine fields live in an HTML comment so this script can stay ASCII:
    #   <!-- LEDGER-SNAPSHOT date=2026-09-25 head=eef98a9 tests=777 pushed=yes -->
    [string] $Marker = 'LEDGER-SNAPSHOT',
    # The stamp is stale after this many days.
    [int] $MaxAgeDays = 7,
    # Working-ledder line budget (the ledger's own rule says "index + last few
    # rounds"; this is the tripwire that keeps it from growing back).
    [int] $MaxLines = 900,
    [int] $MaxRoundSummaries = 3
)

$script:fail = 0
function Fail([string] $m) { $script:fail++; Write-Host ('FAIL  ' + $m) }
function Warn([string] $m) { Write-Host ('WARN  ' + $m) }
function Pass([string] $m) { Write-Host ('ok    ' + $m) }

if (-not (Test-Path -LiteralPath $Ledger)) {
    Write-Host ('SKIP  ledger not found: ' + $Ledger + ' (local file, not in the repo)')
    exit 0
}

$resolved = (Resolve-Path -LiteralPath $Ledger).Path
$lines = [System.IO.File]::ReadAllLines($resolved, [System.Text.Encoding]::UTF8)
Write-Host ('--- ledger-check: ' + $Ledger + ' (' + $lines.Count + ' lines) ---')

# ---------- 1. the machine-readable snapshot stamp ----------
$stampLine = $null
foreach ($l in $lines) { if ($l -like ('*' + $Marker + '*')) { $stampLine = $l; break } }
if ($null -eq $stampLine) {
    Fail ('no "' + $Marker + '" stamp found; add one near the top of the ledger')
    Write-Host ('FAIL  ' + $script:fail + ' problem(s)')
    exit 1
}
function Field([string] $name) {
    if ($stampLine -match ($name + '=([^\s>]+)')) { return $Matches[1] }
    return $null
}
$stampDate = Field 'date'
$stampHead = Field 'head'
$stampTests = Field 'tests'
$stampPushed = Field 'pushed'
foreach ($pair in @(@('date', $stampDate), @('head', $stampHead), @('tests', $stampTests), @('pushed', $stampPushed))) {
    if ([string]::IsNullOrEmpty($pair[1])) { Fail ('stamp is missing ' + $pair[0] + '=...') }
}
if ($stampDate) {
    $parsed = [DateTime]::MinValue
    if ([DateTime]::TryParseExact($stampDate, 'yyyy-MM-dd', [System.Globalization.CultureInfo]::InvariantCulture,
            [System.Globalization.DateTimeStyles]::None, [ref] $parsed)) {
        $age = [int] ((Get-Date) - $parsed).TotalDays
        if ($age -lt 0) { Warn ('stamp date is in the future (' + $stampDate + ')') }
        elseif ($age -gt $MaxAgeDays) { Warn ('stamp is ' + $age + ' days old (budget ' + $MaxAgeDays + ') - re-verify the numbers') }
        else { Pass ('stamp age ' + $age + ' day(s) <= ' + $MaxAgeDays) }
    }
    else { Fail ('stamp date is not yyyy-MM-dd: ' + $stampDate) }
}

# ---------- 2. git agreement (skipped when git is not on PATH) ----------
$git = Get-Command git -ErrorAction SilentlyContinue
if ($null -eq $git) {
    # Deliberately no hardcoded fallback path here: a personal drive letter in a
    # tracked script breaks every other machine (that was ledger item E-13).
    Warn 'git not found on PATH; HEAD / push-state checks skipped'
}
else {
    $head = (& git rev-parse --short HEAD)
    if ($LASTEXITCODE -ne 0 -or -not $head) { Fail 'git rev-parse --short HEAD failed' }
    elseif ($head -ne $stampHead) { Fail ('stamp head=' + $stampHead + ' but HEAD is ' + $head) }
    else { Pass ('HEAD matches the stamp: ' + $head) }

    $aheadRaw = & git rev-list --count 'origin/main..HEAD' 2>$null
    if ($LASTEXITCODE -ne 0) {
        Warn 'no origin/main to compare against; push-state check skipped'
    }
    else {
        $ahead = [int] ($aheadRaw | Select-Object -Last 1)
        $claimed = $stampPushed
        if ($ahead -eq 0 -and $claimed -ne 'yes') {
            Fail ('stamp says pushed=' + $claimed + ' but origin/main..HEAD is empty (everything is pushed)')
        }
        elseif ($ahead -gt 0 -and $claimed -ne 'no') {
            Fail ('stamp says pushed=' + $claimed + ' but ' + $ahead + ' commit(s) are not pushed')
        }
        else { Pass ('push state matches: ' + $ahead + ' commit(s) ahead of origin/main') }
    }
}

# ---------- 3. declared test totals must add up ----------
# The baseline line lists per-suite counts and a claimed sum; extracting every
# bold number from it lets us re-do the addition the ledger has mis-done before.
$baseLine = $null
foreach ($l in $lines) { if ($l -match 'core lib') { $baseLine = $l; break } }
if ($null -eq $baseLine) {
    Warn 'no "core lib" baseline line found; cannot re-add the test totals'
}
else {
    # Every bold chunk on that line is one number slot; the label may sit inside
    # the same bold span (a label may share the span with its number), so read
    # the trailing digits of
    # each chunk rather than requiring the digits to be bolded on their own.
    $nums = @()
    foreach ($m in [regex]::Matches($baseLine, '\*\*([^*]+)\*\*')) {
        $tail = [regex]::Match($m.Groups[1].Value, '(\d+)\s*$')
        if ($tail.Success) { $nums += [int] $tail.Groups[1].Value }
    }
    if ($nums.Count -lt 3) {
        Fail ('baseline line carries only ' + $nums.Count + ' bold numbers; expected per-suite + total')
    }
    else {
        $declared = $nums[$nums.Count - 1]
        $sum = ($nums[0..($nums.Count - 2)] | Measure-Object -Sum).Sum
        if ($sum -ne $declared) { Fail ('per-suite numbers sum to ' + [string] $sum + ' but the ledger declares ' + [string] $declared) }
        else { Pass ('test totals add up: ' + [string] $declared + ' = ' + ($nums[0..($nums.Count - 2)] -join ' + ')) }
        if ($stampTests -and [int] $stampTests -ne $declared) {
            Fail ('stamp tests=' + $stampTests + ' disagrees with the baseline line (' + [string] $declared + ')')
        }
        elseif ($stampTests) { Pass ('stamp tests matches the baseline line: ' + $stampTests) }
    }
}

# ---------- 4. size budget ----------
if ($lines.Count -gt $MaxLines) { Warn ('ledger is ' + [string] $lines.Count + ' lines (budget ' + [string] $MaxLines + ') - consider moving the round summaries to the archive') }
else { Pass ('ledger size within budget: ' + [string] $lines.Count + ' <= ' + [string] $MaxLines) }
$rounds = @($lines | Where-Object { $_ -match '^### ' }).Count
if ($rounds -gt $MaxRoundSummaries) {
    Warn ([string] $rounds + ' round summaries in this file (budget ' + [string] $MaxRoundSummaries + ') - older ones belong in the archive')
}
else { Pass ('round summaries within budget: ' + [string] $rounds) }

Write-Host ('--- ' + $(if ($script:fail) { 'FAIL ' + $script:fail } else { 'PASS' }) + ' ---')
if ($script:fail) { exit 1 } else { exit 0 }
