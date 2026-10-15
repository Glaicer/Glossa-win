use std::{
    fs,
    path::{Path, PathBuf},
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
    menu::{CheckMenuItem, ContextMenu, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use glossa_app::{
    ports::{TrayPort, TrayState},
    AppError,
};
use glossa_core::{AppCommand, AppConfig, CommandOrigin, InputMode, UiConfig};

use crate::dialog::{show_confirmation_dialog, show_info_dialog, show_message_dialog};
use crate::hotkey::{parse_shortcut, prompt_hotkey_dialog};
use crate::overlay::UpdateOverlay;
use crate::service::restart_glossa_service;
use crate::updater::{check_for_update, install_update_with_progress, UpdaterStatus};
use crate::winmsg::pump_thread_messages;

use super::file_transcription_dialog::FileTranscriptionUi;
use super::settings::{write_settings_to_config, SettingsValues};
use super::settings_dialog::show_settings_dialog;

const TRAY_THREAD_NAME: &str = "glossa-tray";
const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Best-effort tray port that runs a native Windows notification-area icon.
pub struct BestEffortTrayPort {
    enabled: bool,
    ui: UiConfig,
    shortcut: String,
    input_mode: InputMode,
    settings: SettingsValues,
    file_transcription: FileTranscriptionUi,
    config_path: PathBuf,
    app_command_tx: std::sync::Arc<Mutex<Option<UnboundedSender<AppCommand>>>>,
    command_tx: Mutex<Option<mpsc::Sender<TrayCommand>>>,
    thread_handle: Mutex<Option<thread::JoinHandle<()>>>,
    started: AtomicBool,
}

impl BestEffortTrayPort {
    #[must_use]
    pub fn new(
        config_path: PathBuf,
        config: &AppConfig,
        file_transcription: FileTranscriptionUi,
    ) -> Self {
        Self {
            enabled: config.ui.tray,
            ui: config.ui.clone(),
            shortcut: config.input.shortcut.clone(),
            input_mode: config.input.mode,
            settings: SettingsValues::from_config(config),
            file_transcription,
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
        let file_transcription = self.file_transcription.clone();
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
                    file_transcription,
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

    /// Reloads tray icons from the given UI config (e.g. after theme change + restart).
    pub fn apply_ui_config(&self, ui: UiConfig) {
        self.send_command(TrayCommand::ReloadUi(ui));
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

    async fn show_error(&self, message: &str) -> Result<(), AppError> {
        self.send_command(TrayCommand::ShowError(message.to_owned()));
        Ok(())
    }
}

#[derive(Debug)]
enum TrayCommand {
    SetState(TrayState),
    SetShortcut(String),
    ShowError(String),
    ReloadUi(UiConfig),
}

struct TrayRuntime {
    tray: TrayIcon,
    ui: UiConfig,
    idle_icon: Icon,
    recording_icon: Icon,
    processing_icon: Icon,
    /// Context menu shown on left/right click (kept alive for popup).
    context_menu: Menu,
    transcribe_file_item: MenuItem,
    change_shortcut_item: MenuItem,
    settings_item: MenuItem,
    update_item: MenuItem,
    enhancer_item: CheckMenuItem,
    version_item: MenuItem,
    exit_item: MenuItem,
    settings: SettingsValues,
    file_transcription: FileTranscriptionUi,
    config_path: PathBuf,
    app_command_tx: std::sync::Arc<Mutex<Option<UnboundedSender<AppCommand>>>>,
    popup_hwnd: isize,
}

fn run_tray_thread(
    ui: UiConfig,
    shortcut: String,
    _input_mode: InputMode,
    settings: SettingsValues,
    file_transcription: FileTranscriptionUi,
    config_path: PathBuf,
    app_command_tx: std::sync::Arc<Mutex<Option<UnboundedSender<AppCommand>>>>,
    command_rx: mpsc::Receiver<TrayCommand>,
) {
    let (idle_icon, recording_icon, processing_icon) = match load_tray_icons(&ui) {
        Ok(icons) => icons,
        Err(error) => {
            warn!(error = %error, "failed to load tray icons");
            return;
        }
    };

    let popup_hwnd = match create_popup_owner_window() {
        Ok(hwnd) => hwnd,
        Err(error) => {
            warn!(error = %error, "failed to create tray popup owner window");
            return;
        }
    };

    // --- Right-click context menu ---
    let transcribe_file_item = MenuItem::new("Transcribe File", true, None);
    let change_shortcut_item = MenuItem::new("Change shortcut…", true, None);
    let settings_item = MenuItem::new("Settings", true, None);
    let update_item = MenuItem::new("Update…", true, None);
    let enhancer_item = CheckMenuItem::new("AI enhancer", true, settings.llm_enabled, None);
    let version_item = MenuItem::new(format!("Version {APP_VERSION}"), false, None);
    let exit_item = MenuItem::new("Exit", true, None);

    let context_menu = Menu::new();
    if let Err(error) = context_menu.append_items(&[
        &transcribe_file_item,
        &PredefinedMenuItem::separator(),
        &change_shortcut_item,
        &settings_item,
        &update_item,
        &PredefinedMenuItem::separator(),
        &enhancer_item,
        &PredefinedMenuItem::separator(),
        &version_item,
        &exit_item,
    ]) {
        warn!(error = %error, "failed to build tray context menu");
        destroy_popup_owner_window(popup_hwnd);
        return;
    }

    // No built-in menu: we show the context menu ourselves on left and right click.
    let tray = match TrayIconBuilder::new()
        .with_tooltip("Glossa")
        .with_icon(idle_icon.clone())
        .with_menu_on_left_click(false)
        .build()
    {
        Ok(tray) => tray,
        Err(error) => {
            warn!(error = %error, "failed to create tray icon");
            destroy_popup_owner_window(popup_hwnd);
            return;
        }
    };

    let _ = shortcut; // initial shortcut lives on settings; label updates via SetShortcut

    let mut runtime = TrayRuntime {
        tray,
        ui,
        idle_icon,
        recording_icon,
        processing_icon,
        context_menu,
        transcribe_file_item,
        change_shortcut_item,
        settings_item,
        update_item,
        enhancer_item,
        version_item,
        exit_item,
        settings,
        file_transcription,
        config_path,
        app_command_tx,
        popup_hwnd,
    };

    info!("tray icon started");
    let menu_channel = MenuEvent::receiver();
    let tray_channel = TrayIconEvent::receiver();

    loop {
        // tray-icon delivers clicks via the Win32 message queue on this thread.
        pump_thread_messages();

        while let Ok(command) = command_rx.try_recv() {
            runtime.handle_command(command);
        }

        while let Ok(event) = tray_channel.try_recv() {
            if let TrayIconEvent::Click {
                button,
                button_state: MouseButtonState::Down,
                ..
            } = event
            {
                match button {
                    MouseButton::Left | MouseButton::Right => runtime.show_context_menu(),
                    _ => {}
                }
            }
        }

        while let Ok(event) = menu_channel.try_recv() {
            if runtime.handle_menu_event(&event.id) {
                info!("tray exit requested");
                destroy_popup_owner_window(runtime.popup_hwnd);
                return;
            }
        }

        thread::sleep(Duration::from_millis(16));
    }
}

impl TrayRuntime {
    fn show_context_menu(&self) {
        self.context_menu
            .show_context_menu_for_hwnd(self.popup_hwnd, None);
    }

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
                if !description.is_empty() {
                    self.settings.shortcut = description;
                }
            }
            TrayCommand::ShowError(message) => {
                let _ = show_message_dialog("Glossa", &message);
            }
            TrayCommand::ReloadUi(ui) => {
                self.apply_ui(ui);
            }
        }
    }

    fn apply_ui(&mut self, ui: UiConfig) {
        self.settings.ui_theme = ui.theme;
        self.ui = ui;
        self.file_transcription.set_theme(self.ui.theme);
        self.reload_theme_icons();
    }

    fn reload_theme_icons(&mut self) {
        match load_tray_icons(&self.ui) {
            Ok((idle, recording, processing)) => {
                self.idle_icon = idle;
                self.recording_icon = recording;
                self.processing_icon = processing;
                if let Err(error) = self.tray.set_icon(Some(self.idle_icon.clone())) {
                    warn!(error = %error, "failed to apply reloaded tray icon");
                }
            }
            Err(error) => {
                warn!(error = %error, "failed to reload tray icons for theme");
            }
        }
    }

    fn handle_menu_event(&mut self, id: &tray_icon::menu::MenuId) -> bool {
        if id == self.transcribe_file_item.id() {
            self.file_transcription.show();
            return false;
        }
        if id == self.change_shortcut_item.id() {
            self.handle_change_shortcut();
            return false;
        }
        if id == self.settings_item.id() {
            self.handle_settings();
            return false;
        }
        if id == self.update_item.id() {
            self.handle_update();
            return false;
        }
        if id == self.enhancer_item.id() {
            self.settings.llm_enabled = self.enhancer_item.is_checked();
            self.persist_settings();
            self.request_restart();
            return false;
        }
        if id == self.exit_item.id() {
            self.send_app_command(AppCommand::Shutdown {
                origin: CommandOrigin::TrayMenu,
            });
            return true;
        }
        let _ = self.version_item.id();
        false
    }

    fn handle_settings(&mut self) {
        match show_settings_dialog(&self.settings) {
            Ok(Some(updated)) => {
                let theme_changed = updated.ui_theme != self.settings.ui_theme;
                self.settings = updated;
                self.enhancer_item.set_checked(self.settings.llm_enabled);
                if theme_changed {
                    // Icons are loaded once at tray start; refresh immediately so the
                    // notification-area icon matches the new color scheme without waiting
                    // for a process-level restart (in-process restart keeps this thread).
                    self.ui.theme = self.settings.ui_theme;
                    self.reload_theme_icons();
                }
                self.persist_settings();
                self.request_restart();
            }
            Ok(None) => {}
            Err(error) => {
                let _ = show_message_dialog(
                    "Glossa",
                    &format!("Failed to open Settings:\n{error}"),
                );
            }
        }
    }

    fn handle_change_shortcut(&mut self) {
        let current = self.settings.shortcut.clone();
        let entered = match prompt_hotkey_dialog("Change shortcut", &current) {
            Ok(value) => value,
            Err(error) => {
                let _ = show_message_dialog("Glossa", &format!("Failed to open prompt:\n{error}"));
                return;
            }
        };
        let Some(raw) = entered else {
            return;
        };
        let trimmed = raw.trim().to_owned();
        if trimmed.is_empty() {
            let _ = show_message_dialog("Glossa", "Shortcut cannot be empty.");
            return;
        }
        if let Err(error) = parse_shortcut(&trimmed) {
            let _ = show_message_dialog(
                "Glossa",
                &format!("Invalid shortcut `{trimmed}`:\n{error}"),
            );
            return;
        }
        self.settings.shortcut = trimmed;
        self.persist_settings();
        self.request_restart();
    }

    fn persist_settings(&mut self) {
        if let Err(error) = write_settings_to_config(&self.config_path, &mut self.settings) {
            warn!(error = %error, "failed to write settings to config.toml");
            let _ = show_message_dialog(
                "Glossa",
                &format!("Failed to save settings:\n{error}"),
            );
        }
    }

    fn request_restart(&self) {
        self.send_app_command(AppCommand::Restart {
            origin: CommandOrigin::TrayMenu,
        });
        let _ = restart_glossa_service();
    }

    fn handle_update(&self) {
        let check_result = match check_for_update() {
            Ok(result) => result,
            Err(error) => {
                warn!(error = %error, "failed to check for updates from tray");
                let _ = show_message_dialog("Update", &error.to_string());
                return;
            }
        };

        match check_result.status {
            UpdaterStatus::UpToDate => {
                let _ = show_info_dialog(
                    "Update",
                    &format!(
                        "The latest version is already installed ({}).",
                        check_result.version
                    ),
                );
            }
            UpdaterStatus::Available => {
                let confirmed = show_confirmation_dialog(
                    "Update",
                    &format!(
                        "A new version {} is available.\n\nDo you want to install it now?",
                        check_result.version
                    ),
                );
                if !confirmed {
                    return;
                }

                let overlay =
                    UpdateOverlay::show(&format!("Updating Glossa to {}…", check_result.version));
                let result = install_update_with_progress(&mut |percent| {
                    overlay.set_progress(percent);
                });
                drop(overlay);

                match result {
                    Ok(result) if result.status == UpdaterStatus::Updated => {
                        // The updater's restart helper relaunches the daemon on
                        // its own; no dialog so the restart is seamless.
                        info!(
                            version = %result.version,
                            "update installed; Glossa restarts automatically"
                        );
                    }
                    Ok(result) if result.status == UpdaterStatus::UpToDate => {
                        let _ = show_info_dialog(
                            "Update",
                            "The latest version is already installed.",
                        );
                    }
                    Ok(result) => {
                        warn!(?result, "unexpected updater result after install");
                        let _ = show_message_dialog(
                            "Update",
                            "The updater returned an unexpected result.",
                        );
                    }
                    Err(error) => {
                        warn!(error = %error, "failed to install update from tray");
                        let _ = show_message_dialog("Update", &error.to_string());
                    }
                }
            }
            UpdaterStatus::Updated => {
                warn!("update check unexpectedly returned 'updated' status");
                let _ = show_message_dialog(
                    "Update",
                    "The updater returned an unexpected result.",
                );
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

fn load_tray_icons(ui: &UiConfig) -> Result<(Icon, Icon, Icon), AppError> {
    let idle_icon = load_icon(ui.idle_tray_icon().as_std_path())?;
    let recording_icon = load_icon(ui.recording_tray_icon().as_std_path())?;
    let processing_icon = match load_icon(ui.processing_tray_icon().as_std_path()) {
        Ok(icon) => icon,
        Err(error) => {
            warn!(error = %error, "failed to load processing tray icon; falling back to idle");
            idle_icon.clone()
        }
    };
    Ok((idle_icon, recording_icon, processing_icon))
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

/// Hidden owner window for `TrackPopupMenu` (must not be message-only).
#[cfg(windows)]
fn create_popup_owner_window() -> Result<isize, String> {
    use std::{mem::size_of, os::windows::ffi::OsStrExt, ptr, sync::Once};

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, RegisterClassExW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSEXW, WS_POPUP,
        },
    };

    const CLASS_NAME: &str = "GlossaTrayPopupOwner";

    fn to_wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        let class_name = to_wide(CLASS_NAME);
        // SAFETY: process-local window class registration.
        unsafe {
            let hinstance = GetModuleHandleW(ptr::null());
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(),
                hCursor: ptr::null_mut(),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: ptr::null_mut(),
            };
            let _ = RegisterClassExW(&class);
        }
    });

    let class_name = to_wide(CLASS_NAME);
    // SAFETY: zero-size popup window owns TrackPopupMenu without appearing on-screen.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            to_wide("Glossa Tray").as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        return Err("CreateWindowExW failed for tray popup owner".into());
    }
    Ok(hwnd as isize)
}

#[cfg(not(windows))]
fn create_popup_owner_window() -> Result<isize, String> {
    Ok(0)
}

#[cfg(windows)]
fn destroy_popup_owner_window(hwnd: isize) {
    if hwnd != 0 {
        // SAFETY: hwnd was created by create_popup_owner_window on this thread.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd as _);
        }
    }
}

#[cfg(not(windows))]
fn destroy_popup_owner_window(_hwnd: isize) {}
