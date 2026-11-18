//! Global hotkey strings such as `ctrl+alt+space`, as Win32 `RegisterHotKey` modifiers and
//! virtual-key code, or as a Qt key code for KDE. Platform-neutral so the settings file is
//! validated (and tested) everywhere.

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

impl Hotkey {
    /// The key as a Qt key code with modifier bits, as KDE's kglobalaccel takes it.
    pub fn qt(&self) -> i32 {
        let key = match self.vk {
            0x30..=0x39 | 0x41..=0x5A | 0x20 => self.vk as i32,
            0x70..=0x87 => 0x0100_0030 + (self.vk - 0x70) as i32,
            0x1B => 0x0100_0000,
            0x09 => 0x0100_0001,
            0x08 => 0x0100_0003,
            0x0D => 0x0100_0004,
            0x2D => 0x0100_0006,
            0x2E => 0x0100_0007,
            0x13 => 0x0100_0008,
            0x24 => 0x0100_0010,
            0x23 => 0x0100_0011,
            0x25 => 0x0100_0012,
            0x26 => 0x0100_0013,
            0x27 => 0x0100_0014,
            0x28 => 0x0100_0015,
            0x21 => 0x0100_0016,
            0x22 => 0x0100_0017,
            0xC0 => 0x60,
            0xBD => 0x2D,
            0xBB => 0x3D,
            0xBC => 0x2C,
            0xBE => 0x2E,
            0xBF => 0x2F,
            0xBA => 0x3B,
            _ => unreachable!("key() only yields the keys above"),
        };
        let m = |bit: u32, qt: i32| if self.mods & bit != 0 { qt } else { 0 };
        key | m(MOD_SHIFT, 0x0200_0000)
            | m(MOD_CONTROL, 0x0400_0000)
            | m(MOD_ALT, 0x0800_0000)
            | m(MOD_WIN, 0x1000_0000)
    }
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

/// The setting value for `s`: lowercase `mod+key` without spaces, `""` for none; `None` if invalid.
pub fn normalize(s: &str) -> Option<String> {
    if s.trim().is_empty() {
        return Some(String::new());
    }
    parse(s)?;
    Some(s.split('+').map(|p| p.trim().to_ascii_lowercase()).collect::<Vec<_>>().join("+"))
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
    fn qt_codes() {
        let qt = |s: &str| parse(s).unwrap().qt();
        assert_eq!(qt("ctrl+alt+f7"), 0x0400_0000 | 0x0800_0000 | 0x0100_0036);
        assert_eq!(qt("meta+space"), 0x1000_0000 | 0x20);
        assert_eq!(qt("shift+alt+`"), 0x0200_0000 | 0x0800_0000 | 0x60);
        assert_eq!(qt("ctrl+x"), 0x0400_0000 | 0x58);
        assert_eq!(qt("f1"), 0x0100_0030);
        // Every key name maps.
        for k in "abcdefghijklmnopqrstuvwxyz0123456789".chars().map(String::from).chain(
            [
                "space",
                "enter",
                "tab",
                "esc",
                "backspace",
                "insert",
                "delete",
                "home",
                "end",
                "pageup",
                "pagedown",
                "left",
                "up",
                "right",
                "down",
                "pause",
                "`",
                "-",
                "=",
                ",",
                ".",
                "/",
                ";",
                "f24",
            ]
            .map(String::from),
        ) {
            assert!(qt(&format!("ctrl+{k}")) & 0x01ff_ffff != 0, "{k}");
        }
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize(" Ctrl + Alt + Space ").as_deref(), Some("ctrl+alt+space"));
        assert_eq!(normalize("  ").as_deref(), Some(""));
        assert_eq!(normalize("ctrl+nope"), None);
    }

    #[test]
    fn rejects() {
        for s in ["", "ctrl", "ctrl+alt", "a", "space", "ctrl+a+b", "ctrl+f25", "ctrl+foo", "ctrl++"] {
            assert_eq!(parse(s), None, "{s}");
        }
    }
}
