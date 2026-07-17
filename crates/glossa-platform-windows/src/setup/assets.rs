//! Embedded tray icons and cue sounds written during first-run install.

pub struct EmbeddedAsset {
    pub relative_path: &'static str,
    pub bytes: &'static [u8],
}

macro_rules! asset {
    ($rel:expr) => {
        EmbeddedAsset {
            relative_path: $rel,
            bytes: include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../contrib/assets/",
                $rel
            )),
        }
    };
}

pub const EMBEDDED_ASSETS: &[EmbeddedAsset] = &[
    asset!("tray/idle.png"),
    asset!("tray/idle_dark.png"),
    asset!("tray/recording.png"),
    asset!("tray/recording_dark.png"),
    asset!("tray/processing.png"),
    asset!("tray/processing_dark.png"),
    asset!("sounds/start.wav"),
    asset!("sounds/stop.wav"),
];
