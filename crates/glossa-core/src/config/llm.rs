use serde::{Deserialize, Serialize};

use super::SecretSource;
use crate::CoreError;

/// LLM enhancement settings for transcription post-processing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmConfig {
    pub enabled: bool,
    pub base_url: String,
    pub model: String,
    pub api_key: SecretSource,
}

impl LlmConfig {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if !self.enabled {
            return Ok(());
        }
        if self.base_url.trim().is_empty() {
            return Err(CoreError::InvalidConfig(
                "llm.base_url must not be empty when enabled".into(),
            ));
        }
        if self.model.trim().is_empty() {
            return Err(CoreError::InvalidConfig(
                "llm.model must not be empty when enabled".into(),
            ));
        }
        Ok(())
    }

    /// Resolves the LLM API key without logging the secret.
    pub fn resolve_api_key(&self) -> Result<String, CoreError> {
        self.api_key.resolve()
    }

    /// If the enhancer is enabled but its API key cannot be resolved (for example
    /// a missing `env:…` variable), turns the enhancer off.
    ///
    /// Empty keys still resolve successfully (local/no-auth models), so those
    /// stay enabled. Returns `true` when a disable happened.
    pub fn disable_if_api_key_unavailable(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.resolve_api_key().is_ok() {
            return false;
        }
        self.enabled = false;
        true
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            model: String::new(),
            api_key: SecretSource::Empty,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppConfig;

    #[test]
    fn default_llm_config_should_be_disabled() {
        let config = LlmConfig::default();
        assert!(!config.enabled);
        assert!(config.base_url.is_empty());
        assert!(config.model.is_empty());
        assert_eq!(config.api_key, SecretSource::Empty);
    }

    #[test]
    fn disabled_llm_should_allow_empty_base_url_and_model() {
        let config = LlmConfig {
            enabled: false,
            base_url: String::new(),
            model: String::new(),
            api_key: SecretSource::Empty,
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn enabled_llm_should_require_non_empty_base_url() {
        let config = LlmConfig {
            enabled: true,
            base_url: String::new(),
            model: "gpt-4".into(),
            api_key: SecretSource::Empty,
        };
        let error = config.validate().expect_err("validation should fail");
        assert!(error.to_string().contains("base_url"));
    }

    #[test]
    fn enabled_llm_should_require_non_empty_model() {
        let config = LlmConfig {
            enabled: true,
            base_url: "http://localhost:11434/v1".into(),
            model: String::new(),
            api_key: SecretSource::Empty,
        };
        let error = config.validate().expect_err("validation should fail");
        assert!(error.to_string().contains("model"));
    }

    #[test]
    fn enabled_llm_should_accept_empty_api_key() {
        let config = LlmConfig {
            enabled: true,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3".into(),
            api_key: SecretSource::Empty,
        };
        assert!(config.validate().is_ok());
        assert_eq!(
            config
                .resolve_api_key()
                .expect("empty secret should resolve"),
            ""
        );
    }

    #[test]
    fn disable_if_api_key_unavailable_should_leave_empty_key_enabled() {
        let mut config = LlmConfig {
            enabled: true,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3".into(),
            api_key: SecretSource::Empty,
        };
        assert!(!config.disable_if_api_key_unavailable());
        assert!(config.enabled);
    }

    #[test]
    fn disable_if_api_key_unavailable_should_turn_off_when_env_missing() {
        let mut config = LlmConfig {
            enabled: true,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3".into(),
            api_key: SecretSource::Env(
                "GLOSSA_TEST_LLM_KEY_THAT_MUST_NOT_EXIST_9f3a2c".into(),
            ),
        };
        assert!(config.disable_if_api_key_unavailable());
        assert!(!config.enabled);
    }

    #[test]
    fn disable_if_api_key_unavailable_should_noop_when_already_disabled() {
        let mut config = LlmConfig {
            enabled: false,
            base_url: "http://localhost:11434/v1".into(),
            model: "llama3".into(),
            api_key: SecretSource::Env(
                "GLOSSA_TEST_LLM_KEY_THAT_MUST_NOT_EXIST_9f3a2c".into(),
            ),
        };
        assert!(!config.disable_if_api_key_unavailable());
        assert!(!config.enabled);
    }

    #[test]
    fn app_config_should_accept_no_llm_section() {
        let config = AppConfig::from_toml_str(
            r#"
[input]
backend = "hotkey"
shortcut = "Ctrl+Alt+Space"
mode = "toggle"

[control]
enable_cli = true
socket_path = "auto"

[provider]
kind = "groq"
base_url = "https://api.groq.com/openai/v1"
model = "whisper-large-v3"
api_key = "env:GROQ_API_KEY"

[audio]
enabled = true
work_dir = "auto"
format = "wav"
sample_rate_hz = 16000
channels = 1
trim_silence = true
trim_threshold = 500
min_duration_ms = 150
max_duration_sec = 1200
persist_audio = false

[paste]
mode = "ctrl-v"
append_space = false

[ui]
tray = true
idle_icon = "C:\\idle.png"
recording_icon = "C:\\recording.png"
start_sound = "C:\\start.wav"
stop_sound = "C:\\stop.wav"

[logging]
level = "warn"
console = true
"#,
        )
        .expect("config should parse");

        assert!(!config.llm.enabled);
        assert!(config.llm.base_url.is_empty());
        assert!(config.llm.model.is_empty());
    }

    #[test]
    fn app_config_should_accept_llm_section() {
        let config = AppConfig::from_toml_str(
            r#"
[input]
backend = "hotkey"
shortcut = "Ctrl+Alt+Space"
mode = "toggle"

[control]
enable_cli = true
socket_path = "auto"

[provider]
kind = "groq"
base_url = "https://api.groq.com/openai/v1"
model = "whisper-large-v3"
api_key = "env:GROQ_API_KEY"

[audio]
enabled = true
work_dir = "auto"
format = "wav"
sample_rate_hz = 16000
channels = 1
trim_silence = true
trim_threshold = 500
min_duration_ms = 150
max_duration_sec = 1200
persist_audio = false

[paste]
mode = "ctrl-v"
append_space = false

[ui]
tray = true
idle_icon = "C:\\idle.png"
recording_icon = "C:\\recording.png"
start_sound = "C:\\start.wav"
stop_sound = "C:\\stop.wav"

[logging]
level = "warn"
console = true

[LLM]
enabled = true
base_url = "http://localhost:11434/v1"
model = "llama3"
api_key = "env:LLM_API_KEY"
"#,
        )
        .expect("config should parse");

        assert!(config.llm.enabled);
        assert_eq!(config.llm.base_url, "http://localhost:11434/v1");
        assert_eq!(config.llm.model, "llama3");
        assert_eq!(config.llm.api_key.describe(), "env:LLM_API_KEY");
    }
}
