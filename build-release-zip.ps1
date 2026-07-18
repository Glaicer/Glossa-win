# Glossa Windows release packager.
# Builds the release binary and assembles GitHub release assets under
# target/release/github/<version>/.
#
# Usage (from repo root):
#   powershell -NoProfile -ExecutionPolicy Bypass -File build-release-zip.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File build-release-zip.ps1 -Version 0.9.0

param(
    [string]$Version = ""
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$BuildTargetDir = Join-Path $ScriptDir "target\release"
$GithubReleaseDir = Join-Path $BuildTargetDir "github"
$GlossaAssetName = "glossa-windows-x86_64.zip"
$UpdaterAssetName = "glossa-update.ps1"
$ChecksumsName = "sha256sums.txt"

function Write-Log {
    param([string]$Message)
    Write-Host $Message
}

function Die {
    param([string]$Message)
    Write-Error $Message
    exit 1
}

if (-not $Version) {
    $Version = Read-Host "Release version"
}
if (-not $Version) {
    Die "The release version cannot be empty."
}

Write-Log "Glossa release packager"
Write-Log "Version: $Version"
Write-Log "Building release binary..."
Push-Location $ScriptDir
try {
    cargo build --release --package glossa-bin
    if ($LASTEXITCODE -ne 0) {
        Die "cargo build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

$ReleaseExe = Join-Path $BuildTargetDir "glossa.exe"
if (-not (Test-Path $ReleaseExe)) {
    Die "Missing release binary at $ReleaseExe"
}

$TargetDir = Join-Path $GithubReleaseDir $Version
New-Item -ItemType Directory -Force -Path $TargetDir | Out-Null

$staging = Join-Path ([System.IO.Path]::GetTempPath()) ("glossa-stage-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $staging | Out-Null

try {
    $traySrc = Join-Path $ScriptDir "contrib\assets\tray"
    $soundSrc = Join-Path $ScriptDir "contrib\assets\sounds"
    $updateSrc = Join-Path $ScriptDir "contrib\windows\update.ps1"
    $updaterTemplate = Join-Path $ScriptDir "contrib\release\glossa-update.ps1"

    if (-not (Test-Path $updateSrc)) { Die "Missing $updateSrc" }
    if (-not (Test-Path $updaterTemplate)) { Die "Missing $updaterTemplate" }

    New-Item -ItemType Directory -Force -Path (Join-Path $staging "assets\tray") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $staging "assets\sounds") | Out-Null
    Copy-Item $ReleaseExe (Join-Path $staging "glossa.exe") -Force
    Copy-Item $updateSrc (Join-Path $staging "update.ps1") -Force
    Copy-Item (Join-Path $traySrc "*") (Join-Path $staging "assets\tray") -Force
    Copy-Item (Join-Path $soundSrc "*") (Join-Path $staging "assets\sounds") -Force
    Set-Content -Path (Join-Path $staging "VERSION") -Value $Version -NoNewline -Encoding ascii

    $exeOut = Join-Path $TargetDir "glossa.exe"
    Write-Log "Copying glossa.exe"
    Copy-Item $ReleaseExe $exeOut -Force

    $zipPath = Join-Path $TargetDir $GlossaAssetName
    if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
    Write-Log "Creating $GlossaAssetName"
    Compress-Archive -Path (Join-Path $staging "*") -DestinationPath $zipPath -Force

    Write-Log "Rendering $UpdaterAssetName"
    $updaterText = Get-Content -Path $updaterTemplate -Raw
    $updaterText = $updaterText.Replace("__GLOSSA_RELEASE_VERSION__", $Version)
    $updaterOut = Join-Path $TargetDir $UpdaterAssetName
    Set-Content -Path $updaterOut -Value $updaterText -Encoding utf8

    Write-Log "Writing $ChecksumsName"
    $checksumLines = @()
    foreach ($name in @($GlossaAssetName, $UpdaterAssetName)) {
        $path = Join-Path $TargetDir $name
        $hash = (Get-FileHash -Path $path -Algorithm SHA256).Hash.ToLowerInvariant()
        $checksumLines += "$hash  $name"
    }
    $checksumsPath = Join-Path $TargetDir $ChecksumsName
    Set-Content -Path $checksumsPath -Value ($checksumLines -join "`n") -Encoding ascii

    Write-Log ""
    Write-Log "Release packaging complete."
    Write-Log "- Version: $Version"
    Write-Log "- Binary: $exeOut"
    Write-Log "- Zip: $zipPath"
    Write-Log "- Updater: $updaterOut"
    Write-Log "- Checksums: $checksumsPath"
    Write-Log ""
    Write-Log "Upload the zip, updater, and checksums to a GitHub release on Glaicer/Glossa-win."
    Write-Log "glossa.exe is also copied for local use / optional direct upload."
} finally {
    if (Test-Path $staging) {
        Remove-Item -Recurse -Force $staging -ErrorAction SilentlyContinue
    }
}
