use std::{env, path::PathBuf};

use glossa_app::AppError;

/// Default install directory: `%LOCALAPPDATA%\glossa`.
pub fn default_install_dir() -> Result<PathBuf, AppError> {
    let local = env::var_os("LOCALAPPDATA").ok_or_else(|| {
        AppError::message("LOCALAPPDATA is required to resolve the Glossa install directory")
    })?;
    Ok(PathBuf::from(local).join("glossa"))
}

/// Default config path: `%APPDATA%\glossa\config.toml`.
pub fn default_config_path() -> Result<PathBuf, AppError> {
    let appdata = env::var_os("APPDATA").ok_or_else(|| {
        AppError::message("APPDATA is required to resolve the Glossa config path")
    })?;
    Ok(PathBuf::from(appdata).join("glossa").join("config.toml"))
}

/// Whether a usable install appears present (config file exists).
#[must_use]
pub fn is_installed() -> bool {
    default_config_path()
        .map(|path| path.is_file())
        .unwrap_or(false)
}

/// Status of a candidate config path for first-run detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigPathStatus {
    /// Config file exists and can be used.
    Ready(PathBuf),
    /// No config yet; first-run setup should run and write to this path.
    NeedsSetup(PathBuf),
}

impl ConfigPathStatus {
    /// Resolves an optional CLI config path into a ready path or a setup target.
    pub fn resolve(explicit: Option<PathBuf>) -> Result<Self, AppError> {
        let path = match explicit {
            Some(path) => path,
            None => default_config_path()?,
        };
        if path.is_file() {
            Ok(Self::Ready(path))
        } else {
            Ok(Self::NeedsSetup(path))
        }
    }

    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        match self {
            Self::Ready(path) | Self::NeedsSetup(path) => path,
        }
    }

    #[must_use]
    pub fn needs_setup(&self) -> bool {
        matches!(self, Self::NeedsSetup(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_paths_should_end_with_glossa() {
        let install = default_install_dir().expect("LOCALAPPDATA");
        assert!(install.ends_with("glossa"));
        let config = default_config_path().expect("APPDATA");
        assert!(config.ends_with(std::path::Path::new("glossa").join("config.toml")));
    }
}
