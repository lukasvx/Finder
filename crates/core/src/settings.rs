//! `settings.json`: the hotkey and case sensitivity. Written once by the first-run setup in
//! `finder`; after that only edited by hand. `finder` reads it on every launch, `finderd` re-reads
//! it when it changes (hotkey).

use std::fs;
use std::io;

use crate::{hotkey, paths};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Global shortcut that opens `finder`, such as `ctrl+alt+space`; empty for none.
    pub hotkey: String,
    pub case_sensitive: bool,
}

impl Settings {
    /// Parse settings text. Unknown keys and invalid values produce warnings and keep the default.
    pub fn parse(text: &str) -> (Settings, Vec<String>) {
        let mut s = Settings::default();
        let fields = match json::parse_object(text) {
            Ok(f) => f,
            Err(e) => return (s, vec![e]),
        };
        let mut warnings = Vec::new();
        for (key, value) in fields {
            let invalid = || format!("invalid value for `{key}`, using the default");
            match (key.as_str(), value) {
                ("hotkey", json::Value::Str(v)) => match hotkey::normalize(&v) {
                    Some(h) => s.hotkey = h,
                    None => warnings.push(format!("`{v}` is not a valid hotkey (example: \"ctrl+alt+space\")")),
                },
                ("case_sensitive", json::Value::Bool(b)) => s.case_sensitive = b,
                ("hotkey" | "case_sensitive", _) => warnings.push(invalid()),
                _ => warnings.push(format!("unknown setting `{key}`")),
            }
        }
        (s, warnings)
    }

    /// The settings as file text.
    pub fn to_text(&self) -> String {
        format!("{{\n  \"hotkey\": {},\n  \"case_sensitive\": {}\n}}\n", json::quote(&self.hotkey), self.case_sensitive)
    }

    /// Read the settings file; `None` if there is none yet (first run).
    pub fn load() -> Option<(Settings, Vec<String>)> {
        let path = paths::settings_file();
        match fs::read_to_string(&path) {
            Ok(text) => {
                let (s, warnings) = Settings::parse(&text);
                let file = path.display();
                Some((s, warnings.into_iter().map(|w| format!("{file}: {w}")).collect()))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => Some((Settings::default(), vec![format!("cannot read {}: {e}", path.display())])),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        fs::create_dir_all(paths::data_dir())?;
        fs::write(paths::settings_file(), self.to_text())
    }
}

/// Just enough JSON for a flat settings object: any value is parsed (so unknown keys can be
/// skipped), but only strings and booleans are kept.
mod json {
    pub enum Value {
        Str(String),
        Bool(bool),
        Other,
    }

    pub fn quote(s: &str) -> String {
        let mut out = String::from("\"");
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    struct P<'a> {
        s: &'a [u8],
        i: usize,
    }

    impl P<'_> {
        fn err(&self, what: &str) -> String {
            let before = &self.s[..self.i.min(self.s.len())];
            let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
            let col = before.iter().rev().take_while(|&&b| b != b'\n').count() + 1;
            format!("line {line}, column {col}: {what}")
        }

        fn ws(&mut self) {
            while self.s.get(self.i).is_some_and(|b| b.is_ascii_whitespace()) {
                self.i += 1;
            }
        }

        fn eat(&mut self, b: u8) -> bool {
            self.ws();
            let ok = self.s.get(self.i) == Some(&b);
            self.i += ok as usize;
            ok
        }

        fn expect(&mut self, b: u8) -> Result<(), String> {
            if self.eat(b) { Ok(()) } else { Err(self.err(&format!("expected `{}`", b as char))) }
        }

        fn string(&mut self) -> Result<String, String> {
            self.expect(b'"')?;
            let mut out = Vec::new();
            loop {
                let Some(&b) = self.s.get(self.i) else { return Err(self.err("unterminated string")) };
                self.i += 1;
                match b {
                    b'"' => break,
                    b'\\' => {
                        let Some(&e) = self.s.get(self.i) else { return Err(self.err("unterminated string")) };
                        self.i += 1;
                        let c = match e {
                            b'"' | b'\\' | b'/' => e as char,
                            b'n' => '\n',
                            b't' => '\t',
                            b'r' => '\r',
                            b'b' => '\u{8}',
                            b'f' => '\u{c}',
                            b'u' => {
                                let hex = self.s.get(self.i..self.i + 4).and_then(|h| std::str::from_utf8(h).ok());
                                let c = hex.and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32);
                                self.i += 4;
                                c.ok_or_else(|| self.err("invalid \\u escape"))?
                            }
                            _ => return Err(self.err("invalid escape")),
                        };
                        out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                    }
                    _ => out.push(b),
                }
            }
            String::from_utf8(out).map_err(|_| self.err("invalid UTF-8"))
        }

        fn value(&mut self) -> Result<Value, String> {
            self.ws();
            let rest = &self.s[self.i..];
            for (word, v) in [("true", Some(true)), ("false", Some(false)), ("null", None)] {
                if rest.starts_with(word.as_bytes()) {
                    self.i += word.len();
                    return Ok(v.map_or(Value::Other, Value::Bool));
                }
            }
            match rest.first() {
                Some(b'"') => self.string().map(Value::Str),
                Some(b'{') => {
                    self.object()?;
                    Ok(Value::Other)
                }
                Some(b'[') => {
                    self.i += 1;
                    if !self.eat(b']') {
                        loop {
                            self.value()?;
                            if self.eat(b']') {
                                break;
                            }
                            self.expect(b',')?;
                        }
                    }
                    Ok(Value::Other)
                }
                Some(b'-' | b'0'..=b'9') => {
                    let n = rest.iter().take_while(|b| b"+-.eE0123456789".contains(b)).count();
                    self.i += n;
                    Ok(Value::Other)
                }
                _ => Err(self.err("expected a value")),
            }
        }

        fn object(&mut self) -> Result<Vec<(String, Value)>, String> {
            self.expect(b'{')?;
            let mut fields = Vec::new();
            if self.eat(b'}') {
                return Ok(fields);
            }
            loop {
                self.ws();
                let key = self.string()?;
                self.expect(b':')?;
                fields.push((key, self.value()?));
                if self.eat(b'}') {
                    return Ok(fields);
                }
                self.expect(b',')?;
            }
        }
    }

    /// The fields of the top-level object, in file order.
    pub fn parse_object(text: &str) -> Result<Vec<(String, Value)>, String> {
        let mut p = P { s: text.trim_start_matches('\u{feff}').as_bytes(), i: 0 };
        let fields = p.object()?;
        p.ws();
        if p.i < p.s.len() {
            return Err(p.err("unexpected text after the closing `}`"));
        }
        Ok(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for s in [Settings::default(), Settings { hotkey: "ctrl+alt+space".into(), case_sensitive: true }] {
            assert_eq!(Settings::parse(&s.to_text()), (s, vec![]));
        }
    }

    #[test]
    fn parses_values_and_warns() {
        let (s, w) = Settings::parse(
            "\u{feff}{ \"hotkey\" : \" Ctrl + Alt + Space \", \"case_sensitive\": true,\n \"theme\": [1, {\"a\": null}], \"x\": -1.5e3 }",
        );
        assert_eq!(s, Settings { hotkey: "ctrl+alt+space".into(), case_sensitive: true });
        assert_eq!(w, ["unknown setting `theme`", "unknown setting `x`"]);

        let (s, w) = Settings::parse(r#"{"hotkey": "ctrl+nope", "case_sensitive": "yes"}"#);
        assert_eq!(s, Settings::default());
        assert_eq!(w.len(), 2);
        assert!(w[0].contains("not a valid hotkey"));
        assert!(w[1].starts_with("invalid value for `case_sensitive`"));
    }

    #[test]
    fn reports_syntax_errors() {
        for (text, want) in [
            ("{\n  \"hotkey\": \"f12\"\n  \"case_sensitive\": true\n}", "line 3, column 3: expected `,`"),
            ("", "line 1, column 1: expected `{`"),
            ("{\"hotkey\": tru}", "line 1, column 12: expected a value"),
            ("{} x", "line 1, column 4: unexpected text"),
            ("{\"hotkey\": \"f1", "unterminated string"),
        ] {
            let (s, w) = Settings::parse(text);
            assert_eq!(s, Settings::default());
            assert!(w.len() == 1 && w[0].contains(want), "{text:?}: {w:?}");
        }
        assert_eq!(json::quote("a\"b\\c\n"), r#""a\"b\\c\u000a""#);
    }
}
