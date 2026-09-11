# Generate the large-file acceptance sample referenced by the README
# ("open dev-assets/bench-50mb.log") and by core/examples/*_bench.rs.
#
# The sample itself is NOT in the repository: dev-assets/ is gitignored,
# so a fresh clone has no bench-50mb.log. Run this script to create one.
#
# Usage (from the repo root, Windows PowerShell 5.1+):
#   powershell -ExecutionPolicy Bypass -File .\tools\gen-bench-log.ps1
#   powershell -File .\tools\gen-bench-log.ps1 -Lines 600000 -Out dev-assets\bench-50mb.log
#
# Tuning: -Lines 600000 yields about 68 MB, matching the README claim.
# NOTE: keep this file ASCII-only. The harness shell reads .ps1 files without
# a BOM using the legacy ANSI codepage, so non-ASCII comments turn into
# mojibake (same rule as package.ps1).

param(
    [int]$Lines = 600000,
    [string]$Out = "dev-assets\bench-50mb.log"
)

$ErrorActionPreference = 'Stop'

$dir = [System.IO.Path]::GetDirectoryName([System.IO.Path]::GetFullPath($Out))
if ($dir -and -not (Test-Path -LiteralPath $dir)) {
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
}

$levels = @('INFO', 'INFO', 'INFO', 'DEBUG', 'WARN', 'ERROR')
$paths = @('/api/v1/items', '/api/v1/users', '/api/v1/orders', '/health', '/static/app.js')

$full = [System.IO.Path]::GetFullPath($Out)
# StreamWriter with an explicit UTF8 encoding (no BOM) stays fast at 600k lines
# and keeps the file readable by the loader's UTF-8 path.
$writer = [System.IO.StreamWriter]::new($full, $false, [System.Text.UTF8Encoding]::new($false))
$sw = [System.Diagnostics.Stopwatch]::StartNew()
try {
    for ($i = 0; $i -lt $Lines; $i++) {
        $lv = $levels[$i % $levels.Length]
        $p = $paths[$i % $paths.Length]
        $writer.WriteLine("2026-09-11 10:00:00.$i [$lv] request id=$i path=$p status=200 duration=$($i % 997)ms user=uid-$($i % 8192)")
        if (($i % 200000) -eq 0) { Write-Host "  $i / $Lines" }
    }
} finally {
    $writer.Close()
}

$size = (Get-Item -LiteralPath $full).Length
Write-Host "wrote $full ($Lines lines, $([math]::Round($size / 1MB, 1)) MB) in $([math]::Round($sw.Elapsed.TotalSeconds, 1))s"
