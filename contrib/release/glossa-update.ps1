# Glossa Windows versioned updater (release asset).
# RELEASE_VERSION is substituted at packaging time.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File glossa-update.ps1 [check|install]
#
# Machine-readable result lines (parsed by the daemon tray/CLI):
#   GLOSSA_UPDATE_STATUS=up-to-date|available|updated
#   GLOSSA_UPDATE_VERSION=<version>
#   GLOSSA_UPDATE_PROGRESS=<0-100>  (stage completion)

$ErrorActionPreference = "Stop"

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

$GLOSSA_REPO = if ($env:GLOSSA_REPO) { $env:GLOSSA_REPO } else { "Glaicer/Glossa-win" }
$GLOSSA_VERSION = if ($env:GLOSSA_VERSION) { $env:GLOSSA_VERSION } else { "latest" }
$GLOSSA_ASSET_NAME = if ($env:GLOSSA_ASSET_NAME) { $env:GLOSSA_ASSET_NAME } else { "glossa-windows-x86_64.zip" }
$CHECKSUMS_ASSET_NAME = if ($env:CHECKSUMS_ASSET_NAME) { $env:CHECKSUMS_ASSET_NAME } else { "sha256sums.txt" }
$GLOSSA_RELEASE_BASE_URL = if ($env:GLOSSA_RELEASE_BASE_URL) { $env:GLOSSA_RELEASE_BASE_URL } else { "" }
$RELEASE_VERSION = "__GLOSSA_RELEASE_VERSION__"

if (-not $env:LOCALAPPDATA) {
    Die "LOCALAPPDATA is required."
}
$InstallDir = Join-Path $env:LOCALAPPDATA "glossa"
$InstalledExePath = Join-Path $InstallDir "glossa.exe"
$InstalledVersionPath = Join-Path $InstallDir "VERSION"
$InstalledUpdatePath = Join-Path $InstallDir "update.ps1"
$ConfigPath = if ($env:APPDATA) {
    Join-Path $env:APPDATA "glossa\config.toml"
} else {
    $null
}

function Emit-Result {
    param(
        [string]$Status,
        [string]$Version
    )
    Write-Host "GLOSSA_UPDATE_STATUS=$Status"
    Write-Host "GLOSSA_UPDATE_VERSION=$Version"
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

function Assert-InstalledLayout {
    if (-not (Test-Path -LiteralPath $InstalledExePath)) {
        Die "Glossa is not installed at $InstalledExePath."
    }
}

function Get-InstalledVersion {
    if (Test-Path -LiteralPath $InstalledVersionPath) {
        $fromFile = (Get-Content -LiteralPath $InstalledVersionPath -Raw).Trim()
        if ($fromFile) {
            return $fromFile
        }
    }

    try {
        $output = & $InstalledExePath --version 2>$null
        if ($output) {
            $text = ($output | Out-String).Trim()
            # clap --version: "glossa 0.9.0"
            if ($text -match '(\d+\.\d+\.\d+\S*)$') {
                return $Matches[1]
            }
            return $text
        }
    } catch {
        # fall through
    }

    return "unknown"
}

function Test-DownloadedAssets {
    param(
        [string]$ChecksumsPath,
        [string[]]$AssetNames
    )

    $entries = @{}
    foreach ($line in Get-Content -Path $ChecksumsPath) {
        $trimmed = $line.Trim()
        if (-not $trimmed) { continue }
        if ($trimmed -match '^(?<hash>[0-9a-fA-F]{64})\s+\*?(?<name>\S+)$') {
            $entries[$Matches.name] = $Matches.hash.ToLowerInvariant()
        }
    }

    foreach ($name in $AssetNames) {
        if (-not $entries.ContainsKey($name)) {
            Die "Checksum file does not contain an entry for $name."
        }
        $path = Join-Path $script:tmpdir $name
        $actual = Get-FileSha256Hex -Path $path
        if ($actual -ne $entries[$name]) {
            Die "Checksum verification failed for $name (expected $($entries[$name]), got $actual)."
        }
    }
}

function Expand-GlossaBundle {
    param([string]$ZipPath)

    $bundleDir = Join-Path $script:tmpdir "glossa"
    if (Test-Path $bundleDir) {
        Remove-Item -Recurse -Force $bundleDir
    }
    New-Item -ItemType Directory -Path $bundleDir | Out-Null
    Expand-Archive -Path $ZipPath -DestinationPath $bundleDir -Force
    return $bundleDir
}

function Install-DirectoryFiles {
    param(
        [string]$SourceDir,
        [string]$DestinationDir
    )

    if (-not (Test-Path -LiteralPath $SourceDir)) {
        Die "The release bundle does not contain files under $SourceDir."
    }

    New-Item -ItemType Directory -Force -Path $DestinationDir | Out-Null
    Get-ChildItem -LiteralPath $DestinationDir -File -ErrorAction SilentlyContinue | Remove-Item -Force
    $files = Get-ChildItem -LiteralPath $SourceDir -File
    if ($files.Count -eq 0) {
        Die "The release bundle does not contain files under $SourceDir."
    }
    foreach ($file in $files) {
        Copy-Item -LiteralPath $file.FullName -Destination (Join-Path $DestinationDir $file.Name) -Force
    }
}

function Replace-RunningExecutable {
    param(
        [string]$SourceExe,
        [string]$DestExe
    )

    # On Windows a running .exe can usually be renamed but not overwritten.
    $tmp = "$DestExe.new"
    $bak = "$DestExe.bak"
    Copy-Item -LiteralPath $SourceExe -Destination $tmp -Force
    if (Test-Path -LiteralPath $DestExe) {
        Remove-Item -LiteralPath $bak -Force -ErrorAction SilentlyContinue
        Rename-Item -LiteralPath $DestExe -NewName (Split-Path $bak -Leaf) -Force
    }
    Rename-Item -LiteralPath $tmp -NewName (Split-Path $DestExe -Leaf) -Force
    Remove-Item -LiteralPath $bak -Force -ErrorAction SilentlyContinue
}

function Install-GlossaBundle {
    param([string]$BundleDir)

    $exeSource = Join-Path $BundleDir "glossa.exe"
    $updateSource = Join-Path $BundleDir "update.ps1"
    $traySource = Join-Path $BundleDir "assets\tray"
    $soundSource = Join-Path $BundleDir "assets\sounds"
    $versionSource = Join-Path $BundleDir "VERSION"

    if (-not (Test-Path -LiteralPath $exeSource)) {
        Die "The Glossa release bundle does not contain glossa.exe."
    }
    if (-not (Test-Path -LiteralPath $updateSource)) {
        Die "The Glossa release bundle does not contain update.ps1."
    }
    if (-not (Test-Path -LiteralPath $traySource)) {
        Die "The Glossa release bundle does not contain tray icons under assets\tray."
    }
    if (-not (Test-Path -LiteralPath $soundSource)) {
        Die "The Glossa release bundle does not contain cue sounds under assets\sounds."
    }
    if (-not (Test-Path -LiteralPath $versionSource)) {
        Die "The Glossa release bundle does not contain VERSION."
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Install-DirectoryFiles -SourceDir $traySource -DestinationDir (Join-Path $InstallDir "assets\tray")
    Install-DirectoryFiles -SourceDir $soundSource -DestinationDir (Join-Path $InstallDir "assets\sounds")
    Copy-Item -LiteralPath $versionSource -Destination $InstalledVersionPath -Force
    Copy-Item -LiteralPath $updateSource -Destination $InstalledUpdatePath -Force
    Replace-RunningExecutable -SourceExe $exeSource -DestExe $InstalledExePath
}

function Schedule-Restart {
    # Detach a short-lived helper so the current daemon can finish showing UI,
    # then stop and re-launch the updated binary.
    if (-not $ConfigPath -or -not (Test-Path -LiteralPath $ConfigPath)) {
        Write-Log "Config not found; skipping automatic restart. Start the daemon manually."
        return
    }

    $exe = $InstalledExePath.Replace("'", "''")
    $cfg = $ConfigPath.Replace("'", "''")
    $helper = @"
Start-Sleep -Seconds 2
Get-Process -Name 'glossa' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500
Start-Process -FilePath '$exe' -ArgumentList '--config','$cfg','daemon' -WorkingDirectory '$(Split-Path $InstalledExePath -Parent)'
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($helper))
    Start-Process -FilePath "powershell.exe" -ArgumentList @(
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-WindowStyle", "Hidden",
        "-EncodedCommand", $encoded
    ) -WindowStyle Hidden | Out-Null
    Write-Log "Scheduled daemon restart"
}

$script:tmpdir = $null
try {
    $commandName = if ($args.Count -ge 1) { [string]$args[0] } else { "install" }
    if ($commandName -notin @("check", "install")) {
        Die "Unsupported updater command '$commandName'. Expected 'check' or 'install'."
    }

    Assert-InstalledLayout
    $currentVersion = Get-InstalledVersion

    if ($currentVersion -eq $RELEASE_VERSION) {
        Write-Log "Glossa $RELEASE_VERSION is already installed."
        Emit-Result -Status "up-to-date" -Version $RELEASE_VERSION
        exit 0
    }

    if ($commandName -eq "check") {
        Emit-Result -Status "available" -Version $RELEASE_VERSION
        exit 0
    }

    Write-Log "Updating Glossa from $currentVersion to $RELEASE_VERSION"
    Write-Progress-Line 18
    $script:tmpdir = Join-Path ([System.IO.Path]::GetTempPath()) ("glossa-install-" + [guid]::NewGuid().ToString("N"))
    New-Item -ItemType Directory -Path $script:tmpdir | Out-Null

    $zipPath = Join-Path $script:tmpdir $GLOSSA_ASSET_NAME
    $checksumsPath = Join-Path $script:tmpdir $CHECKSUMS_ASSET_NAME
    Download-Asset -AssetName $GLOSSA_ASSET_NAME -Destination $zipPath
    Write-Progress-Line 55
    Download-Asset -AssetName $CHECKSUMS_ASSET_NAME -Destination $checksumsPath
    Test-DownloadedAssets -ChecksumsPath $checksumsPath -AssetNames @($GLOSSA_ASSET_NAME)
    Write-Progress-Line 60

    $bundleDir = Expand-GlossaBundle -ZipPath $zipPath
    Write-Progress-Line 68
    Install-GlossaBundle -BundleDir $bundleDir
    Write-Progress-Line 80
    Schedule-Restart
    Write-Progress-Line 90

    Write-Log "Update complete."
    Write-Progress-Line 95
    Emit-Result -Status "updated" -Version $RELEASE_VERSION
    exit 0
} finally {
    if ($script:tmpdir -and (Test-Path $script:tmpdir)) {
        Remove-Item -Recurse -Force $script:tmpdir -ErrorAction SilentlyContinue
    }
}
