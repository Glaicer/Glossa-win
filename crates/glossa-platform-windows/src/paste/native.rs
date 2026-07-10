use std::time::Duration;

use async_trait::async_trait;
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

use glossa_app::{ports::PasteBackend, AppError};
use glossa_core::PasteMode;

/// Paste backend that emulates keyboard shortcuts via Windows `SendInput`.
#[derive(Debug, Default, Clone)]
pub struct NativePasteBackend;

impl NativePasteBackend {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    fn paste_sync(mode: PasteMode) -> Result<(), AppError> {
        // Brief delay so the clipboard commit is visible to the focused app.
        std::thread::sleep(Duration::from_millis(40));

        let mut enigo = Enigo::new(&Settings::default())
            .map_err(|error| AppError::message(format!("failed to initialize input backend: {error}")))?;

        match mode {
            PasteMode::CtrlV => {
                enigo
                    .key(Key::Control, Direction::Press)
                    .map_err(map_enigo_error)?;
                enigo.key(Key::Unicode('v'), Direction::Click).map_err(map_enigo_error)?;
                enigo
                    .key(Key::Control, Direction::Release)
                    .map_err(map_enigo_error)?;
            }
            PasteMode::CtrlShiftV => {
                enigo
                    .key(Key::Control, Direction::Press)
                    .map_err(map_enigo_error)?;
                enigo
                    .key(Key::Shift, Direction::Press)
                    .map_err(map_enigo_error)?;
                enigo.key(Key::Unicode('v'), Direction::Click).map_err(map_enigo_error)?;
                enigo
                    .key(Key::Shift, Direction::Release)
                    .map_err(map_enigo_error)?;
                enigo
                    .key(Key::Control, Direction::Release)
                    .map_err(map_enigo_error)?;
            }
            PasteMode::ShiftInsert => {
                enigo
                    .key(Key::Shift, Direction::Press)
                    .map_err(map_enigo_error)?;
                enigo
                    .key(Key::Insert, Direction::Click)
                    .map_err(map_enigo_error)?;
                enigo
                    .key(Key::Shift, Direction::Release)
                    .map_err(map_enigo_error)?;
            }
        }

        Ok(())
    }
}

fn map_enigo_error(error: impl std::fmt::Display) -> AppError {
    AppError::message(format!("failed to synthesize paste shortcut: {error}"))
}

#[async_trait]
impl PasteBackend for NativePasteBackend {
    async fn paste(&self, mode: PasteMode) -> Result<(), AppError> {
        tokio::task::spawn_blocking(move || Self::paste_sync(mode))
            .await
            .map_err(|error| AppError::message(format!("paste task failed: {error}")))?
    }
}
