# Install the latest Azimut release (with RTKLIB-EX) on Windows, for the current user:
#   irm https://raw.githubusercontent.com/tofunori/azimut/main/scripts/install.ps1 | iex
# Installs into %LOCALAPPDATA%\azimut and adds it to the user PATH.
$ErrorActionPreference = 'Stop'
$url = 'https://github.com/tofunori/azimut/releases/latest/download/azimut-x86_64-pc-windows-msvc.zip'
$dest = Join-Path $env:LOCALAPPDATA 'azimut'
$zip = Join-Path $env:TEMP 'azimut.zip'
Write-Host "Downloading $url"
Invoke-WebRequest -Uri $url -OutFile $zip
$unpack = Join-Path $env:TEMP 'azimut-unpack'
if (Test-Path $unpack) { Remove-Item $unpack -Recurse -Force }
Expand-Archive -Path $zip -DestinationPath $unpack
if (Test-Path $dest) { Remove-Item $dest -Recurse -Force }
Move-Item (Get-ChildItem $unpack -Directory | Select-Object -First 1).FullName $dest
Remove-Item $zip, $unpack -Recurse -Force
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($path -split ';' -contains $dest)) {
  [Environment]::SetEnvironmentVariable('Path', "$path;$dest", 'User')
  Write-Host 'Added to your PATH: open a new terminal to use azimut.'
}
& (Join-Path $dest 'azimut.exe') --version
