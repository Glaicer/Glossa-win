//! Win32 message-pump helpers.
//!
//! `tray-icon` and `global-hotkey` deliver events through a thread's message
//! queue on Windows. Without pumping that queue, tray clicks and hotkeys appear dead.

/// Drains pending messages for the calling thread.
#[cfg(windows)]
pub(crate) fn pump_thread_messages() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    // SAFETY: PeekMessage/Translate/Dispatch on this thread's queue only; null
    // HWND means "any window belonging to this thread".
    unsafe {
        let mut msg = std::mem::zeroed::<MSG>();
        while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn pump_thread_messages() {}
