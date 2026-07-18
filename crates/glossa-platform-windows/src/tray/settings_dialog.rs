//! Native Win32 Settings dialog (tray menu → Settings).
//!
//! Uses Common Controls v6 visual styles (via the app manifest) and a
//! modern card-style layout instead of classic Windows 95 chrome.

use glossa_app::AppError;

use super::settings::SettingsValues;

/// Shows the Settings dialog. Returns `Some` when the user clicks Save.
pub(super) fn show_settings_dialog(
    initial: &SettingsValues,
) -> Result<Option<SettingsValues>, AppError> {
    #[cfg(windows)]
    {
        ui::show_settings_dialog(initial)
    }
    #[cfg(not(windows))]
    {
        let _ = initial;
        Err(AppError::message(
            "the Glossa settings dialog is only available on Windows",
        ))
    }
}

#[cfg(windows)]
mod ui {
    use std::{
        cell::RefCell,
        mem::size_of,
        os::windows::ffi::OsStrExt,
        ptr,
        sync::atomic::{AtomicBool, AtomicI32, Ordering},
    };

    use glossa_app::AppError;
    use glossa_core::{InputBackend, InputMode, PasteMode, ProviderKind, UiTheme};
    use windows_sys::Win32::{
        Foundation::{
            COLORREF, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM, INVALID_HANDLE_VALUE,
        },
        Graphics::{
            Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE},
            Gdi::{
                CreateFontW, CreateSolidBrush, DeleteObject, GetStockObject, SetBkColor,
                SetBkMode, SetTextColor, UpdateWindow, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS,
                DEFAULT_CHARSET, DEFAULT_GUI_FONT, FW_NORMAL, FW_SEMIBOLD, HBRUSH, HFONT,
                OUT_DEFAULT_PRECIS, TRANSPARENT,
            },
        },
        System::{
            ApplicationInstallationAndServicing::{
                ActivateActCtx, CreateActCtxW, DeactivateActCtx, ReleaseActCtx, ACTCTXW,
            },
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            Controls::{
                InitCommonControlsEx, BST_CHECKED, EM_SETCUEBANNER, ICC_STANDARD_CLASSES,
                ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX,
            },
            Input::KeyboardAndMouse::{EnableWindow, SetFocus},
            WindowsAndMessaging::{
                AdjustWindowRectEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
                DispatchMessageW, GetClientRect, GetMessageW, GetSystemMetrics, GetWindowRect,
                GetWindowTextLengthW, GetWindowTextW, LoadCursorW, MessageBoxW, PostQuitMessage,
                RegisterClassExW, SendMessageW, SetWindowPos, SetWindowTextW, ShowWindow,
                TranslateMessage, BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTOCHECKBOX,
                BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CBS_DROPDOWNLIST, CB_ADDSTRING, CB_GETCURSEL,
                CB_SETCURSEL, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, ES_AUTOHSCROLL, ES_PASSWORD,
                IDC_ARROW, MB_ICONERROR, MB_OK, MSG, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SWP_NOMOVE,
                SWP_NOZORDER, SWP_NOSIZE, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT,
                WM_CTLCOLORSTATIC, WM_DESTROY, WM_ERASEBKGND, WM_SETFONT, WNDCLASSEXW, WS_CAPTION,
                WS_CHILD, WS_EX_CLIENTEDGE, WS_EX_CONTROLPARENT, WS_OVERLAPPED, WS_SYSMENU,
                WS_TABSTOP, WS_VISIBLE,
            },
        },
    };

    use super::super::settings::keep_or_replace_llm_api_key;
    use super::SettingsValues;

    const CLASS_NAME: &str = "GlossaSettingsDialog";
    const LLM_CLASS_NAME: &str = "GlossaLlmSettingsDialog";
    /// Desired *client* area width (outer frame is added via AdjustWindowRectEx).
    const WIN_WIDTH: i32 = 480;
    /// Initial outer height placeholder; resized to content after layout.
    const WIN_HEIGHT: i32 = 720;
    const LLM_WIN_WIDTH: i32 = 460;
    const LLM_WIN_HEIGHT: i32 = 380;
    const WINDOW_STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    const WINDOW_EX_STYLE: u32 = WS_EX_CONTROLPARENT;

    const ID_BACKEND: i32 = 1001;
    const ID_MODE: i32 = 1002;
    const ID_ENABLE_CLI: i32 = 1003;
    const ID_PROVIDER_KIND: i32 = 1004;
    const ID_BASE_URL: i32 = 1005;
    const ID_MODEL: i32 = 1006;
    const ID_API_KEY: i32 = 1007;
    const ID_PASTE_MODE: i32 = 1008;
    const ID_APPEND_SPACE: i32 = 1009;
    const ID_THEME: i32 = 1010;
    const ID_LLM: i32 = 1011;
    const ID_CANCEL: i32 = 1012;
    const ID_SAVE: i32 = 1013;

    const ID_LLM_ENABLED: i32 = 1101;
    const ID_LLM_BASE_URL: i32 = 1102;
    const ID_LLM_MODEL: i32 = 1103;
    const ID_LLM_API_KEY: i32 = 1104;
    const ID_LLM_CANCEL: i32 = 1105;
    const ID_LLM_OK: i32 = 1106;

    // Light palette (Windows 11 settings-inspired).
    const LIGHT_BG: COLORREF = rgb(243, 243, 243);
    const LIGHT_TEXT: COLORREF = rgb(28, 28, 28);
    const LIGHT_EDIT_BG: COLORREF = rgb(255, 255, 255);

    // Dark palette.
    const DARK_BG: COLORREF = rgb(32, 32, 32);
    const DARK_TEXT: COLORREF = rgb(250, 250, 250);
    const DARK_EDIT_BG: COLORREF = rgb(45, 45, 45);

    thread_local! {
        static STATE: RefCell<Option<DialogState>> = const { RefCell::new(None) };
        static LLM_STATE: RefCell<Option<LlmDialogState>> = const { RefCell::new(None) };
    }

    static NEXT_CLASS_ID: AtomicI32 = AtomicI32::new(1);
    /// Nested LLM dialog is open; used instead of PostQuitMessage so the
    /// parent Settings message loop is not terminated.
    static LLM_DIALOG_OPEN: AtomicBool = AtomicBool::new(false);

    struct DialogState {
        font: HFONT,
        header_font: HFONT,
        bg_brush: HBRUSH,
        edit_brush: HBRUSH,
        dark: bool,
        backend: HWND,
        mode: HWND,
        enable_cli: HWND,
        provider_kind: HWND,
        base_url: HWND,
        model: HWND,
        api_key: HWND,
        paste_mode: HWND,
        append_space: HWND,
        theme: HWND,
        /// LLM fields edited via the LLM enhancer sub-dialog.
        llm_enabled: bool,
        llm_base_url: String,
        llm_model: String,
        llm_api_key: String,
        /// Preserved (not shown in this dialog).
        shortcut: String,
        result: Option<SettingsValues>,
        cancelled: bool,
    }

    struct LlmDialogState {
        font: HFONT,
        header_font: HFONT,
        bg_brush: HBRUSH,
        edit_brush: HBRUSH,
        dark: bool,
        enabled: HWND,
        base_url: HWND,
        model: HWND,
        api_key: HWND,
        /// Config-side value when the dialog opened (kept if the field is left blank).
        previous_api_key: String,
        result: Option<LlmDraft>,
        cancelled: bool,
    }

    #[derive(Clone)]
    struct LlmDraft {
        enabled: bool,
        base_url: String,
        model: String,
        api_key: String,
    }

    pub(super) fn show_settings_dialog(
        initial: &SettingsValues,
    ) -> Result<Option<SettingsValues>, AppError> {
        // SAFETY: Win32 window class registration and message pump for a modal-style dialog.
        unsafe { show_settings_dialog_impl(initial) }
    }

    unsafe fn show_settings_dialog_impl(
        initial: &SettingsValues,
    ) -> Result<Option<SettingsValues>, AppError> {
        // Keep ComCtl v6 active for the whole dialog lifetime (themed controls).
        let _visual_styles = activate_visual_styles();
        ensure_common_controls();

        let unique = NEXT_CLASS_ID.fetch_add(1, Ordering::Relaxed);
        let class_name = format!("{CLASS_NAME}{unique}");
        let class_wide = to_wide(&class_name);
        let hinstance = GetModuleHandleW(ptr::null());
        let dark = matches!(initial.ui_theme, UiTheme::Dark);

        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            // Background is painted in WM_ERASEBKGND with a themed brush.
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_wide.as_ptr(),
            hIconSm: ptr::null_mut(),
        };

        if RegisterClassExW(&wc) == 0 {
            return Err(AppError::message(
                "failed to register Glossa settings window class",
            ));
        }

        STATE.with(|cell| {
            *cell.borrow_mut() = Some(DialogState {
                font: ptr::null_mut(),
                header_font: ptr::null_mut(),
                bg_brush: ptr::null_mut(),
                edit_brush: ptr::null_mut(),
                dark,
                backend: ptr::null_mut(),
                mode: ptr::null_mut(),
                enable_cli: ptr::null_mut(),
                provider_kind: ptr::null_mut(),
                base_url: ptr::null_mut(),
                model: ptr::null_mut(),
                api_key: ptr::null_mut(),
                paste_mode: ptr::null_mut(),
                append_space: ptr::null_mut(),
                theme: ptr::null_mut(),
                llm_enabled: initial.llm_enabled,
                llm_base_url: initial.llm_base_url.clone(),
                llm_model: initial.llm_model.clone(),
                llm_api_key: initial.llm_api_key.clone(),
                shortcut: initial.shortcut.clone(),
                result: None,
                cancelled: true,
            });
        });

        let title = to_wide("Glossa Settings");
        let (outer_w, outer_h) = outer_size_for_client(WIN_WIDTH, WIN_HEIGHT);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE,
            class_wide.as_ptr(),
            title.as_ptr(),
            WINDOW_STYLE | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            outer_w,
            outer_h,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            initial as *const SettingsValues as *const _,
        );

        if hwnd.is_null() {
            STATE.with(|cell| {
                cell.borrow_mut().take();
            });
            return Err(AppError::message("failed to create Glossa settings window"));
        }

        apply_titlebar_theme(hwnd, dark);
        center_window(hwnd);
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

        STATE.with(|cell| {
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
                let create = &*(lparam
                    as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
                let initial = &*(create.lpCreateParams as *const SettingsValues);
                if let Err(error) = on_create(hwnd, initial) {
                    show_error(&error.to_string());
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_ERASEBKGND => {
                if paint_background(hwnd, wparam as _, |s| (s.bg_brush, s.dark)) {
                    1
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_CTLCOLORSTATIC => color_static(wparam as _, |s| (s.bg_brush, s.dark)),
            WM_CTLCOLOREDIT => color_edit(wparam as _, |s| (s.edit_brush, s.dark)),
            // Leave buttons to the visual styles theme engine (do not handle WM_CTLCOLORBTN).
            WM_COMMAND => {
                let id = loword(wparam as usize) as i32;
                let code = hiword(wparam as usize) as u32;
                on_command(hwnd, id, code);
                0
            }
            WM_CLOSE => {
                STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.cancelled = true;
                        state.result = None;
                    }
                });
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        destroy_fonts_and_brushes(
                            &mut state.font,
                            &mut state.header_font,
                            &mut state.bg_brush,
                            &mut state.edit_brush,
                        );
                    }
                });
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn on_create(hwnd: HWND, initial: &SettingsValues) -> Result<(), AppError> {
        let dark = matches!(initial.ui_theme, UiTheme::Dark);
        let font = create_ui_font(FW_NORMAL as i32, -15);
        let header_font = create_ui_font(FW_SEMIBOLD as i32, -15);
        let bg_brush = CreateSolidBrush(if dark { DARK_BG } else { LIGHT_BG });
        let edit_brush = CreateSolidBrush(if dark { DARK_EDIT_BG } else { LIGHT_EDIT_BG });

        let margin = 20i32;
        let label_w = 100i32;
        let field_x = margin + label_w + 12;
        let field_w = WIN_WIDTH - field_x - margin - 16;
        let row_h = 30i32;
        let field_h = 26i32;
        let section_gap = 18i32;
        let mut y = margin;

        // --- Input ---
        create_section_header(hwnd, margin, y, field_w + label_w + 12, "Input", 2000, header_font);
        y += 26;
        create_label(hwnd, margin, y + 4, label_w, 20, "Backend", 2001, font);
        let backend = create_combo(hwnd, field_x, y, field_w, 220, ID_BACKEND, font);
        for label in ["portal", "none"] {
            send_combo_string(backend, label);
        }
        SendMessageW(
            backend,
            CB_SETCURSEL,
            match initial.input_backend {
                InputBackend::Portal => 0,
                InputBackend::None => 1,
            },
            0,
        );
        y += row_h + 6;
        create_label(hwnd, margin, y + 4, label_w, 20, "Mode", 2002, font);
        let mode = create_combo(hwnd, field_x, y, field_w, 220, ID_MODE, font);
        for label in ["toggle", "push-to-talk"] {
            send_combo_string(mode, label);
        }
        SendMessageW(
            mode,
            CB_SETCURSEL,
            match initial.input_mode {
                InputMode::Toggle => 0,
                InputMode::PushToTalk => 1,
            },
            0,
        );
        y += row_h + section_gap;

        // --- Control ---
        create_section_header(hwnd, margin, y, field_w + label_w + 12, "Control", 2003, header_font);
        y += 26;
        let enable_cli = create_checkbox(
            hwnd,
            margin,
            y,
            220,
            24,
            ID_ENABLE_CLI,
            "Enable CLI",
            font,
            initial.enable_cli,
        );
        y += row_h + section_gap;

        // --- Provider ---
        create_section_header(hwnd, margin, y, field_w + label_w + 12, "Provider", 2004, header_font);
        y += 26;
        create_label(hwnd, margin, y + 4, label_w, 20, "Kind", 2005, font);
        let provider_kind = create_combo(hwnd, field_x, y, field_w, 220, ID_PROVIDER_KIND, font);
        for label in ["groq", "openai", "openai-compatible"] {
            send_combo_string(provider_kind, label);
        }
        SendMessageW(
            provider_kind,
            CB_SETCURSEL,
            match initial.provider_kind {
                ProviderKind::Groq => 0,
                ProviderKind::OpenAi => 1,
                ProviderKind::OpenAiCompatible => 2,
            },
            0,
        );
        y += row_h + 6;
        create_label(hwnd, margin, y + 4, label_w, 20, "Base URL", 2006, font);
        let base_url = create_edit(hwnd, field_x, y, field_w, field_h, ID_BASE_URL, font, false);
        set_cue_banner(base_url, "https://api.example.com/v1");
        set_text(base_url, &initial.provider_base_url);
        y += row_h + 6;
        create_label(hwnd, margin, y + 4, label_w, 20, "Model", 2007, font);
        let model = create_edit(hwnd, field_x, y, field_w, field_h, ID_MODEL, font, false);
        set_text(model, &initial.provider_model);
        y += row_h + 6;
        create_label(hwnd, margin, y + 4, label_w, 20, "API Key", 2008, font);
        let api_key = create_edit(hwnd, field_x, y, field_w, field_h, ID_API_KEY, font, true);
        set_text(api_key, &initial.provider_api_key);
        y += row_h + section_gap;

        // --- Paste ---
        create_section_header(hwnd, margin, y, field_w + label_w + 12, "Paste", 2009, header_font);
        y += 26;
        create_label(hwnd, margin, y + 4, label_w, 20, "Mode", 2010, font);
        let paste_mode = create_combo(hwnd, field_x, y, field_w, 220, ID_PASTE_MODE, font);
        for label in ["ctrl-v", "ctrl-shift-v", "shift-insert"] {
            send_combo_string(paste_mode, label);
        }
        SendMessageW(
            paste_mode,
            CB_SETCURSEL,
            match initial.paste_mode {
                PasteMode::CtrlV => 0,
                PasteMode::CtrlShiftV => 1,
                PasteMode::ShiftInsert => 2,
            },
            0,
        );
        y += row_h + 6;
        let append_space = create_checkbox(
            hwnd,
            margin,
            y,
            220,
            24,
            ID_APPEND_SPACE,
            "Append space",
            font,
            initial.append_space,
        );
        y += row_h + section_gap;

        // --- UI ---
        create_section_header(hwnd, margin, y, field_w + label_w + 12, "UI", 2011, header_font);
        y += 26;
        create_label(hwnd, margin, y + 4, label_w, 20, "Theme", 2012, font);
        let theme = create_combo(hwnd, field_x, y, field_w, 220, ID_THEME, font);
        for label in ["dark", "light"] {
            send_combo_string(theme, label);
        }
        SendMessageW(
            theme,
            CB_SETCURSEL,
            match initial.ui_theme {
                UiTheme::Dark => 0,
                UiTheme::Light => 1,
            },
            0,
        );
        y += row_h + 22;

        // --- Action bar ---
        create_separator(hwnd, margin, y, WIN_WIDTH - margin * 2 - 16, 2013);
        y += 14;

        let btn_w = 100i32;
        let btn_h = 32i32;
        create_button(hwnd, margin, y, 120, btn_h, ID_LLM, "LLM enhancer…", font, false);
        create_button(
            hwnd,
            WIN_WIDTH - margin - 16 - btn_w * 2 - 10,
            y,
            btn_w,
            btn_h,
            ID_CANCEL,
            "Cancel",
            font,
            false,
        );
        create_button(
            hwnd,
            WIN_WIDTH - margin - 16 - btn_w,
            y,
            btn_w,
            btn_h,
            ID_SAVE,
            "Save",
            font,
            true,
        );

        // Grow/shrink the outer window so the full action bar is visible.
        // CreateWindow sizes include the non-client frame; layout uses client coords.
        let content_bottom = y + btn_h + margin;
        fit_window_to_client(hwnd, WIN_WIDTH, content_bottom);

        STATE.with(|cell| {
            if let Some(state) = cell.borrow_mut().as_mut() {
                state.font = font;
                state.header_font = header_font;
                state.bg_brush = bg_brush;
                state.edit_brush = edit_brush;
                state.dark = dark;
                state.backend = backend;
                state.mode = mode;
                state.enable_cli = enable_cli;
                state.provider_kind = provider_kind;
                state.base_url = base_url;
                state.model = model;
                state.api_key = api_key;
                state.paste_mode = paste_mode;
                state.append_space = append_space;
                state.theme = theme;
            }
        });

        SetFocus(backend);
        Ok(())
    }

    unsafe fn on_command(hwnd: HWND, id: i32, code: u32) {
        match id {
            ID_LLM if code == BN_CLICKED || code == 0 => {
                let draft = STATE.with(|cell| {
                    cell.borrow().as_ref().map(|state| LlmDraft {
                        enabled: state.llm_enabled,
                        base_url: state.llm_base_url.clone(),
                        model: state.llm_model.clone(),
                        api_key: state.llm_api_key.clone(),
                    })
                });
                let dark = STATE.with(|cell| cell.borrow().as_ref().map(|s| s.dark).unwrap_or(false));
                let Some(draft) = draft else {
                    return;
                };
                EnableWindow(hwnd, 0);
                let llm_result = show_llm_dialog(&draft, dark);
                EnableWindow(hwnd, 1);
                match llm_result {
                    Ok(Some(updated)) => {
                        STATE.with(|cell| {
                            if let Some(state) = cell.borrow_mut().as_mut() {
                                state.llm_enabled = updated.enabled;
                                state.llm_base_url = updated.base_url;
                                state.llm_model = updated.model;
                                state.llm_api_key = updated.api_key;
                            }
                        });
                    }
                    Ok(None) => {}
                    Err(error) => show_error(&error.to_string()),
                }
            }
            ID_SAVE if code == BN_CLICKED || code == 0 => match collect_settings() {
                Ok(settings) => {
                    if let Err(error) = validate_settings(&settings) {
                        show_error(&error);
                        return;
                    }
                    STATE.with(|cell| {
                        if let Some(state) = cell.borrow_mut().as_mut() {
                            state.result = Some(settings);
                            state.cancelled = false;
                        }
                    });
                    DestroyWindow(hwnd);
                }
                Err(error) => show_error(&error.to_string()),
            },
            ID_CANCEL if code == BN_CLICKED || code == 0 => {
                STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.cancelled = true;
                        state.result = None;
                    }
                });
                DestroyWindow(hwnd);
            }
            _ => {}
        }
    }

    fn collect_settings() -> Result<SettingsValues, AppError> {
        STATE.with(|cell| {
            let state_ref = cell.borrow();
            let state = state_ref
                .as_ref()
                .ok_or_else(|| AppError::message("settings dialog state missing"))?;
            // SAFETY: control HWNDs created in this process.
            unsafe {
                Ok(SettingsValues {
                    input_backend: match SendMessageW(state.backend, CB_GETCURSEL, 0, 0) {
                        1 => InputBackend::None,
                        _ => InputBackend::Portal,
                    },
                    input_mode: match SendMessageW(state.mode, CB_GETCURSEL, 0, 0) {
                        1 => InputMode::PushToTalk,
                        _ => InputMode::Toggle,
                    },
                    shortcut: state.shortcut.clone(),
                    enable_cli: is_checked(state.enable_cli),
                    provider_kind: match SendMessageW(state.provider_kind, CB_GETCURSEL, 0, 0) {
                        1 => ProviderKind::OpenAi,
                        2 => ProviderKind::OpenAiCompatible,
                        _ => ProviderKind::Groq,
                    },
                    provider_base_url: get_text(state.base_url),
                    provider_model: get_text(state.model),
                    provider_api_key: get_text(state.api_key),
                    llm_enabled: state.llm_enabled,
                    llm_base_url: state.llm_base_url.clone(),
                    llm_model: state.llm_model.clone(),
                    llm_api_key: state.llm_api_key.clone(),
                    paste_mode: match SendMessageW(state.paste_mode, CB_GETCURSEL, 0, 0) {
                        1 => PasteMode::CtrlShiftV,
                        2 => PasteMode::ShiftInsert,
                        _ => PasteMode::CtrlV,
                    },
                    append_space: is_checked(state.append_space),
                    ui_theme: match SendMessageW(state.theme, CB_GETCURSEL, 0, 0) {
                        1 => UiTheme::Light,
                        _ => UiTheme::Dark,
                    },
                })
            }
        })
    }

    fn validate_settings(settings: &SettingsValues) -> Result<(), String> {
        if settings.provider_model.trim().is_empty() {
            return Err("Provider model must not be empty.".into());
        }
        if settings.provider_kind == ProviderKind::OpenAiCompatible
            && settings.provider_base_url.trim().is_empty()
        {
            return Err("Base URL is required for openai-compatible providers.".into());
        }
        if settings.provider_kind != ProviderKind::OpenAiCompatible
            && settings.provider_api_key.trim().is_empty()
        {
            return Err("API key must not be empty for this provider.".into());
        }
        if settings.llm_enabled {
            if settings.llm_base_url.trim().is_empty() {
                return Err("LLM base URL must not be empty when the enhancer is enabled.".into());
            }
            if settings.llm_model.trim().is_empty() {
                return Err("LLM model must not be empty when the enhancer is enabled.".into());
            }
        }
        Ok(())
    }

    // --- LLM enhancer sub-dialog ---

    unsafe fn show_llm_dialog(initial: &LlmDraft, dark: bool) -> Result<Option<LlmDraft>, AppError> {
        let _visual_styles = activate_visual_styles();
        ensure_common_controls();

        let unique = NEXT_CLASS_ID.fetch_add(1, Ordering::Relaxed);
        let class_name = format!("{LLM_CLASS_NAME}{unique}");
        let class_wide = to_wide(&class_name);
        let hinstance = GetModuleHandleW(ptr::null());

        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(llm_wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            // Background is painted in WM_ERASEBKGND with a themed brush.
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_wide.as_ptr(),
            hIconSm: ptr::null_mut(),
        };

        if RegisterClassExW(&wc) == 0 {
            return Err(AppError::message(
                "failed to register Glossa LLM settings window class",
            ));
        }

        LLM_STATE.with(|cell| {
            *cell.borrow_mut() = Some(LlmDialogState {
                font: ptr::null_mut(),
                header_font: ptr::null_mut(),
                bg_brush: ptr::null_mut(),
                edit_brush: ptr::null_mut(),
                dark,
                enabled: ptr::null_mut(),
                base_url: ptr::null_mut(),
                model: ptr::null_mut(),
                api_key: ptr::null_mut(),
                previous_api_key: initial.api_key.clone(),
                result: None,
                cancelled: true,
            });
        });

        let title = to_wide("LLM enhancer");
        let (outer_w, outer_h) = outer_size_for_client(LLM_WIN_WIDTH, LLM_WIN_HEIGHT);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE,
            class_wide.as_ptr(),
            title.as_ptr(),
            WINDOW_STYLE | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            outer_w,
            outer_h,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            initial as *const LlmDraft as *const _,
        );

        if hwnd.is_null() {
            LLM_STATE.with(|cell| {
                cell.borrow_mut().take();
            });
            return Err(AppError::message(
                "failed to create Glossa LLM settings window",
            ));
        }

        apply_titlebar_theme(hwnd, dark);
        center_window(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        LLM_DIALOG_OPEN.store(true, Ordering::SeqCst);

        let mut msg = MSG {
            hwnd: ptr::null_mut(),
            message: 0,
            wParam: 0,
            lParam: 0,
            time: 0,
            pt: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 },
        };

        // Nested modal: pump until the LLM dialog closes. Do not use
        // PostQuitMessage here — that would also end the parent Settings loop.
        while LLM_DIALOG_OPEN.load(Ordering::SeqCst) {
            let status = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
            if status <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        LLM_STATE.with(|cell| {
            let state = cell.borrow_mut().take();
            match state {
                Some(state) if !state.cancelled => Ok(state.result),
                _ => Ok(None),
            }
        })
    }

    unsafe extern "system" fn llm_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                let create = &*(lparam
                    as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
                let initial = &*(create.lpCreateParams as *const LlmDraft);
                if let Err(error) = on_llm_create(hwnd, initial) {
                    show_error(&error.to_string());
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_ERASEBKGND => {
                if paint_background_llm(hwnd, wparam as _) {
                    1
                } else {
                    DefWindowProcW(hwnd, msg, wparam, lparam)
                }
            }
            WM_CTLCOLORSTATIC => color_static_llm(wparam as _, |s| (s.bg_brush, s.dark)),
            WM_CTLCOLOREDIT => color_edit_llm(wparam as _, |s| (s.edit_brush, s.dark)),
            WM_COMMAND => {
                let id = loword(wparam as usize) as i32;
                let code = hiword(wparam as usize) as u32;
                on_llm_command(hwnd, id, code);
                0
            }
            WM_CLOSE => {
                LLM_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.cancelled = true;
                        state.result = None;
                    }
                });
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                LLM_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        destroy_fonts_and_brushes(
                            &mut state.font,
                            &mut state.header_font,
                            &mut state.bg_brush,
                            &mut state.edit_brush,
                        );
                    }
                });
                LLM_DIALOG_OPEN.store(false, Ordering::SeqCst);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn on_llm_create(hwnd: HWND, initial: &LlmDraft) -> Result<(), AppError> {
        let dark = LLM_STATE.with(|cell| cell.borrow().as_ref().map(|s| s.dark).unwrap_or(false));
        let font = create_ui_font(FW_NORMAL as i32, -15);
        let header_font = create_ui_font(FW_SEMIBOLD as i32, -15);
        let bg_brush = CreateSolidBrush(if dark { DARK_BG } else { LIGHT_BG });
        let edit_brush = CreateSolidBrush(if dark { DARK_EDIT_BG } else { LIGHT_EDIT_BG });

        let margin = 20i32;
        let label_w = 90i32;
        let field_x = margin + label_w + 12;
        let field_w = LLM_WIN_WIDTH - field_x - margin - 16;
        let row_h = 32i32;
        let field_h = 26i32;
        let mut y = 20i32;

        create_section_header(
            hwnd,
            margin,
            y,
            field_w + label_w + 12,
            "Enhancer",
            2100,
            header_font,
        );
        y += 28;

        let enabled = create_checkbox(
            hwnd,
            margin,
            y,
            240,
            24,
            ID_LLM_ENABLED,
            "Enable LLM enhancer",
            font,
            initial.enabled,
        );
        y += row_h + 10;

        create_label(hwnd, margin, y + 4, label_w, 20, "Base URL", 2101, font);
        let base_url = create_edit(hwnd, field_x, y, field_w, field_h, ID_LLM_BASE_URL, font, false);
        // Base (…/v1) or full (…/chat/completions) both work.
        set_cue_banner(base_url, "https://openrouter.ai/api/v1");
        set_text(base_url, &initial.base_url);
        y += row_h + 6;

        create_label(hwnd, margin, y + 4, label_w, 20, "Model", 2102, font);
        let model = create_edit(hwnd, field_x, y, field_w, field_h, ID_LLM_MODEL, font, false);
        set_text(model, &initial.model);
        y += row_h + 6;

        create_label(hwnd, margin, y + 4, label_w, 20, "API Key", 2103, font);
        let api_key = create_edit(hwnd, field_x, y, field_w, field_h, ID_LLM_API_KEY, font, true);
        // Never show the secret or env: ref in the field — leave blank to keep saved key.
        set_text(api_key, "");
        let has_saved_key = !initial.api_key.trim().is_empty();
        set_cue_banner(
            api_key,
            if has_saved_key {
                "Key saved. Paste a new one to replace it"
            } else {
                "Paste API key (optional for local models)"
            },
        );
        y += row_h + 18;

        create_separator(hwnd, margin, y, LLM_WIN_WIDTH - margin * 2 - 16, 2104);
        y += 14;

        let btn_w = 96i32;
        let btn_h = 32i32;
        create_button(
            hwnd,
            LLM_WIN_WIDTH - margin - 16 - btn_w * 2 - 10,
            y,
            btn_w,
            btn_h,
            ID_LLM_CANCEL,
            "Cancel",
            font,
            false,
        );
        create_button(
            hwnd,
            LLM_WIN_WIDTH - margin - 16 - btn_w,
            y,
            btn_w,
            btn_h,
            ID_LLM_OK,
            "OK",
            font,
            true,
        );

        let content_bottom = y + btn_h + margin;
        fit_window_to_client(hwnd, LLM_WIN_WIDTH, content_bottom);

        LLM_STATE.with(|cell| {
            if let Some(state) = cell.borrow_mut().as_mut() {
                state.font = font;
                state.header_font = header_font;
                state.bg_brush = bg_brush;
                state.edit_brush = edit_brush;
                state.dark = dark;
                state.enabled = enabled;
                state.base_url = base_url;
                state.model = model;
                state.api_key = api_key;
            }
        });

        SetFocus(base_url);
        Ok(())
    }

    unsafe fn on_llm_command(hwnd: HWND, id: i32, code: u32) {
        match id {
            ID_LLM_OK if code == BN_CLICKED || code == 0 => {
                let draft = LLM_STATE.with(|cell| {
                    cell.borrow().as_ref().map(|state| {
                        let entered = get_text(state.api_key);
                        LlmDraft {
                            enabled: is_checked(state.enabled),
                            base_url: get_text(state.base_url),
                            model: get_text(state.model),
                            api_key: keep_or_replace_llm_api_key(
                                &entered,
                                &state.previous_api_key,
                            ),
                        }
                    })
                });
                let Some(draft) = draft else {
                    return;
                };
                if draft.enabled {
                    if draft.base_url.trim().is_empty() {
                        show_error("LLM base URL must not be empty when enabled.");
                        return;
                    }
                    if draft.model.trim().is_empty() {
                        show_error("LLM model must not be empty when enabled.");
                        return;
                    }
                }
                LLM_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.result = Some(draft);
                        state.cancelled = false;
                    }
                });
                DestroyWindow(hwnd);
            }
            ID_LLM_CANCEL if code == BN_CLICKED || code == 0 => {
                LLM_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.cancelled = true;
                        state.result = None;
                    }
                });
                DestroyWindow(hwnd);
            }
            _ => {}
        }
    }

    // --- Theming helpers ---

    /// Isolation context that loads Common Controls v6 for themed Win32 chrome.
    /// Dropping deactivates and releases the context.
    struct VisualStylesGuard {
        cookie: usize,
        actctx: HANDLE,
    }

    impl Drop for VisualStylesGuard {
        fn drop(&mut self) {
            // SAFETY: cookie/actctx from a matching ActivateActCtx / CreateActCtx pair.
            unsafe {
                if self.cookie != 0 {
                    let _ = DeactivateActCtx(0, self.cookie);
                    self.cookie = 0;
                }
                if !self.actctx.is_null() && self.actctx != INVALID_HANDLE_VALUE {
                    ReleaseActCtx(self.actctx);
                    self.actctx = ptr::null_mut();
                }
            }
        }
    }

    /// Activates ComCtl32 v6 via a temporary assembly manifest.
    ///
    /// Works even when the process was not linked with a Common-Controls
    /// dependency (the main reason classic dialogs look like Windows 95).
    unsafe fn activate_visual_styles() -> Option<VisualStylesGuard> {
        const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>
"#;

        let dir = std::env::temp_dir().join("glossa-visual-styles");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("comctl.v6.manifest");
        if std::fs::write(&path, MANIFEST).is_err() {
            return None;
        }

        let path_wide = to_wide(path.to_string_lossy().as_ref());
        let act = ACTCTXW {
            cbSize: size_of::<ACTCTXW>() as u32,
            dwFlags: 0,
            lpSource: path_wide.as_ptr(),
            wProcessorArchitecture: 0,
            wLangId: 0,
            lpAssemblyDirectory: ptr::null(),
            lpResourceName: ptr::null(),
            lpApplicationName: ptr::null(),
            hModule: ptr::null_mut(),
        };

        let actctx = CreateActCtxW(&act);
        if actctx.is_null() || actctx == INVALID_HANDLE_VALUE {
            return None;
        }

        let mut cookie: usize = 0;
        if ActivateActCtx(actctx, &mut cookie) == 0 {
            ReleaseActCtx(actctx);
            return None;
        }

        Some(VisualStylesGuard { cookie, actctx })
    }

    unsafe fn ensure_common_controls() {
        static INIT: AtomicBool = AtomicBool::new(false);
        if INIT.swap(true, Ordering::SeqCst) {
            return;
        }
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_WIN95_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }

    unsafe fn apply_titlebar_theme(hwnd: HWND, dark: bool) {
        let use_dark: i32 = if dark { 1 } else { 0 };
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            &use_dark as *const i32 as *const _,
            size_of::<i32>() as u32,
        );
    }

    unsafe fn paint_background<F>(hwnd: HWND, hdc: windows_sys::Win32::Graphics::Gdi::HDC, f: F) -> bool
    where
        F: FnOnce(&DialogState) -> (HBRUSH, bool),
    {
        let brush = STATE.with(|cell| cell.borrow().as_ref().map(|s| f(s).0));
        let Some(brush) = brush.filter(|b| !b.is_null()) else {
            return false;
        };
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut rect) == 0 {
            return false;
        }
        windows_sys::Win32::Graphics::Gdi::FillRect(hdc, &rect, brush);
        true
    }

    // Overload for LLM state — separate because paint_background is typed on DialogState.
    // (The LLM WM_ERASEBKGND path uses a local copy of the fill logic via this helper.)
    unsafe fn paint_background_llm(
        hwnd: HWND,
        hdc: windows_sys::Win32::Graphics::Gdi::HDC,
    ) -> bool {
        let brush = LLM_STATE.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|s| s.bg_brush)
                .filter(|b| !b.is_null())
        });
        let Some(brush) = brush else {
            return false;
        };
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(hwnd, &mut rect) == 0 {
            return false;
        }
        windows_sys::Win32::Graphics::Gdi::FillRect(hdc, &rect, brush);
        true
    }

    unsafe fn color_static<F>(hdc: windows_sys::Win32::Graphics::Gdi::HDC, f: F) -> LRESULT
    where
        F: FnOnce(&DialogState) -> (HBRUSH, bool),
    {
        let themed = STATE.with(|cell| cell.borrow().as_ref().map(|s| f(s)));
        let Some((brush, dark)) = themed else {
            return 0;
        };
        let text = if dark { DARK_TEXT } else { LIGHT_TEXT };
        SetTextColor(hdc, text);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetBkColor(hdc, if dark { DARK_BG } else { LIGHT_BG });
        brush as LRESULT
    }

    unsafe fn color_edit<F>(hdc: windows_sys::Win32::Graphics::Gdi::HDC, f: F) -> LRESULT
    where
        F: FnOnce(&DialogState) -> (HBRUSH, bool),
    {
        let themed = STATE.with(|cell| cell.borrow().as_ref().map(|s| f(s)));
        let Some((brush, dark)) = themed else {
            return 0;
        };
        let text = if dark { DARK_TEXT } else { LIGHT_TEXT };
        let bg = if dark { DARK_EDIT_BG } else { LIGHT_EDIT_BG };
        SetTextColor(hdc, text);
        SetBkColor(hdc, bg);
        SetBkMode(hdc, windows_sys::Win32::Graphics::Gdi::OPAQUE as i32);
        brush as LRESULT
    }

    unsafe fn color_static_llm<F>(hdc: windows_sys::Win32::Graphics::Gdi::HDC, f: F) -> LRESULT
    where
        F: FnOnce(&LlmDialogState) -> (HBRUSH, bool),
    {
        let themed = LLM_STATE.with(|cell| cell.borrow().as_ref().map(|s| f(s)));
        let Some((brush, dark)) = themed else {
            return 0;
        };
        let text = if dark { DARK_TEXT } else { LIGHT_TEXT };
        SetTextColor(hdc, text);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetBkColor(hdc, if dark { DARK_BG } else { LIGHT_BG });
        brush as LRESULT
    }

    unsafe fn color_edit_llm<F>(hdc: windows_sys::Win32::Graphics::Gdi::HDC, f: F) -> LRESULT
    where
        F: FnOnce(&LlmDialogState) -> (HBRUSH, bool),
    {
        let themed = LLM_STATE.with(|cell| cell.borrow().as_ref().map(|s| f(s)));
        let Some((brush, dark)) = themed else {
            return 0;
        };
        let text = if dark { DARK_TEXT } else { LIGHT_TEXT };
        let bg = if dark { DARK_EDIT_BG } else { LIGHT_EDIT_BG };
        SetTextColor(hdc, text);
        SetBkColor(hdc, bg);
        SetBkMode(hdc, windows_sys::Win32::Graphics::Gdi::OPAQUE as i32);
        brush as LRESULT
    }

    unsafe fn destroy_fonts_and_brushes(
        font: &mut HFONT,
        header_font: &mut HFONT,
        bg_brush: &mut HBRUSH,
        edit_brush: &mut HBRUSH,
    ) {
        if !font.is_null() {
            DeleteObject(*font);
            *font = ptr::null_mut();
        }
        if !header_font.is_null() {
            DeleteObject(*header_font);
            *header_font = ptr::null_mut();
        }
        if !bg_brush.is_null() {
            DeleteObject(*bg_brush);
            *bg_brush = ptr::null_mut();
        }
        if !edit_brush.is_null() {
            DeleteObject(*edit_brush);
            *edit_brush = ptr::null_mut();
        }
    }

    // --- Win32 helpers ---

    unsafe fn create_ui_font(weight: i32, height: i32) -> HFONT {
        let font = CreateFontW(
            height,
            0,
            0,
            0,
            weight,
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
        if font.is_null() {
            GetStockObject(DEFAULT_GUI_FONT) as HFONT
        } else {
            font
        }
    }

    /// Convert a desired client size into outer CreateWindow dimensions
    /// (title bar + borders included).
    unsafe fn outer_size_for_client(client_w: i32, client_h: i32) -> (i32, i32) {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: client_w,
            bottom: client_h,
        };
        if AdjustWindowRectEx(&mut rect, WINDOW_STYLE, 0, WINDOW_EX_STYLE) != 0 {
            (rect.right - rect.left, rect.bottom - rect.top)
        } else {
            // Fallback: typical caption + thin border padding.
            (client_w + 16, client_h + 40)
        }
    }

    /// Resize `hwnd` so its client area is exactly `client_w` × `client_h`.
    unsafe fn fit_window_to_client(hwnd: HWND, client_w: i32, client_h: i32) {
        let (outer_w, outer_h) = outer_size_for_client(client_w, client_h);
        let _ = SetWindowPos(
            hwnd,
            ptr::null_mut(),
            0,
            0,
            outer_w,
            outer_h,
            SWP_NOMOVE | SWP_NOZORDER,
        );
    }

    unsafe fn center_window(hwnd: HWND) {
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(hwnd, &mut rect) != 0 {
            let width = rect.right - rect.left;
            let height = rect.bottom - rect.top;
            let screen_w = GetSystemMetrics(SM_CXSCREEN);
            let screen_h = GetSystemMetrics(SM_CYSCREEN);
            let x = (screen_w - width) / 2;
            let y = (screen_h - height) / 2;
            let _ = SetWindowPos(hwnd, ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
        }
    }

    unsafe fn create_section_header(
        parent: HWND,
        x: i32,
        y: i32,
        w: i32,
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
            22,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(hwnd, WM_SETFONT, font as WPARAM, 1);
        hwnd
    }

    unsafe fn create_label(
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

    unsafe fn create_separator(parent: HWND, x: i32, y: i32, w: i32, id: i32) -> HWND {
        // SS_ETCHEDHORZ = 0x10 — thin modern horizontal rule.
        const SS_ETCHEDHORZ: u32 = 0x10;
        CreateWindowExW(
            0,
            to_wide("STATIC").as_ptr(),
            to_wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | SS_ETCHEDHORZ,
            x,
            y,
            w,
            2,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        )
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
        // WS_EX_CLIENTEDGE themes under ComCtl v6; avoid classic WS_BORDER.
        let mut style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32;
        if password {
            style |= ES_PASSWORD as u32;
        }
        let hwnd = CreateWindowExW(
            WS_EX_CLIENTEDGE,
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

    unsafe fn send_combo_string(combo: HWND, text: &str) {
        let wide = to_wide(text);
        SendMessageW(combo, CB_ADDSTRING, 0, wide.as_ptr() as LPARAM);
    }

    unsafe fn set_text(hwnd: HWND, text: &str) {
        SetWindowTextW(hwnd, to_wide(text).as_ptr());
    }

    /// Gray placeholder text shown while an edit control is empty.
    unsafe fn set_cue_banner(hwnd: HWND, text: &str) {
        let wide = to_wide(text);
        // wParam FALSE: hide cue when the control has focus (standard Windows behavior).
        SendMessageW(hwnd, EM_SETCUEBANNER, 0, wide.as_ptr() as LPARAM);
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

    fn show_error(message: &str) {
        let title = to_wide("Settings");
        let body = to_wide(message);
        // SAFETY: NUL-terminated wide strings.
        unsafe {
            MessageBoxW(
                ptr::null_mut(),
                body.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
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

    const fn rgb(r: u32, g: u32, b: u32) -> COLORREF {
        r | (g << 8) | (b << 16)
    }
}
