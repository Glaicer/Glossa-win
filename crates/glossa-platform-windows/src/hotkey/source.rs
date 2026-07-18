use std::time::Duration;

use async_trait::async_trait;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tokio::sync::mpsc;
use tracing::{info, warn};

use glossa_app::{ports::CommandSource, AppError};
use glossa_core::{AppCommand, InputConfig, InputMode};

use crate::winmsg::pump_thread_messages;

use super::mapping::{map_hotkey_signal_to_command, HotkeySignal};
use super::shortcut_parser::parse_shortcut;

/// How long to keep retrying `RegisterHotKey` after an in-process daemon restart.
///
/// The previous hotkey task is aborted without awaiting its blocking unregister
/// (see `daemon.rs`); the OS may still hold the combo for a short window.
const REGISTER_RETRY_WINDOW: Duration = Duration::from_millis(1_500);
const REGISTER_RETRY_STEP: Duration = Duration::from_millis(50);

fn register_hotkey_with_retry(manager: &GlobalHotKeyManager, hotkey: HotKey) -> Result<(), AppError> {
    let deadline = std::time::Instant::now() + REGISTER_RETRY_WINDOW;
    loop {
        match manager.register(hotkey) {
            Ok(()) => return Ok(()),
            Err(error) if is_already_registered(&error) && std::time::Instant::now() < deadline => {
                std::thread::sleep(REGISTER_RETRY_STEP);
            }
            Err(error) => {
                return Err(AppError::message(format!(
                    "failed to register global hotkey: {error}"
                )));
            }
        }
    }
}

fn is_already_registered(error: &global_hotkey::Error) -> bool {
    matches!(error, global_hotkey::Error::AlreadyRegistered(_))
}

/// Global hotkey command source for Windows.
pub struct GlobalHotkeySource {
    config: InputConfig,
}

impl GlobalHotkeySource {
    #[must_use]
    pub fn new(config: InputConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl CommandSource for GlobalHotkeySource {
    fn name(&self) -> &'static str {
        "global-hotkey"
    }

    async fn run(self: Box<Self>, tx: mpsc::UnboundedSender<AppCommand>) -> Result<(), AppError> {
        let mode = self.config.mode;
        let shortcut = self.config.shortcut.clone();
        let hotkey = parse_shortcut(&shortcut)?;

        info!(
            shortcut = %shortcut,
            mode = ?mode,
            "registering global hotkey"
        );

        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let manager = tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let manager = GlobalHotKeyManager::new().map_err(|error| {
                AppError::message(format!("failed to create global hotkey manager: {error}"))
            })?;
            // Settings save restarts the daemon in-process by aborting this task
            // and immediately starting a new one. Abort drops the async side without
            // waiting for UnregisterHotKey, so a naive register races and fails with
            // AlreadyRegistered — leaving the daemon alive but deaf to hotkeys.
            register_hotkey_with_retry(&manager, hotkey)?;

            let receiver = GlobalHotKeyEvent::receiver();
            loop {
                // global-hotkey delivers events via the Win32 message queue on
                // this thread; pump it so RegisterHotKey notifications arrive.
                pump_thread_messages();

                match receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok(event) if event.id == hotkey.id() => {
                        let signal = match event.state {
                            HotKeyState::Pressed => HotkeySignal::Activated,
                            HotKeyState::Released => HotkeySignal::Deactivated,
                        };
                        if event_tx.send(signal).is_err() {
                            break;
                        }
                    }
                    Ok(_) => {}
                    Err(_) => {
                        // Timeout or disconnect from the hotkey event channel.
                        if event_tx.is_closed() {
                            break;
                        }
                    }
                }
            }

            let _ = manager.unregister(hotkey);
            Ok(())
        });

        while let Some(signal) = event_rx.recv().await {
            if let Some(command) = map_hotkey_signal_to_command(mode, signal) {
                if tx.send(command).is_err() {
                    warn!("hotkey command channel closed");
                    break;
                }
            } else if mode == InputMode::Toggle {
                // Deactivated events are intentionally ignored in toggle mode.
            }
        }

        match manager.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(error) => Err(AppError::message(format!(
                "global hotkey manager task failed: {error}"
            ))),
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::time::Duration;

    use glossa_app::ports::CommandSource;
    use glossa_core::{InputConfig, InputMode};
    use tokio::sync::mpsc;

    use super::GlobalHotkeySource;

    /// Shortcut unlikely to collide with a running user daemon or other tests.
    const TEST_SHORTCUT: &str = "Ctrl+Shift+F9";

    fn test_input_config() -> InputConfig {
        InputConfig {
            backend: glossa_core::InputBackend::Hotkey,
            shortcut: TEST_SHORTCUT.into(),
            mode: InputMode::Toggle,
        }
    }

    /// Documents the OS constraint: Windows rejects a second RegisterHotKey for
    /// the same combo while the first registration is still live.
    #[test]
    fn concurrent_register_of_same_hotkey_should_fail() {
        use global_hotkey::GlobalHotKeyManager;

        use crate::hotkey::shortcut_parser::parse_shortcut;

        let hotkey = parse_shortcut(TEST_SHORTCUT).expect("test shortcut should parse");
        let first = GlobalHotKeyManager::new().expect("first manager");
        first
            .register(hotkey)
            .expect("first registration should succeed");

        let second = GlobalHotKeyManager::new().expect("second manager");
        let err = second
            .register(hotkey)
            .expect_err("second registration must fail while first is held");
        let message = err.to_string();
        assert!(
            message.to_ascii_lowercase().contains("already")
                || message.contains("register"),
            "unexpected error for double-register: {message}"
        );
    }

    /// Mirrors `daemon.rs` on Settings save: abort the hotkey task and start a
    /// new one immediately, without waiting for the Win32 unregister to finish.
    ///
    /// Expected (correct) behavior: the second source stays running (registered).
    /// Bug: second `RegisterHotKey` fails with AlreadyRegistered and `run` exits.
    #[tokio::test]
    async fn hotkey_source_should_survive_abort_and_immediate_restart() {
        let (tx1, _rx1) = mpsc::unbounded_channel();
        let source1 = Box::new(GlobalHotkeySource::new(test_input_config()));
        let handle1 = tokio::spawn(async move { source1.run(tx1).await });

        // Allow the first manager to create its HWND and RegisterHotKey.
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert!(
            !handle1.is_finished(),
            "first hotkey source should still be running after register"
        );

        // Same as daemon: abort and do not await the blocking unregister path.
        handle1.abort();

        let (tx2, _rx2) = mpsc::unbounded_channel();
        let source2 = Box::new(GlobalHotkeySource::new(test_input_config()));
        let handle2 = tokio::spawn(async move { source2.run(tx2).await });

        // Registration either fails fast (bug) or the source stays alive (good).
        tokio::time::sleep(Duration::from_millis(400)).await;

        if handle2.is_finished() {
            let outcome = handle2
                .await
                .expect("join of finished second source should succeed");
            panic!(
                "second hotkey source exited after restart-like abort; \
                 hotkeys would stop working. outcome={outcome:?}"
            );
        }

        handle2.abort();
        // Best-effort: give the aborted task a moment to unregister before other tests.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
