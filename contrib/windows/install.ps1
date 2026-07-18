# Glossa Windows installer (local build)
# Usage (from repo root, after cargo build --release):
#   powershell -ExecutionPolicy Bypass -File contrib\windows\install.ps1

$ErrorActionPreference = "Stop"

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$ReleaseExe = Join-Path $RepoRoot "target\release\glossa.exe"
if (-not (Test-Path $ReleaseExe)) {
    Write-Error "Build the release binary first: cargo build --release"
}

$AppLocal = Join-Path $env:LOCALAPPDATA "glossa"
$AppRoam = Join-Path $env:APPDATA "glossa"
$AssetsTray = Join-Path $AppLocal "assets\tray"
$AssetsSounds = Join-Path $AppLocal "assets\sounds"

New-Item -ItemType Directory -Force -Path $AssetsTray, $AssetsSounds, $AppRoam | Out-Null

Copy-Item $ReleaseExe (Join-Path $AppLocal "glossa.exe") -Force
Copy-Item (Join-Path $RepoRoot "contrib\assets\tray\*") $AssetsTray -Force
Copy-Item (Join-Path $RepoRoot "contrib\assets\sounds\*") $AssetsSounds -Force
Copy-Item (Join-Path $RepoRoot "contrib\windows\update.ps1") (Join-Path $AppLocal "update.ps1") -Force

$version = "0.0.0"
try {
    $verText = (& $ReleaseExe --version 2>$null | Out-String).Trim()
    if ($verText -match '(\d+\.\d+\.\d+\S*)$') {
        $version = $Matches[1]
    }
} catch {
    # keep default
}
Set-Content -Path (Join-Path $AppLocal "VERSION") -Value $version -NoNewline

$ConfigPath = Join-Path $AppRoam "config.toml"
if (-not (Test-Path $ConfigPath)) {
    $Example = Get-Content (Join-Path $RepoRoot "contrib\examples\config.toml") -Raw
    $Example = $Example -replace '<user>', $env:USERNAME
    $Example = $Example -replace 'C:\\\\Users\\\\[^\\]+\\\\AppData\\\\Local\\\\glossa', ($AppLocal -replace '\\', '\\')
    # Prefer direct path substitution for icons/sounds
    $Example = $Example `
        -replace 'idle_icon = ".*"', ("idle_icon = `"{0}`"" -f (Join-Path $AssetsTray "idle.png" | ForEach-Object { $_.Replace('\', '\\') })) `
        -replace 'recording_icon = ".*"', ("recording_icon = `"{0}`"" -f (Join-Path $AssetsTray "recording.png" | ForEach-Object { $_.Replace('\', '\\') })) `
        -replace 'processing_icon = ".*"', ("processing_icon = `"{0}`"" -f (Join-Path $AssetsTray "processing.png" | ForEach-Object { $_.Replace('\', '\\') })) `
        -replace 'idle_dark_icon = ".*"', ("idle_dark_icon = `"{0}`"" -f (Join-Path $AssetsTray "idle_dark.png" | ForEach-Object { $_.Replace('\', '\\') })) `
        -replace 'recording_dark_icon = ".*"', ("recording_dark_icon = `"{0}`"" -f (Join-Path $AssetsTray "recording_dark.png" | ForEach-Object { $_.Replace('\', '\\') })) `
        -replace 'processing_dark_icon = ".*"', ("processing_dark_icon = `"{0}`"" -f (Join-Path $AssetsTray "processing_dark.png" | ForEach-Object { $_.Replace('\', '\\') })) `
        -replace 'start_sound = ".*"', ("start_sound = `"{0}`"" -f (Join-Path $AssetsSounds "start.wav" | ForEach-Object { $_.Replace('\', '\\') })) `
        -replace 'stop_sound = ".*"', ("stop_sound = `"{0}`"" -f (Join-Path $AssetsSounds "stop.wav" | ForEach-Object { $_.Replace('\', '\\') }))
    Set-Content -Path $ConfigPath -Value $Example -Encoding UTF8
    Write-Host "Wrote $ConfigPath"
} else {
    Write-Host "Keeping existing $ConfigPath"
}

Write-Host ""
Write-Host "Installed Glossa to $AppLocal"
Write-Host "Config: $ConfigPath"
Write-Host ""
Write-Host "Set GROQ_API_KEY (User environment), then run:"
Write-Host "  & `"$AppLocal\glossa.exe`" --config `"$ConfigPath`" daemon"
Write-Host ""
Write-Host "Optional autostart:"
Write-Host "  & `"$AppLocal\glossa.exe`" service start"
