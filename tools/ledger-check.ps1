# Checks that the local working ledger (HANDOFF.md) still agrees with git and
# with its own declared numbers. It exists because the ledger has repeatedly gone
# quietly stale: HEAD rows left a dozen commits behind, hand-added test totals
# that did not sum up, "not pushed" notes written after the push, and one
# "watch this flaky test" row whose root cause had already been fixed.
#
# It also guards the layout itself: round logs must live in dated day files, and
# the frozen historical volume must not grow (its append-only rule was the reason
# it gained 1349 lines in one day, which is what section 6 fails on now).
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
    # Dated round logs: ONE file per day, holding both the round summary and the
    # mechanism-level detail that used to go to the append-only archive.
    [string] $Days = 'docs/days',
    # Index of the frozen historical volume; it carries that volume's machine
    # readable size cap, which is what stops the archive from growing again.
    [string] $ArchiveIndex = 'docs/ARCHIVE-INDEX.md',
    # Machine fields live in an HTML comment so this script can stay ASCII:
    #   <!-- ARCHIVE-FROZEN file=docs/HANDOFF_ARCHIVE.md lines=9733 date=2026-09-25 -->
    [string] $FrozenMarker = 'ARCHIVE-FROZEN',
    # The ledger's own stamp, also an HTML comment (see the encoding note below):
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
    Warn ([string] $rounds + ' round summaries in this file (budget ' + [string] $MaxRoundSummaries + ') - they belong in the day file under ' + $Days)
}
else { Pass ('round summaries within budget: ' + [string] $rounds) }

# ---------- 5. dated day files ----------
# Since round 152 every round log is ONE file per day (docs/days/YYYY-MM-DD.md)
# holding both the summary and the detail. "Read the newest one" has to stay
# reliable: a stray file in that folder breaks `ls docs/days | tail -1`, and a
# newest file older than the last commit means a round was committed but never
# written up (that is how round 149 lost its detail).
if (Test-Path -LiteralPath $Days) {
    # NB: every local below is named apart from the param `$Days`. PowerShell
    # variable names are case-INsensitive, and a param's type constraint stays on
    # the variable for its whole life -- an earlier revision of this file wrote
    # `$periods = @(<2 FileInfo>)` under `[string] $Periods`, which silently stored
    # the space-joined string instead: .Count became 1, [0] became the char '2',
    # and its .Name became the empty string. No error was raised.
    $mdFiles = @(Get-ChildItem -LiteralPath $Days -Filter '*.md')
    $dayFiles = @($mdFiles | Where-Object { $_.Name -match '^\d{4}-\d{2}-\d{2}\.md$' } | Sort-Object Name)
    if ($dayFiles.Count -eq 0) {
        Fail ('no YYYY-MM-DD.md day file under ' + $Days)
    }
    else {
        $newestDay = $dayFiles[($dayFiles.Count - 1)]
        Pass ('latest day file: ' + $newestDay.Name + ' (of ' + [string] $dayFiles.Count + ')')
        $strays = @($mdFiles | Where-Object { $_.Name -notmatch '^\d{4}-\d{2}-\d{2}\.md$' })
        if ($strays.Count -gt 0) {
            Warn (([string] $strays.Count) + ' non-day-named file(s) in ' + $Days + ': ' + (($strays | ForEach-Object { $_.Name }) -join ', '))
        }
        if ($null -ne $git) {
            $committed = (& git log -1 --format=%cd --date=short)
            if ($LASTEXITCODE -eq 0 -and $committed) {
                $committed = ($committed | Select-Object -Last 1).Trim()
                if ($committed -gt $newestDay.BaseName) {
                    Warn ('last commit is ' + $committed + ' but the newest day file is ' + $newestDay.BaseName + ' - a round may be unwritten')
                }
                else { Pass ('newest day file covers the last commit (' + $committed + ')') }
            }
        }
    }
}
else {
    Warn ('day folder not found: ' + $Days)
}

# ---------- 6. the frozen historical volume ----------
# The old archive was append-only, so it could only ever grow -- it added 1349
# lines in a single day (8384 -> 9733), and the line budget in section 4 caught
# none of it because that budget only measures HANDOFF.md. Round logs now go to
# the day files above, the volume is frozen, and its frozen size is declared in
# the index so growth (or truncation, which is worse) fails here.
if (Test-Path -LiteralPath $ArchiveIndex) {
    $idxLines = [System.IO.File]::ReadAllLines((Resolve-Path -LiteralPath $ArchiveIndex).Path, [System.Text.Encoding]::UTF8)
    $frozenLine = $null
    foreach ($l in $idxLines) { if ($l -like ('*' + $FrozenMarker + '*')) { $frozenLine = $l; break } }
    if ($null -eq $frozenLine) {
        Fail ('no "' + $FrozenMarker + '" cap line found in ' + $ArchiveIndex)
    }
    else {
        $frozenPath = $null
        $frozenDeclared = 0
        if ($frozenLine -match 'file=([^\s>]+)') { $frozenPath = $Matches[1] }
        if ($frozenLine -match 'lines=(\d+)') { $frozenDeclared = [int] $Matches[1] }
        if ([string]::IsNullOrEmpty($frozenPath) -or -not (Test-Path -LiteralPath $frozenPath)) {
            Fail ('frozen volume named in the index does not exist: ' + $frozenPath)
        }
        elseif ($frozenDeclared -le 0) {
            Fail ('index declares no usable lines=... cap')
        }
        else {
            $frozenActual = @([System.IO.File]::ReadAllLines((Resolve-Path -LiteralPath $frozenPath).Path, [System.Text.Encoding]::UTF8)).Count
            if ($frozenActual -gt $frozenDeclared) {
                Fail ($frozenPath + ' grew ' + [string] ($frozenActual - $frozenDeclared) + ' line(s) past its freeze at ' + [string] $frozenDeclared + ' - new round logs belong in ' + $Days)
            }
            elseif ($frozenActual -lt $frozenDeclared) {
                Fail ($frozenPath + ' SHRANK to ' + [string] $frozenActual + ' lines (frozen value was ' + [string] $frozenDeclared + ') - history was edited or truncated')
            }
            else { Pass ('historical volume frozen at ' + [string] $frozenDeclared + ' lines: ' + $frozenPath) }
        }
    }
}
else {
    Warn ('archive index not found: ' + $ArchiveIndex)
}

Write-Host ('--- ' + $(if ($script:fail) { 'FAIL ' + $script:fail } else { 'PASS' }) + ' ---')
if ($script:fail) { exit 1 } else { exit 0 }
