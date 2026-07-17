//! Native Win32 first-run setup dialog.

use glossa_app::AppError;

use super::install::{apply_setup, start_daemon_detached, SetupResult};

/// Result of showing the setup wizard UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardOutcome {
    /// User completed setup successfully.
    Completed(SetupResult),
    /// User cancelled the wizard.
    Cancelled,
}

/// Runs the interactive first-run wizard and applies the chosen setup.
pub fn run_setup_wizard() -> Result<WizardOutcome, AppError> {
    #[cfg(windows)]
    {
        run_setup_wizard_windows()
    }
    #[cfg(not(windows))]
    {
        Err(AppError::message(
            "the Glossa setup wizard is only available on Windows",
        ))
    }
}

#[cfg(windows)]
fn run_setup_wizard_windows() -> Result<WizardOutcome, AppError> {
    let answers = match show_wizard_dialog()? {
        Some(answers) => answers,
        None => return Ok(WizardOutcome::Cancelled),
    };

    let result = apply_setup(&answers)?;
    if result.start_now {
        start_daemon_detached(&result.exe_path, &result.config_path)?;
    }

    let summary = format!(
        "Glossa is ready.\n\n\
         Config: {}\n\
         Install: {}\n\n\
         Hotkey: {}\n\
         Hold or press the hotkey, speak, and the transcription is pasted into the focused window.\n\
         Esc cancels an in-progress recording.{}",
        result.config_path.display(),
        result.install_dir.display(),
        answers.shortcut,
        if result.start_now {
            "\n\nThe daemon has been started (tray icon)."
        } else {
            "\n\nStart later with:\n  glossa daemon"
        }
    );
    show_info_dialog("Glossa Setup", &summary);

    Ok(WizardOutcome::Completed(result))
}

#[cfg(windows)]
fn show_info_dialog(title: &str, message: &str) {
    use std::os::windows::ffi::OsStrExt;

    fn to_wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let title_wide = to_wide(title);
    let message_wide = to_wide(message);
    // SAFETY: NUL-terminated wide strings; no owner window required.
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
            std::ptr::null_mut(),
            message_wide.as_ptr(),
            title_wide.as_ptr(),
            windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
        );
    }
}

#[cfg(windows)]
mod ui {
    use std::{
        cell::RefCell,
        mem::size_of,
        os::windows::ffi::OsStrExt,
        ptr,
        sync::atomic::{AtomicI32, Ordering},
    };

    use glossa_app::AppError;
    use glossa_core::{InputMode, ProviderKind, UiTheme};
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        Graphics::Gdi::{
            CreateFontW, DeleteObject, GetStockObject, UpdateWindow, CLEARTYPE_QUALITY,
            CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_GUI_FONT, FW_NORMAL, HFONT,
            OUT_DEFAULT_PRECIS,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::BST_CHECKED,
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                GetWindowTextLengthW, GetWindowTextW, LoadCursorW, PostQuitMessage, RegisterClassExW,
                SendMessageW, SetWindowLongPtrW, SetWindowTextW, ShowWindow, TranslateMessage,
                BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_AUTORADIOBUTTON,
                BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CBN_SELCHANGE, CBS_DROPDOWNLIST, CB_ADDSTRING,
                CB_GETCURSEL, CB_SETCURSEL, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, EN_CHANGE,
                ES_AUTOHSCROLL, ES_PASSWORD, GWLP_USERDATA, IDC_ARROW, MSG, SW_SHOW, WM_CLOSE,
                WM_COMMAND, WM_CREATE, WM_DESTROY, WM_SETFONT, WNDCLASSEXW, WS_BORDER, WS_CAPTION,
                WS_CHILD, WS_GROUP, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
            },
        },
    };

    use crate::setup::install::{default_base_url, default_model, validate_answers, SetupAnswers};

    const CLASS_NAME: &str = "GlossaSetupWizard";
    const WIN_WIDTH: i32 = 520;
    const WIN_HEIGHT: i32 = 560;

    const ID_PROVIDER: i32 = 1001;
    const ID_API_KEY: i32 = 1002;
    const ID_BASE_URL: i32 = 1003;
    const ID_MODEL: i32 = 1004;
    const ID_SHORTCUT: i32 = 1005;
    const ID_MODE_PTT: i32 = 1006;
    const ID_MODE_TOGGLE: i32 = 1007;
    const ID_THEME_DARK: i32 = 1008;
    const ID_THEME_LIGHT: i32 = 1009;
    const ID_AUTOSTART: i32 = 1010;
    const ID_START_NOW: i32 = 1011;
    const ID_INSTALL: i32 = 1012;
    const ID_CANCEL: i32 = 1013;
    const ID_LBL_BASE: i32 = 1014;

    thread_local! {
        static WIZARD_STATE: RefCell<Option<WizardState>> = const { RefCell::new(None) };
    }

    static NEXT_CLASS_ID: AtomicI32 = AtomicI32::new(1);

    struct WizardState {
        font: HFONT,
        provider: HWND,
        api_key: HWND,
        base_url: HWND,
        model: HWND,
        shortcut: HWND,
        mode_ptt: HWND,
        theme_dark: HWND,
        autostart: HWND,
        start_now: HWND,
        result: Option<SetupAnswers>,
        cancelled: bool,
    }

    pub(super) fn show_wizard_dialog() -> Result<Option<SetupAnswers>, AppError> {
        // SAFETY: Win32 window class registration and message pump for a modal-style dialog.
        unsafe { show_wizard_dialog_impl() }
    }

    unsafe fn show_wizard_dialog_impl() -> Result<Option<SetupAnswers>, AppError> {
        let class_atom = {
            let unique = NEXT_CLASS_ID.fetch_add(1, Ordering::Relaxed);
            let class_name = format!("{CLASS_NAME}{unique}");
            let class_wide = to_wide(&class_name);
            let hinstance = GetModuleHandleW(ptr::null());

            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                // COLOR_WINDOW+1 is the idiomatic brush constant for dialog backgrounds.
                hbrBackground: (windows_sys::Win32::Graphics::Gdi::COLOR_WINDOW as isize + 1)
                    as windows_sys::Win32::Graphics::Gdi::HBRUSH,
                lpszMenuName: ptr::null(),
                lpszClassName: class_wide.as_ptr(),
                hIconSm: ptr::null_mut(),
            };

            let atom = RegisterClassExW(&wc);
            if atom == 0 {
                return Err(AppError::message(
                    "failed to register Glossa setup window class",
                ));
            }
            (class_wide, hinstance)
        };

        let (class_wide, hinstance) = class_atom;
        let title = to_wide("Glossa Setup");
        let hwnd = CreateWindowExW(
            0,
            class_wide.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIN_WIDTH,
            WIN_HEIGHT,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );

        if hwnd.is_null() {
            return Err(AppError::message("failed to create Glossa setup window"));
        }

        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        let mut msg = MSG {
            hwnd: ptr::null_mut(),
            message: 0,
            wParam: 0,
            lParam: 0,
            time: 0,
            pt: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
        };

        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        WIZARD_STATE.with(|cell| {
            let state = cell.borrow_mut().take();
            match state {
                Some(state) if !state.cancelled => Ok(state.result),
                _ => Ok(None),
            }
        })
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                if let Err(error) = on_create(hwnd) {
                    show_error(&error.to_string());
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_COMMAND => {
                let id = loword(wparam as usize) as i32;
                let code = hiword(wparam as usize) as u32;
                on_command(hwnd, id, code);
                0
            }
            WM_CLOSE => {
                WIZARD_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.cancelled = true;
                    }
                });
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                WIZARD_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        if !state.font.is_null() {
                            DeleteObject(state.font);
                            state.font = ptr::null_mut();
                        }
                    }
                });
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn on_create(hwnd: HWND) -> Result<(), AppError> {
        let font = CreateFontW(
            -16,
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            0,
            to_wide("Segoe UI").as_ptr(),
        );
        let font = if font.is_null() {
            GetStockObject(DEFAULT_GUI_FONT) as HFONT
        } else {
            font
        };

        let mut y = 16i32;
        let label_w = 110i32;
        let field_x = 130i32;
        let field_w = 350i32;
        let row_h = 28i32;
        let gap = 10i32;

        create_static(hwnd, 16, y, 480, 40, "Welcome to Glossa. Configure speech-to-text and install tray assets.", 2000, font);
        y += 48;

        create_static(hwnd, 16, y + 4, label_w, 20, "Provider", 2001, font);
        let provider = create_combo(hwnd, field_x, y, field_w, 200, ID_PROVIDER, font);
        for label in ["Groq (recommended)", "OpenAI", "OpenAI-compatible"] {
            let wide = to_wide(label);
            SendMessageW(provider, CB_ADDSTRING, 0, wide.as_ptr() as LPARAM);
        }
        SendMessageW(provider, CB_SETCURSEL, 0, 0);
        y += row_h + gap;

        create_static(hwnd, 16, y + 4, label_w, 20, "API key", 2002, font);
        let api_key = create_edit(hwnd, field_x, y, field_w, 24, ID_API_KEY, font, true);
        y += row_h + gap;

        create_static(hwnd, 16, y + 4, label_w, 20, "Base URL", ID_LBL_BASE, font);
        let base_url = create_edit(hwnd, field_x, y, field_w, 24, ID_BASE_URL, font, false);
        set_text(base_url, default_base_url(ProviderKind::Groq));
        y += row_h + gap;

        create_static(hwnd, 16, y + 4, label_w, 20, "Model", 2003, font);
        let model = create_edit(hwnd, field_x, y, field_w, 24, ID_MODEL, font, false);
        set_text(model, default_model(ProviderKind::Groq));
        y += row_h + gap;

        create_static(hwnd, 16, y + 4, label_w, 20, "Hotkey", 2004, font);
        let shortcut = create_edit(hwnd, field_x, y, field_w, 24, ID_SHORTCUT, font, false);
        set_text(shortcut, "Ctrl+Alt+Space");
        y += row_h + gap + 4;

        create_static(hwnd, 16, y, label_w, 20, "Mode", 2005, font);
        let mode_ptt = create_radio(
            hwnd,
            field_x,
            y,
            160,
            22,
            ID_MODE_PTT,
            "Push-to-talk",
            font,
            true,
            true,
        );
        let _mode_toggle = create_radio(
            hwnd,
            field_x + 170,
            y,
            120,
            22,
            ID_MODE_TOGGLE,
            "Toggle",
            font,
            false,
            false,
        );
        y += row_h + gap;

        create_static(hwnd, 16, y, label_w, 20, "Tray theme", 2006, font);
        let theme_dark = create_radio(
            hwnd,
            field_x,
            y,
            100,
            22,
            ID_THEME_DARK,
            "Dark",
            font,
            true,
            true,
        );
        let _theme_light = create_radio(
            hwnd,
            field_x + 110,
            y,
            100,
            22,
            ID_THEME_LIGHT,
            "Light",
            font,
            false,
            false,
        );
        y += row_h + gap + 6;

        let autostart = create_checkbox(
            hwnd,
            16,
            y,
            360,
            22,
            ID_AUTOSTART,
            "Start Glossa when I log in to Windows",
            font,
            true,
        );
        y += row_h + 4;
        let start_now = create_checkbox(
            hwnd,
            16,
            y,
            360,
            22,
            ID_START_NOW,
            "Start Glossa now after setup",
            font,
            true,
        );
        y += row_h + 24;

        create_button(hwnd, 250, y, 100, 30, ID_CANCEL, "Cancel", font, false);
        create_button(hwnd, 370, y, 110, 30, ID_INSTALL, "Install", font, true);

        enable_window(base_url, false);

        let state = WizardState {
            font,
            provider,
            api_key,
            base_url,
            model,
            shortcut,
            mode_ptt,
            theme_dark,
            autostart,
            start_now,
            result: None,
            cancelled: false,
        };

        WIZARD_STATE.with(|cell| {
            *cell.borrow_mut() = Some(state);
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 1);
        SetFocus(api_key);
        Ok(())
    }

    unsafe fn on_command(hwnd: HWND, id: i32, code: u32) {
        match id {
            ID_PROVIDER if code == CBN_SELCHANGE => {
                WIZARD_STATE.with(|cell| {
                    if let Some(state) = cell.borrow().as_ref() {
                        let kind = provider_from_combo(state.provider);
                        set_text(state.base_url, default_base_url(kind));
                        set_text(state.model, default_model(kind));
                        enable_window(state.base_url, kind == ProviderKind::OpenAiCompatible);
                    }
                });
            }
            ID_INSTALL if code == BN_CLICKED || code == 0 => {
                match collect_answers() {
                    Ok(answers) => {
                        if let Err(error) = validate_answers(&answers) {
                            show_error(&error.to_string());
                            return;
                        }
                        WIZARD_STATE.with(|cell| {
                            if let Some(state) = cell.borrow_mut().as_mut() {
                                state.result = Some(answers);
                                state.cancelled = false;
                            }
                        });
                        DestroyWindow(hwnd);
                    }
                    Err(error) => show_error(&error.to_string()),
                }
            }
            ID_CANCEL if code == BN_CLICKED || code == 0 => {
                WIZARD_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.cancelled = true;
                        state.result = None;
                    }
                });
                DestroyWindow(hwnd);
            }
            ID_API_KEY if code == EN_CHANGE => {}
            _ => {}
        }
    }

    fn collect_answers() -> Result<SetupAnswers, AppError> {
        WIZARD_STATE.with(|cell| {
            let state_ref = cell.borrow();
            let state = state_ref
                .as_ref()
                .ok_or_else(|| AppError::message("setup wizard state missing"))?;
            // SAFETY: control HWNDs created in this process.
            unsafe {
                let provider = provider_from_combo(state.provider);
                let mode = if is_checked(state.mode_ptt) {
                    InputMode::PushToTalk
                } else {
                    InputMode::Toggle
                };
                let theme = if is_checked(state.theme_dark) {
                    UiTheme::Dark
                } else {
                    UiTheme::Light
                };
                Ok(SetupAnswers {
                    provider,
                    api_key: get_text(state.api_key),
                    base_url: get_text(state.base_url),
                    model: get_text(state.model),
                    shortcut: get_text(state.shortcut),
                    mode,
                    theme,
                    autostart: is_checked(state.autostart),
                    start_now: is_checked(state.start_now),
                })
            }
        })
    }

    unsafe fn provider_from_combo(combo: HWND) -> ProviderKind {
        match SendMessageW(combo, CB_GETCURSEL, 0, 0) {
            1 => ProviderKind::OpenAi,
            2 => ProviderKind::OpenAiCompatible,
            _ => ProviderKind::Groq,
        }
    }

    unsafe fn create_static(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        text: &str,
        id: i32,
        font: HFONT,
    ) -> HWND {
        let hwnd = CreateWindowExW(
            0,
            to_wide("STATIC").as_ptr(),
            to_wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        hwnd
    }

    unsafe fn create_edit(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: i32,
        font: HFONT,
        password: bool,
    ) -> HWND {
        let mut style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER | ES_AUTOHSCROLL as u32;
        if password {
            style |= ES_PASSWORD as u32;
        }
        let hwnd = CreateWindowExW(
            0,
            to_wide("EDIT").as_ptr(),
            to_wide("").as_ptr(),
            style,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        hwnd
    }

    unsafe fn create_combo(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: i32,
        font: HFONT,
    ) -> HWND {
        let hwnd = CreateWindowExW(
            0,
            to_wide("COMBOBOX").as_ptr(),
            ptr::null(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | CBS_DROPDOWNLIST as u32,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        hwnd
    }

    unsafe fn create_radio(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: i32,
        text: &str,
        font: HFONT,
        group: bool,
        checked: bool,
    ) -> HWND {
        let mut style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTORADIOBUTTON as u32;
        if group {
            style |= WS_GROUP;
        }
        let hwnd = CreateWindowExW(
            0,
            to_wide("BUTTON").as_ptr(),
            to_wide(text).as_ptr(),
            style,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        if checked {
            SendMessageW(hwnd, BM_SETCHECK, BST_CHECKED as WPARAM, 0);
        }
        hwnd
    }

    unsafe fn create_checkbox(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: i32,
        text: &str,
        font: HFONT,
        checked: bool,
    ) -> HWND {
        let hwnd = CreateWindowExW(
            0,
            to_wide("BUTTON").as_ptr(),
            to_wide(text).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_AUTOCHECKBOX as u32,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        if checked {
            SendMessageW(hwnd, BM_SETCHECK, BST_CHECKED as WPARAM, 0);
        }
        hwnd
    }

    unsafe fn create_button(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: i32,
        text: &str,
        font: HFONT,
        default: bool,
    ) -> HWND {
        let style = if default {
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32
        } else {
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32
        };
        let hwnd = CreateWindowExW(
            0,
            to_wide("BUTTON").as_ptr(),
            to_wide(text).as_ptr(),
            style,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        hwnd
    }

    unsafe fn set_text(hwnd: HWND, text: &str) {
        SetWindowTextW(hwnd, to_wide(text).as_ptr());
    }

    unsafe fn get_text(hwnd: HWND) -> String {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; (len + 1) as usize];
        let read = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if read <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..read as usize])
    }

    unsafe fn is_checked(hwnd: HWND) -> bool {
        let state = SendMessageW(hwnd, BM_GETCHECK, 0, 0);
        state == BST_CHECKED as isize
    }

    unsafe fn enable_window(hwnd: HWND, enabled: bool) {
        EnableWindow(hwnd, i32::from(enabled));
    }

    fn show_error(message: &str) {
        let title = to_wide("Glossa Setup");
        let body = to_wide(message);
        // SAFETY: NUL-terminated wide strings.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                ptr::null_mut(),
                body.as_ptr(),
                title.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
    }

    fn to_wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn loword(value: usize) -> u16 {
        (value & 0xFFFF) as u16
    }

    fn hiword(value: usize) -> u16 {
        ((value >> 16) & 0xFFFF) as u16
    }
}

#[cfg(windows)]
use ui::show_wizard_dialog;
