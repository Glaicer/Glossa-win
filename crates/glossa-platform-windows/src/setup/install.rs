use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use glossa_app::AppError;
use glossa_core::{
    AppConfig, InputBackend, InputConfig, InputMode, ProviderConfig, ProviderKind, SecretSource,
    UiConfig, UiTheme,
};

use super::assets::EMBEDDED_ASSETS;
use super::paths::{default_config_path, default_install_dir};

/// User answers collected by the setup wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupAnswers {
    pub provider: ProviderKind,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub shortcut: String,
    pub mode: InputMode,
    pub theme: UiTheme,
    pub autostart: bool,
    pub start_now: bool,
}

impl Default for SetupAnswers {
    fn default() -> Self {
        Self {
            provider: ProviderKind::Groq,
            api_key: String::new(),
            base_url: default_base_url(ProviderKind::Groq).to_owned(),
            model: default_model(ProviderKind::Groq).to_owned(),
            shortcut: "Ctrl+Alt+Space".into(),
            mode: InputMode::Toggle,
            theme: UiTheme::Light,
            autostart: true,
            start_now: true,
        }
    }
}

/// Outcome of applying setup answers to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupResult {
    pub install_dir: PathBuf,
    pub config_path: PathBuf,
    pub exe_path: PathBuf,
    pub autostart: bool,
    pub start_now: bool,
}

/// Default base URL for a provider kind.
#[must_use]
pub fn default_base_url(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Groq => "https://api.groq.com/openai/v1",
        ProviderKind::OpenAi => "https://api.openai.com/v1",
        ProviderKind::OpenAiCompatible => "http://127.0.0.1:8000/v1",
    }
}

/// Default transcription model for a provider kind.
#[must_use]
pub fn default_model(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Groq => "whisper-large-v3-turbo",
        ProviderKind::OpenAi => "gpt-4o-mini-transcribe",
        ProviderKind::OpenAiCompatible => "whisper-1",
    }
}

/// Environment variable name used for the provider API key.
#[must_use]
pub fn api_key_env_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Groq => "GROQ_API_KEY",
        ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => "OPENAI_API_KEY",
    }
}

/// Validates wizard answers before writing files.
pub fn validate_answers(answers: &SetupAnswers) -> Result<(), AppError> {
    if answers.model.trim().is_empty() {
        return Err(AppError::message("Model must not be empty."));
    }
    if answers.shortcut.trim().is_empty() {
        return Err(AppError::message("Shortcut must not be empty."));
    }
    crate::hotkey::parse_shortcut(answers.shortcut.trim()).map_err(|error| {
        AppError::message(format!("Invalid hotkey: {error}"))
    })?;
    if answers.provider == ProviderKind::OpenAiCompatible && answers.base_url.trim().is_empty() {
        return Err(AppError::message(
            "Base URL is required for OpenAI-compatible providers.",
        ));
    }
    if answers.provider != ProviderKind::OpenAiCompatible && answers.api_key.trim().is_empty() {
        return Err(AppError::message("API key is required for this provider."));
    }
    Ok(())
}

/// Builds a validated `AppConfig` from wizard answers and install paths.
pub fn build_app_config(answers: &SetupAnswers, install_dir: &Path) -> Result<AppConfig, AppError> {
    validate_answers(answers)?;

    let assets = install_dir.join("assets");
    let tray = assets.join("tray");
    let sounds = assets.join("sounds");
    let env_name = api_key_env_name(answers.provider);

    let base_url = {
        let trimmed = answers.base_url.trim();
        if trimmed.is_empty() {
            Some(default_base_url(answers.provider).to_owned())
        } else {
            Some(trimmed.to_owned())
        }
    };

    let api_key = if answers.api_key.trim().is_empty() {
        SecretSource::Empty
    } else {
        SecretSource::Env(env_name.to_owned())
    };

    let utf8 = |path: PathBuf| -> Result<camino::Utf8PathBuf, AppError> {
        camino::Utf8PathBuf::from_path_buf(path).map_err(|path| {
            AppError::message(format!(
                "install path is not valid UTF-8: {}",
                path.display()
            ))
        })
    };

    let config = AppConfig {
        input: InputConfig {
            backend: InputBackend::Hotkey,
            shortcut: answers.shortcut.trim().to_owned(),
            mode: answers.mode,
        },
        provider: ProviderConfig {
            kind: answers.provider,
            base_url,
            model: answers.model.trim().to_owned(),
            api_key,
        },
        ui: UiConfig {
            tray: true,
            theme: answers.theme,
            idle_icon: utf8(tray.join("idle.png"))?,
            recording_icon: utf8(tray.join("recording.png"))?,
            processing_icon: Some(utf8(tray.join("processing.png"))?),
            idle_dark_icon: Some(utf8(tray.join("idle_dark.png"))?),
            recording_dark_icon: Some(utf8(tray.join("recording_dark.png"))?),
            processing_dark_icon: Some(utf8(tray.join("processing_dark.png"))?),
            start_sound: utf8(sounds.join("start.wav"))?,
            stop_sound: utf8(sounds.join("stop.wav"))?,
        },
        ..AppConfig::default()
    };

    config
        .validate()
        .map_err(|error| AppError::message(error.to_string()))?;
    Ok(config)
}

/// Serializes wizard answers into a `config.toml` document.
pub fn build_config_toml(answers: &SetupAnswers, install_dir: &Path) -> Result<String, AppError> {
    let config = build_app_config(answers, install_dir)?;
    toml::to_string_pretty(&config)
        .map_err(|error| AppError::message(format!("failed to serialize config: {error}")))
}

/// Installs Glossa files, writes config, sets the API key env var, and optionally enables autostart.
pub fn apply_setup(answers: &SetupAnswers) -> Result<SetupResult, AppError> {
    validate_answers(answers)?;

    let install_dir = default_install_dir()?;
    let config_path = default_config_path()?;
    let exe_path = install_dir.join("glossa.exe");

    fs::create_dir_all(install_dir.join("assets").join("tray"))
        .map_err(|error| AppError::io("failed to create tray assets directory", error))?;
    fs::create_dir_all(install_dir.join("assets").join("sounds"))
        .map_err(|error| AppError::io("failed to create sound assets directory", error))?;
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| AppError::io("failed to create config directory", error))?;
    }

    for asset in EMBEDDED_ASSETS {
        let dest = install_dir.join("assets").join(asset.relative_path);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| AppError::io("failed to create asset directory", error))?;
        }
        fs::write(&dest, asset.bytes)
            .map_err(|error| AppError::io("failed to write embedded asset", error))?;
    }

    copy_current_exe_to(&exe_path)?;

    fs::write(
        install_dir.join("VERSION"),
        env!("CARGO_PKG_VERSION").as_bytes(),
    )
    .map_err(|error| AppError::io("failed to write VERSION file", error))?;

    // Bootstrapper that fetches the versioned updater from GitHub releases.
    const UPDATE_PS1: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contrib/windows/update.ps1"
    ));
    fs::write(install_dir.join("update.ps1"), UPDATE_PS1)
        .map_err(|error| AppError::io("failed to write update.ps1", error))?;

    let config_toml = build_config_toml(answers, &install_dir)?;
    fs::write(&config_path, config_toml)
        .map_err(|error| AppError::io("failed to write config.toml", error))?;

    let env_name = api_key_env_name(answers.provider);
    if !answers.api_key.trim().is_empty() {
        set_user_environment_variable(env_name, answers.api_key.trim())?;
        // So "start now" in this process can resolve env:GROQ_API_KEY immediately.
        // SAFETY: single-threaded setup path; no other threads rely on this env yet.
        unsafe {
            env::set_var(env_name, answers.api_key.trim());
        }
    }

    if answers.autostart {
        create_startup_shortcut(&exe_path, &config_path)?;
    }

    Ok(SetupResult {
        install_dir,
        config_path,
        exe_path,
        autostart: answers.autostart,
        start_now: answers.start_now,
    })
}

/// Spawns the installed daemon detached from the setup process.
pub fn start_daemon_detached(exe_path: &Path, config_path: &Path) -> Result<(), AppError> {
    use std::os::windows::process::CommandExt;

    // CREATE_NO_WINDOW: do not allocate a console for the child (silent tray daemon).
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    Command::new(exe_path)
        .arg("--config")
        .arg(config_path)
        .arg("daemon")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| AppError::io("failed to start Glossa daemon", error))?;
    Ok(())
}

fn copy_current_exe_to(dest: &Path) -> Result<(), AppError> {
    let current = env::current_exe()
        .map_err(|error| AppError::io("failed to resolve current executable", error))?;

    if same_file(&current, dest) {
        return Ok(());
    }

    // On Windows, overwriting a running exe can fail; write to a temp name then replace.
    let tmp = dest.with_extension("exe.new");
    fs::copy(&current, &tmp)
        .map_err(|error| AppError::io("failed to copy glossa.exe into install directory", error))?;
    if dest.exists() {
        let bak = dest.with_extension("exe.bak");
        let _ = fs::remove_file(&bak);
        let _ = fs::rename(dest, &bak);
    }
    fs::rename(&tmp, dest)
        .map_err(|error| AppError::io("failed to finalize glossa.exe install", error))?;
    let _ = fs::remove_file(dest.with_extension("exe.bak"));
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn create_startup_shortcut(exe: &Path, config: &Path) -> Result<(), AppError> {
    let appdata = env::var("APPDATA").map_err(|_| {
        AppError::message("APPDATA is required to create the Startup folder shortcut")
    })?;
    let shortcut = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Startup")
        .join("Glossa.lnk");

    if let Some(parent) = shortcut.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| AppError::io("failed to create Startup folder", error))?;
    }

    let working_dir = exe
        .parent()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".into());

    let script = format!(
        r#"$ws = New-Object -ComObject WScript.Shell; $s = $ws.CreateShortcut('{shortcut}'); $s.TargetPath = '{exe}'; $s.Arguments = '--config "{config}" daemon'; $s.WorkingDirectory = '{dir}'; $s.Save()"#,
        shortcut = shortcut.display().to_string().replace('\'', "''"),
        exe = exe.display().to_string().replace('\'', "''"),
        config = config.display().to_string().replace('\'', "''"),
        dir = working_dir.replace('\'', "''"),
    );

    let output = Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
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

/// Writes a user-scoped environment variable (`HKCU\Environment`) and broadcasts the change.
pub(crate) fn set_user_environment_variable(name: &str, value: &str) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        set_user_environment_variable_windows(name, value)
    }
    #[cfg(not(windows))]
    {
        let _ = (name, value);
        Err(AppError::message(
            "setting user environment variables is only supported on Windows",
        ))
    }
}

#[cfg(windows)]
fn set_user_environment_variable_windows(name: &str, value: &str) -> Result<(), AppError> {
    use std::os::windows::ffi::OsStrExt;

    fn to_wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let name_wide = to_wide(name);
    let value_wide = to_wide(value);
    let sub_key = to_wide("Environment");

    // SAFETY: Registry APIs receive valid NUL-terminated wide strings and a stack-allocated handle.
    unsafe {
        let mut key = std::ptr::null_mut();
        let status = windows_sys::Win32::System::Registry::RegOpenKeyExW(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            sub_key.as_ptr(),
            0,
            windows_sys::Win32::System::Registry::KEY_SET_VALUE,
            &mut key,
        );
        if status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
            return Err(AppError::message(format!(
                "failed to open HKCU\\Environment (error {status})"
            )));
        }

        let bytes = (value_wide.len() * 2) as u32;
        let status = windows_sys::Win32::System::Registry::RegSetValueExW(
            key,
            name_wide.as_ptr(),
            0,
            windows_sys::Win32::System::Registry::REG_SZ,
            value_wide.as_ptr().cast::<u8>(),
            bytes,
        );
        let _ = windows_sys::Win32::System::Registry::RegCloseKey(key);
        if status != windows_sys::Win32::Foundation::ERROR_SUCCESS {
            return Err(AppError::message(format!(
                "failed to set user environment variable {name} (error {status})"
            )));
        }

        // Notify other processes that environment variables changed.
        let env_wide = to_wide("Environment");
        windows_sys::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
            windows_sys::Win32::UI::WindowsAndMessaging::HWND_BROADCAST,
            windows_sys::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE,
            0,
            env_wide.as_ptr() as isize,
            windows_sys::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
            5000,
            std::ptr::null_mut(),
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::InputMode;

    #[test]
    fn default_answers_should_target_groq_toggle_light() {
        let answers = SetupAnswers::default();
        assert_eq!(answers.provider, ProviderKind::Groq);
        assert_eq!(answers.mode, InputMode::Toggle);
        assert_eq!(answers.theme, UiTheme::Light);
        assert_eq!(answers.model, "whisper-large-v3-turbo");
        assert!(answers.autostart);
        assert!(answers.start_now);
    }

    #[test]
    fn validate_answers_should_require_api_key_for_groq() {
        let mut answers = SetupAnswers::default();
        answers.api_key.clear();
        let error = validate_answers(&answers).expect_err("api key required");
        assert!(error.to_string().contains("API key"));
    }

    #[test]
    fn validate_answers_should_allow_empty_key_for_compatible() {
        let mut answers = SetupAnswers::default();
        answers.provider = ProviderKind::OpenAiCompatible;
        answers.api_key.clear();
        answers.base_url = "http://127.0.0.1:9000/v1".into();
        answers.model = "whisper-1".into();
        assert!(validate_answers(&answers).is_ok());
    }

    #[test]
    fn build_config_toml_should_reference_install_assets_and_env_key() {
        let mut answers = SetupAnswers::default();
        answers.api_key = "gsk_test".into();
        let install = PathBuf::from(r"C:\Users\test\AppData\Local\glossa");
        let toml = build_config_toml(&answers, &install).expect("toml");
        assert!(toml.contains("kind = \"groq\""));
        assert!(toml.contains("api_key = \"env:GROQ_API_KEY\""));
        assert!(toml.contains("whisper-large-v3-turbo"));
        assert!(
            toml.contains("toggle")
                || toml.contains("Toggle")
                || toml.contains("\"toggle\"")
        );
        assert!(toml.contains("light") || toml.contains("Light") || toml.contains("\"light\""));
        assert!(toml.contains("idle.png"));
        assert!(toml.contains("start.wav"));
    }

    #[test]
    fn api_key_env_name_should_match_provider() {
        assert_eq!(api_key_env_name(ProviderKind::Groq), "GROQ_API_KEY");
        assert_eq!(api_key_env_name(ProviderKind::OpenAi), "OPENAI_API_KEY");
        assert_eq!(
            api_key_env_name(ProviderKind::OpenAiCompatible),
            "OPENAI_API_KEY"
        );
    }
}
