//! Global hotkey strings such as `ctrl+alt+space`, as Win32 `RegisterHotKey` modifiers and
//! virtual-key code. Platform-neutral so the settings file is validated (and tested) everywhere.

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

fn modifier(name: &str) -> Option<u32> {
    match name {
        "ctrl" | "control" => Some(MOD_CONTROL),
        "alt" => Some(MOD_ALT),
        "shift" => Some(MOD_SHIFT),
        "win" | "super" | "meta" => Some(MOD_WIN),
        _ => None,
    }
}

/// Virtual-key code of a key name. Punctuation uses the US-layout key positions.
fn key(name: &str) -> Option<u32> {
    let b = name.as_bytes();
    if let [c @ (b'a'..=b'z' | b'0'..=b'9')] = b {
        return Some(c.to_ascii_uppercase() as u32);
    }
    if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()).filter(|n| (1..=24).contains(n)) {
        return Some(0x6F + n);
    }
    Some(match name {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "esc" | "escape" => 0x1B,
        "backspace" => 0x08,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "pause" => 0x13,
        "`" | "backquote" => 0xC0,
        "-" | "minus" => 0xBD,
        "=" | "equals" => 0xBB,
        "," | "comma" => 0xBC,
        "." | "period" => 0xBE,
        "/" | "slash" => 0xBF,
        ";" | "semicolon" => 0xBA,
        _ => return None,
    })
}

/// Parse `mod+mod+key` (case-insensitive). Exactly one key; at least one modifier unless the key
/// is F1–F24, so a plain letter can't swallow typing system-wide.
pub fn parse(s: &str) -> Option<Hotkey> {
    let mut mods = 0;
    let mut vk = None;
    for part in s.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        if let Some(m) = modifier(&part) {
            mods |= m;
        } else if vk.is_none() {
            vk = Some(key(&part)?);
        } else {
            return None;
        }
    }
    let vk = vk?;
    (mods != 0 || (0x70..=0x87).contains(&vk)).then_some(Hotkey { mods, vk })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        assert_eq!(parse("ctrl+alt+space"), Some(Hotkey { mods: MOD_CONTROL | MOD_ALT, vk: 0x20 }));
        assert_eq!(parse(" Win + Shift + F "), Some(Hotkey { mods: MOD_WIN | MOD_SHIFT, vk: 0x46 }));
        assert_eq!(parse("alt+`"), Some(Hotkey { mods: MOD_ALT, vk: 0xC0 }));
        assert_eq!(parse("f12"), Some(Hotkey { mods: 0, vk: 0x7B }));
        assert_eq!(parse("ctrl+1"), Some(Hotkey { mods: MOD_CONTROL, vk: 0x31 }));
    }

    #[test]
    fn rejects() {
        for s in ["", "ctrl", "ctrl+alt", "a", "space", "ctrl+a+b", "ctrl+f25", "ctrl+foo", "ctrl++"] {
            assert_eq!(parse(s), None, "{s}");
        }
    }
}
