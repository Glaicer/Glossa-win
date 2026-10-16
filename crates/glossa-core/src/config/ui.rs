use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::CoreError;

/// Color variant of the tray icons.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconStyle {
    #[default]
    Light,
    Dark,
}

/// Tray icons and cue sounds used by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiConfig {
    pub tray: bool,
    #[serde(default = "default_overlay")]
    pub overlay: bool,
    // Legacy key `theme` is still accepted; it used to also color the dialogs.
    #[serde(default, alias = "theme")]
    pub icon: IconStyle,
    pub idle_icon: Utf8PathBuf,
    pub recording_icon: Utf8PathBuf,
    pub processing_icon: Option<Utf8PathBuf>,
    pub idle_dark_icon: Option<Utf8PathBuf>,
    pub recording_dark_icon: Option<Utf8PathBuf>,
    pub processing_dark_icon: Option<Utf8PathBuf>,
    pub start_sound: Utf8PathBuf,
    pub stop_sound: Utf8PathBuf,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            tray: true,
            overlay: true,
            icon: IconStyle::Light,
            idle_icon: "/path/to/idle.png".into(),
            recording_icon: "/path/to/recording.png".into(),
            processing_icon: Some("/path/to/processing.png".into()),
            idle_dark_icon: Some("/path/to/idle_dark.png".into()),
            recording_dark_icon: Some("/path/to/recording_dark.png".into()),
            processing_dark_icon: Some("/path/to/processing_dark.png".into()),
            start_sound: "/path/to/start.wav".into(),
            stop_sound: "/path/to/stop.wav".into(),
        }
    }
}

fn default_overlay() -> bool {
    true
}

impl UiConfig {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        if self.idle_icon.as_str().is_empty() || self.recording_icon.as_str().is_empty() {
            return Err(CoreError::InvalidConfig(
                "ui icons must not be empty".into(),
            ));
        }
        validate_optional_path(&self.processing_icon, "ui processing_icon")?;
        validate_optional_path(&self.idle_dark_icon, "ui idle_dark_icon")?;
        validate_optional_path(&self.recording_dark_icon, "ui recording_dark_icon")?;
        validate_optional_path(&self.processing_dark_icon, "ui processing_dark_icon")?;
        if self.start_sound.as_str().is_empty() || self.stop_sound.as_str().is_empty() {
            return Err(CoreError::InvalidConfig(
                "ui sound paths must not be empty".into(),
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn idle_tray_icon(&self) -> &Utf8Path {
        match self.icon {
            IconStyle::Light => &self.idle_icon,
            IconStyle::Dark => self.idle_dark_icon.as_deref().unwrap_or(&self.idle_icon),
        }
    }

    #[must_use]
    pub fn recording_tray_icon(&self) -> &Utf8Path {
        match self.icon {
            IconStyle::Light => &self.recording_icon,
            IconStyle::Dark => self
                .recording_dark_icon
                .as_deref()
                .unwrap_or(&self.recording_icon),
        }
    }

    #[must_use]
    pub fn processing_tray_icon(&self) -> &Utf8Path {
        match self.icon {
            IconStyle::Light => self.processing_icon.as_deref().unwrap_or(&self.idle_icon),
            IconStyle::Dark => self
                .processing_dark_icon
                .as_deref()
                .or(self.idle_dark_icon.as_deref())
                .or(self.processing_icon.as_deref())
                .unwrap_or(&self.idle_icon),
        }
    }
}

fn validate_optional_path(path: &Option<Utf8PathBuf>, label: &str) -> Result<(), CoreError> {
    if path.as_ref().is_some_and(|path| path.as_str().is_empty()) {
        return Err(CoreError::InvalidConfig(format!(
            "{label} must not be empty"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{IconStyle, UiConfig};

    #[test]
    fn dark_icon_style_should_prefer_dark_icon_overrides() {
        let ui = UiConfig {
            icon: IconStyle::Dark,
            idle_dark_icon: Some(r"C:\glossa\idle_dark.png".into()),
            recording_dark_icon: Some(r"C:\glossa\recording_dark.png".into()),
            processing_dark_icon: Some(r"C:\glossa\processing_dark.png".into()),
            ..UiConfig::default()
        };

        assert_eq!(ui.idle_tray_icon().as_str(), r"C:\glossa\idle_dark.png");
        assert_eq!(
            ui.recording_tray_icon().as_str(),
            r"C:\glossa\recording_dark.png"
        );
        assert_eq!(
            ui.processing_tray_icon().as_str(),
            r"C:\glossa\processing_dark.png"
        );
    }

    #[test]
    fn dark_icon_style_should_fallback_to_light_icons() {
        let ui = UiConfig {
            icon: IconStyle::Dark,
            idle_dark_icon: None,
            recording_dark_icon: None,
            processing_dark_icon: None,
            ..UiConfig::default()
        };

        assert_eq!(ui.idle_tray_icon(), ui.idle_icon.as_path());
        assert_eq!(ui.recording_tray_icon(), ui.recording_icon.as_path());
        assert_eq!(
            ui.processing_tray_icon(),
            ui.processing_icon.as_deref().unwrap()
        );
    }

    #[test]
    fn changing_icon_style_should_switch_selected_tray_icon_paths() {
        // Regression: settings save only updated config.ui.icon; tray must re-resolve
        // paths when the icon style flips (see BestEffortTrayPort reload_theme_icons).
        let mut ui = UiConfig {
            icon: IconStyle::Light,
            idle_icon: r"C:\glossa\idle.png".into(),
            recording_icon: r"C:\glossa\recording.png".into(),
            processing_icon: Some(r"C:\glossa\processing.png".into()),
            idle_dark_icon: Some(r"C:\glossa\idle_dark.png".into()),
            recording_dark_icon: Some(r"C:\glossa\recording_dark.png".into()),
            processing_dark_icon: Some(r"C:\glossa\processing_dark.png".into()),
            ..UiConfig::default()
        };

        assert_eq!(ui.idle_tray_icon().as_str(), r"C:\glossa\idle.png");

        ui.icon = IconStyle::Dark;
        assert_eq!(ui.idle_tray_icon().as_str(), r"C:\glossa\idle_dark.png");
        assert_eq!(
            ui.recording_tray_icon().as_str(),
            r"C:\glossa\recording_dark.png"
        );

        ui.icon = IconStyle::Light;
        assert_eq!(ui.idle_tray_icon().as_str(), r"C:\glossa\idle.png");
    }
}
