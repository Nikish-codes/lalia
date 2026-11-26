# Build the Windows downloads from target\release\lalia.exe:
#
#   dist\lalia-<version>-windows-x64-setup.exe   installer (NSIS)
#   dist\lalia-<version>-windows-x64.zip         portable: unzip and run
#
#   cargo build --release
#   .\packaging\windows\build.ps1
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\..\.."
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches.Groups[1].Value
$exe = Join-Path $root "target\release\lalia.exe"
if (-not (Test-Path $exe)) { throw "Build first: cargo build --release" }
New-Item -ItemType Directory -Force dist | Out-Null

$makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $makensis) { $makensis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe" }
if (-not (Test-Path $makensis)) {
    Write-Host "==> Installing NSIS"
    choco install nsis -y --no-progress | Out-Null
    $makensis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
}

Write-Host "==> Building the installer (lalia $version)"
$setup = Join-Path $root "dist\lalia-$version-windows-x64-setup.exe"
& $makensis /V2 "/DVERSION=$version" "/DROOT=$root" "/DEXE=$exe" "/DOUTFILE=$setup" (Join-Path $root "packaging\windows\installer.nsi")
if ($LASTEXITCODE -ne 0) { throw "makensis failed" }

Write-Host "==> Building the portable zip"
$stage = Join-Path $root "dist\Lalia"
Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory $stage | Out-Null
Copy-Item $exe "$stage\lalia.exe"
Copy-Item README.md "$stage\README.md"
Copy-Item LICENSE "$stage\LICENSE.txt"
Copy-Item assets\fonts\Inter-LICENSE.txt "$stage\Inter-LICENSE.txt"
$zip = Join-Path $root "dist\lalia-$version-windows-x64.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $stage -DestinationPath $zip
Remove-Item -Recurse -Force $stage

Get-ChildItem dist\lalia-$version-windows-x64* | ForEach-Object { Write-Host ("Done: {0}  ({1:N1} MB)" -f $_.Name, ($_.Length / 1MB)) }
