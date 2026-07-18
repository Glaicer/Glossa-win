use std::{collections::BTreeSet, env, fs, path::Path};

use glossa_app::AppError;
use glossa_core::{AppConfig, InputBackend, InputMode, PasteMode, ProviderKind, UiTheme};

use crate::setup::set_user_environment_variable;

/// User env var used when an LLM API key is pasted in Settings (never written literally to config).
pub(super) const LLM_API_KEY_ENV: &str = "GLOSSA_LLM_API_KEY";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SettingsValues {
    pub(super) input_backend: InputBackend,
    pub(super) input_mode: InputMode,
    pub(super) shortcut: String,
    pub(super) enable_cli: bool,
    pub(super) provider_kind: ProviderKind,
    pub(super) provider_base_url: String,
    pub(super) provider_model: String,
    pub(super) provider_api_key: String,
    pub(super) llm_enabled: bool,
    pub(super) llm_base_url: String,
    pub(super) llm_model: String,
    pub(super) llm_api_key: String,
    pub(super) paste_mode: PasteMode,
    pub(super) append_space: bool,
    pub(super) ui_theme: UiTheme,
}

impl SettingsValues {
    pub(super) fn from_config(config: &AppConfig) -> Self {
        Self {
            input_backend: config.input.backend,
            input_mode: config.input.mode,
            shortcut: config.input.shortcut.clone(),
            enable_cli: config.control.enable_cli,
            provider_kind: config.provider.kind,
            provider_base_url: config.provider.base_url.clone().unwrap_or_default(),
            provider_model: config.provider.model.clone(),
            provider_api_key: String::from(config.provider.api_key.clone()),
            llm_enabled: config.llm.enabled,
            llm_base_url: config.llm.base_url.clone(),
            llm_model: config.llm.model.clone(),
            llm_api_key: String::from(config.llm.api_key.clone()),
            paste_mode: config.paste.mode,
            append_space: config.paste.append_space,
            ui_theme: config.ui.theme,
        }
    }
}

pub(super) fn apply_settings_to_config(
    source: &str,
    settings: &SettingsValues,
) -> Result<String, AppError> {
    let updates = build_updates(settings);
    let mut seen = BTreeSet::new();
    let mut current_section = None::<&str>;
    let mut updated = String::with_capacity(source.len() + 128);

    for raw_line in lines_with_endings(source) {
        let (line, newline) = split_line_ending(raw_line);

        if let Some(section) = parse_section_name(line) {
            maybe_insert_missing_supported_keys(current_section, &updates, &mut seen, &mut updated);
            current_section = Some(section);
            updated.push_str(line);
            updated.push_str(newline);
            continue;
        }

        let replacement = current_section.and_then(|section| {
            updates
                .iter()
                .find(|update| update.section == section && line_matches_key(line, update.key))
        });

        if let Some(update) = replacement {
            updated.push_str(&replace_line_value(line, &update.value));
            updated.push_str(newline);
            seen.insert((update.section, update.key));
            continue;
        }

        updated.push_str(line);
        updated.push_str(newline);
    }

    maybe_insert_missing_supported_keys(current_section, &updates, &mut seen, &mut updated);
    insert_missing_supported_sections(&updates, &mut seen, &mut updated);

    let missing: Vec<String> = updates
        .iter()
        .filter(|update| !seen.contains(&(update.section, update.key)))
        .map(|update| format!("[{}].{}", update.section, update.key))
        .collect();
    if !missing.is_empty() {
        return Err(AppError::message(format!(
            "config.toml is missing required setting lines: {}",
            missing.join(", ")
        )));
    }

    AppConfig::from_toml_str(&updated)?;
    Ok(updated)
}

fn maybe_insert_missing_supported_keys(
    current_section: Option<&str>,
    updates: &[SettingUpdate],
    seen: &mut BTreeSet<(&'static str, &'static str)>,
    updated: &mut String,
) {
    let missing: Vec<&SettingUpdate> = updates
        .iter()
        .filter(|update| {
            current_section == Some(update.section)
                && is_supported_missing_key(update)
                && !seen.contains(&(update.section, update.key))
        })
        .collect();
    if missing.is_empty() {
        return;
    }

    let trailing_blank_lines = take_trailing_blank_lines(updated);
    for update in missing {
        insert_missing_setting(update, seen, updated);
    }
    updated.push_str(&trailing_blank_lines);
}

fn insert_missing_setting(
    update: &SettingUpdate,
    seen: &mut BTreeSet<(&'static str, &'static str)>,
    updated: &mut String,
) {
    updated.push_str(update.key);
    updated.push_str(" = ");
    updated.push_str(&update.value);
    updated.push('\n');
    seen.insert((update.section, update.key));
}

fn is_supported_missing_key(update: &SettingUpdate) -> bool {
    matches!(
        (update.section, update.key),
        ("paste", "append_space")
            | ("LLM", "enabled")
            | ("LLM", "base_url")
            | ("LLM", "model")
            | ("LLM", "api_key")
    )
}

fn insert_missing_supported_sections(
    updates: &[SettingUpdate],
    seen: &mut BTreeSet<(&'static str, &'static str)>,
    updated: &mut String,
) {
    insert_missing_section("LLM", updates, seen, updated);
}

fn insert_missing_section(
    section: &'static str,
    updates: &[SettingUpdate],
    seen: &mut BTreeSet<(&'static str, &'static str)>,
    updated: &mut String,
) {
    let section_updates: Vec<&SettingUpdate> = updates
        .iter()
        .filter(|update| update.section == section)
        .collect();
    if section_updates.is_empty()
        || section_updates
            .iter()
            .any(|update| seen.contains(&(update.section, update.key)))
    {
        return;
    }

    if !updated.is_empty() {
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        if !updated.ends_with("\n\n") {
            updated.push('\n');
        }
    }
    updated.push('[');
    updated.push_str(section);
    updated.push_str("]\n");
    for update in section_updates {
        insert_missing_setting(update, seen, updated);
    }
}

fn take_trailing_blank_lines(updated: &mut String) -> String {
    let mut trailing = String::new();

    loop {
        let Some(line_end) = updated.rfind('\n') else {
            break;
        };
        let line_start = updated[..line_end].rfind('\n').map_or(0, |index| index + 1);
        let line = &updated[line_start..line_end];

        if !line.trim().is_empty() {
            break;
        }

        let removed = updated.split_off(line_start);
        trailing.insert_str(0, &removed);
    }

    trailing
}

pub(super) fn write_settings_to_config(
    path: &Path,
    settings: &mut SettingsValues,
) -> Result<(), AppError> {
    settings.llm_api_key = secure_llm_api_key_for_config(&settings.llm_api_key)?;
    let source = fs::read_to_string(path)
        .map_err(|error| AppError::io("failed to read config.toml", error))?;
    let updated = apply_settings_to_config(&source, settings)?;
    fs::write(path, updated).map_err(|error| AppError::io("failed to write config.toml", error))
}

/// Turns a pasted LLM API key into a config-safe value.
///
/// - empty → empty (local models with no auth)
/// - `env:NAME` → kept as-is
/// - any other text (pasted secret) → stored in the user environment as
///   [`LLM_API_KEY_ENV`], config gets `env:GLOSSA_LLM_API_KEY`
pub(super) fn secure_llm_api_key_for_config(entered: &str) -> Result<String, AppError> {
    match plan_llm_api_key_persist(entered)? {
        LlmApiKeyPersist::Empty => Ok(String::new()),
        LlmApiKeyPersist::EnvRef(value) => Ok(value),
        LlmApiKeyPersist::StoreSecret { env_name, secret } => {
            set_user_environment_variable(env_name, &secret)?;
            // So a same-process restart resolves the key without requiring a full logoff.
            // SAFETY: settings save path; tray is single-threaded for this call.
            unsafe {
                env::set_var(env_name, &secret);
            }
            Ok(format!("env:{env_name}"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LlmApiKeyPersist {
    Empty,
    EnvRef(String),
    StoreSecret {
        env_name: &'static str,
        secret: String,
    },
}

/// Pure planning step for LLM API key persistence (unit-tested without registry writes).
pub(super) fn plan_llm_api_key_persist(entered: &str) -> Result<LlmApiKeyPersist, AppError> {
    let trimmed = entered.trim();
    if trimmed.is_empty() {
        return Ok(LlmApiKeyPersist::Empty);
    }
    if let Some(name) = trimmed.strip_prefix("env:") {
        if name.is_empty() {
            return Err(AppError::message(
                "LLM API key env source must not be empty (use env:VARIABLE_NAME).",
            ));
        }
        return Ok(LlmApiKeyPersist::EnvRef(format!("env:{name}")));
    }
    Ok(LlmApiKeyPersist::StoreSecret {
        env_name: LLM_API_KEY_ENV,
        secret: trimmed.to_owned(),
    })
}

/// Resolves what to keep when the API key field is left blank in the LLM dialog.
#[must_use]
pub(super) fn keep_or_replace_llm_api_key(entered: &str, previous: &str) -> String {
    let trimmed = entered.trim();
    if trimmed.is_empty() {
        previous.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Whether the configured LLM API key actually resolves to a non-empty secret.
///
/// Config may hold `env:NAME` even when the variable is unset; the settings
/// dialog must not claim a key is saved in that case.
#[must_use]
pub(super) fn llm_api_key_is_available(configured: &str) -> bool {
    use glossa_core::SecretSource;

    let trimmed = configured.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Ok(source) = SecretSource::try_from(trimmed.to_owned()) else {
        return false;
    };
    match source.resolve() {
        Ok(value) => !value.trim().is_empty(),
        Err(_) => false,
    }
}

pub(super) fn input_backend_id(value: InputBackend) -> &'static str {
    match value {
        // Prefer the Windows-facing name; "portal" remains a accepted config alias.
        InputBackend::Portal => "hotkey",
        InputBackend::None => "none",
    }
}

pub(super) fn input_mode_id(value: InputMode) -> &'static str {
    match value {
        InputMode::PushToTalk => "push-to-talk",
        InputMode::Toggle => "toggle",
    }
}

pub(super) fn provider_kind_id(value: ProviderKind) -> &'static str {
    match value {
        ProviderKind::Groq => "groq",
        ProviderKind::OpenAi => "openai",
        ProviderKind::OpenAiCompatible => "openai-compatible",
    }
}

pub(super) fn paste_mode_id(value: PasteMode) -> &'static str {
    match value {
        PasteMode::CtrlV => "ctrl-v",
        PasteMode::CtrlShiftV => "ctrl-shift-v",
        PasteMode::ShiftInsert => "shift-insert",
    }
}

pub(super) fn ui_theme_id(value: UiTheme) -> &'static str {
    match value {
        UiTheme::Dark => "dark",
        UiTheme::Light => "light",
    }
}

fn build_updates(settings: &SettingsValues) -> Vec<SettingUpdate> {
    vec![
        SettingUpdate::new(
            "input",
            "backend",
            quoted(input_backend_id(settings.input_backend)),
        ),
        SettingUpdate::new("input", "mode", quoted(input_mode_id(settings.input_mode))),
        SettingUpdate::new("input", "shortcut", quoted(&settings.shortcut)),
        SettingUpdate::new("control", "enable_cli", settings.enable_cli.to_string()),
        SettingUpdate::new(
            "provider",
            "kind",
            quoted(provider_kind_id(settings.provider_kind)),
        ),
        SettingUpdate::new("provider", "base_url", quoted(&settings.provider_base_url)),
        SettingUpdate::new("provider", "model", quoted(&settings.provider_model)),
        SettingUpdate::new("provider", "api_key", quoted(&settings.provider_api_key)),
        SettingUpdate::new("LLM", "enabled", settings.llm_enabled.to_string()),
        SettingUpdate::new("LLM", "base_url", quoted(&settings.llm_base_url)),
        SettingUpdate::new("LLM", "model", quoted(&settings.llm_model)),
        SettingUpdate::new("LLM", "api_key", quoted(&settings.llm_api_key)),
        SettingUpdate::new("paste", "mode", quoted(paste_mode_id(settings.paste_mode))),
        SettingUpdate::new("paste", "append_space", settings.append_space.to_string()),
        SettingUpdate::new("ui", "theme", quoted(ui_theme_id(settings.ui_theme))),
    ]
}

fn quoted(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');

    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\u{08}' => escaped.push_str("\\b"),
            '\u{0C}' => escaped.push_str("\\f"),
            _ => escaped.push(ch),
        }
    }

    escaped.push('"');
    escaped
}

fn lines_with_endings(input: &str) -> Vec<&str> {
    if input.is_empty() {
        return Vec::new();
    }

    input.split_inclusive('\n').collect()
}

fn split_line_ending(line: &str) -> (&str, &str) {
    line.strip_suffix('\n')
        .map_or((line, ""), |line| (line, "\n"))
}

fn parse_section_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.starts_with('#') {
        return None;
    }

    trimmed.strip_prefix('[')?.strip_suffix(']')
}

fn line_matches_key(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }

    let Some(eq_index) = trimmed.find('=') else {
        return false;
    };

    trimmed[..eq_index].trim_end() == key
}

fn replace_line_value(line: &str, new_value: &str) -> String {
    let trimmed = line.trim_start();
    let leading_offset = line.len() - trimmed.len();
    let eq_index = leading_offset
        + trimmed
            .find('=')
            .expect("matched key lines must contain '='");
    let prefix = &line[..=eq_index];
    let after_eq = &line[eq_index + 1..];
    let comment_index = find_comment_start(after_eq).unwrap_or(after_eq.len());
    let value_segment = &after_eq[..comment_index];
    let leading_ws_len =
        value_segment.len() - value_segment.trim_start_matches(char::is_whitespace).len();
    let trailing_ws_len =
        value_segment.len() - value_segment.trim_end_matches(char::is_whitespace).len();
    let leading_ws = &value_segment[..leading_ws_len];
    let trailing_ws = &value_segment[value_segment.len() - trailing_ws_len..];
    let suffix = &after_eq[comment_index..];

    format!("{prefix}{leading_ws}{new_value}{trailing_ws}{suffix}")
}

fn find_comment_start(value: &str) -> Option<usize> {
    let mut in_basic_string = false;
    let mut in_literal_string = false;
    let mut escape_next = false;

    for (index, ch) in value.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_basic_string => escape_next = true,
            '"' if !in_literal_string => in_basic_string = !in_basic_string,
            '\'' if !in_basic_string => in_literal_string = !in_literal_string,
            '#' if !in_basic_string && !in_literal_string => return Some(index),
            _ => {}
        }
    }

    None
}

struct SettingUpdate {
    section: &'static str,
    key: &'static str,
    value: String,
}

impl SettingUpdate {
    fn new(section: &'static str, key: &'static str, value: String) -> Self {
        Self {
            section,
            key,
            value,
        }
    }
}

#[cfg(test)]
mod tests {
    use glossa_core::{InputBackend, InputMode, PasteMode, ProviderKind, UiTheme};

    use super::{
        apply_settings_to_config, keep_or_replace_llm_api_key, llm_api_key_is_available,
        plan_llm_api_key_persist, quoted, LlmApiKeyPersist, SettingsValues, LLM_API_KEY_ENV,
    };

    fn valid_config_source() -> String {
        r#"[input]
# Backend comment
backend = "portal"
mode = "push-to-talk"
shortcut = "<Ctrl><Alt>space"

[control]
enable_cli = true # keep this comment
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
max_duration_sec = 120
persist_audio = false

[paste]
mode = "ctrl-v"
append_space = false # keep this comment
clipboard_command = "wl-copy"
type_command = "dotoolc"

[ui]
theme = "dark"
tray = true
idle_icon = "/tmp/idle.png"
recording_icon = "/tmp/recording.png"
processing_icon = "/tmp/processing.png"
idle_dark_icon = "/tmp/idle_dark.png"
recording_dark_icon = "/tmp/recording_dark.png"
processing_dark_icon = "/tmp/processing_dark.png"
start_sound = "/tmp/start.wav"
stop_sound = "/tmp/stop.wav"

[logging]
level = "info"
journal = true
file = false

[LLM]
enabled = false
base_url = ""
model = ""
api_key = ""
"#
        .into()
    }

    fn updated_settings() -> SettingsValues {
        SettingsValues {
            input_backend: InputBackend::None,
            input_mode: InputMode::Toggle,
            shortcut: "Ctrl+Shift+Space".into(),
            enable_cli: false,
            provider_kind: ProviderKind::OpenAiCompatible,
            provider_base_url: "https://example.com/v1".into(),
            provider_model: "custom-model".into(),
            provider_api_key: "env:CUSTOM_KEY".into(),
            llm_enabled: true,
            llm_base_url: "https://llm.example.com/v1".into(),
            llm_model: "custom-llm".into(),
            llm_api_key: "env:LLM_KEY".into(),
            paste_mode: PasteMode::ShiftInsert,
            append_space: true,
            ui_theme: UiTheme::Light,
        }
    }

    #[test]
    fn apply_settings_to_config_should_update_only_supported_lines_and_preserve_comments() {
        let updated = apply_settings_to_config(&valid_config_source(), &updated_settings())
            .expect("config patch should succeed");

        let expected = r#"[input]
# Backend comment
backend = "none"
mode = "toggle"
shortcut = "Ctrl+Shift+Space"

[control]
enable_cli = false # keep this comment
socket_path = "auto"

[provider]
kind = "openai-compatible"
base_url = "https://example.com/v1"
model = "custom-model"
api_key = "env:CUSTOM_KEY"

[audio]
enabled = true
work_dir = "auto"
format = "wav"
sample_rate_hz = 16000
channels = 1
trim_silence = true
trim_threshold = 500
min_duration_ms = 150
max_duration_sec = 120
persist_audio = false

[paste]
mode = "shift-insert"
append_space = true # keep this comment
clipboard_command = "wl-copy"
type_command = "dotoolc"

[ui]
theme = "light"
tray = true
idle_icon = "/tmp/idle.png"
recording_icon = "/tmp/recording.png"
processing_icon = "/tmp/processing.png"
idle_dark_icon = "/tmp/idle_dark.png"
recording_dark_icon = "/tmp/recording_dark.png"
processing_dark_icon = "/tmp/processing_dark.png"
start_sound = "/tmp/start.wav"
stop_sound = "/tmp/stop.wav"

[logging]
level = "info"
journal = true
file = false

[LLM]
enabled = true
base_url = "https://llm.example.com/v1"
model = "custom-llm"
api_key = "env:LLM_KEY"
"#;

        assert_eq!(updated, expected);
    }

    #[test]
    fn apply_settings_to_config_should_fail_when_a_supported_key_is_missing() {
        let source = valid_config_source().replace("mode = \"push-to-talk\"\n", "");

        let error = apply_settings_to_config(&source, &updated_settings())
            .expect_err("missing mode line should fail");

        assert!(error.to_string().contains("[input].mode"));
    }

    #[test]
    fn apply_settings_to_config_should_insert_missing_append_space_into_paste_section() {
        let source =
            valid_config_source().replace("append_space = false # keep this comment\n", "");

        let updated = apply_settings_to_config(&source, &updated_settings())
            .expect("missing append_space should be inserted");

        let expected = r#"[input]
# Backend comment
backend = "none"
mode = "toggle"
shortcut = "Ctrl+Shift+Space"

[control]
enable_cli = false # keep this comment
socket_path = "auto"

[provider]
kind = "openai-compatible"
base_url = "https://example.com/v1"
model = "custom-model"
api_key = "env:CUSTOM_KEY"

[audio]
enabled = true
work_dir = "auto"
format = "wav"
sample_rate_hz = 16000
channels = 1
trim_silence = true
trim_threshold = 500
min_duration_ms = 150
max_duration_sec = 120
persist_audio = false

[paste]
mode = "shift-insert"
clipboard_command = "wl-copy"
type_command = "dotoolc"
append_space = true

[ui]
theme = "light"
tray = true
idle_icon = "/tmp/idle.png"
recording_icon = "/tmp/recording.png"
processing_icon = "/tmp/processing.png"
idle_dark_icon = "/tmp/idle_dark.png"
recording_dark_icon = "/tmp/recording_dark.png"
processing_dark_icon = "/tmp/processing_dark.png"
start_sound = "/tmp/start.wav"
stop_sound = "/tmp/stop.wav"

[logging]
level = "info"
journal = true
file = false

[LLM]
enabled = true
base_url = "https://llm.example.com/v1"
model = "custom-llm"
api_key = "env:LLM_KEY"
"#;

        assert_eq!(updated, expected);
    }

    #[test]
    fn apply_settings_to_config_should_insert_missing_llm_section() {
        let source = valid_config_source().replace(
            r#"
[LLM]
enabled = false
base_url = ""
model = ""
api_key = ""
"#,
            "",
        );

        let updated = apply_settings_to_config(&source, &updated_settings())
            .expect("missing LLM section should be inserted");

        assert!(updated.contains(
            r#"[LLM]
enabled = true
base_url = "https://llm.example.com/v1"
model = "custom-llm"
api_key = "env:LLM_KEY"
"#
        ));
    }

    #[test]
    fn quoted_should_escape_backslashes_and_quotes() {
        let quoted = quoted(r#"env:\"quoted\"\key"#);

        assert_eq!(quoted, r#""env:\\\"quoted\\\"\\key""#);
    }

    #[test]
    fn plan_llm_api_key_should_store_pasted_secret_in_fixed_env_var() {
        let plan = plan_llm_api_key_persist("  sk-or-v1-secret  ").expect("plan");
        assert_eq!(
            plan,
            LlmApiKeyPersist::StoreSecret {
                env_name: LLM_API_KEY_ENV,
                secret: "sk-or-v1-secret".into(),
            }
        );
    }

    #[test]
    fn plan_llm_api_key_should_keep_env_refs() {
        let plan = plan_llm_api_key_persist("env:OPENROUTER_API_KEY").expect("plan");
        assert_eq!(
            plan,
            LlmApiKeyPersist::EnvRef("env:OPENROUTER_API_KEY".into())
        );
    }

    #[test]
    fn plan_llm_api_key_should_treat_blank_as_empty() {
        assert_eq!(
            plan_llm_api_key_persist("   ").expect("plan"),
            LlmApiKeyPersist::Empty
        );
    }

    #[test]
    fn plan_llm_api_key_should_reject_empty_env_name() {
        let err = plan_llm_api_key_persist("env:").expect_err("empty env name");
        assert!(err.to_string().contains("env"));
    }

    #[test]
    fn keep_or_replace_should_preserve_previous_when_field_blank() {
        assert_eq!(
            keep_or_replace_llm_api_key("", "env:GLOSSA_LLM_API_KEY"),
            "env:GLOSSA_LLM_API_KEY"
        );
        assert_eq!(
            keep_or_replace_llm_api_key("  ", "env:GLOSSA_LLM_API_KEY"),
            "env:GLOSSA_LLM_API_KEY"
        );
    }

    #[test]
    fn keep_or_replace_should_use_new_value_when_entered() {
        assert_eq!(
            keep_or_replace_llm_api_key("sk-new", "env:GLOSSA_LLM_API_KEY"),
            "sk-new"
        );
    }

    #[test]
    fn llm_api_key_is_available_should_be_false_for_empty() {
        assert!(!llm_api_key_is_available(""));
        assert!(!llm_api_key_is_available("   "));
    }

    #[test]
    fn llm_api_key_is_available_should_be_true_for_literal() {
        assert!(llm_api_key_is_available("sk-test-key"));
    }

    #[test]
    fn llm_api_key_is_available_should_be_false_for_missing_env() {
        assert!(!llm_api_key_is_available(
            "env:GLOSSA_TEST_LLM_KEY_THAT_MUST_NOT_EXIST_9f3a2c"
        ));
    }
}
