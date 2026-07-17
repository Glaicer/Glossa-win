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
    use std::{cell::RefCell, mem::size_of, os::windows::ffi::OsStrExt, ptr, sync::Once};

    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::SetFocus,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
                GetSystemMetrics, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
                LoadCursorW, MessageBoxW, PostQuitMessage, RegisterClassExW, SendMessageW,
                SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
                CW_USEDEFAULT, IDC_ARROW, MB_ICONERROR, MB_OK, MSG, SM_CXSCREEN, SM_CYSCREEN,
                SW_SHOW, SWP_NOSIZE, SWP_NOZORDER, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_SETFONT,
                WNDCLASSEXW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_OVERLAPPED, WS_SYSMENU,
                WS_TABSTOP, WS_VISIBLE,
            },
        },
    };

    const CLASS_NAME: &str = "GlossaPromptDialog";
    const ID_EDIT: isize = 1001;
    const ID_OK: i32 = 1;
    const ID_CANCEL: i32 = 2;

    thread_local! {
        static PROMPT_STATE: RefCell<Option<PromptState>> = const { RefCell::new(None) };
    }

    struct PromptState {
        edit: HWND,
        result: Option<String>,
        cancelled: bool,
    }

    struct PromptInit {
        label: String,
        initial: String,
    }

    fn to_wide(value: &str) -> Vec<u16> {
        std::ffi::OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
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
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn build_prompt_ui(hwnd: HWND, init: &PromptInit) -> Result<(), String> {
        let hinstance = GetModuleHandleW(ptr::null());
        let font = GetStockObject(DEFAULT_GUI_FONT);

        let static_class = to_wide("STATIC");
        let edit_class = to_wide("EDIT");
        let button_class = to_wide("BUTTON");

        let label = CreateWindowExW(
            0,
            static_class.as_ptr(),
            to_wide(&init.label).as_ptr(),
            WS_CHILD | WS_VISIBLE,
            16,
            16,
            360,
            20,
            hwnd,
            ptr::null_mut(),
            hinstance,
            ptr::null(),
        );
        if label.is_null() {
            return Err("failed to create label".into());
        }
        SendMessageW(label, WM_SETFONT, font as usize, 1);

        let edit = CreateWindowExW(
            0,
            edit_class.as_ptr(),
            to_wide(&init.initial).as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER,
            16,
            44,
            360,
            26,
            hwnd,
            ID_EDIT as _,
            hinstance,
            ptr::null(),
        );
        if edit.is_null() {
            return Err("failed to create edit control".into());
        }
        SendMessageW(edit, WM_SETFONT, font as usize, 1);

        let ok = CreateWindowExW(
            0,
            button_class.as_ptr(),
            to_wide("OK").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            196,
            86,
            84,
            28,
            hwnd,
            ID_OK as _,
            hinstance,
            ptr::null(),
        );
        let cancel = CreateWindowExW(
            0,
            button_class.as_ptr(),
            to_wide("Cancel").as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP,
            292,
            86,
            84,
            28,
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

        PROMPT_STATE.with(|cell| {
            *cell.borrow_mut() = Some(PromptState {
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

    let init = PromptInit {
        label: label.to_owned(),
        initial: initial.to_owned(),
    };
    let class_name = to_wide(CLASS_NAME);
    let title_wide = to_wide(title);
    let hinstance = unsafe { GetModuleHandleW(ptr::null()) };

    // SAFETY: ownerless modal dialog on the calling thread.
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            title_wide.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            410,
            160,
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
