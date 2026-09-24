# P72 release packaging: build -> verify -> stage -> zip -> SHA256.
#
# Usage (run from repo root, inside the harness PowerShell session):
#   & .\package.ps1             # full flow (includes release build)
#   & .\package.ps1 -SkipBuild  # repackage existing exe only
#   & .\package.ps1 -SkipCheck  # skip the release-tag check (local trials)
#
# Output: dist/Editpad-<version>-win64.zip (exe + README + license) and
# dist/Editpad-<version>-sha256.txt (full hashes for the release page).
# dist/ is gitignored (artifacts are reproducible).
# NOTE: keep this file ASCII-only. The harness shell reads .ps1 files
# without a BOM using the legacy ANSI codepage, so non-ASCII comments
# would turn into mojibake and can break parsing.

param([switch]$SkipBuild, [switch]$SkipCheck)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

# Cargo location: PATH first (custom toolchains, CI images), then CARGO_HOME,
# then the default per-user install. A single hard-coded path breaks the whole
# script on any machine that puts cargo elsewhere.
function Resolve-Cargo {
    $found = Get-Command cargo -ErrorAction SilentlyContinue
    if ($found) { return $found.Source }
    if ($env:CARGO_HOME) {
        $p = Join-Path $env:CARGO_HOME 'bin\cargo.exe'
        if (Test-Path -LiteralPath $p) { return $p }
    }
    $p = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $p) { return $p }
    throw 'cargo not found - put it on PATH or set CARGO_HOME'
}
$cargo = Resolve-Cargo

# Git is used for the release check and for the revision line in the checksum
# file. Resolved once; absence is non-fatal because packaging does not need it.
function Resolve-Git {
    $found = Get-Command git -ErrorAction SilentlyContinue
    if ($found) { return $found.Source }
    if ($env:EDITPAD_GIT) {
        if (Test-Path -LiteralPath $env:EDITPAD_GIT) { return $env:EDITPAD_GIT }
    }
    # 注册表按机器实况给出安装目录（与 crates/app/build.rs 读 KitsRoot10 同一
    # 手法）。此前这里写死过一个个人盘符路径，等于把一个人的环境塞进仓库脚本。
    foreach ($hive in 'HKLM:\SOFTWARE\GitForWindows', 'HKCU:\SOFTWARE\GitForWindows') {
        $ip = (Get-ItemProperty -LiteralPath $hive -ErrorAction SilentlyContinue).InstallPath
        if ($ip) {
            $exe = Join-Path $ip 'cmd\git.exe'
            if (Test-Path -LiteralPath $exe) { return $exe }
        }
    }
    foreach ($p in @("$env:ProgramFiles\Git\cmd\git.exe", "${env:ProgramFiles(x86)}\Git\cmd\git.exe")) {
        if (Test-Path -LiteralPath $p) { return $p }
    }
    return $null
}
$git = Resolve-Git
$rev = ''
if ($git) { $rev = (& $git -C $root rev-parse --short HEAD 2>$null) }

# 0. Release traceability: a package must come from a tagged, clean revision so
#    the zip can be traced back to its source. Skipped with -SkipCheck, and also
#    skipped (with a warning) when git is unavailable - packaging itself does
#    not depend on git.
$tag = ''
if (-not $SkipCheck) {
    if ($git) {
        $dirty = & $git -C $root status --porcelain
        if ($dirty) { throw 'working tree is dirty - commit before packaging (or pass -SkipCheck)' }
        # ⚠️ 本机实测：$ErrorActionPreference='Stop' 下，未打 tag 的 HEAD 上跑
        # `git describe --tags --exact-match` 会抛 RemoteException（原生 stderr 被
        # 转成 ErrorRecord，`2>$null` 也拦不住），把下面那句人话提示顶成难懂报错
        # ——而这恰是最常见的路径。取码期间临时放宽，按 $LASTEXITCODE 判定。
        $prevEAP = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $tag = & $git -C $root describe --tags --exact-match HEAD 2>$null
        if ($LASTEXITCODE -ne 0) { $tag = $null }
        $ErrorActionPreference = $prevEAP
        if (-not $tag) { throw 'HEAD is not tagged - tag the release commit before packaging (or pass -SkipCheck)' }
        Write-Host ("== release: {0} @ {1} ==" -f $tag, $rev)
    } else {
        Write-Warning 'git not found - skipping release-tag check'
    }
}

# Version comes from [workspace.package]. The match is scoped to that single
# table so a top-level `version = ` line appearing earlier can never win.
$cargoTomlPath = Join-Path $root 'Cargo.toml'
$cargoToml = Get-Content -LiteralPath $cargoTomlPath
$table = $cargoToml | Select-String -Pattern '^\s*\[workspace\.package\]\s*$' | Select-Object -First 1
if (-not $table) { throw "no [workspace.package] table in $cargoTomlPath" }
$version = $null
for ($i = $table.LineNumber; $i -lt $cargoToml.Count; $i++) {
    if ($cargoToml[$i] -match '^\s*\[') { break }
    if ($cargoToml[$i] -match '^\s*version\s*=\s*"(.+)"') { $version = $Matches[1]; break }
}
if (-not $version) { throw "no version in [workspace.package] ($cargoTomlPath)" }

# 1. Build release.
if (-not $SkipBuild) {
    Write-Host '== cargo build --release =='
    & $cargo build --release --manifest-path (Join-Path $root 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) { throw "build failed (exit $LASTEXITCODE)" }
}
$exe = Join-Path $root 'target\release\editpad.exe'
if (-not (Test-Path -LiteralPath $exe)) { throw "missing $exe - build first" }

# 1b. -SkipBuild reuses whatever sits in target/. Warn instead of silently
#     shipping a binary that predates the newest edit.
if ($SkipBuild) {
    $newest = Get-ChildItem -LiteralPath (Join-Path $root 'crates') -Recurse -File -Filter *.rs |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($newest -and $newest.LastWriteTime -gt (Get-Item -LiteralPath $exe).LastWriteTime) {
        Write-Warning ("exe is older than {0} - drop -SkipBuild unless reusing it is intended" -f $newest.Name)
    }
}

# 1c. Version-resource sanity check runs BEFORE staging: a resource that fell
#     out of sync (build.rs version injection regressing) must never reach a
#     zip, or a bad package sits in dist/ waiting to be published by mistake.
$vi = (Get-Item -LiteralPath $exe).VersionInfo
if ($vi.ProductName -ne 'Editpad') { throw 'version resource broken (ProductName != Editpad) - check build.rs resource pipeline' }
if ($vi.FileVersion -ne $version) { throw "version resource stale (FileVersion '$($vi.FileVersion)' != workspace version '$version') - check build.rs version injection" }
Write-Host ("version resource: {0} / {1} v{2}" -f $vi.ProductName, $vi.FileDescription, $vi.FileVersion)

# 2. Stage.
$stageName = "Editpad-$version"
$stage = Join-Path $root "dist\$stageName"
if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item -LiteralPath $exe -Destination $stage
Copy-Item -LiteralPath (Join-Path $root 'README.md') -Destination $stage
Copy-Item -LiteralPath (Join-Path $root 'README.zh.md') -Destination $stage
Copy-Item -LiteralPath (Join-Path $root 'LICENSE-APACHE') -Destination $stage
# 两份 README 首屏都是相对路径引用的截图（README.md:13 的
# docs/assets/screenshot-dark.png——那张真实截图本身就是卖点）。只拷 README
# 不拷资源，离线用户解开 zip 看到的就是碎图标。
$assets = Join-Path $root 'docs\assets'
if (Test-Path -LiteralPath $assets) {
    New-Item -ItemType Directory -Force -Path (Join-Path $stage 'docs') | Out-Null
    Copy-Item -LiteralPath $assets -Destination (Join-Path $stage 'docs\assets') -Recurse -Force
}

# 3. Zip (top-level folder keeps the archive self-contained on extraction).
$zip = Join-Path $root "dist\$stageName-win64.zip"
if (Test-Path -LiteralPath $zip) { Remove-Item -LiteralPath $zip -Force }
Compress-Archive -LiteralPath $stage -DestinationPath $zip

# 4. Report.
$zipSha = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
$exeSha = (Get-FileHash -LiteralPath (Join-Path $stage 'editpad.exe') -Algorithm SHA256).Hash
Write-Host ''
Write-Host '== package done =='
Write-Host ("zip     : {0}" -f $zip)
Write-Host ("zip size: {0:N1} MB" -f ((Get-Item -LiteralPath $zip).Length / 1MB))
Write-Host ("zip sha256: {0}" -f $zipSha)
Write-Host ("exe sha256: {0}" -f $exeSha)
Write-Host 'package contents:'
Get-ChildItem -LiteralPath $stage | ForEach-Object { Write-Host ("  {0,-20} {1,10:N0} bytes" -f $_.Name, $_.Length) }

# 5. Checksum manifest: full hashes on disk, ready to paste into release
#    notes. Copying them by hand off the console is how typos get shipped.
$shaFile = Join-Path $root "dist\$stageName-sha256.txt"
$revLabel = if ($rev) { $rev } else { 'unknown' }
# 走 -SkipCheck 时清单行过去只是「少了 (tag …) 后缀」——隐式痕迹，事后无法与
# 「忘了打 tag」区分（已发生过：产物名/版本串/tag 三者互不对应）。显式落字。
$tagLabel = if ($tag) { " (tag $tag)" } else { ' [UNTAGGED]' }
$shaLines = @(
    ("Editpad {0} @ {1}{2}" -f $version, $revLabel, $tagLabel),
    ("zip  {0}  sha256={1}" -f (Split-Path $zip -Leaf), $zipSha),
    ('exe  editpad.exe           sha256={0}' -f $exeSha)
)
$shaLines | Set-Content -LiteralPath $shaFile -Encoding ASCII
Write-Host ("checksums: {0}" -f $shaFile)
