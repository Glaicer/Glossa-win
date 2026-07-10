use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tracing::{debug, warn};

use glossa_app::AppError;
use glossa_core::{AppCommand, AppStateKind, AppStatus, CommandOrigin};

/// Watches the app status and cancels recording when Escape is pressed.
pub async fn run_escape_cancel_monitor(
    mut status_rx: watch::Receiver<AppStatus>,
    command_tx: mpsc::UnboundedSender<AppCommand>,
) -> Result<(), AppError> {
    let mut was_recording = status_rx.borrow().state == AppStateKind::Recording;

    loop {
        tokio::select! {
            changed = status_rx.changed() => {
                if changed.is_err() {
                    debug!("escape cancel monitor stopping: status channel closed");
                    return Ok(());
                }
                was_recording = status_rx.borrow().state == AppStateKind::Recording;
            }
            _ = tokio::time::sleep(Duration::from_millis(40)) => {
                if !was_recording {
                    continue;
                }
                if status_rx.borrow().state != AppStateKind::Recording {
                    was_recording = false;
                    continue;
                }
                if is_escape_pressed() {
                    debug!("escape pressed while recording; cancelling");
                    if command_tx
                        .send(AppCommand::CancelRecording {
                            origin: CommandOrigin::EscapeKey,
                        })
                        .is_err()
                    {
                        warn!("failed to send cancel recording command");
                        return Ok(());
                    }
                    // Debounce so a held Escape key does not spam cancels.
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
            }
        }
    }
}

#[cfg(windows)]
fn is_escape_pressed() -> bool {
    // VK_ESCAPE = 0x1B. High bit set means the key is currently down.
    // SAFETY: GetAsyncKeyState is a pure query of keyboard state.
    let state = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(0x1B) };
    state as u16 & 0x8000 != 0
}

#[cfg(not(windows))]
fn is_escape_pressed() -> bool {
    false
}
