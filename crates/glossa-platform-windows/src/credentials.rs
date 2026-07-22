//! Windows Credential Manager storage for API keys.
//!
//! Secrets are stored as generic credentials under target names `glossa/<NAME>`,
//! where `NAME` matches the historical environment variable (for example
//! `GROQ_API_KEY`). Config references them as `cred:NAME`.

use std::path::Path;
use std::sync::Arc;

use glossa_app::AppError;
use glossa_core::{set_credential_store, AppConfig, CredentialStore, SecretSource};
use tracing::{info, warn};

/// Prefix applied to logical secret names when writing Credential Manager targets.
pub const CREDENTIAL_TARGET_PREFIX: &str = "glossa/";

/// Builds the Credential Manager target for a logical secret name.
#[must_use]
pub fn credential_target(name: &str) -> String {
    format!("{CREDENTIAL_TARGET_PREFIX}{name}")
}

/// Installs the Windows Credential Manager backend for [`SecretSource::Credential`].
pub fn install_default_store() {
    set_credential_store(Some(Arc::new(WindowsCredentialStore)));
}

struct WindowsCredentialStore;

impl CredentialStore for WindowsCredentialStore {
    fn get(&self, name: &str) -> Result<Option<String>, String> {
        read_secret(name).map_err(|error| error.to_string())
    }
}

/// Stores `secret` under the logical name `name` (target `glossa/<name>`).
pub fn store_secret(name: &str, secret: &str) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        store_secret_windows(name, secret)
    }
    #[cfg(not(windows))]
    {
        let _ = (name, secret);
        Err(AppError::message(
            "Credential Manager storage is only supported on Windows",
        ))
    }
}

/// Reads a secret by logical name. Returns `Ok(None)` when no entry exists.
pub fn read_secret(name: &str) -> Result<Option<String>, AppError> {
    #[cfg(windows)]
    {
        read_secret_windows(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Err(AppError::message(
            "Credential Manager storage is only supported on Windows",
        ))
    }
}

/// Deletes a secret by logical name. Missing entries are not an error.
pub fn delete_secret(name: &str) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        delete_secret_windows(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Err(AppError::message(
            "Credential Manager storage is only supported on Windows",
        ))
    }
}

/// Reads a user-scoped environment variable from `HKCU\Environment` (not process env).
pub fn get_user_environment_variable(name: &str) -> Result<Option<String>, AppError> {
    #[cfg(windows)]
    {
        get_user_environment_variable_windows(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Ok(None)
    }
}

/// Deletes a user-scoped environment variable from `HKCU\Environment`.
/// Missing values are not an error.
pub fn delete_user_environment_variable(name: &str) -> Result<(), AppError> {
    #[cfg(windows)]
    {
        delete_user_environment_variable_windows(name)
    }
    #[cfg(not(windows))]
    {
        let _ = name;
        Ok(())
    }
}

/// Migrates provider/LLM secrets from `HKCU\Environment` (and process env fallback)
/// into Credential Manager, rewrites config `env:` → `cred:`, and clears leftover
/// user environment values.
pub fn migrate_secrets_from_environment(
    config_path: &Path,
    config: &mut AppConfig,
) -> Result<(), AppError> {
    install_default_store();

    let provider_fallback = match config.provider.kind {
        glossa_core::ProviderKind::Groq => Some("GROQ_API_KEY"),
        glossa_core::ProviderKind::OpenAi | glossa_core::ProviderKind::OpenAiCompatible => {
            Some("OPENAI_API_KEY")
        }
    };
    let provider_changed = migrate_one_secret(
        &mut config.provider.api_key,
        provider_fallback,
        "provider API key",
    )?;
    let llm_changed = migrate_one_secret(
        &mut config.llm.api_key,
        Some("GLOSSA_LLM_API_KEY"),
        "LLM API key",
    )?;

    if provider_changed || llm_changed {
        rewrite_config_secret_sources(
            config_path,
            &config.provider.api_key,
            &config.llm.api_key,
        )?;
        info!("migrated API key storage from user environment to Credential Manager");
    }

    Ok(())
}

fn migrate_one_secret(
    source: &mut SecretSource,
    literal_fallback_name: Option<&str>,
    label: &str,
) -> Result<bool, AppError> {
    let plan = plan_secret_migration(
        source,
        literal_fallback_name,
        |name| get_user_environment_variable(name).unwrap_or(None),
        |name| std::env::var(name).ok(),
        |name| read_secret(name).unwrap_or(None),
    );

    match plan {
        SecretMigrationPlan::Unchanged => Ok(false),
        SecretMigrationPlan::ConfigOnlyToCredential { name, clear_user_env } => {
            if clear_user_env {
                if let Err(error) = delete_user_environment_variable(&name) {
                    warn!(error = %error, name = %name, "failed to clear user environment variable after credential migration");
                }
            }
            *source = SecretSource::Credential(name);
            Ok(true)
        }
        SecretMigrationPlan::StoreAndUseCredential {
            name,
            secret,
            clear_user_env,
        } => {
            store_secret(&name, &secret)?;
            if clear_user_env {
                if let Err(error) = delete_user_environment_variable(&name) {
                    warn!(error = %error, name = %name, "failed to clear user environment variable after credential migration");
                }
            }
            // Drop process-level copy so later code does not prefer a stale env value.
            // SAFETY: migration runs on the daemon startup path before worker threads need this name.
            unsafe {
                std::env::remove_var(&name);
            }
            info!(secret = %label, name = %name, "stored secret in Credential Manager");
            *source = SecretSource::Credential(name);
            Ok(true)
        }
    }
}

/// Pure migration plan for one secret source (unit-tested without Win32).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretMigrationPlan {
    Unchanged,
    /// Config still said `env:` but the secret is already in Credential Manager.
    ConfigOnlyToCredential {
        name: String,
        clear_user_env: bool,
    },
    /// Persist `secret` under `name`, then switch config to `cred:name`.
    StoreAndUseCredential {
        name: String,
        secret: String,
        clear_user_env: bool,
    },
}

/// Decides how to migrate one secret. Lookup closures return values when present.
pub fn plan_secret_migration(
    source: &SecretSource,
    literal_fallback_name: Option<&str>,
    mut user_env: impl FnMut(&str) -> Option<String>,
    mut process_env: impl FnMut(&str) -> Option<String>,
    mut credential: impl FnMut(&str) -> Option<String>,
) -> SecretMigrationPlan {
    match source {
        SecretSource::Empty => SecretMigrationPlan::Unchanged,
        SecretSource::Literal(value) => {
            let secret = value.trim();
            if secret.is_empty() {
                return SecretMigrationPlan::Unchanged;
            }
            let Some(name) = literal_fallback_name.map(str::to_owned) else {
                return SecretMigrationPlan::Unchanged;
            };
            let user = user_env(&name);
            SecretMigrationPlan::StoreAndUseCredential {
                name,
                secret: secret.to_owned(),
                clear_user_env: user.is_some(),
            }
        }
        SecretSource::Env(name) | SecretSource::Credential(name) => {
            let name = name.clone();
            let in_cred = credential(&name);
            let user = user_env(&name);
            let process = process_env(&name);
            let clear_user_env = user.is_some();

            if in_cred.is_some() {
                // Already vaulted: still flip env: → cred: and scrub HKCU leftovers.
                if matches!(source, SecretSource::Env(_)) || clear_user_env {
                    return SecretMigrationPlan::ConfigOnlyToCredential {
                        name,
                        clear_user_env,
                    };
                }
                return SecretMigrationPlan::Unchanged;
            }

            if let Some(secret) = user.or(process) {
                if secret.trim().is_empty() {
                    return SecretMigrationPlan::Unchanged;
                }
                return SecretMigrationPlan::StoreAndUseCredential {
                    name,
                    secret,
                    clear_user_env,
                };
            }

            SecretMigrationPlan::Unchanged
        }
    }
}

fn rewrite_config_secret_sources(
    path: &Path,
    provider: &SecretSource,
    llm: &SecretSource,
) -> Result<(), AppError> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| AppError::io("failed to read config.toml for secret migration", error))?;
    let updated = patch_secret_sources_in_toml(&source, provider, llm);
    if updated != source {
        std::fs::write(path, updated).map_err(|error| {
            AppError::io("failed to write config.toml after secret migration", error)
        })?;
    }
    Ok(())
}

/// Updates `[provider].api_key` and `[LLM].api_key` lines to match the given sources.
pub fn patch_secret_sources_in_toml(
    source: &str,
    provider: &SecretSource,
    llm: &SecretSource,
) -> String {
    let provider_value = String::from(provider.clone());
    let llm_value = String::from(llm.clone());
    let mut current_section = None::<String>;
    let mut out = String::with_capacity(source.len() + 32);

    for raw_line in lines_with_endings(source) {
        let (line, newline) = split_line_ending(raw_line);
        if let Some(section) = parse_section_name(line) {
            current_section = Some(section.to_owned());
            out.push_str(line);
            out.push_str(newline);
            continue;
        }

        let replacement = match current_section.as_deref() {
            Some("provider") if line_matches_key(line, "api_key") => {
                Some(quoted_toml(&provider_value))
            }
            Some("LLM") if line_matches_key(line, "api_key") => Some(quoted_toml(&llm_value)),
            _ => None,
        };

        if let Some(value) = replacement {
            out.push_str(&replace_line_value(line, &value));
            out.push_str(newline);
        } else {
            out.push_str(line);
            out.push_str(newline);
        }
    }

    out
}

fn quoted_toml(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
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

#[cfg(windows)]
fn to_wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn store_secret_windows(name: &str, secret: &str) -> Result<(), AppError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::Credentials::{
        CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    let target = credential_target(name);
    let target_wide = to_wide(&target);
    let user_wide = to_wide("");
    let comment_wide = to_wide("Glossa API key");
    let mut blob: Vec<u8> = secret.as_bytes().to_vec();

    // SAFETY: CREDENTIALW pointers reference stack/local buffers that live for the CredWriteW call.
    unsafe {
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target_wide.as_ptr() as *mut u16,
            Comment: comment_wide.as_ptr() as *mut u16,
            LastWritten: std::mem::zeroed(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: user_wide.as_ptr() as *mut u16,
        };

        if CredWriteW(&credential, 0) == 0 {
            let status = GetLastError();
            return Err(AppError::message(format!(
                "failed to store credential {target} (error {status})"
            )));
        }
    }

    // Best-effort: wipe local copy of the secret bytes.
    for byte in &mut blob {
        *byte = 0;
    }
    Ok(())
}

#[cfg(windows)]
fn read_secret_windows(name: &str) -> Result<Option<String>, AppError> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
    };

    let target = credential_target(name);
    let target_wide = to_wide(&target);
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();

    // SAFETY: CredReadW allocates; CredFree releases. Blob is only read while pointer is valid.
    unsafe {
        if CredReadW(
            target_wide.as_ptr(),
            CRED_TYPE_GENERIC,
            0,
            &mut credential,
        ) == 0
        {
            let status = GetLastError();
            if status == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(AppError::message(format!(
                "failed to read credential {target} (error {status})"
            )));
        }

        let cred = &*credential;
        let bytes = if cred.CredentialBlob.is_null() || cred.CredentialBlobSize == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
                .to_vec()
        };
        CredFree(credential.cast());

        let secret = String::from_utf8(bytes).map_err(|_| {
            AppError::message(format!(
                "credential {target} is not valid UTF-8"
            ))
        })?;
        Ok(Some(secret))
    }
}

#[cfg(windows)]
fn delete_secret_windows(name: &str) -> Result<(), AppError> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};

    let target = credential_target(name);
    let target_wide = to_wide(&target);

    // SAFETY: target_wide is a valid NUL-terminated wide string for the duration of the call.
    unsafe {
        if CredDeleteW(target_wide.as_ptr(), CRED_TYPE_GENERIC, 0) == 0 {
            let status = GetLastError();
            if status == ERROR_NOT_FOUND {
                return Ok(());
            }
            return Err(AppError::message(format!(
                "failed to delete credential {target} (error {status})"
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn get_user_environment_variable_windows(name: &str) -> Result<Option<String>, AppError> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, KEY_QUERY_VALUE, REG_SZ,
    };

    let name_wide = to_wide(name);
    let sub_key = to_wide("Environment");

    // SAFETY: Registry APIs receive valid NUL-terminated wide strings.
    unsafe {
        let mut key = std::ptr::null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sub_key.as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        );
        if status != ERROR_SUCCESS {
            return Err(AppError::message(format!(
                "failed to open HKCU\\Environment (error {status})"
            )));
        }

        let mut value_type = 0u32;
        let mut byte_len = 0u32;
        let status = RegQueryValueExW(
            key,
            name_wide.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut byte_len,
        );
        if status == ERROR_FILE_NOT_FOUND {
            let _ = RegCloseKey(key);
            return Ok(None);
        }
        if status != ERROR_SUCCESS && status != ERROR_MORE_DATA {
            let _ = RegCloseKey(key);
            return Err(AppError::message(format!(
                "failed to query user environment variable {name} (error {status})"
            )));
        }

        let mut buffer = vec![0u16; (byte_len as usize / 2).max(1)];
        let mut buffer_bytes = (buffer.len() * 2) as u32;
        let status = RegQueryValueExW(
            key,
            name_wide.as_ptr(),
            std::ptr::null_mut(),
            &mut value_type,
            buffer.as_mut_ptr().cast::<u8>(),
            &mut buffer_bytes,
        );
        let _ = RegCloseKey(key);
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != ERROR_SUCCESS {
            return Err(AppError::message(format!(
                "failed to read user environment variable {name} (error {status})"
            )));
        }
        if value_type != REG_SZ && value_type != windows_sys::Win32::System::Registry::REG_EXPAND_SZ
        {
            return Err(AppError::message(format!(
                "user environment variable {name} has unsupported registry type {value_type}"
            )));
        }

        let utf16_len = (buffer_bytes as usize / 2).min(buffer.len());
        let wide = &buffer[..utf16_len];
        let end = wide.iter().position(|&c| c == 0).unwrap_or(wide.len());
        Ok(Some(String::from_utf16_lossy(&wide[..end])))
    }
}

#[cfg(windows)]
fn delete_user_environment_variable_windows(name: &str) -> Result<(), AppError> {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_SET_VALUE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let name_wide = to_wide(name);
    let sub_key = to_wide("Environment");

    // SAFETY: Registry APIs receive valid NUL-terminated wide strings.
    unsafe {
        let mut key = std::ptr::null_mut();
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sub_key.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        );
        if status != ERROR_SUCCESS {
            return Err(AppError::message(format!(
                "failed to open HKCU\\Environment (error {status})"
            )));
        }

        let status = RegDeleteValueW(key, name_wide.as_ptr());
        let _ = RegCloseKey(key);
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        if status != ERROR_SUCCESS {
            return Err(AppError::message(format!(
                "failed to delete user environment variable {name} (error {status})"
            )));
        }

        let env_wide = to_wide("Environment");
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            env_wide.as_ptr() as isize,
            SMTO_ABORTIFHUNG,
            5000,
            std::ptr::null_mut(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::SecretSource;

    #[test]
    fn credential_target_should_namespace_name() {
        assert_eq!(credential_target("GROQ_API_KEY"), "glossa/GROQ_API_KEY");
    }

    #[test]
    fn plan_should_migrate_env_from_user_environment() {
        let source = SecretSource::Env("GROQ_API_KEY".into());
        let plan = plan_secret_migration(
            &source,
            None,
            |_| Some("gsk_from_hkcu".into()),
            |_| None,
            |_| None,
        );
        assert_eq!(
            plan,
            SecretMigrationPlan::StoreAndUseCredential {
                name: "GROQ_API_KEY".into(),
                secret: "gsk_from_hkcu".into(),
                clear_user_env: true,
            }
        );
    }

    #[test]
    fn plan_should_prefer_existing_credential_and_flip_env_ref() {
        let source = SecretSource::Env("GROQ_API_KEY".into());
        let plan = plan_secret_migration(
            &source,
            None,
            |_| Some("stale".into()),
            |_| Some("also_stale".into()),
            |_| Some("vaulted".into()),
        );
        assert_eq!(
            plan,
            SecretMigrationPlan::ConfigOnlyToCredential {
                name: "GROQ_API_KEY".into(),
                clear_user_env: true,
            }
        );
    }

    #[test]
    fn plan_should_migrate_process_env_when_user_env_missing() {
        let source = SecretSource::Env("OPENAI_API_KEY".into());
        let plan = plan_secret_migration(
            &source,
            None,
            |_| None,
            |_| Some("sk-process".into()),
            |_| None,
        );
        assert_eq!(
            plan,
            SecretMigrationPlan::StoreAndUseCredential {
                name: "OPENAI_API_KEY".into(),
                secret: "sk-process".into(),
                clear_user_env: false,
            }
        );
    }

    #[test]
    fn plan_should_leave_empty_and_missing_unchanged() {
        assert_eq!(
            plan_secret_migration(&SecretSource::Empty, None, |_| None, |_| None, |_| None),
            SecretMigrationPlan::Unchanged
        );
        let source = SecretSource::Env("MISSING".into());
        assert_eq!(
            plan_secret_migration(&source, None, |_| None, |_| None, |_| None),
            SecretMigrationPlan::Unchanged
        );
    }

    #[test]
    fn plan_should_migrate_literal_llm_key_with_fallback_name() {
        let source = SecretSource::Literal("sk-literal".into());
        let plan = plan_secret_migration(
            &source,
            Some("GLOSSA_LLM_API_KEY"),
            |_| None,
            |_| None,
            |_| None,
        );
        assert_eq!(
            plan,
            SecretMigrationPlan::StoreAndUseCredential {
                name: "GLOSSA_LLM_API_KEY".into(),
                secret: "sk-literal".into(),
                clear_user_env: false,
            }
        );
    }

    #[test]
    fn plan_should_scrape_user_env_for_existing_cred_ref() {
        let source = SecretSource::Credential("GROQ_API_KEY".into());
        let plan = plan_secret_migration(
            &source,
            None,
            |_| Some("leftover".into()),
            |_| None,
            |_| Some("vaulted".into()),
        );
        assert_eq!(
            plan,
            SecretMigrationPlan::ConfigOnlyToCredential {
                name: "GROQ_API_KEY".into(),
                clear_user_env: true,
            }
        );
    }

    #[test]
    fn patch_secret_sources_should_update_provider_and_llm() {
        let source = r#"[provider]
api_key = "env:GROQ_API_KEY"

[LLM]
api_key = "env:GLOSSA_LLM_API_KEY"
"#;
        let updated = patch_secret_sources_in_toml(
            source,
            &SecretSource::Credential("GROQ_API_KEY".into()),
            &SecretSource::Credential("GLOSSA_LLM_API_KEY".into()),
        );
        assert!(updated.contains(r#"api_key = "cred:GROQ_API_KEY""#));
        assert!(updated.contains(r#"api_key = "cred:GLOSSA_LLM_API_KEY""#));
    }
}
