use serde::{Deserialize, Serialize};

use crate::{CoreError, PasteMode};

/// Clipboard and paste settings for the final text insertion phase.
///
/// Windows always uses the native clipboard and `SendInput` paste backends;
/// external command knobs from the Linux port were removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PasteConfig {
    pub mode: PasteMode,
    #[serde(default)]
    pub append_space: bool,
}

impl Default for PasteConfig {
    fn default() -> Self {
        Self {
            mode: PasteMode::CtrlV,
            append_space: false,
        }
    }
}

impl PasteConfig {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        Ok(())
    }
}
