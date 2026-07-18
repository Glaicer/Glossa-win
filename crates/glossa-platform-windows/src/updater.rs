use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Output, Stdio},
};

use glossa_app::AppError;

const UPDATER_SCRIPT_NAME: &str = "update.ps1";
const STATUS_PREFIX: &str = "GLOSSA_UPDATE_STATUS=";
const VERSION_PREFIX: &str = "GLOSSA_UPDATE_VERSION=";
const EMBEDDED_UPDATE_PS1: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../contrib/windows/update.ps1"
));

/// High-level status reported by the local Windows updater scripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdaterStatus {
    UpToDate,
    Available,
    Updated,
}

/// Parsed machine-readable result from `update.ps1 check|install`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdaterResult {
    pub status: UpdaterStatus,
    pub version: String,
}

#[must_use]
fn updater_script_candidates(current_exe: &Path, local_app_data: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(parent) = current_exe.parent() {
        candidates.push(parent.join(UPDATER_SCRIPT_NAME));
    }

    if let Some(local_app_data) = local_app_data {
        let fallback = local_app_data.join("glossa").join(UPDATER_SCRIPT_NAME);
        if !candidates.iter().any(|candidate| candidate == &fallback) {
            candidates.push(fallback);
        }
    }

    candidates
}

fn find_local_updater_script_with<F>(
    current_exe: &Path,
    local_app_data: Option<&Path>,
    exists: F,
) -> Result<PathBuf, AppError>
where
    F: Fn(&Path) -> bool,
{
    updater_script_candidates(current_exe, local_app_data)
        .into_iter()
        .find(|candidate| exists(candidate))
        .ok_or_else(|| {
            AppError::message(
                "could not find update.ps1 next to the glossa binary or in %LOCALAPPDATA%\\glossa",
            )
        })
}

/// Resolves the installed bootstrapper script (`update.ps1`).
///
/// If no script is present yet (older installs), writes the embedded bootstrapper
/// next to the current executable or under `%LOCALAPPDATA%\glossa`.
pub fn find_local_updater_script() -> Result<PathBuf, AppError> {
    let current_exe = env::current_exe()
        .map_err(|error| AppError::io("failed to resolve current executable", error))?;
    let local_app_data = env::var_os("LOCALAPPDATA").map(PathBuf::from);

    if let Ok(path) =
        find_local_updater_script_with(&current_exe, local_app_data.as_deref(), Path::is_file)
    {
        return Ok(path);
    }

    ensure_embedded_updater_script(&current_exe, local_app_data.as_deref())
}

fn ensure_embedded_updater_script(
    current_exe: &Path,
    local_app_data: Option<&Path>,
) -> Result<PathBuf, AppError> {
    let dest = if let Some(parent) = current_exe.parent() {
        parent.join(UPDATER_SCRIPT_NAME)
    } else if let Some(local_app_data) = local_app_data {
        local_app_data.join("glossa").join(UPDATER_SCRIPT_NAME)
    } else {
        return Err(AppError::message(
            "could not find a directory to write update.ps1",
        ));
    };

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| AppError::io("failed to create updater directory", error))?;
    }
    fs::write(&dest, EMBEDDED_UPDATE_PS1)
        .map_err(|error| AppError::io("failed to write embedded update.ps1", error))?;
    Ok(dest)
}

fn run_powershell_script(script: &Path, args: &[&str]) -> Result<Output, AppError> {
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| AppError::io("failed to run local updater", error))
}

/// Runs the local updater bootstrapper with no extra args (defaults to install).
pub fn run_local_updater() -> Result<ExitStatus, AppError> {
    let updater_path = find_local_updater_script()?;
    let output = run_powershell_script(&updater_path, &[])?;
    Ok(output.status)
}

fn parse_updater_result(stdout: &str) -> Option<UpdaterResult> {
    let mut status = None;
    let mut version = None;

    for line in stdout.lines() {
        if let Some(raw_status) = line.strip_prefix(STATUS_PREFIX) {
            status = match raw_status.trim() {
                "up-to-date" => Some(UpdaterStatus::UpToDate),
                "available" => Some(UpdaterStatus::Available),
                "updated" => Some(UpdaterStatus::Updated),
                _ => None,
            };
            continue;
        }

        if let Some(raw_version) = line.strip_prefix(VERSION_PREFIX) {
            version = Some(raw_version.trim().to_owned());
        }
    }

    Some(UpdaterResult {
        status: status?,
        version: version?,
    })
}

fn command_failed_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();

    if !stderr.is_empty() {
        return stderr;
    }

    if !stdout.is_empty() {
        return stdout;
    }

    format!("updater exited with status {}", output.status)
}

fn run_updater_command(args: &[&str]) -> Result<UpdaterResult, AppError> {
    let updater_path = find_local_updater_script()?;
    let output = run_powershell_script(&updater_path, args)?;

    if !output.status.success() {
        return Err(AppError::message(command_failed_message(&output)));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_updater_result(&stdout)
        .ok_or_else(|| AppError::message("updater output did not contain a valid result"))
}

/// Checks GitHub releases (via the installed bootstrapper) for a newer version.
pub fn check_for_update() -> Result<UpdaterResult, AppError> {
    run_updater_command(&["check"])
}

/// Downloads and installs the latest GitHub release via the local updater scripts.
pub fn install_update() -> Result<UpdaterResult, AppError> {
    run_updater_command(&["install"])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        parse_updater_result, updater_script_candidates, UpdaterResult, UpdaterStatus,
    };

    #[test]
    fn updater_candidates_should_prefer_current_executable_directory() {
        let candidates = updater_script_candidates(
            Path::new(r"C:\Program Files\glossa\glossa.exe"),
            Some(Path::new(r"C:\Users\tester\AppData\Local")),
        );

        assert_eq!(
            candidates,
            vec![
                Path::new(r"C:\Program Files\glossa\update.ps1").to_path_buf(),
                Path::new(r"C:\Users\tester\AppData\Local\glossa\update.ps1").to_path_buf(),
            ]
        );
    }

    #[test]
    fn updater_candidates_should_deduplicate_localappdata_fallback() {
        let candidates = updater_script_candidates(
            Path::new(r"C:\Users\tester\AppData\Local\glossa\glossa.exe"),
            Some(Path::new(r"C:\Users\tester\AppData\Local")),
        );

        assert_eq!(
            candidates,
            vec![Path::new(r"C:\Users\tester\AppData\Local\glossa\update.ps1").to_path_buf()]
        );
    }

    #[test]
    fn parse_updater_result_should_extract_up_to_date_status() {
        let result = parse_updater_result(
            "Downloading glossa-update.ps1\nGLOSSA_UPDATE_STATUS=up-to-date\nGLOSSA_UPDATE_VERSION=0.9.0\n",
        );

        assert_eq!(
            result,
            Some(UpdaterResult {
                status: UpdaterStatus::UpToDate,
                version: "0.9.0".into(),
            })
        );
    }

    #[test]
    fn parse_updater_result_should_extract_available_status() {
        let result = parse_updater_result(
            "GLOSSA_UPDATE_STATUS=available\nGLOSSA_UPDATE_VERSION=0.10.0\n",
        );

        assert_eq!(
            result,
            Some(UpdaterResult {
                status: UpdaterStatus::Available,
                version: "0.10.0".into(),
            })
        );
    }

    #[test]
    fn parse_updater_result_should_extract_updated_status() {
        let result = parse_updater_result(
            "Updating Glossa from 0.8.0 to 0.9.0\nGLOSSA_UPDATE_STATUS=updated\nGLOSSA_UPDATE_VERSION=0.9.0\n",
        );

        assert_eq!(
            result,
            Some(UpdaterResult {
                status: UpdaterStatus::Updated,
                version: "0.9.0".into(),
            })
        );
    }
}
