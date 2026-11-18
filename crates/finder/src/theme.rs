//! Colors: a light or dark palette, whichever the desktop uses, with its accent color.
//!
//! Linux: KDE's `kdeglobals` color scheme (exact colors and accent), else the GTK dark preference.
//! Windows: the "app mode" and accent color from the registry.

use fltk::enums::Color;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Window background (footer, settings panel).
    pub bg: Color,
    /// Search field and result list.
    pub surface: Color,
    pub text: Color,
    pub muted: Color,
    pub border: Color,
    pub hover: Color,
    pub accent: Color,
    /// Text on `accent`.
    pub on_accent: Color,
    /// Behind the matched part of a name.
    pub match_bg: Color,
}

pub fn rgb(c: Color) -> (u8, u8, u8) {
    c.to_rgb()
}

/// `a` mixed with `b`; `t` = 0 gives `a`, 1 gives `b`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let ((ar, ag, ab), (br, bg, bb)) = (rgb(a), rgb(b));
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color::from_rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

fn luminance(c: Color) -> f32 {
    let (r, g, b) = rgb(c);
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

const DEFAULT_ACCENT: Color = Color::from_hex(0x3daee9);

impl Palette {
    fn base(dark: bool, accent: Color) -> Palette {
        let (bg, surface, text, muted, border) = if dark {
            (0x1d1f22, 0x25272b, 0xeceff1, 0x9aa1a9, 0x3a3e44)
        } else {
            (0xf3f4f6, 0xffffff, 0x1f2328, 0x6b7280, 0xd5d9df)
        };
        let surface = Color::from_hex(surface);
        Palette {
            dark,
            bg: Color::from_hex(bg),
            surface,
            text: Color::from_hex(text),
            muted: Color::from_hex(muted),
            border: Color::from_hex(border),
            hover: mix(surface, Color::from_hex(text), if dark { 0.07 } else { 0.05 }),
            accent,
            on_accent: if luminance(accent) > 0.72 { Color::from_hex(0x111111) } else { Color::White },
            match_bg: mix(surface, accent, if dark { 0.35 } else { 0.22 }),
        }
    }

    pub fn load() -> Palette {
        let d = Desktop::read();
        let mut p = Palette::base(d.dark.unwrap_or(false), d.accent.unwrap_or(DEFAULT_ACCENT));
        if let Some(c) = d.accent_fg {
            p.on_accent = c;
        }
        p
    }

    /// Set FLTK's global colors (used by stock widgets such as check buttons and text inputs).
    pub fn apply_globals(&self) {
        let set = |f: fn(u8, u8, u8), c: Color| {
            let (r, g, b) = rgb(c);
            f(r, g, b);
        };
        set(fltk::app::background, self.bg);
        set(fltk::app::background2, self.surface);
        set(fltk::app::foreground, self.text);
        set(fltk::app::set_selection_color, self.accent);
        set(fltk::app::set_inactive_color, self.muted);
    }
}

/// What the desktop says about its colors.
#[derive(Default)]
struct Desktop {
    dark: Option<bool>,
    accent: Option<Color>,
    /// Text color on the accent (KDE selection foreground).
    accent_fg: Option<Color>,
}

#[cfg(unix)]
impl Desktop {
    fn read() -> Desktop {
        let config = finder_core::paths::xdg_config_home();
        let kde = std::fs::read_to_string(config.join("kdeglobals"));
        let mut d = kde.map(|t| Desktop::from_kdeglobals(&t)).unwrap_or_default();
        if d.dark.is_none() {
            d.dark = gtk_prefers_dark(&config);
        }
        d
    }

    fn from_kdeglobals(text: &str) -> Desktop {
        let mut d = Desktop::default();
        let mut window_bg = None;
        let mut section = "";
        for line in text.lines().map(str::trim) {
            if let Some(s) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = s;
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let color = || parse_kde_color(value);
            match (section, key.trim()) {
                ("Colors:Window", "BackgroundNormal") => window_bg = color(),
                ("Colors:Selection", "BackgroundNormal") => d.accent = d.accent.or_else(color),
                ("Colors:Selection", "ForegroundNormal") => d.accent_fg = color(),
                ("General", "AccentColor") => d.accent = color(),
                _ => {}
            }
        }
        d.dark = window_bg.map(|c| luminance(c) < 0.5);
        d
    }
}

/// `r,g,b` (KDE color entries).
#[cfg(unix)]
fn parse_kde_color(v: &str) -> Option<Color> {
    let mut it = v.trim().split(',').map(|p| p.trim().parse::<u8>().ok());
    Some(Color::from_rgb(it.next()??, it.next()??, it.next()??))
}

#[cfg(unix)]
fn gtk_prefers_dark(config: &std::path::Path) -> Option<bool> {
    if let Ok(t) = std::env::var("GTK_THEME") {
        return Some(t.to_ascii_lowercase().contains("dark"));
    }
    let ini = std::fs::read_to_string(config.join("gtk-3.0/settings.ini")).ok()?;
    let mut dark = None;
    for line in ini.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().to_ascii_lowercase();
        match k.trim() {
            "gtk-application-prefer-dark-theme" => dark = Some(v == "1" || v == "true"),
            "gtk-theme-name" if v.contains("dark") => dark = Some(true),
            _ => {}
        }
    }
    dark
}

#[cfg(windows)]
impl Desktop {
    fn read() -> Desktop {
        let personalize = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
        let light = reg_dword(personalize, "AppsUseLightTheme");
        // 0xAABBGGRR
        let accent = reg_dword(r"Software\Microsoft\Windows\DWM", "AccentColor")
            .map(|v| Color::from_rgb(v as u8, (v >> 8) as u8, (v >> 16) as u8));
        Desktop { dark: light.map(|l| l == 0), accent, ..Desktop::default() }
    }
}

#[cfg(windows)]
fn reg_dword(key: &str, value: &str) -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, value) = (wide(key), wide(value));
    let mut data = 0u32;
    let mut len = 4u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut data as *mut u32).cast(),
            &mut len,
        )
    };
    (r == 0).then_some(data)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn reads_kde_colors() {
        let d = Desktop::from_kdeglobals(
            "[Colors:Selection]\nBackgroundNormal=61,174,233\n\n[Colors:View]\nBackgroundNormal=20,22,24\n\
             ForegroundNormal=252,252,252\n[Colors:Window]\nBackgroundNormal=32,35,38\n",
        );
        assert_eq!(d.dark, Some(true));
        assert_eq!(d.accent, Some(Color::from_rgb(61, 174, 233)));
        assert_eq!(parse_kde_color("1,2"), None);
    }

    #[test]
    fn mixes() {
        assert_eq!(mix(Color::from_rgb(0, 0, 0), Color::from_rgb(200, 100, 50), 0.5), Color::from_rgb(100, 50, 25));
    }
}
