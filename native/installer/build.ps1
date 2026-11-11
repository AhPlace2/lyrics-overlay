# Builds the release exe and the installer: target\release\Lyrics-Overlay-Setup-<version>.exe
# Needs NSIS 3: makensis on PATH, a normal NSIS install, or the copy electron-builder downloaded.
# Usage (from anywhere): powershell -File native\installer\build.ps1 [-SkipBuild]
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
$native = Split-Path $here -Parent

$version = (Select-String -Path "$native\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $version) { throw 'No version in Cargo.toml' }

if (-not $SkipBuild) {
    Push-Location $native
    try { cargo build --release; if ($LASTEXITCODE) { throw 'cargo build failed' } } finally { Pop-Location }
}
$exe = "$native\target\release\lyrics-overlay.exe"
if (-not (Test-Path $exe)) { throw "Missing $exe" }

$makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $makensis) {
    $makensis = @(
        "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
        "$env:ProgramFiles\NSIS\makensis.exe"
    ) + @(Get-ChildItem "$env:LOCALAPPDATA\electron-builder\Cache\nsis\*\makensis.exe", "$env:LOCALAPPDATA\electron-builder\Cache\nsis-*\*\makensis.exe" -ErrorAction SilentlyContinue | ForEach-Object FullName) |
        Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $makensis) { throw 'NSIS not found. Install it from https://nsis.sourceforge.io/ or put makensis on PATH.' }

$out = "$native\target\release\Lyrics-Overlay-Setup-$version.exe"
& $makensis /V2 "/DVERSION=$version" "/DEXE=$exe" "/DOUTFILE=$out" "$here\installer.nsi"
if ($LASTEXITCODE) { throw 'makensis failed' }
Write-Host "Installer: $out"
