# Build the Windows (x64) release packages:
#   rocketman-<tag>-windows-x64.msi  installer: Program Files, Start menu, PATH
#   rocketman-<tag>-windows-x64.zip  rocketman.exe and rocketman-tui.exe
#
# Needs the MSVC toolchain and the Windows SDK (fxc.exe compiles GPUI's
# shaders), plus WiX Toolset v5 (dotnet tool install --global wix --version 5.0.2).
# The MSI is not code-signed, so SmartScreen may warn before installing it.
# Usage: scripts/package-windows.ps1 <tag> <out-dir>
# Prints the paths of the packages it wrote.
param(
    [Parameter(Mandatory)] [string] $Tag,
    [Parameter(Mandatory)] [string] $OutDir
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
# MSI versions are numeric only (major.minor.patch), so drop any pre-release suffix.
$msiVersion = ($version -split '[-+]')[0]

$target = 'x86_64-pc-windows-msvc'
rustup target add $target | Out-Host
cargo build --release --locked --target $target | Out-Host
$bin = Join-Path $root "target\$target\release"

$zip = Join-Path $OutDir "rocketman-$Tag-windows-x64.zip"
Compress-Archive -Force -Path "$bin\rocketman.exe", "$bin\rocketman-tui.exe", LICENSE, README.md -DestinationPath $zip
Write-Output $zip

$msi = Join-Path $OutDir "rocketman-$Tag-windows-x64.msi"
wix build packaging\windows\rocketman.wxs -arch x64 `
    -d "Version=$msiVersion" -d "BinDir=$bin" -d "SourceDir=$root" `
    -o $msi | Out-Host
Write-Output $msi
