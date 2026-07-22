//! Secret sources and the optional credential-store backend.

use std::env;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::CoreError;

/// Backend used to resolve [`SecretSource::Credential`] values.
///
/// Platform code installs a concrete store (for example Windows Credential Manager)
/// at process start. Unit tests can install an in-memory store.
pub trait CredentialStore: Send + Sync {
    /// Returns the secret for `name`, or `None` when no entry exists.
    fn get(&self, name: &str) -> Result<Option<String>, String>;
}

static CREDENTIAL_STORE: RwLock<Option<Arc<dyn CredentialStore>>> = RwLock::new(None);

/// Installs (or clears, when `store` is `None`) the process-wide credential backend.
pub fn set_credential_store(store: Option<Arc<dyn CredentialStore>>) {
    if let Ok(mut guard) = CREDENTIAL_STORE.write() {
        *guard = store;
    }
}

fn credential_store() -> Option<Arc<dyn CredentialStore>> {
    CREDENTIAL_STORE
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

/// Secret source for API tokens and similar credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum SecretSource {
    Empty,
    Literal(String),
    Env(String),
    /// Named secret held in the platform credential store (`cred:NAME`).
    Credential(String),
}

impl SecretSource {
    /// Returns a redacted description suitable for logs and diagnostics.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Empty => "empty".into(),
            Self::Literal(_) => "literal".into(),
            Self::Env(name) => format!("env:{name}"),
            Self::Credential(name) => format!("cred:{name}"),
        }
    }

    /// Logical name for env/credential-backed secrets, when present.
    #[must_use]
    pub fn reference_name(&self) -> Option<&str> {
        match self {
            Self::Env(name) | Self::Credential(name) => Some(name.as_str()),
            Self::Empty | Self::Literal(_) => None,
        }
    }

    /// Resolves the secret into a string value.
    pub fn resolve(&self) -> Result<String, CoreError> {
        match self {
            Self::Empty => Ok(String::new()),
            Self::Literal(value) => Ok(value.clone()),
            Self::Env(name) => env::var(name).map_err(|_| CoreError::MissingSecret {
                secret_source: format!("env:{name}"),
            }),
            Self::Credential(name) => {
                let Some(store) = credential_store() else {
                    return Err(CoreError::MissingSecret {
                        secret_source: format!("cred:{name}"),
                    });
                };
                match store.get(name) {
                    Ok(Some(value)) => Ok(value),
                    Ok(None) => Err(CoreError::MissingSecret {
                        secret_source: format!("cred:{name}"),
                    }),
                    Err(message) => Err(CoreError::MissingSecret {
                        secret_source: format!("cred:{name} ({message})"),
                    }),
                }
            }
        }
    }
}

impl TryFrom<String> for SecretSource {
    type Error = CoreError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if let Some(name) = value.strip_prefix("env:") {
            if name.is_empty() {
                return Err(CoreError::InvalidConfig(
                    "provider.api_key env source must not be empty".into(),
                ));
            }
            return Ok(Self::Env(name.to_string()));
        }

        if let Some(name) = value.strip_prefix("cred:") {
            if name.is_empty() {
                return Err(CoreError::InvalidConfig(
                    "provider.api_key cred source must not be empty".into(),
                ));
            }
            return Ok(Self::Credential(name.to_string()));
        }

        if value.is_empty() {
            return Ok(Self::Empty);
        }

        Ok(Self::Literal(value))
    }
}

impl From<SecretSource> for String {
    fn from(value: SecretSource) -> Self {
        match value {
            SecretSource::Empty => String::new(),
            SecretSource::Literal(inner) => inner,
            SecretSource::Env(inner) => format!("env:{inner}"),
            SecretSource::Credential(inner) => format!("cred:{inner}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    struct MapStore(Mutex<HashMap<String, String>>);

    impl CredentialStore for MapStore {
        fn get(&self, name: &str) -> Result<Option<String>, String> {
            Ok(self
                .0
                .lock()
                .map_err(|_| "poisoned".to_owned())?
                .get(name)
                .cloned())
        }
    }

    #[test]
    fn secret_source_should_parse_cred_values() {
        let source = SecretSource::try_from("cred:GROQ_API_KEY".to_string())
            .expect("cred-backed secret should parse");
        assert_eq!(source, SecretSource::Credential("GROQ_API_KEY".into()));
        assert_eq!(source.describe(), "cred:GROQ_API_KEY");
        assert_eq!(source.reference_name(), Some("GROQ_API_KEY"));
    }

    #[test]
    fn secret_source_should_reject_empty_cred_name() {
        let error = SecretSource::try_from("cred:".to_string()).expect_err("empty name");
        assert!(error.to_string().contains("cred"));
    }

    #[test]
    fn credential_resolve_should_use_installed_store() {
        set_credential_store(Some(Arc::new(MapStore(Mutex::new(HashMap::from([(
            "TEST_KEY".into(),
            "secret-value".into(),
        )]))))));

        let source = SecretSource::Credential("TEST_KEY".into());
        assert_eq!(
            source.resolve().expect("credential should resolve"),
            "secret-value"
        );

        set_credential_store(None);
    }

    #[test]
    fn credential_resolve_should_fail_when_missing() {
        set_credential_store(Some(Arc::new(MapStore(Mutex::new(HashMap::new())))));
        let source = SecretSource::Credential("MISSING".into());
        let error = source.resolve().expect_err("missing credential");
        assert!(error.to_string().contains("cred:MISSING"));
        set_credential_store(None);
    }
}
