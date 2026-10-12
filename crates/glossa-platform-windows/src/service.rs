use std::{env, fs, path::PathBuf, process::Command};

use glossa_app::AppError;

const STARTUP_SHORTCUT_NAME: &str = "Glossa.lnk";
const TASK_NAME: &str = "Glossa";

/// Restarts the Glossa autostart entry by stopping and starting the scheduled task if present,
/// otherwise by re-launching the current executable as a daemon.
pub(crate) fn restart_glossa_service() -> Result<(), AppError> {
    if schtasks_restart().is_ok() {
        return Ok(());
    }

    // Fall back: the tray will send Restart through the actor when service restart is unavailable.
    Err(AppError::message(
        "no scheduled task named Glossa was found; use `glossa service restart` after installing autostart",
    ))
}

pub fn start_autostart() -> Result<(), AppError> {
    if let Ok(()) = schtasks_action("run") {
        return Ok(());
    }
    create_startup_shortcut()?;
    Ok(())
}

pub fn stop_autostart() -> Result<(), AppError> {
    let _ = schtasks_action("end");
    remove_startup_shortcut()?;
    Ok(())
}

pub fn restart_autostart() -> Result<(), AppError> {
    let _ = stop_autostart();
    start_autostart()
}

fn schtasks_restart() -> Result<(), AppError> {
    let _ = schtasks_action("end");
    schtasks_action("run")
}

fn schtasks_action(action: &str) -> Result<(), AppError> {
    use std::os::windows::process::CommandExt;

    // CREATE_NO_WINDOW: schtasks is console-attached and would flash a terminal
    // window when spawned from the tray daemon.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let output = Command::new("schtasks")
        .args(["/End", "/TN", TASK_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    if action == "end" {
        // Ending a non-running task is fine.
        let _ = output;
        return Ok(());
    }

    let output = Command::new("schtasks")
        .args(["/Run", "/TN", TASK_NAME])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| AppError::io("failed to invoke schtasks", error))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(AppError::message(if stderr.is_empty() {
            format!("`schtasks /Run /TN {TASK_NAME}` failed")
        } else {
            format!("`schtasks /Run /TN {TASK_NAME}` failed: {stderr}")
        }))
    }
}

fn startup_shortcut_path() -> Result<PathBuf, AppError> {
    let appdata = env::var("APPDATA").map_err(|_| {
        AppError::message("APPDATA is required to manage the Startup folder shortcut")
    })?;
    Ok(PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join(STARTUP_SHORTCUT_NAME))
}

fn create_startup_shortcut() -> Result<(), AppError> {
    let exe = env::current_exe()
        .map_err(|error| AppError::io("failed to resolve current executable", error))?;
    let config = default_config_path()?;
    let shortcut = startup_shortcut_path()?;

    if let Some(parent) = shortcut.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| AppError::io("failed to create Startup folder", error))?;
    }

    // Create a .lnk via PowerShell so we do not depend on COM crates.
    let script = format!(
        r#"$ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut('{shortcut}'); $s.TargetPath = '{exe}'; $s.Arguments = '--config "{config}" daemon'; $s.WorkingDirectory = '{dir}'; $s.Save()"#,
        shortcut = shortcut.display().to_string().replace('\'', "''"),
        exe = exe.display().to_string().replace('\'', "''"),
        config = config.display().to_string().replace('\'', "''"),
        dir = exe
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into())
            .replace('\'', "''"),
    );

    // CREATE_NO_WINDOW: the daemon is a GUI-subsystem binary, so spawning the
    // console-attached powershell.exe would otherwise pop up a console window.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    use std::os::windows::process::CommandExt;

    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|error| AppError::io("failed to invoke powershell for Startup shortcut", error))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(AppError::message(format!(
            "failed to create Startup shortcut: {stderr}"
        )))
    }
}

fn remove_startup_shortcut() -> Result<(), AppError> {
    let shortcut = startup_shortcut_path()?;
    if shortcut.exists() {
        fs::remove_file(&shortcut)
            .map_err(|error| AppError::io("failed to remove Startup shortcut", error))?;
    }
    Ok(())
}

fn default_config_path() -> Result<PathBuf, AppError> {
    let appdata = env::var("APPDATA").map_err(|_| {
        AppError::message("APPDATA is required to resolve the default config path")
    })?;
    Ok(PathBuf::from(appdata)
        .join("glossa")
        .join("config.toml"))
}
