//! Console ownership helpers for silent daemon starts on Windows.

/// Detaches from the process console when this process is the only one using it.
///
/// Double-click, Startup-folder, and similar launches allocate a console solely
/// for the daemon. Freeing that console closes the black window. When the
/// daemon is started from an existing terminal (shared console), the attachment
/// is kept so stderr logs remain visible.
pub fn free_orphan_console() {
    // SAFETY: GetConsoleProcessList / FreeConsole are process-local and do not
    // require a held lock; we only free when this process exclusively owns the
    // console (count == 1).
    unsafe {
        let mut pids = [0u32; 2];
        let count = windows_sys::Win32::System::Console::GetConsoleProcessList(
            pids.as_mut_ptr(),
            pids.len() as u32,
        );
        if count == 1 {
            let _ = windows_sys::Win32::System::Console::FreeConsole();
        }
    }
}
