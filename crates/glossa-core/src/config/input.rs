use serde::{Deserialize, Serialize};

use crate::CoreError;

/// Input backend used to receive global recording commands.
///
/// `"portal"` is accepted as a legacy TOML alias from the Linux GlobalShortcuts
/// portal name; on Windows both mean the native global hotkey backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputBackend {
    /// Native global hotkey backend (`RegisterHotKey` on Windows).
    #[serde(alias = "portal")]
    Hotkey,
    None,
}

/// Recording semantics for the global hotkey backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputMode {
    Toggle,
    PushToTalk,
}

/// Global hotkey configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputConfig {
    pub backend: InputBackend,
    #[serde(default)]
    pub shortcut: String,
    pub mode: InputMode,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            backend: InputBackend::Hotkey,
            shortcut: "Ctrl+Alt+Space".into(),
            mode: InputMode::Toggle,
        }
    }
}

impl InputConfig {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        Ok(())
    }
}
