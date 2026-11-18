//! Linux: like Dolphin, the MIME type (by name via shared-mime-info's globs; by content for names
//! without a match) picks an icon name, which is looked up in the icon theme (KDE's setting, else
//! GTK's, else Breeze) and its parents, then rendered. Breeze SVGs color some parts with the
//! color scheme (`ColorScheme-Text`, `ColorScheme-Highlight`, …); that stylesheet is filled
//! from the UI palette, as KDE does.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use finder_core::paths;
use resvg::tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};
use resvg::usvg;

use super::{Found, Pixels};

/// Colors for the scheme stylesheet, as `#rrggbb`.
pub struct Colors {
    pub text: String,
    pub background: String,
    pub highlight: String,
    pub highlighted_text: String,
}

impl Colors {
    fn style_sheet(&self) -> String {
        let Colors { text, background, highlight, highlighted_text } = self;
        [
            ("Text", text.as_str()),
            ("Background", background),
            ("Highlight", highlight),
            ("Accent", highlight),
            ("HighlightedText", highlighted_text),
            ("ViewText", text),
            ("ViewBackground", background),
            ("ButtonText", text),
            ("ButtonBackground", background),
            ("ButtonFocus", highlight),
            ("ViewFocus", highlight),
            ("ViewHover", highlight),
            ("ActiveText", highlight),
            ("PositiveText", "#27ae60"),
            ("NeutralText", "#f67400"),
            ("NegativeText", "#da4453"),
        ]
        .iter()
        .map(|(class, color)| format!(".ColorScheme-{class}{{color:{color};}}"))
        .collect()
    }
}

/// `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`, most important first.
fn data_dirs() -> Vec<PathBuf> {
    let var = |v: &str| std::env::var_os(v).filter(|v| !v.is_empty());
    let home = var("XDG_DATA_HOME").map_or_else(|| paths::home().join(".local/share"), PathBuf::from);
    let rest = var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(home).chain(std::env::split_paths(&rest)).collect()
}

/// shared-mime-info: file name → MIME type, MIME type → icon name.
#[derive(Default)]
struct MimeDb {
    /// Whole names (`Makefile`); case-sensitive ones keep their case, others are lowercase.
    names: HashMap<String, String>,
    /// Extensions without the leading `*.` (`tar.gz`), same case rule.
    exts: HashMap<String, String>,
    icons: HashMap<String, String>,
    generic: HashMap<String, String>,
    /// Alias → canonical type.
    aliases: HashMap<String, String>,
    /// Type → parent types (`text/x-csrc` → `text/plain`).
    parents: HashMap<String, Vec<String>>,
}

impl MimeDb {
    fn load() -> MimeDb {
        let mut db = MimeDb::default();
        for dir in data_dirs() {
            let mime = dir.join("mime");
            // globs2 lines: `weight:type:glob[:flags]`, highest weight first; the first entry wins.
            for line in fs::read_to_string(mime.join("globs2")).unwrap_or_default().lines() {
                let mut f = line.splitn(4, ':');
                let (Some(_), Some(ty), Some(glob)) = (f.next(), f.next(), f.next()) else { continue };
                if line.starts_with('#') || ty == "__NOGLOBS__" {
                    continue;
                }
                let cs = f.next().is_some_and(|flags| flags.split(',').any(|x| x == "cs"));
                let key = |s: &str| if cs { s.to_string() } else { s.to_lowercase() };
                let wild = |s: &str| s.contains(['*', '?', '[']);
                if let Some(ext) = glob.strip_prefix("*.").filter(|e| !wild(e)) {
                    db.exts.entry(key(ext)).or_insert_with(|| ty.to_string());
                } else if !wild(glob) {
                    db.names.entry(key(glob)).or_insert_with(|| ty.to_string());
                }
            }
            for (file, map) in [("icons", &mut db.icons), ("generic-icons", &mut db.generic)] {
                for line in fs::read_to_string(mime.join(file)).unwrap_or_default().lines() {
                    if let Some((ty, icon)) = line.split_once(':') {
                        map.entry(ty.to_string()).or_insert_with(|| icon.to_string());
                    }
                }
            }
            for line in fs::read_to_string(mime.join("aliases")).unwrap_or_default().lines() {
                if let Some((alias, ty)) = line.split_once(' ') {
                    db.aliases.entry(alias.to_string()).or_insert_with(|| ty.to_string());
                }
            }
            for line in fs::read_to_string(mime.join("subclasses")).unwrap_or_default().lines() {
                if let Some((ty, parent)) = line.split_once(' ') {
                    db.parents.entry(ty.to_string()).or_default().push(parent.to_string());
                }
            }
        }
        db
    }

    /// MIME type by name: whole name, then the longest known extension.
    fn by_name(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        let found = self.names.get(name).or_else(|| self.names.get(&lower)).or_else(|| {
            name.match_indices('.').find_map(|(i, _)| {
                self.exts.get(&name[i + 1..]).or_else(|| lower.get(i + 1..).and_then(|e| self.exts.get(e)))
            })
        });
        found.map(String::as_str)
    }

    /// Icon names to try for a MIME type, best first: its own, then its parent types', then the
    /// generic one for its media type.
    fn icon_names(&self, ty: &str) -> Vec<String> {
        let ty = self.aliases.get(ty).map_or(ty, String::as_str);
        let mut names = Vec::new();
        let mut todo = vec![ty.to_string()];
        let mut seen = HashSet::new();
        while !todo.is_empty() {
            let t = todo.remove(0);
            if !seen.insert(t.clone()) {
                continue;
            }
            names.extend(self.icons.get(&t).cloned());
            names.push(t.replace('/', "-"));
            names.extend(self.generic.get(&t).cloned());
            todo.extend(self.parents.get(&t).into_iter().flatten().cloned());
        }
        let media = ty.split('/').next().unwrap_or("application");
        names.push(format!("{media}-x-generic"));
        names.push("unknown".into());
        let mut unique = HashSet::new();
        names.retain(|n| unique.insert(n.clone()));
        names
    }
}

/// MIME type from the first bytes, for names no glob knows.
fn sniff(path: &Path) -> &'static str {
    let mut head = [0u8; 512];
    let n = fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    let head = &head[..n];
    if head.is_empty() {
        return "text/plain";
    }
    if head.starts_with(b"\x7fELF") && head.len() > 17 {
        return match u16::from_le_bytes([head[16], head[17]]) {
            // Position-independent programs are type 3 like libraries; KDE tells them apart by
            // the execute bit.
            2 => "application/x-executable",
            3 if finder_core::launch::is_executable(path) => "application/x-executable",
            3 => "application/x-sharedlib",
            4 => "application/x-core",
            _ => "application/x-object",
        };
    }
    if head.starts_with(b"#!") {
        let line = head.split(|&b| b == b'\n').next().unwrap_or_default();
        return if line.windows(6).any(|w| w == b"python") { "text/x-python3" } else { "text/x-shellscript" };
    }
    // Cut at a character boundary before judging: the buffer may end mid-character.
    let valid = match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none(),
    };
    if valid && !head.contains(&0) { "text/plain" } else { "application/octet-stream" }
}

/// Dolphin's icons for the home folder and the XDG user folders.
fn special_dirs() -> HashMap<PathBuf, &'static str> {
    let home = paths::home();
    let mut m = HashMap::from([(home.clone(), "user-home")]);
    let file = paths::xdg_config_home().join("user-dirs.dirs");
    for line in fs::read_to_string(file).unwrap_or_default().lines() {
        let Some((key, value)) = line.split_once('=') else { continue };
        let icon = match key.trim() {
            "XDG_DESKTOP_DIR" => "user-desktop",
            "XDG_DOCUMENTS_DIR" => "folder-documents",
            "XDG_DOWNLOAD_DIR" => "folder-download",
            "XDG_MUSIC_DIR" => "folder-music",
            "XDG_PICTURES_DIR" => "folder-pictures",
            "XDG_VIDEOS_DIR" => "folder-videos",
            "XDG_TEMPLATES_DIR" => "folder-templates",
            "XDG_PUBLICSHARE_DIR" => "folder-publicshare",
            _ => continue,
        };
        let value = value.trim().trim_matches('"');
        let dir = match value.strip_prefix("$HOME") {
            Some(rest) => home.join(rest.trim_start_matches('/')),
            None => PathBuf::from(value),
        };
        if dir != home {
            m.insert(dir, icon);
        }
    }
    m
}

/// One subdirectory of a theme (`mimetypes/24`).
struct SubDir {
    path: String,
    size: u32,
    scale: u32,
    min: u32,
    max: u32,
}

impl SubDir {
    /// Distance from `px` (0: usable as is), per the icon theme spec.
    fn distance(&self, px: u32) -> u32 {
        let (min, max) = (self.min * self.scale, self.max * self.scale);
        min.saturating_sub(px) + px.saturating_sub(max)
    }
}

struct Theme {
    /// Folders holding this theme (`/usr/share/icons/breeze`).
    roots: Vec<PathBuf>,
    dirs: Vec<SubDir>,
    inherits: Vec<String>,
}

impl Theme {
    fn load(name: &str, bases: &[PathBuf]) -> Option<Theme> {
        let roots: Vec<PathBuf> = bases.iter().map(|b| b.join(name)).filter(|r| r.is_dir()).collect();
        let index = roots.iter().find_map(|r| fs::read_to_string(r.join("index.theme")).ok())?;
        let mut sections: HashMap<&str, HashMap<&str, &str>> = HashMap::new();
        let mut current = "";
        for line in index.lines().map(str::trim) {
            if let Some(s) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                current = s;
            } else if let Some((k, v)) = line.split_once('=') {
                sections.entry(current).or_default().insert(k.trim(), v.trim());
            }
        }
        let main = sections.get("Icon Theme")?;
        let list = |key: &str| -> Vec<String> {
            main.get(key).map_or(Vec::new(), |v| {
                v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
            })
        };
        let mut dirs = Vec::new();
        for d in list("Directories").into_iter().chain(list("ScaledDirectories")) {
            let Some(s) = sections.get(d.as_str()) else { continue };
            let num = |k: &str| s.get(k).and_then(|v| v.parse::<u32>().ok());
            let Some(size) = num("Size") else { continue };
            let (min, max) = match s.get("Type").copied().unwrap_or("Threshold") {
                "Fixed" => (size, size),
                "Scalable" => (num("MinSize").unwrap_or(size), num("MaxSize").unwrap_or(size)),
                _ => {
                    let t = num("Threshold").unwrap_or(2);
                    (size.saturating_sub(t), size + t)
                }
            };
            dirs.push(SubDir { path: d, size, scale: num("Scale").unwrap_or(1).max(1), min, max });
        }
        Some(Theme { roots, dirs, inherits: list("Inherits") })
    }

    /// Best file for `name` at `px` pixels in this theme alone.
    fn find(&self, name: &str, px: u32) -> Option<PathBuf> {
        let mut best: Option<(u32, PathBuf)> = None;
        for d in &self.dirs {
            let dist = d.distance(px);
            // Prefer exact, then 1x directories, then the nominal size closest to `px`.
            let rank = dist * 4 + u32::from(d.scale != 1) * 2 + u32::from(d.size * d.scale != px);
            if best.as_ref().is_some_and(|(r, _)| *r <= rank) {
                continue;
            }
            for root in &self.roots {
                for ext in ["svg", "png"] {
                    let f = root.join(&d.path).join(format!("{name}.{ext}"));
                    if f.is_file() {
                        best = Some((rank, f));
                        break;
                    }
                }
                if best.as_ref().is_some_and(|(r, _)| *r == rank) {
                    break;
                }
            }
        }
        best.map(|(_, f)| f)
    }
}

/// The desktop's icon theme name.
fn theme_name() -> String {
    let config = paths::xdg_config_home();
    let ini_value = |file: PathBuf, section: &str, key: &str| -> Option<String> {
        let text = fs::read_to_string(file).ok()?;
        let mut in_section = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_section = line == format!("[{section}]");
            } else if in_section
                && let Some((k, v)) = line.split_once('=')
                && k.trim() == key
                && !v.trim().is_empty()
            {
                return Some(v.trim().to_string());
            }
        }
        None
    };
    ini_value(config.join("kdeglobals"), "Icons", "Theme")
        .or_else(|| ini_value(config.join("gtk-3.0/settings.ini"), "Settings", "gtk-icon-theme-name"))
        .unwrap_or_else(|| "breeze".into())
}

pub struct Loader {
    px: u32,
    style_sheet: String,
    mime: Option<MimeDb>,
    /// The theme and its parents, then `hicolor`.
    themes: Option<Vec<Theme>>,
    special: Option<HashMap<PathBuf, &'static str>>,
    /// Icon name → file.
    files: HashMap<String, Option<PathBuf>>,
    /// Icon files already sent (by id = file path), or that failed to render.
    sent: HashSet<String>,
    failed: HashSet<String>,
}

impl Loader {
    pub fn new(colors: Colors, px: u32) -> Loader {
        Loader {
            px,
            style_sheet: colors.style_sheet(),
            mime: None,
            themes: None,
            special: None,
            files: HashMap::new(),
            sent: HashSet::new(),
            failed: HashSet::new(),
        }
    }

    fn themes(&mut self) -> &[Theme] {
        self.themes.get_or_insert_with(|| {
            let mut bases: Vec<PathBuf> = data_dirs().into_iter().map(|d| d.join("icons")).collect();
            bases.insert(1, paths::home().join(".icons"));
            let mut themes = Vec::new();
            let mut todo = vec![theme_name()];
            let mut seen = HashSet::new();
            while let Some(name) = todo.pop() {
                if !seen.insert(name.clone()) {
                    continue;
                }
                if let Some(t) = Theme::load(&name, &bases) {
                    todo.extend(t.inherits.iter().rev().cloned());
                    themes.push(t);
                }
            }
            if seen.insert("hicolor".into())
                && let Some(t) = Theme::load("hicolor", &bases)
            {
                themes.push(t);
            }
            themes
        })
    }

    fn file(&mut self, name: &str) -> Option<PathBuf> {
        if let Some(f) = self.files.get(name) {
            return f.clone();
        }
        let px = self.px;
        let f = self.themes().iter().find_map(|t| t.find(name, px)).or_else(|| {
            ["svg", "png"].iter().map(|e| PathBuf::from(format!("/usr/share/pixmaps/{name}.{e}"))).find(|p| p.is_file())
        });
        self.files.insert(name.to_string(), f.clone());
        f
    }

    fn icon_names(&mut self, path: &Path, is_dir: bool) -> Vec<String> {
        if is_dir {
            let special = self.special.get_or_insert_with(special_dirs);
            let mut names: Vec<String> = special.get(path).map(|s| s.to_string()).into_iter().collect();
            names.push("folder".into());
            return names;
        }
        let mime = self.mime.get_or_insert_with(MimeDb::load);
        let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        let ty = match mime.by_name(&name) {
            Some(t) => t.to_string(),
            None => sniff(path).to_string(),
        };
        mime.icon_names(&ty)
    }

    fn render(&self, file: &Path) -> Option<Pixels> {
        let px = self.px;
        let data = fs::read(file).ok()?;
        let mut out = Pixmap::new(px, px)?;
        if file.extension().is_some_and(|e| e == "png") {
            let src = Pixmap::decode_png(&data).ok()?;
            let s = px as f32 / src.width().max(src.height()) as f32;
            let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
            out.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_scale(s, s), None);
        } else {
            let opt = usvg::Options { style_sheet: Some(self.style_sheet.clone()), ..usvg::Options::default() };
            let tree = usvg::Tree::from_data(&data, &opt).ok()?;
            let size = tree.size();
            let s = px as f32 / size.width().max(size.height());
            let (dx, dy) = ((px as f32 - size.width() * s) / 2.0, (px as f32 - size.height() * s) / 2.0);
            resvg::render(&tree, Transform::from_row(s, 0.0, 0.0, s, dx, dy), &mut out.as_mut());
        }
        let rgba = out.pixels().iter().flat_map(|c| {
            let c = c.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        });
        Some(Pixels { w: px, h: px, rgba: rgba.collect() })
    }

    pub fn load(&mut self, path: &[u8], is_dir: bool) -> Found {
        let path = paths::from_bytes(path);
        for name in self.icon_names(&path, is_dir) {
            let Some(file) = self.file(&name) else { continue };
            let id = file.to_string_lossy().into_owned();
            if self.sent.contains(&id) {
                return Some((id, None));
            }
            if self.failed.contains(&id) {
                continue;
            }
            match self.render(&file) {
                Some(p) => {
                    self.sent.insert(id.clone());
                    return Some((id, Some(p)));
                }
                None => {
                    self.failed.insert(id);
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_by_name() {
        let mut db = MimeDb::default();
        db.exts.insert("gz".into(), "application/gzip".into());
        db.exts.insert("tar.gz".into(), "application/x-compressed-tar".into());
        db.exts.insert("C".into(), "text/x-c++src".into());
        db.exts.insert("c".into(), "text/x-csrc".into());
        db.names.insert("makefile".into(), "text/x-makefile".into());
        assert_eq!(db.by_name("a.TAR.GZ"), Some("application/x-compressed-tar"));
        assert_eq!(db.by_name("x.gz"), Some("application/gzip"));
        assert_eq!(db.by_name("main.C"), Some("text/x-c++src"));
        assert_eq!(db.by_name("main.c"), Some("text/x-csrc"));
        assert_eq!(db.by_name("Makefile"), Some("text/x-makefile"));
        assert_eq!(db.by_name("README"), None);
        assert_eq!(db.icon_names("text/x-csrc"), ["text-x-csrc", "text-x-generic", "unknown"]);
        db.aliases.insert("text/x-c".into(), "text/x-csrc".into());
        db.parents.insert("text/x-csrc".into(), vec!["text/plain".into()]);
        db.generic.insert("text/x-csrc".into(), "text-x-script".into());
        assert_eq!(
            db.icon_names("text/x-c"),
            ["text-x-csrc", "text-x-script", "text-plain", "text-x-generic", "unknown"]
        );
    }

    #[test]
    fn sniffs() {
        let dir = std::env::temp_dir().join(format!("finder-sniff-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for (name, data, want) in [
            ("empty", &b""[..], "text/plain"),
            ("script", b"#!/bin/sh\necho", "text/x-shellscript"),
            ("py", b"#!/usr/bin/env python3\n", "text/x-python3"),
            ("text", "Grüße\n".as_bytes(), "text/plain"),
            ("bin", b"\x00\x01\x02", "application/octet-stream"),
        ] {
            fs::write(dir.join(name), data).unwrap();
            assert_eq!(sniff(&dir.join(name)), want, "{name}");
        }
        let _ = fs::remove_dir_all(dir);
        assert_eq!(sniff(Path::new("/proc/self/exe")).split('/').next(), Some("application"));
    }
}
