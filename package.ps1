# P72 release packaging: build -> stage -> zip -> SHA256.
#
# Usage (run from repo root, inside the harness PowerShell session):
#   & .\package.ps1             # full flow (includes release build)
#   & .\package.ps1 -SkipBuild  # repackage existing exe only
#   & .\package.ps1 -Portable   # plus portable.txt marker (isolated data dir)
#
# -Portable (P101): drops an empty portable.txt next to the exe. The app
# then keeps config.toml + snapshot/ in the exe directory instead of
# %APPDATA%\editpad -- a self-contained copy isolated from other installs.
#
# Output: dist/Editpad-<version>-win64.zip (exe + README + license).
# dist/ is gitignored (artifacts are reproducible).
# NOTE: keep this file ASCII-only. The harness shell reads .ps1 files
# without a BOM using the legacy ANSI codepage, so non-ASCII comments
# would turn into mojibake and can break parsing.

param([switch]$SkipBuild, [switch]$Portable)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'

# Version comes from workspace.package (first `version = ` line in Cargo.toml).
$versionLine = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(.+)"'
$version = $versionLine.Matches[0].Groups[1].Value

# 1. Build release.
if (-not $SkipBuild) {
    Write-Host '== cargo build --release =='
    & $cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "build failed (exit $LASTEXITCODE)" }
}
$exe = Join-Path $root 'target\release\editpad.exe'
if (-not (Test-Path $exe)) { throw "missing $exe - build first" }

# 2. Stage.
$stageName = "Editpad-$version"
$stage = Join-Path $root "dist\$stageName"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item $exe $stage
Copy-Item (Join-Path $root 'README.md') $stage
Copy-Item (Join-Path $root 'LICENSE-APACHE') $stage

# 2.5. Portable marker (P101): zero-byte file, presence only matters.
if ($Portable) {
    New-Item -ItemType File -Force -Path (Join-Path $stage 'portable.txt') | Out-Null
    Write-Host '== portable mode: portable.txt dropped (isolated data dir) =='
}

# 3. Zip.
$zip = Join-Path $root "dist\$stageName-win64.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip

# 4. Report: hashes + version-resource sanity check.
$zipSha = (Get-FileHash $zip -Algorithm SHA256).Hash
$exeSha = (Get-FileHash (Join-Path $stage 'editpad.exe') -Algorithm SHA256).Hash
$vi = (Get-Item $exe).VersionInfo
Write-Host ''
Write-Host '== package done =='
Write-Host ("zip     : {0}" -f $zip)
Write-Host ("zip size: {0:N1} MB   sha256: {1}" -f ((Get-Item $zip).Length / 1MB), $zipSha.Substring(0, 16))
Write-Host ("exe sha256: {0}" -f $exeSha.Substring(0, 16))
Write-Host ("version resource: {0} / {1} v{2}" -f $vi.ProductName, $vi.FileDescription, $vi.FileVersion)
if ($vi.ProductName -ne 'Editpad') { throw 'version resource broken (ProductName != Editpad) - check build.rs resource pipeline' }
Write-Host 'package contents:'
Get-ChildItem $stage | ForEach-Object { Write-Host ("  {0,-20} {1,10:N0} bytes" -f $_.Name, $_.Length) }