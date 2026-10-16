use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context};
use tokio::fs;
use tracing_subscriber::EnvFilter;

use glossa_app::{
    ports::{OverlayPort, TrayPort},
    services::file_transcription::FileTranscriptionService,
    AppActor, AppDependencies,
};
use glossa_audio::{CpalAudioCapture, CuePlayerBackend, WavSilenceTrimmer};
use glossa_core::{AppConfig, LogLevel};
use glossa_platform_windows::{
    clipboard::NativeClipboard,
    credentials::{install_default_store, migrate_secrets_from_environment},
    overlay::BestEffortOverlayPort,
    paste::NativePasteBackend,
    temp::WindowsTempStore,
    tray::{BestEffortTrayPort, FileTranscriptionUi},
};
use glossa_stt::{build_client, build_text_enhancer};

/// Loads the Glossa config from disk, installs the credential store, and migrates
/// any API keys still living in `HKCU\Environment` into Credential Manager.
pub async fn load_config(path: &PathBuf) -> anyhow::Result<AppConfig> {
    let content = fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read config from {}", path.display()))?;
    let mut config = AppConfig::from_toml_str(&content)?;
    install_default_store();
    migrate_secrets_from_environment(path, &mut config)
        .with_context(|| format!("failed to migrate secrets for {}", path.display()))?;
    Ok(config)
}

/// Applies runtime guards that keep a bad optional feature from blocking startup.
///
/// Currently: if the LLM enhancer is on but its API key cannot be resolved,
/// the enhancer is switched off (empty keys for local models stay allowed).
pub fn prepare_runtime_config(mut config: AppConfig) -> AppConfig {
    if config.llm.disable_if_api_key_unavailable() {
        tracing::warn!(
            "LLM enhancer was enabled but its API key is unavailable; turning the enhancer off"
        );
    }
    config
}

/// Loads a config from disk when provided, otherwise falls back to the default config.
pub async fn load_config_or_default(path: Option<PathBuf>) -> anyhow::Result<AppConfig> {
    match path {
        Some(path) => load_config(&path).await,
        None => Ok(AppConfig::default()),
    }
}

/// Initializes tracing based on the config logging level and console sink flag.
pub fn init_tracing(config: &AppConfig) -> anyhow::Result<()> {
    let filter = if config.logging.console {
        match config.logging.level {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    } else {
        "off"
    };
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_target(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|error| anyhow!("failed to initialize tracing: {error}"))
}

/// Builds the best-effort tray port from config, including the Transcribe File
/// window's service. Must run inside the tokio runtime.
pub fn build_tray(config_path: &PathBuf, config: &AppConfig) -> anyhow::Result<Arc<BestEffortTrayPort>> {
    let service = Arc::new(FileTranscriptionService::new(
        build_client(config)?,
        build_text_enhancer(config)?,
        config.llm.enabled,
    ));
    let file_transcription = FileTranscriptionUi::new(
        service,
        Arc::new(NativeClipboard::new()),
        tokio::runtime::Handle::current(),
    );
    Ok(Arc::new(BestEffortTrayPort::new(
        config_path.clone(),
        config,
        file_transcription,
    )))
}

/// Builds the recording overlay shown while dictation is active.
pub fn build_overlay(config: &AppConfig) -> Arc<BestEffortOverlayPort> {
    Arc::new(BestEffortOverlayPort::new(config))
}

/// Builds the app actor and its dependencies from a validated config.
pub fn build_actor(
    config: AppConfig,
    tray: Arc<dyn TrayPort>,
    overlay: Arc<dyn OverlayPort>,
) -> anyhow::Result<(AppActor, glossa_app::AppHandle)> {
    let config = prepare_runtime_config(config);
    let temp_store = Arc::new(WindowsTempStore::from_audio_config(&config.audio)?);
    let deps = AppDependencies {
        audio_capture: Arc::new(CpalAudioCapture::new()),
        trimmer: Arc::new(WavSilenceTrimmer::new(config.audio.trim_threshold)),
        cue_player: Arc::new(CuePlayerBackend::from_config(
            config.audio.enabled,
            config.ui.start_sound.clone(),
            config.ui.stop_sound.clone(),
        )),
        stt_client: build_client(&config)?,
        text_enhancer: build_text_enhancer(&config)?,
        clipboard: Arc::new(NativeClipboard::new()),
        paste: Arc::new(NativePasteBackend::new()),
        tray,
        overlay,
        temp_store,
    };
    Ok(AppActor::new(config, deps))
}

/// Resolves the IPC endpoint from an optional config file.
pub async fn resolve_socket_path(
    config_path: Option<PathBuf>,
) -> anyhow::Result<camino::Utf8PathBuf> {
    let config = load_config_or_default(config_path).await?;
    Ok(config.control.socket_path.resolve()?)
}
