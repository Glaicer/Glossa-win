use serde::{Deserialize, Serialize};

use crate::CoreError;

/// Log level used by the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Logging configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: LogLevel,
    /// When true, log to the console/stderr sink.
    /// `"journal"` remains a deserialize alias from the Linux journald name.
    #[serde(default = "default_console", alias = "journal")]
    pub console: bool,
}

fn default_console() -> bool {
    true
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            console: true,
        }
    }
}

impl LoggingConfig {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        // Quiet mode (`console = false`) is allowed; there is no file sink.
        Ok(())
    }
}
