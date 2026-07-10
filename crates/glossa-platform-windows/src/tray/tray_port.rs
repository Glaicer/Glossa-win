use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex,
    },
    thread,
    time::Duration,
};

use async_trait::async_trait;
use png::{ColorType, Decoder};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{info, warn};
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

use glossa_app::{
    ports::{TrayPort, TrayState},
    AppError,
};
use glossa_core::{AppCommand, AppConfig, CommandOrigin, InputMode, UiConfig};

use crate::dialog::show_message_dialog;
use crate::service::restart_glossa_service;
use crate::updater::{check_for_update, install_update, UpdaterStatus};

use super::settings::{write_settings_to_config, SettingsValues};

const TRAY_THREAD_NAME: &str = "glossa-tray";

/// Best-effort tray port that runs a native Windows notification-area icon.
pub struct BestEffortTrayPort {
    enabled: bool,
    ui: UiConfig,
    shortcut: String,
    input_mode: InputMode,
    settings: SettingsValues,
    config_path: PathBuf,
    app_command_tx: std::sync::Arc<Mutex<Option<UnboundedSender<AppCommand>>>>,
    command_tx: Mutex<Option<mpsc::Sender<TrayCommand>>>,
    thread_handle: Mutex<Option<thread::JoinHandle<()>>>,
    started: AtomicBool,
}

impl BestEffortTrayPort {
    #[must_use]
    pub fn new(config_path: PathBuf, config: &AppConfig) -> Self {
        Self {
            enabled: config.ui.tray,
            ui: config.ui.clone(),
            shortcut: config.input.shortcut.clone(),
            input_mode: config.input.mode,
            settings: SettingsValues::from_config(config),
            config_path,
            app_command_tx: std::sync::Arc::new(Mutex::new(None)),
            command_tx: Mutex::new(None),
            thread_handle: Mutex::new(None),
            started: AtomicBool::new(false),
        }
    }

    pub fn bind_command_sender(&self, app_command_tx: UnboundedSender<AppCommand>) {
        if !self.enabled {
            info!("tray is disabled by configuration");
            return;
        }

        match self.app_command_tx.lock() {
            Ok(mut guard) => {
                *guard = Some(app_command_tx);
            }
            Err(_) => {
                warn!("tray app command mutex is poisoned; skipping tray initialization");
                return;
            }
        }

        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }

        let (command_tx, command_rx) = mpsc::channel();
        match self.command_tx.lock() {
            Ok(mut guard) => {
                *guard = Some(command_tx);
            }
            Err(_) => {
                warn!("tray command mutex is poisoned; skipping tray initialization");
                self.started.store(false, Ordering::SeqCst);
                return;
            }
        }

        let ui = self.ui.clone();
        let shortcut = self.shortcut.clone();
        let input_mode = self.input_mode;
        let settings = self.settings.clone();
        let config_path = self.config_path.clone();
        let app_command_tx = self.app_command_tx.clone();
        let handle = match thread::Builder::new()
            .name(TRAY_THREAD_NAME.into())
            .spawn(move || {
                run_tray_thread(
                    ui,
                    shortcut,
                    input_mode,
                    settings,
                    config_path,
                    app_command_tx,
                    command_rx,
                );
            }) {
            Ok(handle) => handle,
            Err(error) => {
                warn!(error = %error, "failed to spawn tray thread");
                self.started.store(false, Ordering::SeqCst);
                if let Ok(mut guard) = self.command_tx.lock() {
                    *guard = None;
                }
                return;
            }
        };

        match self.thread_handle.lock() {
            Ok(mut guard) => {
                *guard = Some(handle);
            }
            Err(_) => {
                warn!("tray thread handle mutex is poisoned");
                self.started.store(false, Ordering::SeqCst);
            }
        }
    }

    fn send_command(&self, command: TrayCommand) {
        let sender = match self.command_tx.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => {
                warn!("tray command mutex is poisoned");
                None
            }
        };
        if let Some(sender) = sender {
            if let Err(error) = sender.send(command) {
                warn!(error = %error, "failed to send tray command");
            }
        }
    }
}

#[async_trait]
impl TrayPort for BestEffortTrayPort {
    async fn set_state(&self, state: TrayState) -> Result<(), AppError> {
        self.send_command(TrayCommand::SetState(state));
        Ok(())
    }

    async fn set_shortcut_description(&self, description: Option<&str>) -> Result<(), AppError> {
        self.send_command(TrayCommand::SetShortcut(
            description.unwrap_or_default().to_owned(),
        ));
        Ok(())
    }

    async fn set_mic_stream_state(&self, active: bool) -> Result<(), AppError> {
        self.send_command(TrayCommand::SetMicStream(active));
        Ok(())
    }

    async fn show_error(&self, message: &str) -> Result<(), AppError> {
        self.send_command(TrayCommand::ShowError(message.to_owned()));
        Ok(())
    }
}

#[derive(Debug)]
enum TrayCommand {
    SetState(TrayState),
    SetShortcut(String),
    SetMicStream(bool),
    ShowError(String),
}

struct TrayRuntime {
    tray: TrayIcon,
    idle_icon: Icon,
    recording_icon: Icon,
    processing_icon: Icon,
    toggle_item: MenuItem,
    mic_stream_item: CheckMenuItem,
    enhancer_item: CheckMenuItem,
    append_space_item: CheckMenuItem,
    mode_toggle_item: CheckMenuItem,
    mode_ptt_item: CheckMenuItem,
    shortcut_item: MenuItem,
    open_config_item: MenuItem,
    update_item: MenuItem,
    quit_item: MenuItem,
    settings: SettingsValues,
    config_path: PathBuf,
    app_command_tx: std::sync::Arc<Mutex<Option<UnboundedSender<AppCommand>>>>,
}

fn run_tray_thread(
    ui: UiConfig,
    shortcut: String,
    input_mode: InputMode,
    settings: SettingsValues,
    config_path: PathBuf,
    app_command_tx: std::sync::Arc<Mutex<Option<UnboundedSender<AppCommand>>>>,
    command_rx: mpsc::Receiver<TrayCommand>,
) {
    let idle_icon = match load_icon(ui.idle_tray_icon().as_std_path()) {
        Ok(icon) => icon,
        Err(error) => {
            warn!(error = %error, "failed to load idle tray icon");
            return;
        }
    };
    let recording_icon = match load_icon(ui.recording_tray_icon().as_std_path()) {
        Ok(icon) => icon,
        Err(error) => {
            warn!(error = %error, "failed to load recording tray icon");
            return;
        }
    };
    let processing_icon = match load_icon(ui.processing_tray_icon().as_std_path()) {
        Ok(icon) => icon,
        Err(error) => {
            warn!(error = %error, "failed to load processing tray icon; falling back to idle");
            idle_icon.clone()
        }
    };

    let toggle_item = MenuItem::new("Toggle recording", true, None);
    let mic_stream_item = CheckMenuItem::new(
        "Mic stream",
        true,
        settings.latency_mode != glossa_core::LatencyMode::Off,
        None,
    );
    let enhancer_item = CheckMenuItem::new("AI enhancer", true, settings.llm_enabled, None);
    let append_space_item = CheckMenuItem::new("Append space", true, settings.append_space, None);
    let mode_toggle_item = CheckMenuItem::new(
        "Mode: Toggle",
        true,
        input_mode == InputMode::Toggle,
        None,
    );
    let mode_ptt_item = CheckMenuItem::new(
        "Mode: Push-to-talk",
        true,
        input_mode == InputMode::PushToTalk,
        None,
    );
    let shortcut_label = if shortcut.is_empty() {
        "Shortcut: (not set)".to_owned()
    } else {
        format!("Shortcut: {shortcut}")
    };
    let shortcut_item = MenuItem::new(shortcut_label, false, None);
    let open_config_item = MenuItem::new("Open config…", true, None);
    let update_item = MenuItem::new("Update…", true, None);
    let quit_item = MenuItem::new("Quit", true, None);

    let menu = Menu::new();
    if let Err(error) = menu.append_items(&[
        &toggle_item,
        &PredefinedMenuItem::separator(),
        &mic_stream_item,
        &enhancer_item,
        &append_space_item,
        &PredefinedMenuItem::separator(),
        &mode_toggle_item,
        &mode_ptt_item,
        &shortcut_item,
        &PredefinedMenuItem::separator(),
        &open_config_item,
        &update_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ]) {
        warn!(error = %error, "failed to build tray menu items");
        return;
    }

    let tray = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Glossa")
        .with_icon(idle_icon.clone())
        .build()
    {
        Ok(tray) => tray,
        Err(error) => {
            warn!(error = %error, "failed to create tray icon");
            return;
        }
    };

    let mut runtime = TrayRuntime {
        tray,
        idle_icon,
        recording_icon,
        processing_icon,
        toggle_item,
        mic_stream_item,
        enhancer_item,
        append_space_item,
        mode_toggle_item,
        mode_ptt_item,
        shortcut_item,
        open_config_item,
        update_item,
        quit_item,
        settings,
        config_path,
        app_command_tx,
    };

    info!("tray icon started");
    let menu_channel = MenuEvent::receiver();

    loop {
        while let Ok(command) = command_rx.try_recv() {
            runtime.handle_command(command);
        }

        while let Ok(event) = menu_channel.try_recv() {
            if runtime.handle_menu_event(&event.id) {
                info!("tray quit requested");
                return;
            }
        }

        thread::sleep(Duration::from_millis(50));
    }
}

impl TrayRuntime {
    fn handle_command(&mut self, command: TrayCommand) {
        match command {
            TrayCommand::SetState(state) => {
                let icon = match state {
                    TrayState::Idle => &self.idle_icon,
                    TrayState::Recording => &self.recording_icon,
                    TrayState::Processing => &self.processing_icon,
                };
                if let Err(error) = self.tray.set_icon(Some(icon.clone())) {
                    warn!(error = %error, "failed to update tray icon");
                }
                let tooltip = match state {
                    TrayState::Idle => "Glossa — idle",
                    TrayState::Recording => "Glossa — recording",
                    TrayState::Processing => "Glossa — processing",
                };
                let _ = self.tray.set_tooltip(Some(tooltip));
            }
            TrayCommand::SetShortcut(description) => {
                let label = if description.is_empty() {
                    "Shortcut: (not set)".to_owned()
                } else {
                    format!("Shortcut: {description}")
                };
                self.shortcut_item.set_text(label);
            }
            TrayCommand::SetMicStream(active) => {
                self.mic_stream_item.set_checked(active);
            }
            TrayCommand::ShowError(message) => {
                let _ = show_message_dialog("Glossa", &message);
            }
        }
    }

    fn handle_menu_event(&mut self, id: &tray_icon::menu::MenuId) -> bool {
        if id == self.toggle_item.id() {
            self.send_app_command(AppCommand::ToggleRecording {
                origin: CommandOrigin::TrayMenu,
            });
            return false;
        }
        if id == self.mic_stream_item.id() {
            self.send_app_command(AppCommand::ToggleInputStream {
                origin: CommandOrigin::TrayMenu,
            });
            return false;
        }
        if id == self.enhancer_item.id() {
            self.settings.llm_enabled = self.enhancer_item.is_checked();
            self.persist_settings();
            self.request_restart();
            return false;
        }
        if id == self.append_space_item.id() {
            self.settings.append_space = self.append_space_item.is_checked();
            self.persist_settings();
            self.request_restart();
            return false;
        }
        if id == self.mode_toggle_item.id() {
            self.settings.input_mode = InputMode::Toggle;
            self.mode_toggle_item.set_checked(true);
            self.mode_ptt_item.set_checked(false);
            self.persist_settings();
            self.request_restart();
            return false;
        }
        if id == self.mode_ptt_item.id() {
            self.settings.input_mode = InputMode::PushToTalk;
            self.mode_toggle_item.set_checked(false);
            self.mode_ptt_item.set_checked(true);
            self.persist_settings();
            self.request_restart();
            return false;
        }
        if id == self.open_config_item.id() {
            open_config(&self.config_path);
            return false;
        }
        if id == self.update_item.id() {
            self.handle_update();
            return false;
        }
        if id == self.quit_item.id() {
            self.send_app_command(AppCommand::Shutdown {
                origin: CommandOrigin::TrayMenu,
            });
            return true;
        }
        false
    }

    fn persist_settings(&self) {
        if let Err(error) = write_settings_to_config(&self.config_path, &self.settings) {
            warn!(error = %error, "failed to write settings to config.toml");
            let _ = show_message_dialog(
                "Glossa",
                &format!("Failed to save settings:\n{error}"),
            );
        }
    }

    fn request_restart(&self) {
        // Prefer a soft in-process restart via the actor.
        self.send_app_command(AppCommand::Restart {
            origin: CommandOrigin::TrayMenu,
        });
        // Also try restarting a registered service / scheduled task when present.
        let _ = restart_glossa_service();
    }

    fn handle_update(&self) {
        match check_for_update() {
            Ok(UpdaterStatus::UpToDate { version }) => {
                let _ = show_message_dialog(
                    "Glossa",
                    &format!("Glossa is up to date ({version})."),
                );
            }
            Ok(UpdaterStatus::UpdateAvailable { current, latest }) => {
                let _ = show_message_dialog(
                    "Glossa",
                    &format!("Update available: {current} → {latest}. Starting updater…"),
                );
                match install_update() {
                    Ok(status) if status.success() => {
                        let _ = show_message_dialog("Glossa", "Update installed.");
                    }
                    Ok(status) => {
                        let _ = show_message_dialog(
                            "Glossa",
                            &format!("Updater exited with status {status}"),
                        );
                    }
                    Err(error) => {
                        let _ = show_message_dialog(
                            "Glossa",
                            &format!("Update failed:\n{error}"),
                        );
                    }
                }
            }
            Ok(UpdaterStatus::Unavailable { reason }) => {
                let _ = show_message_dialog("Glossa", &reason);
            }
            Err(error) => {
                let _ = show_message_dialog("Glossa", &format!("Update check failed:\n{error}"));
            }
        }
    }

    fn send_app_command(&self, command: AppCommand) {
        let sender = match self.app_command_tx.lock() {
            Ok(guard) => guard.clone(),
            Err(_) => None,
        };
        if let Some(sender) = sender {
            if let Err(error) = sender.send(command) {
                warn!(error = %error, "failed to send app command from tray");
            }
        }
    }
}

fn open_config(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Prefer opening the file; fall back to the containing folder.
    #[cfg(windows)]
    {
        let _ = Command::new("cmd")
            .args(["/C", "start", "", &path.display().to_string()])
            .spawn();
        return;
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(path).spawn();
    }
}

fn load_icon(path: &Path) -> Result<Icon, AppError> {
    let file = fs::File::open(path).map_err(|error| {
        AppError::message(format!(
            "failed to open icon {}: {error}",
            path.display()
        ))
    })?;
    let decoder = Decoder::new(file);
    let mut reader = decoder.read_info().map_err(|error| {
        AppError::message(format!(
            "failed to read PNG {}: {error}",
            path.display()
        ))
    })?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut buffer).map_err(|error| {
        AppError::message(format!(
            "failed to decode PNG {}: {error}",
            path.display()
        ))
    })?;

    let rgba = match frame.color_type {
        ColorType::Rgba => buffer[..frame.buffer_size()].to_vec(),
        ColorType::Rgb => {
            let rgb = &buffer[..frame.buffer_size()];
            let mut rgba = Vec::with_capacity(rgb.len() / 3 * 4);
            for chunk in rgb.chunks_exact(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        ColorType::Grayscale => {
            let gray = &buffer[..frame.buffer_size()];
            let mut rgba = Vec::with_capacity(gray.len() * 4);
            for &value in gray {
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
            rgba
        }
        ColorType::GrayscaleAlpha => {
            let ga = &buffer[..frame.buffer_size()];
            let mut rgba = Vec::with_capacity(ga.len() / 2 * 4);
            for chunk in ga.chunks_exact(2) {
                rgba.extend_from_slice(&[chunk[0], chunk[0], chunk[0], chunk[1]]);
            }
            rgba
        }
        other => {
            return Err(AppError::message(format!(
                "unsupported icon color type {other:?} in {}",
                path.display()
            )));
        }
    };

    Icon::from_rgba(rgba, frame.width, frame.height)
        .map_err(|error| AppError::message(format!("failed to build tray icon: {error}")))
}
