//! First-run setup wizard and local install helpers for Windows.

mod assets;
mod install;
mod paths;
mod wizard;

pub use install::{
    api_key_env_name, apply_setup, build_config_toml, SetupAnswers, SetupResult,
};
pub use paths::{
    default_config_path, default_install_dir, is_installed, ConfigPathStatus,
};
pub use wizard::{run_setup_wizard, WizardOutcome};
