use global_hotkey::hotkey::{Code, HotKey, Modifiers};

use glossa_app::AppError;

/// Parses human-readable shortcut strings into a `global-hotkey` `HotKey`.
///
/// Accepts both Windows-style (`Ctrl+Alt+Space`) and Linux portal-style
/// (`<Ctrl><Alt>space`) notations.
pub fn parse_shortcut(input: &str) -> Result<HotKey, AppError> {
    let normalized = normalize_shortcut(input);
    if normalized.is_empty() {
        return Err(AppError::message("input.shortcut must not be empty"));
    }

    let mut mods = Modifiers::empty();
    let mut key_token = None::<String>;

    for token in normalized.split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "leftctrl" | "rightctrl" => mods |= Modifiers::CONTROL,
            "alt" | "leftalt" | "rightalt" | "option" => mods |= Modifiers::ALT,
            "shift" | "leftshift" | "rightshift" => mods |= Modifiers::SHIFT,
            "super" | "meta" | "win" | "windows" | "cmd" | "command" => mods |= Modifiers::SUPER,
            other => {
                if key_token.is_some() {
                    return Err(AppError::message(format!(
                        "shortcut `{input}` has more than one non-modifier key"
                    )));
                }
                key_token = Some(other.to_owned());
            }
        }
    }

    let key_token = key_token.ok_or_else(|| {
        AppError::message(format!("shortcut `{input}` is missing a primary key"))
    })?;
    let code = parse_code(&key_token).ok_or_else(|| {
        AppError::message(format!(
            "unsupported shortcut key `{key_token}` in `{input}`"
        ))
    })?;

    Ok(HotKey::new(Some(mods), code))
}

fn normalize_shortcut(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.contains('<') {
        // Portal style: <Ctrl><Alt>space
        let mut out = String::new();
        let mut rest = trimmed;
        while let Some(start) = rest.find('<') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('>') else {
                break;
            };
            let token = &after[..end];
            if !out.is_empty() {
                out.push('+');
            }
            out.push_str(token);
            rest = &after[end + 1..];
        }
        let trailing = rest.trim();
        if !trailing.is_empty() {
            if !out.is_empty() {
                out.push('+');
            }
            out.push_str(trailing);
        }
        out
    } else {
        trimmed.replace(' ', "")
    }
}

fn parse_code(token: &str) -> Option<Code> {
    match token.to_ascii_lowercase().as_str() {
        "space" => Some(Code::Space),
        "enter" | "return" => Some(Code::Enter),
        "tab" => Some(Code::Tab),
        "escape" | "esc" => Some(Code::Escape),
        "backspace" => Some(Code::Backspace),
        "delete" | "del" => Some(Code::Delete),
        "insert" | "ins" => Some(Code::Insert),
        "home" => Some(Code::Home),
        "end" => Some(Code::End),
        "pageup" | "pgup" => Some(Code::PageUp),
        "pagedown" | "pgdn" => Some(Code::PageDown),
        "up" | "arrowup" => Some(Code::ArrowUp),
        "down" | "arrowdown" => Some(Code::ArrowDown),
        "left" | "arrowleft" => Some(Code::ArrowLeft),
        "right" | "arrowright" => Some(Code::ArrowRight),
        "f1" => Some(Code::F1),
        "f2" => Some(Code::F2),
        "f3" => Some(Code::F3),
        "f4" => Some(Code::F4),
        "f5" => Some(Code::F5),
        "f6" => Some(Code::F6),
        "f7" => Some(Code::F7),
        "f8" => Some(Code::F8),
        "f9" => Some(Code::F9),
        "f10" => Some(Code::F10),
        "f11" => Some(Code::F11),
        "f12" => Some(Code::F12),
        "0" | "digit0" => Some(Code::Digit0),
        "1" | "digit1" => Some(Code::Digit1),
        "2" | "digit2" => Some(Code::Digit2),
        "3" | "digit3" => Some(Code::Digit3),
        "4" | "digit4" => Some(Code::Digit4),
        "5" | "digit5" => Some(Code::Digit5),
        "6" | "digit6" => Some(Code::Digit6),
        "7" | "digit7" => Some(Code::Digit7),
        "8" | "digit8" => Some(Code::Digit8),
        "9" | "digit9" => Some(Code::Digit9),
        "a" | "keya" => Some(Code::KeyA),
        "b" | "keyb" => Some(Code::KeyB),
        "c" | "keyc" => Some(Code::KeyC),
        "d" | "keyd" => Some(Code::KeyD),
        "e" | "keye" => Some(Code::KeyE),
        "f" | "keyf" => Some(Code::KeyF),
        "g" | "keyg" => Some(Code::KeyG),
        "h" | "keyh" => Some(Code::KeyH),
        "i" | "keyi" => Some(Code::KeyI),
        "j" | "keyj" => Some(Code::KeyJ),
        "k" | "keyk" => Some(Code::KeyK),
        "l" | "keyl" => Some(Code::KeyL),
        "m" | "keym" => Some(Code::KeyM),
        "n" | "keyn" => Some(Code::KeyN),
        "o" | "keyo" => Some(Code::KeyO),
        "p" | "keyp" => Some(Code::KeyP),
        "q" | "keyq" => Some(Code::KeyQ),
        "r" | "keyr" => Some(Code::KeyR),
        "s" | "keys" => Some(Code::KeyS),
        "t" | "keyt" => Some(Code::KeyT),
        "u" | "keyu" => Some(Code::KeyU),
        "v" | "keyv" => Some(Code::KeyV),
        "w" | "keyw" => Some(Code::KeyW),
        "x" | "keyx" => Some(Code::KeyX),
        "y" | "keyy" => Some(Code::KeyY),
        "z" | "keyz" => Some(Code::KeyZ),
        _ => None,
    }
}

/// Formats a config shortcut for the picker UI: `Ctrl+Alt+Space` → `[CTRL] + [ALT] + [SPACE]`.
#[must_use]
pub fn format_shortcut_display(input: &str) -> String {
    let normalized = normalize_shortcut(input);
    if normalized.is_empty() {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::new();
    let mut key_label = None::<String>;

    for token in normalized.split('+') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "leftctrl" | "rightctrl" => {
                parts.push(chip("CTRL"));
            }
            "alt" | "leftalt" | "rightalt" | "option" => {
                parts.push(chip("ALT"));
            }
            "shift" | "leftshift" | "rightshift" => {
                parts.push(chip("SHIFT"));
            }
            "super" | "meta" | "win" | "windows" | "cmd" | "command" => {
                parts.push(chip("WIN"));
            }
            other => {
                key_label = Some(chip(&display_key_label(other)));
            }
        }
    }

    if let Some(key) = key_label {
        parts.push(key);
    }

    parts.join(" + ")
}

/// Builds a canonical config string (`Ctrl+Alt+Space`) from modifier flags and a key token.
#[must_use]
pub fn format_shortcut_config(ctrl: bool, alt: bool, shift: bool, win: bool, key: &str) -> String {
    let mut parts = modifier_config_parts(ctrl, alt, shift, win);
    parts.push(canonical_key_token(key).to_owned());
    parts.join("+")
}

/// Live picker chips for currently held keys.
///
/// Modifier-only: `[CTRL] + [ALT]`. With a primary key: `[CTRL] + [ALT] + [SPACE]`.
#[must_use]
pub fn format_chord_display(
    ctrl: bool,
    alt: bool,
    shift: bool,
    win: bool,
    key: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    if ctrl {
        parts.push(chip("CTRL"));
    }
    if alt {
        parts.push(chip("ALT"));
    }
    if shift {
        parts.push(chip("SHIFT"));
    }
    if win {
        parts.push(chip("WIN"));
    }
    if let Some(key) = key {
        parts.push(chip(&display_key_label(key)));
    }
    parts.join(" + ")
}

fn modifier_config_parts(ctrl: bool, alt: bool, shift: bool, win: bool) -> Vec<String> {
    let mut parts = Vec::new();
    if ctrl {
        parts.push("Ctrl".to_owned());
    }
    if alt {
        parts.push("Alt".to_owned());
    }
    if shift {
        parts.push("Shift".to_owned());
    }
    if win {
        parts.push("Win".to_owned());
    }
    parts
}

/// Maps a Windows virtual-key code to a config key token (`Space`, `A`, `F5`, …).
#[must_use]
pub fn vk_to_token(vk: u32) -> Option<&'static str> {
    match vk {
        0x08 => Some("Backspace"),
        0x09 => Some("Tab"),
        0x0D => Some("Enter"),
        0x1B => Some("Escape"),
        0x20 => Some("Space"),
        0x21 => Some("PageUp"),
        0x22 => Some("PageDown"),
        0x23 => Some("End"),
        0x24 => Some("Home"),
        0x25 => Some("Left"),
        0x26 => Some("Up"),
        0x27 => Some("Right"),
        0x28 => Some("Down"),
        0x2D => Some("Insert"),
        0x2E => Some("Delete"),
        0x30 => Some("0"),
        0x31 => Some("1"),
        0x32 => Some("2"),
        0x33 => Some("3"),
        0x34 => Some("4"),
        0x35 => Some("5"),
        0x36 => Some("6"),
        0x37 => Some("7"),
        0x38 => Some("8"),
        0x39 => Some("9"),
        0x41 => Some("A"),
        0x42 => Some("B"),
        0x43 => Some("C"),
        0x44 => Some("D"),
        0x45 => Some("E"),
        0x46 => Some("F"),
        0x47 => Some("G"),
        0x48 => Some("H"),
        0x49 => Some("I"),
        0x4A => Some("J"),
        0x4B => Some("K"),
        0x4C => Some("L"),
        0x4D => Some("M"),
        0x4E => Some("N"),
        0x4F => Some("O"),
        0x50 => Some("P"),
        0x51 => Some("Q"),
        0x52 => Some("R"),
        0x53 => Some("S"),
        0x54 => Some("T"),
        0x55 => Some("U"),
        0x56 => Some("V"),
        0x57 => Some("W"),
        0x58 => Some("X"),
        0x59 => Some("Y"),
        0x5A => Some("Z"),
        0x70 => Some("F1"),
        0x71 => Some("F2"),
        0x72 => Some("F3"),
        0x73 => Some("F4"),
        0x74 => Some("F5"),
        0x75 => Some("F6"),
        0x76 => Some("F7"),
        0x77 => Some("F8"),
        0x78 => Some("F9"),
        0x79 => Some("F10"),
        0x7A => Some("F11"),
        0x7B => Some("F12"),
        _ => None,
    }
}

/// Returns true when the virtual-key is a modifier (Ctrl/Alt/Shift/Win).
#[must_use]
pub fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        0x10 | 0x11 | 0x12 | // SHIFT, CONTROL, MENU (Alt)
        0xA0 | 0xA1 | // LSHIFT, RSHIFT
        0xA2 | 0xA3 | // LCONTROL, RCONTROL
        0xA4 | 0xA5 | // LMENU, RMENU
        0x5B | 0x5C // LWIN, RWIN
    )
}

fn chip(label: &str) -> String {
    format!("[{label}]")
}

fn display_key_label(token: &str) -> String {
    match token.to_ascii_lowercase().as_str() {
        "space" => "SPACE".into(),
        "enter" | "return" => "ENTER".into(),
        "tab" => "TAB".into(),
        "escape" | "esc" => "ESC".into(),
        "backspace" => "BACKSPACE".into(),
        "delete" | "del" => "DEL".into(),
        "insert" | "ins" => "INS".into(),
        "home" => "HOME".into(),
        "end" => "END".into(),
        "pageup" | "pgup" => "PGUP".into(),
        "pagedown" | "pgdn" => "PGDN".into(),
        "up" | "arrowup" => "UP".into(),
        "down" | "arrowdown" => "DOWN".into(),
        "left" | "arrowleft" => "LEFT".into(),
        "right" | "arrowright" => "RIGHT".into(),
        other if other.len() == 1 => other.to_ascii_uppercase(),
        other if other.starts_with('f')
            && other.len() <= 3
            && other[1..].chars().all(|c| c.is_ascii_digit()) =>
        {
            other.to_ascii_uppercase()
        }
        other => other.to_ascii_uppercase(),
    }
}

fn canonical_key_token(key: &str) -> &str {
    match key.to_ascii_lowercase().as_str() {
        "space" => "Space",
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "pgup" => "PageUp",
        "pagedown" | "pgdn" => "PageDown",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        "f1" => "F1",
        "f2" => "F2",
        "f3" => "F3",
        "f4" => "F4",
        "f5" => "F5",
        "f6" => "F6",
        "f7" => "F7",
        "f8" => "F8",
        "f9" => "F9",
        "f10" => "F10",
        "f11" => "F11",
        "f12" => "F12",
        other if other.len() == 1 => {
            // Single letter/digit — use uppercase for letters.
            // Caller already passes "A"/"0" style tokens from vk_to_token.
            key
        }
        _ => key,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_windows_style_shortcut() {
        let hotkey = parse_shortcut("Ctrl+Alt+Space").expect("should parse");
        assert_eq!(hotkey, HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space));
    }

    #[test]
    fn should_parse_portal_style_shortcut() {
        let hotkey = parse_shortcut("<Ctrl><Alt>space").expect("should parse");
        assert_eq!(hotkey, HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::Space));
    }

    #[test]
    fn should_format_display_chips() {
        assert_eq!(
            format_shortcut_display("Ctrl+Alt+Space"),
            "[CTRL] + [ALT] + [SPACE]"
        );
        assert_eq!(
            format_shortcut_display("<Ctrl><Shift>s"),
            "[CTRL] + [SHIFT] + [S]"
        );
    }

    #[test]
    fn should_format_config_from_flags() {
        assert_eq!(
            format_shortcut_config(true, true, false, false, "Space"),
            "Ctrl+Alt+Space"
        );
    }

    #[test]
    fn should_format_live_chord_as_keys_are_held() {
        assert_eq!(
            format_chord_display(true, false, false, false, None),
            "[CTRL]"
        );
        assert_eq!(
            format_chord_display(true, true, false, false, None),
            "[CTRL] + [ALT]"
        );
        assert_eq!(
            format_chord_display(true, true, false, false, Some("Space")),
            "[CTRL] + [ALT] + [SPACE]"
        );
    }

    #[test]
    fn should_map_common_virtual_keys() {
        assert_eq!(vk_to_token(0x20), Some("Space"));
        assert_eq!(vk_to_token(0x41), Some("A"));
        assert_eq!(vk_to_token(0x70), Some("F1"));
        assert!(is_modifier_vk(0x11));
        assert!(!is_modifier_vk(0x20));
    }
}
