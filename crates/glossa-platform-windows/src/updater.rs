use std::{
    env, fs,
    path::PathBuf,
    process::{Command, ExitStatus},
};

use glossa_app::AppError;

/// Result of checking for an available update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdaterStatus {
    UpToDate { version: String },
    UpdateAvailable { current: String, latest: String },
    Unavailable { reason: String },
}

/// Runs the local Windows updater script/binary if present.
pub fn run_local_updater() -> Result<ExitStatus, AppError> {
    let updater = find_local_updater()?;
    Command::new(&updater)
        .status()
        .map_err(|error| AppError::io("failed to run local updater", error))
}

/// Checks whether a newer release appears available.
///
/// The Windows port currently reports the installed version and points users at
/// GitHub releases; a full auto-updater can be wired later.
pub fn check_for_update() -> Result<UpdaterStatus, AppError> {
    let version = read_installed_version().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    Ok(UpdaterStatus::Unavailable {
        reason: format!(
            "automatic updates are not fully wired on Windows yet (installed {version}); download the latest release from GitHub"
        ),
    })
}

/// Installs an update by invoking the local updater when available.
pub fn install_update() -> Result<ExitStatus, AppError> {
    run_local_updater()
}

fn find_local_updater() -> Result<PathBuf, AppError> {
    let candidates = [
        env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("glossa-update.ps1"))),
        env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("glossa").join("glossa-update.ps1")),
        env::var_os("APPDATA").map(|p| PathBuf::from(p).join("glossa").join("glossa-update.ps1")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    Err(AppError::message(
        "no local updater found; install glossa-update.ps1 next to the binary or under %LOCALAPPDATA%\\glossa",
    ))
}

fn read_installed_version() -> Option<String> {
    let paths = [
        env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("glossa").join("VERSION")),
        env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("VERSION"))),
    ];

    for path in paths.into_iter().flatten() {
        if let Ok(content) = fs::read_to_string(path) {
            let version = content.trim();
            if !version.is_empty() {
                return Some(version.to_owned());
            }
        }
    }
    None
}
