//! `settings.conf`: plain `key = value` lines, `#` starts a comment.

use std::fs;
use std::io;

use crate::paths;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub case_sensitive: bool,
    pub match_full_path: bool,
}

pub const DEFAULT_FILE: &str = "\
# Finder settings. Format: key = value; # starts a comment.

# Match case exactly.
case_sensitive = false

# Match against the full path instead of the name only.
match_full_path = false
";

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
            let slot = match key {
                "case_sensitive" => &mut s.case_sensitive,
                "match_full_path" => &mut s.match_full_path,
                _ => {
                    warnings.push(format!("line {n}: unknown key `{key}`"));
                    continue;
                }
            };
            match parse_bool(value) {
                Some(b) => *slot = b,
                None => warnings.push(format!("line {n}: invalid value `{value}` for `{key}`, using default")),
            }
        }
        (s, warnings)
    }

    /// Read the settings file, creating it with defaults if missing.
    pub fn load() -> (Settings, Vec<String>) {
        let path = paths::settings_file();
        match fs::read_to_string(&path) {
            Ok(text) => Settings::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let created = fs::create_dir_all(paths::config_dir()).and_then(|_| fs::write(&path, DEFAULT_FILE));
                let warnings = match created {
                    Ok(()) => Vec::new(),
                    Err(e) => vec![format!("cannot create {}: {e}", path.display())],
                };
                (Settings::default(), warnings)
            }
            Err(e) => (Settings::default(), vec![format!("cannot read {}: {e}", path.display())]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_parses_to_defaults() {
        assert_eq!(Settings::parse(DEFAULT_FILE), (Settings::default(), vec![]));
    }

    #[test]
    fn parses_values_and_warns() {
        let (s, w) = Settings::parse("case_sensitive = yes # trailing\nmatch_full_path=maybe\nfoo = 1\njunk\n");
        assert!(s.case_sensitive);
        assert!(!s.match_full_path);
        assert_eq!(w.len(), 3);
        assert!(w[0].starts_with("line 2: invalid value"));
        assert!(w[1].starts_with("line 3: unknown key"));
        assert!(w[2].starts_with("line 4: expected"));
    }
}
