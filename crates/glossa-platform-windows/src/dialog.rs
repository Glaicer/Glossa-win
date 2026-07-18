#[derive(Debug)]
pub enum DialogError {
    Message(String),
}

impl std::fmt::Display for DialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DialogError {}

/// Shows a fatal error dialog using a native Windows message box when possible.
pub fn show_fatal_error_dialog(title: &str, message: &str) -> Result<(), DialogError> {
    show_message_dialog(title, message)
}

pub(crate) fn show_message_dialog(title: &str, message: &str) -> Result<(), DialogError> {
    show_message_box(title, message, MessageKind::Error)
}

pub(crate) fn show_info_dialog(title: &str, message: &str) -> Result<(), DialogError> {
    show_message_box(title, message, MessageKind::Info)
}

enum MessageKind {
    Error,
    Info,
}

fn show_message_box(title: &str, message: &str, kind: MessageKind) -> Result<(), DialogError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        fn to_wide(value: &str) -> Vec<u16> {
            std::ffi::OsStr::new(value)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        }

        let title_wide = to_wide(title);
        let message_wide = to_wide(message);
        let icon = match kind {
            MessageKind::Error => windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            MessageKind::Info => windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONINFORMATION,
        };
        // SAFETY: title/message are NUL-terminated wide strings; no window handle is required.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message_wide.as_ptr(),
                title_wide.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_OK | icon,
            );
        }
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let _ = kind;
        // Fallback for non-Windows host builds (e.g. Linux CI compiling the crate).
        eprintln!("{title}: {message}");
        Ok(())
    }
}

/// Modal single-line text prompt. Returns `None` when the user cancels.
pub(crate) fn prompt_text_dialog(
    title: &str,
    label: &str,
    initial: &str,
) -> Result<Option<String>, DialogError> {
    #[cfg(windows)]
    {
        prompt_text_dialog_windows(title, label, initial)
    }
    #[cfg(not(windows))]
    {
        let _ = (title, label, initial);
        Err(DialogError::Message(
            "text prompt dialog is only available on Windows".into(),
        ))
    }
}

#[cfg(windows)]
fn prompt_text_dialog_windows(
    title: &str,
    label: &str,
    initial: &str,
) -> Result<Option<String>, DialogError> {
    use std::{cell::RefCell, mem::size_of, ptr, sync::Once};

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            CreateSolidBrush, DeleteObject, FillRect, SetBkColor, SetBkMode, SetTextColor, HBRUSH,
            HFONT, OPAQUE, TRANSPARENT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
                GetMessageW, GetWindowTextLengthW, GetWindowTextW, LoadCursorW, MessageBoxW,
                PostQuitMessage, RegisterClassExW, SendMessageW, SetWindowTextW, ShowWindow,
                TranslateMessage, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW,
                CW_USEDEFAULT, ES_AUTOHSCROLL, IDC_ARROW, MB_ICONERROR, MB_OK, MSG, SW_SHOW,
                WM_COMMAND, WM_CREATE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY,
                WM_ERASEBKGND, WM_SETFONT, WNDCLASSEXW, WS_CHILD, WS_EX_CLIENTEDGE, WS_TABSTOP,
                WS_VISIBLE,
            },
        },
    };

    use crate::win_style::{
        activate_visual_styles, apply_titlebar_theme, center_window, create_body_font,
        create_header_font, ensure_common_controls, fit_window_to_client, outer_size_for_client,
        to_wide, LIGHT_BG, LIGHT_EDIT_BG, LIGHT_TEXT, WINDOW_EX_STYLE, WINDOW_STYLE,
    };

    const CLASS_NAME: &str = "GlossaPromptDialog";
    const CLIENT_W: i32 = 420;
    const CLIENT_H: i32 = 180;
    const ID_EDIT: isize = 1001;
    const ID_OK: i32 = 1;
    const ID_CANCEL: i32 = 2;

    thread_local! {
        static PROMPT_STATE: RefCell<Option<PromptState>> = const { RefCell::new(None) };
    }

    struct PromptState {
        font: HFONT,
        header_font: HFONT,
        bg_brush: HBRUSH,
        edit_brush: HBRUSH,
        edit: HWND,
        result: Option<String>,
        cancelled: bool,
    }

    struct PromptInit {
        label: String,
        initial: String,
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
                lpfnWndProc: Some(prompt_wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinstance,
                hIcon: ptr::null_mut(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
                hbrBackground: ptr::null_mut(),
                lpszMenuName: ptr::null(),
                lpszClassName: class_name.as_ptr(),
                hIconSm: ptr::null_mut(),
            };
            let _ = RegisterClassExW(&class);
        }
    });

    unsafe extern "system" fn prompt_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                let create = &*(lparam
                    as *const windows_sys::Win32::UI::WindowsAndMessaging::CREATESTRUCTW);
                let init = &*(create.lpCreateParams as *const PromptInit);
                if let Err(error) = build_prompt_ui(hwnd, init) {
                    let title = to_wide("Glossa");
                    let message = to_wide(&error);
                    MessageBoxW(hwnd, message.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
                    DestroyWindow(hwnd);
                    return -1;
                }
                0
            }
            WM_ERASEBKGND => {
                let brush = PROMPT_STATE.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .map(|s| s.bg_brush)
                        .filter(|b| !b.is_null())
                });
                if let Some(brush) = brush {
                    let mut rect = RECT {
                        left: 0,
                        top: 0,
                        right: 0,
                        bottom: 0,
                    };
                    if GetClientRect(hwnd, &mut rect) != 0 {
                        FillRect(wparam as _, &rect, brush);
                        return 1;
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_CTLCOLORSTATIC => {
                let hdc = wparam as _;
                let brush = PROMPT_STATE.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .map(|s| s.bg_brush)
                        .unwrap_or(ptr::null_mut())
                });
                SetTextColor(hdc, LIGHT_TEXT);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetBkColor(hdc, LIGHT_BG);
                brush as LRESULT
            }
            WM_CTLCOLOREDIT => {
                let hdc = wparam as _;
                let brush = PROMPT_STATE.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .map(|s| s.edit_brush)
                        .unwrap_or(ptr::null_mut())
                });
                SetTextColor(hdc, LIGHT_TEXT);
                SetBkColor(hdc, LIGHT_EDIT_BG);
                SetBkMode(hdc, OPAQUE as i32);
                brush as LRESULT
            }
            WM_COMMAND => {
                let id = (wparam & 0xFFFF) as i32;
                match id {
                    ID_OK => {
                        PROMPT_STATE.with(|cell| {
                            if let Some(state) = cell.borrow_mut().as_mut() {
                                state.result = Some(get_text(state.edit));
                                state.cancelled = false;
                            }
                        });
                        DestroyWindow(hwnd);
                    }
                    ID_CANCEL => {
                        PROMPT_STATE.with(|cell| {
                            if let Some(state) = cell.borrow_mut().as_mut() {
                                state.cancelled = true;
                                state.result = None;
                            }
                        });
                        DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                0
            }
            WM_DESTROY => {
                PROMPT_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        if !state.font.is_null() {
                            DeleteObject(state.font);
                            state.font = ptr::null_mut();
                        }
                        if !state.header_font.is_null() {
                            DeleteObject(state.header_font);
                            state.header_font = ptr::null_mut();
                        }
                        if !state.bg_brush.is_null() {
                            DeleteObject(state.bg_brush);
                            state.bg_brush = ptr::null_mut();
                        }
                        if !state.edit_brush.is_null() {
                            DeleteObject(state.edit_brush);
                            state.edit_brush = ptr::null_mut();
                        }
                    }
                });
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn build_prompt_ui(hwnd: HWND, init: &PromptInit) -> Result<(), String> {
        let hinstance = GetModuleHandleW(ptr::null());
        let font = create_body_font();
        let header_font = create_header_font();
        let bg_brush = CreateSolidBrush(LIGHT_BG);
        let edit_brush = CreateSolidBrush(LIGHT_EDIT_BG);

        let margin = 20i32;
        let field_w = CLIENT_W - margin * 2;
        let mut y = margin;

        // Title-style label
        let label = CreateWindowExW(
            0,
            to_wide("STATIC").as_ptr(),
            to_wide(&init.label).as_ptr(),
            WS_CHILD | WS_VISIBLE,
            margin,
            y,
            field_w,
            22,
            hwnd,
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        if label.is_null() {
            return Err("failed to create label".into());
        }
        SendMessageW(label, WM_SETFONT, header_font as usize, 1);
        y += 30;

        let edit = CreateWindowExW(
            WS_EX_CLIENTEDGE,
            to_wide("EDIT").as_ptr(),
            to_wide(&init.initial).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            margin,
            y,
            field_w,
            28,
            hwnd,
            ID_EDIT as _,
            hinstance,
            ptr::null(),
        );
        if edit.is_null() {
            return Err("failed to create edit control".into());
        }
        SendMessageW(edit, WM_SETFONT, font as usize, 1);
        y += 40;

        // Separator (SS_ETCHEDHORZ)
        const SS_ETCHEDHORZ: u32 = 0x10;
        let _ = CreateWindowExW(
            0,
            to_wide("STATIC").as_ptr(),
            to_wide("").as_ptr(),
            WS_CHILD | WS_VISIBLE | SS_ETCHEDHORZ,
            margin,
            y,
            field_w,
            2,
            hwnd,
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        y += 14;

        let btn_w = 96i32;
        let btn_h = 32i32;
        let ok = CreateWindowExW(
            0,
            to_wide("BUTTON").as_ptr(),
            to_wide("OK").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_DEFPUSHBUTTON as u32,
            CLIENT_W - margin - btn_w,
            y,
            btn_w,
            btn_h,
            hwnd,
            ID_OK as _,
            hinstance,
            ptr::null(),
        );
        let cancel = CreateWindowExW(
            0,
            to_wide("BUTTON").as_ptr(),
            to_wide("Cancel").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            CLIENT_W - margin - btn_w * 2 - 10,
            y,
            btn_w,
            btn_h,
            hwnd,
            ID_CANCEL as _,
            hinstance,
            ptr::null(),
        );
        if ok.is_null() || cancel.is_null() {
            return Err("failed to create buttons".into());
        }
        SendMessageW(ok, WM_SETFONT, font as usize, 1);
        SendMessageW(cancel, WM_SETFONT, font as usize, 1);

        let content_bottom = y + btn_h + margin;
        fit_window_to_client(hwnd, CLIENT_W, content_bottom);

        PROMPT_STATE.with(|cell| {
            *cell.borrow_mut() = Some(PromptState {
                font,
                header_font,
                bg_brush,
                edit_brush,
                edit,
                result: None,
                cancelled: true,
            });
        });
        SetFocus(edit);
        let _ = SetWindowTextW;
        Ok(())
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

    let _visual_styles = unsafe { activate_visual_styles() };
    unsafe { ensure_common_controls() };

    let init = PromptInit {
        label: label.to_owned(),
        initial: initial.to_owned(),
    };
    let class_name = to_wide(CLASS_NAME);
    let title_wide = to_wide(title);
    let hinstance = unsafe { GetModuleHandleW(ptr::null()) };
    let (outer_w, outer_h) = unsafe { outer_size_for_client(CLIENT_W, CLIENT_H) };

    // SAFETY: ownerless modal dialog on the calling thread.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE,
            class_name.as_ptr(),
            title_wide.as_ptr(),
            WINDOW_STYLE | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            outer_w,
            outer_h,
            ptr::null_mut(),
            ptr::null_mut(),
            hinstance,
            &init as *const PromptInit as *const _,
        )
    };
    if hwnd.is_null() {
        return Err(DialogError::Message(
            "failed to create prompt dialog".into(),
        ));
    }

    unsafe {
        apply_titlebar_theme(hwnd, false);
        center_window(hwnd);
        ShowWindow(hwnd, SW_SHOW);
    }

    // SAFETY: standard GetMessage loop until the dialog posts quit.
    unsafe {
        let mut msg = std::mem::zeroed::<MSG>();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    let result = PROMPT_STATE.with(|cell| {
        cell.borrow_mut()
            .take()
            .and_then(|state| if state.cancelled { None } else { state.result })
    });
    Ok(result)
}
