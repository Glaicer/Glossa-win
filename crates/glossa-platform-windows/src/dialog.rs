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
        // SAFETY: title/message are NUL-terminated wide strings; no window handle is required.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message_wide.as_ptr(),
                title_wide.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        // Fallback for non-Windows host builds (e.g. Linux CI compiling the crate).
        eprintln!("{title}: {message}");
        Ok(())
    }
}
