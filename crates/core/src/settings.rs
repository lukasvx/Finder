//! `settings.conf`: plain `key = value` lines, `#` starts a comment.

use std::fs;
use std::io;

use crate::paths;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    /// Follow the desktop (KDE/GTK color scheme on Linux, app mode on Windows).
    #[default]
    Auto,
    Light,
    Dark,
}

impl Theme {
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Auto => "auto",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    fn parse(v: &str) -> Option<Theme> {
        [Theme::Auto, Theme::Light, Theme::Dark].into_iter().find(|t| t.as_str().eq_ignore_ascii_case(v))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub case_sensitive: bool,
    pub match_full_path: bool,
    /// Show dotfiles (and, on Windows, files marked hidden) and everything below hidden folders.
    pub show_hidden: bool,
    /// Folder names (`node_modules`) or paths (`~/Downloads/old`) left out of results with everything below them.
    pub exclude: Vec<String>,
    pub theme: Theme,
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Some(true),
        "false" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}

impl Settings {
    /// Parse settings text. Unknown keys and invalid values produce warnings and keep the default.
    pub fn parse(text: &str) -> (Settings, Vec<String>) {
        let mut s = Settings::default();
        let mut warnings = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let n = n + 1;
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("line {n}: expected `key = value`"));
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let invalid = || format!("line {n}: invalid value `{value}` for `{key}`, using default");
            let flag = match key {
                "case_sensitive" => &mut s.case_sensitive,
                "match_full_path" => &mut s.match_full_path,
                "show_hidden" => &mut s.show_hidden,
                "exclude" => {
                    s.exclude = split_list(value);
                    continue;
                }
                "theme" => {
                    match Theme::parse(value) {
                        Some(t) => s.theme = t,
                        None => warnings.push(invalid()),
                    }
                    continue;
                }
                _ => {
                    warnings.push(format!("line {n}: unknown key `{key}`"));
                    continue;
                }
            };
            match parse_bool(value) {
                Some(b) => *flag = b,
                None => warnings.push(invalid()),
            }
        }
        (s, warnings)
    }

    /// The settings as file text, with a comment for each key.
    pub fn to_text(&self) -> String {
        format!(
            "\
# Finder settings. Format: key = value; # starts a comment.
# Finder rewrites this file when settings are changed in the app.

# Match case exactly.
case_sensitive = {}

# Match against the full path instead of the name only.
match_full_path = {}

# Show hidden files and folders (names starting with `.`; on Windows also files marked hidden).
show_hidden = {}

# Comma-separated folder names (node_modules) or paths (~/Downloads/old) to leave out of results.
exclude = {}

# Colors: auto (follow the desktop), light or dark.
theme = {}
",
            self.case_sensitive,
            self.match_full_path,
            self.show_hidden,
            self.exclude.join(", "),
            self.theme.as_str()
        )
    }

    /// Read the settings file, creating it with defaults if missing.
    pub fn load() -> (Settings, Vec<String>) {
        let path = paths::settings_file();
        match fs::read_to_string(&path) {
            Ok(text) => Settings::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let warnings = match Settings::default().save() {
                    Ok(()) => Vec::new(),
                    Err(e) => vec![format!("cannot create {}: {e}", path.display())],
                };
                (Settings::default(), warnings)
            }
            Err(e) => (Settings::default(), vec![format!("cannot read {}: {e}", path.display())]),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        fs::create_dir_all(paths::config_dir())?;
        fs::write(paths::settings_file(), self.to_text())
    }

    /// Exclusions as sent to the daemon: `~` expanded, empty entries dropped.
    pub fn exclude_bytes(&self) -> Vec<Vec<u8>> {
        self.exclude.iter().filter(|e| !e.is_empty()).map(|e| paths::to_bytes(&paths::expand_home(e))).collect()
    }
}

/// Split a comma- or newline-separated list, trimming entries and dropping empty ones.
pub fn split_list(s: &str) -> Vec<String> {
    s.split([',', '\n']).map(str::trim).filter(|e| !e.is_empty()).map(String::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_parses_to_defaults() {
        assert_eq!(Settings::parse(&Settings::default().to_text()), (Settings::default(), vec![]));
    }

    #[test]
    fn round_trips() {
        let s = Settings {
            case_sensitive: true,
            match_full_path: false,
            show_hidden: true,
            exclude: vec!["node_modules".into(), "~/Downloads/old stuff".into()],
            theme: Theme::Dark,
        };
        assert_eq!(Settings::parse(&s.to_text()), (s, vec![]));
    }

    #[test]
    fn parses_values_and_warns() {
        let (s, w) = Settings::parse(
            "case_sensitive = yes # trailing\nmatch_full_path=maybe\nfoo = 1\njunk\nexclude = a, ,b\ntheme = neon\n",
        );
        assert!(s.case_sensitive);
        assert!(!s.match_full_path);
        assert_eq!(s.exclude, ["a", "b"]);
        assert_eq!(s.theme, Theme::Auto);
        assert_eq!(w.len(), 4);
        assert!(w[0].starts_with("line 2: invalid value"));
        assert!(w[1].starts_with("line 3: unknown key"));
        assert!(w[2].starts_with("line 4: expected"));
        assert!(w[3].starts_with("line 6: invalid value"));
    }
}
