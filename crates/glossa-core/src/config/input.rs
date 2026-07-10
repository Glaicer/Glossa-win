use serde::{Deserialize, Serialize};

use crate::CoreError;

/// Input backend used to receive global recording commands.
///
/// `Portal` is the historical name from the Linux GlobalShortcuts portal.
/// On Windows it means the native global hotkey backend. The TOML value
/// `"hotkey"` is accepted as an alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InputBackend {
    #[serde(alias = "hotkey")]
    Portal,
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
            backend: InputBackend::Portal,
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
