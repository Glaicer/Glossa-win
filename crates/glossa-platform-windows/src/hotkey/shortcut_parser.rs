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
}
