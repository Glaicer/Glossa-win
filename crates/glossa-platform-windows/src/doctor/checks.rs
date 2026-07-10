use std::env;

use glossa_app::AppError;
use glossa_core::{AppConfig, InputBackend};

use super::{DoctorFinding, DoctorLevel, DoctorReport};

/// Environment diagnostics for the Windows Glossa runtime.
#[derive(Debug, Clone)]
pub struct Doctor;

impl Doctor {
    pub async fn run(config: &AppConfig) -> Result<DoctorReport, AppError> {
        let findings = vec![
            check_windows(),
            check_hotkey(config),
            check_clipboard(),
            check_paste_backend(config),
            check_tray(config),
            check_ipc(config),
            check_work_dir(config),
            check_config(config),
            check_api_key(config),
        ];

        Ok(DoctorReport { findings })
    }
}

fn check_windows() -> DoctorFinding {
    if cfg!(windows) {
        ok("Windows host", "running on Windows")
    } else {
        warn(
            "Windows host",
            "binary is being inspected on a non-Windows host; runtime checks are limited",
        )
    }
}

fn check_hotkey(config: &AppConfig) -> DoctorFinding {
    match config.input.backend {
        InputBackend::None => warn(
            "Global hotkey",
            "hotkey backend is disabled by configuration",
        ),
        InputBackend::Portal if config.input.shortcut.trim().is_empty() => fail(
            "Global hotkey",
            "input.shortcut is empty; set a shortcut such as Ctrl+Alt+Space",
        ),
        InputBackend::Portal => ok(
            "Global hotkey",
            format!("configured shortcut: {}", config.input.shortcut),
        ),
    }
}

fn check_clipboard() -> DoctorFinding {
    ok(
        "Clipboard backend",
        "native Windows clipboard via arboard",
    )
}

fn check_paste_backend(config: &AppConfig) -> DoctorFinding {
    if config.paste.type_command == "native" || config.paste.type_command.is_empty() {
        ok("Paste backend", "native SendInput keyboard emulation")
    } else {
        warn(
            "Paste backend",
            format!(
                "type_command is `{}`; Windows build uses the native paste backend",
                config.paste.type_command
            ),
        )
    }
}

fn check_tray(config: &AppConfig) -> DoctorFinding {
    if !config.ui.tray {
        warn("Tray backend", "tray is disabled by configuration")
    } else {
        ok(
            "Tray backend",
            "tray-icon is available on Windows notification area",
        )
    }
}

fn check_ipc(config: &AppConfig) -> DoctorFinding {
    match config.control.socket_path.resolve() {
        Ok(endpoint) => ok(
            "IPC endpoint",
            format!("control endpoint resolves to {endpoint}"),
        ),
        Err(error) => fail("IPC endpoint", error.to_string()),
    }
}

fn check_work_dir(config: &AppConfig) -> DoctorFinding {
    match &config.audio.work_dir {
        glossa_core::WorkDir::Auto => {
            let local = env::var("LOCALAPPDATA").ok();
            if let Some(path) = local {
                ok(
                    "Work directory",
                    format!("auto → {path}\\Temp\\glossa"),
                )
            } else {
                warn(
                    "Work directory",
                    "LOCALAPPDATA is unset; will fall back to the process temp directory",
                )
            }
        }
        glossa_core::WorkDir::Path(path) => ok("Work directory", format!("configured path: {path}")),
    }
}

fn check_config(config: &AppConfig) -> DoctorFinding {
    match config.validate() {
        Ok(()) => ok("Configuration", "config validates successfully"),
        Err(error) => fail("Configuration", error.to_string()),
    }
}

fn check_api_key(config: &AppConfig) -> DoctorFinding {
    match config.resolve_api_key() {
        Ok(key) if key.is_empty() => {
            if config.provider.kind == glossa_core::ProviderKind::OpenAiCompatible {
                warn(
                    "API key",
                    "API key is empty (allowed for openai-compatible providers)",
                )
            } else {
                fail("API key", "API key resolved to an empty value")
            }
        }
        Ok(_) => ok(
            "API key",
            format!(
                "resolved from {}",
                config.provider.api_key.describe()
            ),
        ),
        Err(error) => fail("API key", error.to_string()),
    }
}

fn ok(name: impl Into<String>, detail: impl Into<String>) -> DoctorFinding {
    DoctorFinding {
        level: DoctorLevel::Ok,
        name: name.into(),
        detail: detail.into(),
    }
}

fn warn(name: impl Into<String>, detail: impl Into<String>) -> DoctorFinding {
    DoctorFinding {
        level: DoctorLevel::Warn,
        name: name.into(),
        detail: detail.into(),
    }
}

fn fail(name: impl Into<String>, detail: impl Into<String>) -> DoctorFinding {
    DoctorFinding {
        level: DoctorLevel::Fail,
        name: name.into(),
        detail: detail.into(),
    }
}
