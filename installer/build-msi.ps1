# Builds Kaldor's Windows installer: target\installer\Kaldor-<version>-beta.msi
#
#   powershell -ExecutionPolicy Bypass -File installer\build-msi.ps1
#
# Needs the WiX Toolset v3 binaries (candle.exe, light.exe) unpacked in
# local\wix314 (git-ignored): the official wix314-binaries.zip from
# https://github.com/wixtoolset/wix3/releases (v3.14.1). Nothing is
# installed on the system.
#
# The installer build links the Visual C++ runtime into kaldor.exe
# (crt-static), so the target PC needs no Visual C++ Redistributable.
# It builds into its own target folder, so the normal builds are not
# rebuilt.

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$wix = Join-Path $root 'local\wix314'
if (-not (Test-Path (Join-Path $wix 'candle.exe'))) {
    throw "WiX v3 not found: unpack wix314-binaries.zip into $wix"
}

# The version from the workspace (MSI versions are three numbers).
$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "(\d+\.\d+\.\d+)"').Matches[0].Groups[1].Value

$buildDir = Join-Path $root 'target\installer-build'
$outDir = Join-Path $root 'target\installer'
New-Item -ItemType Directory -Force $outDir | Out-Null

Write-Host "Building kaldor.exe $version and the installer's actions (release, static C runtime)..."
$env:RUSTFLAGS = '-C target-feature=+crt-static'
& cargo build --release -p dj-app -p installer-actions -j 4 --target-dir $buildDir
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
Remove-Item Env:RUSTFLAGS

$exe = Join-Path $buildDir 'release\kaldor.exe'
$actions = Join-Path $buildDir 'release\installer_actions.dll'
$icon = Join-Path $root 'crates\dj-app\assets\kaldor.ico'
$obj = Join-Path $outDir 'kaldor.wixobj'
$msi = Join-Path $outDir "Kaldor-$version-beta.msi"

$license = Join-Path $PSScriptRoot 'license.rtf'
# The window's pictures (made from the mark by
# `cargo run -p dj-ui --example installer_art -- crates/dj-ui/assets/kaldor-mark.svg installer/banner.bmp installer/dialog.bmp`).
$banner = Join-Path $PSScriptRoot 'banner.bmp'
$dialog = Join-Path $PSScriptRoot 'dialog.bmp'

Write-Host "Packaging $msi..."
& (Join-Path $wix 'candle.exe') -nologo -arch x64 -ext WixUIExtension -ext WixUtilExtension `
    "-dVersion=$version" "-dExePath=$exe" "-dIconPath=$icon" "-dLicensePath=$license" "-dActionsPath=$actions" `
    "-dBannerPath=$banner" "-dDialogPath=$dialog" `
    -out $obj (Join-Path $PSScriptRoot 'kaldor.wxs')
if ($LASTEXITCODE -ne 0) { throw "candle failed" }
& (Join-Path $wix 'light.exe') -nologo -ext WixUIExtension -ext WixUtilExtension -cultures:en-us `
    -out $msi $obj
if ($LASTEXITCODE -ne 0) { throw "light failed" }

Write-Host "Done: $msi"
