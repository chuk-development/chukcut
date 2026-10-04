# EXPERIMENTAL. Pack the Windows build of chukcut as a portable zip.
#
#   pwsh packaging/windows/package.ps1 -Version 0.2.0 -FfmpegDir C:\ffmpeg
#
# Takes the three binaries from target\release, the FFmpeg DLLs from
# <FfmpegDir>\bin (a shared build, the same one the binaries were linked
# against through FFMPEG_DIR) and the licence files, and writes
#   target\dist\chukcut-<version>-x86_64-windows.zip
# The staging directory target\windows\chukcut-<version>-x86_64-windows is
# kept: packaging\windows\chukcut.iss builds the installer from it.
#
# chukcut is a Linux program: today the engine does not compile for Windows
# (docs/decisions/0033-release-builds.md says what blocks it), so this
# script is only reached once that is fixed.
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$FfmpegDir
)
$ErrorActionPreference = 'Stop'

$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
Set-Location $root
$Version = $Version.TrimStart('v')
$name = "chukcut-$Version-x86_64-windows"
$stage = Join-Path $root "target\windows\$name"
$dist = Join-Path $root 'target\dist'

if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path $stage, $dist | Out-Null

# The editor finds the worker next to itself (engine::modules::ml::worker::beside).
foreach ($bin in 'chukcut', 'chukcut-ml-worker', 'chukcut-cli') {
    $exe = "target\release\$bin.exe"
    if (-not (Test-Path $exe)) { throw "package.ps1: no binary at $exe" }
    Copy-Item $exe $stage
}

# FFmpeg's DLLs beside the executables, where Windows looks first.
$dlls = Get-ChildItem (Join-Path $FfmpegDir 'bin') -Filter '*.dll'
if ($dlls.Count -eq 0) { throw "package.ps1: no DLLs in $FfmpegDir\bin" }
$dlls | Copy-Item -Destination $stage
Copy-Item (Join-Path $FfmpegDir 'LICENSE.txt') (Join-Path $stage 'FFMPEG-LICENSE.txt') -ErrorAction SilentlyContinue

Copy-Item LICENSE (Join-Path $stage 'LICENSE.txt')
Copy-Item NOTICE.md, README.md $stage
Copy-Item packaging\linux\icons\hicolor\256x256\apps\chukcut.png $stage

$zip = Join-Path $dist "$name.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $stage -DestinationPath $zip
(Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower() + "  $name.zip" | Out-File -Encoding ascii "$zip.sha256"
Write-Output $zip
