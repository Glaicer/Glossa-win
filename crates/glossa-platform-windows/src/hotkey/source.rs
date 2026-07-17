use std::time::Duration;

use async_trait::async_trait;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use tokio::sync::mpsc;
use tracing::{info, warn};

use glossa_app::{ports::CommandSource, AppError};
use glossa_core::{AppCommand, InputConfig, InputMode};

use crate::winmsg::pump_thread_messages;

use super::mapping::{map_hotkey_signal_to_command, HotkeySignal};
use super::shortcut_parser::parse_shortcut;

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
            manager.register(hotkey).map_err(|error| {
                AppError::message(format!("failed to register global hotkey: {error}"))
            })?;

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
