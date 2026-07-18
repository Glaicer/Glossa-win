use glossa_core::{AppCommand, CommandOrigin, InputMode};

/// Hotkey signal emitted by the global hotkey backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeySignal {
    Activated,
    Deactivated,
}

/// Maps a hotkey signal to the appropriate state-machine command.
#[must_use]
pub fn map_hotkey_signal_to_command(mode: InputMode, signal: HotkeySignal) -> Option<AppCommand> {
    match (mode, signal) {
        (InputMode::Toggle, HotkeySignal::Activated) => Some(AppCommand::ToggleRecording {
            origin: CommandOrigin::HotkeyShortcut,
        }),
        (InputMode::Toggle, HotkeySignal::Deactivated) => None,
        (InputMode::PushToTalk, HotkeySignal::Activated) => Some(AppCommand::StartRecording {
            origin: CommandOrigin::HotkeyShortcut,
        }),
        (InputMode::PushToTalk, HotkeySignal::Deactivated) => Some(AppCommand::StopRecording {
            origin: CommandOrigin::HotkeyShortcut,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_should_start_on_press() {
        let command = map_hotkey_signal_to_command(InputMode::Toggle, HotkeySignal::Activated);
        assert!(matches!(
            command,
            Some(AppCommand::ToggleRecording {
                origin: CommandOrigin::HotkeyShortcut,
            })
        ));
    }

    #[test]
    fn toggle_should_ignore_release() {
        let command = map_hotkey_signal_to_command(InputMode::Toggle, HotkeySignal::Deactivated);
        assert!(command.is_none());
    }

    #[test]
    fn push_to_talk_should_start_on_press() {
        let command = map_hotkey_signal_to_command(InputMode::PushToTalk, HotkeySignal::Activated);
        assert!(matches!(
            command,
            Some(AppCommand::StartRecording {
                origin: CommandOrigin::HotkeyShortcut,
            })
        ));
    }

    #[test]
    fn push_to_talk_should_stop_on_release() {
        let command =
            map_hotkey_signal_to_command(InputMode::PushToTalk, HotkeySignal::Deactivated);
        assert!(matches!(
            command,
            Some(AppCommand::StopRecording {
                origin: CommandOrigin::HotkeyShortcut,
            })
        ));
    }
}
