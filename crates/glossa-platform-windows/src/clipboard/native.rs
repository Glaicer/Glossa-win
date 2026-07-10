use std::sync::Mutex;

use async_trait::async_trait;
use arboard::Clipboard;

use glossa_app::{
    ports::{ClipboardSnapshot, ClipboardWriter},
    AppError,
};

/// Clipboard writer backed by the native Windows clipboard (`arboard`).
#[derive(Default)]
pub struct NativeClipboard {
    inner: Mutex<Option<Clipboard>>,
}

impl std::fmt::Debug for NativeClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeClipboard").finish_non_exhaustive()
    }
}

impl NativeClipboard {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    fn with_clipboard<T>(
        &self,
        f: impl FnOnce(&mut Clipboard) -> Result<T, arboard::Error>,
    ) -> Result<T, AppError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| AppError::message("clipboard mutex is poisoned"))?;
        if guard.is_none() {
            *guard = Some(
                Clipboard::new()
                    .map_err(|error| AppError::message(format!("failed to open clipboard: {error}")))?,
            );
        }
        let clipboard = guard
            .as_mut()
            .ok_or_else(|| AppError::message("clipboard is unavailable"))?;
        f(clipboard).map_err(|error| AppError::message(format!("clipboard operation failed: {error}")))
    }
}

#[async_trait]
impl ClipboardWriter for NativeClipboard {
    async fn snapshot(&self) -> Result<Option<ClipboardSnapshot>, AppError> {
        match self.with_clipboard(|clipboard| clipboard.get_text()) {
            Ok(text) => Ok(Some(ClipboardSnapshot::new(
                "text/plain;charset=utf-8".into(),
                text.into_bytes(),
            ))),
            Err(_) => Ok(None),
        }
    }

    async fn set_text(&self, text: &str) -> Result<(), AppError> {
        let text = text.to_owned();
        self.with_clipboard(move |clipboard| clipboard.set_text(text))?;
        Ok(())
    }

    async fn restore(&self, snapshot: ClipboardSnapshot) -> Result<(), AppError> {
        if snapshot.mime_type.starts_with("text/") {
            let text = String::from_utf8_lossy(&snapshot.bytes).into_owned();
            self.with_clipboard(move |clipboard| clipboard.set_text(text))?;
        }
        Ok(())
    }
}
