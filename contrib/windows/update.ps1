# Glossa Windows update bootstrapper.
# Downloads the versioned updater from the latest GitHub release, verifies its
# checksum, then runs it. Installed next to glossa.exe as update.ps1.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File update.ps1 [check|install]
#
# Machine-readable lines (parsed by the daemon tray/CLI):
#   GLOSSA_UPDATE_PROGRESS=<0-100>  (stage completion)
#   GLOSSA_UPDATE_STATUS=...        (forwarded from the versioned updater)

$ErrorActionPreference = "Stop"

$GLOSSA_REPO = if ($env:GLOSSA_REPO) { $env:GLOSSA_REPO } else { "Glaicer/Glossa-win" }
$GLOSSA_VERSION = if ($env:GLOSSA_VERSION) { $env:GLOSSA_VERSION } else { "latest" }
$UPDATER_ASSET_NAME = if ($env:UPDATER_ASSET_NAME) { $env:UPDATER_ASSET_NAME } else { "glossa-update.ps1" }
$CHECKSUMS_ASSET_NAME = if ($env:CHECKSUMS_ASSET_NAME) { $env:CHECKSUMS_ASSET_NAME } else { "sha256sums.txt" }
$GLOSSA_RELEASE_BASE_URL = if ($env:GLOSSA_RELEASE_BASE_URL) { $env:GLOSSA_RELEASE_BASE_URL } else { "" }

function Write-Progress-Line {
    param([int]$Percent)
    Write-Host "GLOSSA_UPDATE_PROGRESS=$Percent"
}

function Write-Log {
    param([string]$Message)
    Write-Host $Message
}

function Die {
    param([string]$Message)
    [Console]::Error.WriteLine("Error: $Message")
    exit 1
}

function Get-ReleaseDownloadUrl {
    param([string]$AssetName)

    if ($GLOSSA_RELEASE_BASE_URL) {
        $base = $GLOSSA_RELEASE_BASE_URL.TrimEnd("/")
        return "$base/$AssetName"
    }

    if ($GLOSSA_VERSION -eq "latest") {
        return "https://github.com/$GLOSSA_REPO/releases/latest/download/$AssetName"
    }

    return "https://github.com/$GLOSSA_REPO/releases/download/$GLOSSA_VERSION/$AssetName"
}

function Download-Asset {
    param(
        [string]$AssetName,
        [string]$Destination
    )

    $url = Get-ReleaseDownloadUrl -AssetName $AssetName
    Write-Log "Downloading $AssetName"
    try {
        Invoke-WebRequest -Uri $url -OutFile $Destination -UseBasicParsing
    } catch {
        Die "Failed to download $AssetName from $url : $($_.Exception.Message)"
    }
}

function Get-FileSha256Hex {
    param([string]$Path)
    return (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Test-UpdaterChecksum {
    param(
        [string]$ChecksumsPath,
        [string]$UpdaterPath,
        [string]$AssetName
    )

    $expected = $null
    foreach ($line in Get-Content -Path $ChecksumsPath) {
        $trimmed = $line.Trim()
        if (-not $trimmed) { continue }
        # sha256sum format: "<hash>  <filename>" or "<hash> *<filename>"
        if ($trimmed -match '^(?<hash>[0-9a-fA-F]{64})\s+\*?(?<name>\S+)$') {
            if ($Matches.name -eq $AssetName) {
                $expected = $Matches.hash.ToLowerInvariant()
                break
            }
        }
    }

    if (-not $expected) {
        Die "Checksum file does not contain an entry for $AssetName."
    }

    $actual = Get-FileSha256Hex -Path $UpdaterPath
    if ($actual -ne $expected) {
        Die "Checksum verification failed for $AssetName (expected $expected, got $actual)."
    }
}

$forwardArgs = @($args)
$tmpdir = $null
try {
    $tmpdir = Join-Path ([System.IO.Path]::GetTempPath()) ("glossa-update-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $tmpdir | Out-Null

    Write-Progress-Line 2
    $updaterPath = Join-Path $tmpdir $UPDATER_ASSET_NAME
    $checksumsPath = Join-Path $tmpdir $CHECKSUMS_ASSET_NAME

    Download-Asset -AssetName $UPDATER_ASSET_NAME -Destination $updaterPath
    Write-Progress-Line 8
    Download-Asset -AssetName $CHECKSUMS_ASSET_NAME -Destination $checksumsPath
    Test-UpdaterChecksum -ChecksumsPath $checksumsPath -UpdaterPath $updaterPath -AssetName $UPDATER_ASSET_NAME
    Write-Progress-Line 12

    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $updaterPath @forwardArgs
    Write-Progress-Line 95
    exit $LASTEXITCODE
} finally {
    if ($tmpdir -and (Test-Path $tmpdir)) {
        Remove-Item -Recurse -Force $tmpdir -ErrorAction SilentlyContinue
    }
}
