//! The result list: custom-drawn rows with a file-type icon, the name with the matched part
//! highlighted, and the parent folder below it. Programs named like the keyword (see
//! `finder_core::launch`) are marked; opening one runs it.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use finder_core::fold::fold;
use finder_core::launch;
use finder_core::paths::SEP;
use finder_core::proto::Hit;
use fltk::enums::{Align, Color, Event, Font};
use fltk::prelude::*;
use fltk::{app, draw, widget::Widget};

use crate::theme::{Palette, mix};

pub const ROW_H: i32 = 46;
const ICON_X: i32 = 18;
const TEXT_X: i32 = 54;
const SCROLL_W: i32 = 10;

/// One result, prepared for drawing.
struct Row {
    hit: Hit,
    name: String,
    parent: String,
    kind: Kind,
    name_hl: Option<Range<usize>>,
    /// A program named like the keyword: opening it runs it.
    program: bool,
}

struct State {
    rows: Vec<Row>,
    selected: Option<usize>,
    hover: Option<usize>,
    /// Pixels scrolled from the top.
    scroll: i32,
    /// Grab offset while dragging the scrollbar thumb.
    drag: Option<i32>,
    palette: Palette,
}

#[derive(Clone)]
pub struct ResultList {
    w: Widget,
    st: Rc<RefCell<State>>,
}

impl ResultList {
    /// `on_open(hit, program)` is called on double-click; `program` if the hit is a program to run.
    pub fn new(x: i32, y: i32, w: i32, h: i32, palette: Palette, on_open: impl Fn(&Hit, bool) + 'static) -> ResultList {
        let mut widget = Widget::new(x, y, w, h, None);
        let st = Rc::new(RefCell::new(State {
            rows: Vec::new(),
            selected: None,
            hover: None,
            scroll: 0,
            drag: None,
            palette,
        }));
        widget.draw({
            let st = st.clone();
            move |w| draw_list(w, &st.borrow())
        });
        widget.handle({
            let st = st.clone();
            move |w, ev| handle(w, &st, ev, &on_open)
        });
        ResultList { w: widget, st }
    }

    pub fn widget(&mut self) -> &mut Widget {
        &mut self.w
    }

    pub fn len(&self) -> usize {
        self.st.borrow().rows.len()
    }

    /// Replace the rows; the first one is selected.
    pub fn set_hits(&mut self, hits: Vec<Hit>, query: &str, case_sensitive: bool) {
        let mut st = self.st.borrow_mut();
        st.rows = hits.into_iter().map(|h| Row::new(h, query, case_sensitive)).collect();
        st.selected = (!st.rows.is_empty()).then_some(0);
        st.hover = None;
        st.scroll = 0;
        drop(st);
        self.w.redraw();
    }

    pub fn clear(&mut self) {
        self.set_hits(Vec::new(), "", false);
    }

    /// The selected hit, and whether it is a program to run.
    pub fn selected_hit(&self) -> Option<(Hit, bool)> {
        let st = self.st.borrow();
        st.selected.and_then(|i| st.rows.get(i)).map(|r| (r.hit.clone(), r.program))
    }

    /// Move the selection by `delta` rows and scroll it into view.
    pub fn move_selection(&mut self, delta: i32) {
        let mut st = self.st.borrow_mut();
        let n = st.rows.len() as i32;
        if n == 0 {
            return;
        }
        let cur = st.selected.map_or(-1, |i| i as i32);
        let target = (cur + delta).clamp(0, n - 1);
        st.selected = Some(target as usize);
        let (top, bottom) = (target * ROW_H, (target + 1) * ROW_H);
        if top < st.scroll {
            st.scroll = top;
        } else if bottom > st.scroll + self.w.h() {
            st.scroll = bottom - self.w.h();
        }
        drop(st);
        self.w.redraw();
    }
}

impl Row {
    fn new(hit: Hit, query: &str, case_sensitive: bool) -> Row {
        let path = String::from_utf8_lossy(&hit.path).into_owned();
        let cut = path.trim_end_matches(SEP as char).rfind(SEP as char);
        let (parent, name) = match cut {
            Some(i) if i + 1 < path.len() => (path[..i.max(root_len(&path))].to_string(), path[i + 1..].to_string()),
            _ => (String::new(), path.clone()),
        };
        let name_hl = find(&name, query, case_sensitive);
        let program = launch::is_launch_hit(&hit.path, hit.is_dir, query, case_sensitive);
        let kind = if hit.is_dir {
            Kind::Folder
        } else if program {
            Kind::Program
        } else {
            Kind::of(&name)
        };
        Row { hit, name, parent, kind, name_hl, program }
    }
}

/// Length of the root part that must stay in a parent path (`/`, `C:\`).
fn root_len(path: &str) -> usize {
    if cfg!(windows) { 3.min(path.len()) } else { 1 }
}

/// First occurrence of `needle` in `hay` as a byte range of `hay` (folding keeps byte offsets).
fn find(hay: &str, needle: &str, case_sensitive: bool) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let (h, n) = if case_sensitive {
        (hay.as_bytes().to_vec(), needle.as_bytes().to_vec())
    } else {
        (fold(hay.as_bytes()), fold(needle.as_bytes()))
    };
    let start = memchr_find(&h, &n)?;
    let r = start..start + n.len();
    (hay.is_char_boundary(r.start) && hay.is_char_boundary(r.end)).then_some(r)
}

fn memchr_find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// File categories, each with an icon color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Folder,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Text,
    Pdf,
    Sheet,
    Slides,
    Program,
    Other,
}

impl Kind {
    pub fn of(name: &str) -> Kind {
        let Some((stem, ext)) = name.rsplit_once('.') else { return Kind::Other };
        if stem.is_empty() {
            return Kind::Other;
        }
        match ext.to_ascii_lowercase().as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "tif" | "tiff" | "heic" | "avif"
            | "raw" | "cr2" | "nef" | "psd" | "xcf" | "kra" => Kind::Image,
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg" => Kind::Video,
            "mp3" | "flac" | "ogg" | "opus" | "wav" | "m4a" | "aac" | "wma" | "mid" | "midi" => Kind::Audio,
            "zip" | "tar" | "gz" | "tgz" | "xz" | "zst" | "bz2" | "7z" | "rar" | "iso" | "deb" | "rpm" | "dmg"
            | "cab" | "jar" => Kind::Archive,
            "rs" | "c" | "h" | "cpp" | "hpp" | "cc" | "py" | "js" | "ts" | "tsx" | "jsx" | "java" | "kt" | "go"
            | "rb" | "php" | "cs" | "swift" | "lua" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd"
            | "html" | "htm" | "css" | "scss" | "json" | "toml" | "yaml" | "yml" | "xml" | "sql" | "vue" | "svelte"
            | "zig" | "hs" | "ml" | "ex" | "exs" | "dart" | "r" | "m" | "cmake" | "mk" | "ini" | "conf" | "cfg"
            | "lock" => Kind::Code,
            "txt" | "md" | "rst" | "doc" | "docx" | "odt" | "rtf" | "tex" | "log" | "csv" | "epub" => Kind::Text,
            "pdf" => Kind::Pdf,
            "xls" | "xlsx" | "ods" => Kind::Sheet,
            "ppt" | "pptx" | "odp" | "key" => Kind::Slides,
            "exe" | "msi" | "appimage" | "dll" | "so" | "bin" | "run" | "flatpakref" | "apk" => Kind::Program,
            _ => Kind::Other,
        }
    }

    fn color(self, p: &Palette) -> Color {
        let hex = match self {
            Kind::Folder => return p.accent,
            Kind::Image => 0xa855f7,
            Kind::Video => 0xe11d48,
            Kind::Audio => 0xf97316,
            Kind::Archive => 0xb45309,
            Kind::Code => 0x16a34a,
            Kind::Text => 0x2563eb,
            Kind::Pdf => 0xdc2626,
            Kind::Sheet => 0x059669,
            Kind::Slides => 0xea580c,
            Kind::Program => 0x64748b,
            Kind::Other => return p.muted,
        };
        Color::from_hex(hex)
    }
}

/// Short uppercase extension for the file icon label (`PDF`, `RS`), at most 4 characters.
fn ext_label(name: &str) -> Option<String> {
    let (stem, ext) = name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() || ext.len() > 4 || !ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(ext.to_ascii_uppercase())
}

fn polygon(points: &[(i32, i32)]) {
    draw::begin_complex_polygon();
    for &(x, y) in points {
        draw::vertex(x as f64, y as f64);
    }
    draw::end_complex_polygon();
}

fn outline(points: &[(i32, i32)]) {
    draw::begin_loop();
    for &(x, y) in points {
        draw::vertex(x as f64, y as f64);
    }
    draw::end_loop();
}

/// 24×24 icon at (x, y). `on` = drawn on the selection color.
fn draw_icon(x: i32, y: i32, kind: Kind, name: &str, p: &Palette, on: bool) {
    let color = if on { p.on_accent } else { kind.color(p) };
    if kind == Kind::Folder {
        draw::set_draw_color(if on { mix(p.accent, p.on_accent, 0.7) } else { mix(color, p.surface, 0.3) });
        draw::draw_rounded_rectf(x + 1, y + 3, 10, 6, 2);
        draw::set_draw_color(color);
        draw::draw_rounded_rectf(x + 1, y + 6, 22, 15, 3);
        return;
    }
    let page = [(x + 4, y + 1), (x + 15, y + 1), (x + 21, y + 7), (x + 21, y + 23), (x + 4, y + 23)];
    let paper = match (on, p.dark) {
        (true, _) => mix(p.accent, p.on_accent, 0.85),
        (false, true) => mix(p.surface, p.text, 0.12),
        (false, false) => p.surface,
    };
    let line = if on { p.on_accent } else { mix(p.surface, p.text, if p.dark { 0.35 } else { 0.3 }) };
    draw::set_draw_color(paper);
    polygon(&page);
    draw::set_draw_color(line);
    outline(&page);
    polygon(&[(x + 15, y + 1), (x + 15, y + 7), (x + 21, y + 7)]);
    match ext_label(name) {
        Some(label) if kind != Kind::Other || label.len() <= 4 => {
            draw::set_font(Font::HelveticaBold, 7);
            let tw = draw::width(&label) as i32;
            let bw = (tw + 4).max(14);
            let bx = (x + 13 - bw / 2).max(x);
            draw::set_draw_color(if on { mix(p.accent, p.on_accent, 0.25) } else { kind.color(p) });
            draw::draw_rounded_rectf(bx, y + 12, bw, 9, 2);
            draw::set_draw_color(if on { p.on_accent } else { Color::White });
            draw::draw_text(&label, bx + (bw - tw) / 2, y + 19);
        }
        _ => {}
    }
}

/// `text` shortened from the left to fit `max` pixels with the current font (`…/c/d`).
fn elide_left(text: &str, max: i32) -> String {
    if draw::width(text) as i32 <= max {
        return text.to_string();
    }
    let mut start = 0;
    for (i, _) in text.char_indices() {
        start = i;
        if draw::width(&format!("…{}", &text[i..])) as i32 <= max {
            break;
        }
    }
    format!("…{}", &text[start..])
}

/// `text` shortened at the end to fit `max` pixels; returns the kept byte length.
fn elide_right(text: &str, max: i32) -> (String, usize) {
    if draw::width(text) as i32 <= max {
        return (text.to_string(), text.len());
    }
    let mut end = text.len();
    while end > 0 {
        end = text.floor_char_boundary(end - 1);
        if draw::width(&format!("{}…", &text[..end])) as i32 <= max {
            break;
        }
    }
    (format!("{}…", &text[..end]), end)
}

/// Text colors of a row: normal text, and the highlighted match (pill behind it, text on it).
struct Ink {
    fg: Color,
    hl_bg: Color,
    hl_fg: Color,
}

/// Draw `text` at (x, baseline y) with byte range `hl` highlighted: a tinted pill behind it and bold text.
fn draw_highlighted(text: &str, hl: Option<Range<usize>>, x: i32, y: i32, size: i32, ink: &Ink) {
    let Ink { fg, hl_bg, hl_fg } = *ink;
    let Some(r) = hl.filter(|r| r.end <= text.len() && r.start < r.end) else {
        draw::set_font(Font::Helvetica, size);
        draw::set_draw_color(fg);
        draw::draw_text(text, x, y);
        return;
    };
    let (pre, mid, post) = (&text[..r.start], &text[r.start..r.end], &text[r.end..]);
    draw::set_font(Font::Helvetica, size);
    let pre_w = draw::width(pre) as i32;
    draw::set_font(Font::HelveticaBold, size);
    let mid_w = draw::width(mid) as i32;
    let (asc, desc) = (draw::height() - draw::descent(), draw::descent());
    draw::set_draw_color(hl_bg);
    draw::draw_rounded_rectf(x + pre_w - 2, y - asc - 1, mid_w + 4, asc + desc + 2, 3);
    draw::set_font(Font::Helvetica, size);
    draw::set_draw_color(fg);
    draw::draw_text(pre, x, y);
    draw::set_font(Font::HelveticaBold, size);
    draw::set_draw_color(hl_fg);
    draw::draw_text(mid, x + pre_w, y);
    draw::set_font(Font::Helvetica, size);
    draw::set_draw_color(fg);
    draw::draw_text(post, x + pre_w + mid_w, y);
}

fn content_h(st: &State) -> i32 {
    st.rows.len() as i32 * ROW_H
}

/// Scrollbar thumb as (y offset within the widget, height); `None` if everything fits.
fn thumb(st: &State, h: i32) -> Option<(i32, i32)> {
    let total = content_h(st);
    if total <= h {
        return None;
    }
    let len = ((h as i64 * h as i64 / total as i64) as i32).max(24);
    let pos = ((h - len) as i64 * st.scroll as i64 / (total - h) as i64) as i32;
    Some((pos, len))
}

fn draw_list(w: &mut Widget, st: &State) {
    let p = &st.palette;
    let (x, y, ww, h) = (w.x(), w.y(), w.w(), w.h());
    draw::push_clip(x, y, ww, h);
    draw::set_draw_color(p.surface);
    draw::draw_rectf(x, y, ww, h);
    let first = (st.scroll / ROW_H) as usize;
    let last = ((st.scroll + h) / ROW_H + 1) as usize;
    for i in first..last.min(st.rows.len()) {
        let row = &st.rows[i];
        let ry = y + i as i32 * ROW_H - st.scroll;
        let on = st.selected == Some(i);
        if on || st.hover == Some(i) {
            draw::set_draw_color(if on { p.accent } else { p.hover });
            draw::draw_rounded_rectf(x + 6, ry + 2, ww - 12 - SCROLL_W / 2, ROW_H - 4, 7);
        }
        draw_icon(x + ICON_X, ry + (ROW_H - 24) / 2, row.kind, &row.name, p, on);
        let mut text_w = ww - TEXT_X - 16 - SCROLL_W;
        let (fg, sub) = if on { (p.on_accent, mix(p.accent, p.on_accent, 0.8)) } else { (p.text, p.muted) };
        if row.program {
            let tag = if on { "Enter: run   Ctrl+Enter: show in folder" } else { "Program" };
            draw::set_font(Font::Helvetica, 12);
            let tw = draw::measure(tag, false).0;
            draw::set_draw_color(sub);
            draw::draw_text2(tag, x + ww - 16 - SCROLL_W - tw, ry, tw, ROW_H, Align::Right | Align::Inside);
            text_w -= tw + 16;
        }
        let (hl_bg, hl_fg) = if on { (mix(p.accent, p.on_accent, 0.25), p.on_accent) } else { (p.match_bg, p.text) };
        // Name.
        draw::set_font(Font::Helvetica, 14);
        let (name, kept) = elide_right(&row.name, text_w);
        let name_hl = row.name_hl.clone().filter(|r| r.end <= kept);
        draw_highlighted(&name, name_hl, x + TEXT_X, ry + 20, 14, &Ink { fg, hl_bg, hl_fg });
        // Parent folder.
        draw::set_font(Font::Helvetica, 12);
        draw::set_draw_color(sub);
        draw::draw_text(&elide_left(&row.parent, text_w), x + TEXT_X, ry + 37);
    }
    if let Some((pos, len)) = thumb(st, h) {
        draw::set_draw_color(mix(p.surface, p.text, if st.drag.is_some() { 0.45 } else { 0.25 }));
        draw::draw_rounded_rectf(x + ww - SCROLL_W + 2, y + pos + 2, SCROLL_W - 5, len - 4, 3);
    }
    draw::pop_clip();
}

fn clamp_scroll(st: &mut State, h: i32) {
    st.scroll = st.scroll.clamp(0, (content_h(st) - h).max(0));
}

/// Scroll offset that puts a thumb of length `len` at `pos` within a view of height `h`.
fn scroll_for_thumb(st: &State, h: i32, pos: i32, len: i32) -> i32 {
    (pos as i64 * (content_h(st) - h) as i64 / (h - len).max(1) as i64) as i32
}

fn row_at(w: &Widget, st: &State, ey: i32) -> Option<usize> {
    let i = ((ey - w.y() + st.scroll) / ROW_H) as usize;
    (ey >= w.y() && i < st.rows.len()).then_some(i)
}

fn handle(w: &mut Widget, st: &Rc<RefCell<State>>, ev: Event, on_open: &dyn Fn(&Hit, bool)) -> bool {
    let (ex, ey) = app::event_coords();
    let on_scrollbar = ex >= w.x() + w.w() - SCROLL_W;
    match ev {
        Event::Enter => true,
        Event::Move => {
            let mut s = st.borrow_mut();
            let hover = if on_scrollbar { None } else { row_at(w, &s, ey) };
            if hover != s.hover {
                s.hover = hover;
                w.redraw();
            }
            true
        }
        Event::Leave => {
            st.borrow_mut().hover = None;
            w.redraw();
            true
        }
        Event::MouseWheel => {
            let mut s = st.borrow_mut();
            s.scroll += app::event_dy_value() * ROW_H;
            clamp_scroll(&mut s, w.h());
            s.hover = row_at(w, &s, ey);
            w.redraw();
            true
        }
        Event::Push => {
            let mut s = st.borrow_mut();
            if on_scrollbar {
                if let Some((pos, len)) = thumb(&s, w.h()) {
                    let rel = ey - w.y();
                    if !(pos..pos + len).contains(&rel) {
                        // Jump so the thumb centers on the click.
                        s.scroll = scroll_for_thumb(&s, w.h(), rel - len / 2, len);
                        clamp_scroll(&mut s, w.h());
                    }
                    let (pos, _) = thumb(&s, w.h()).unwrap();
                    s.drag = Some(rel - pos);
                    w.redraw();
                }
                return true;
            }
            let Some(i) = row_at(w, &s, ey) else { return true };
            s.selected = Some(i);
            w.redraw();
            if app::event_clicks() {
                let (hit, program) = (s.rows[i].hit.clone(), s.rows[i].program);
                drop(s);
                on_open(&hit, program);
            }
            true
        }
        Event::Drag => {
            let mut s = st.borrow_mut();
            if let (Some(grab), Some((_, len))) = (s.drag, thumb(&s, w.h())) {
                s.scroll = scroll_for_thumb(&s, w.h(), ey - w.y() - grab, len);
                clamp_scroll(&mut s, w.h());
                w.redraw();
            }
            true
        }
        Event::Released => {
            if st.borrow_mut().drag.take().is_some() {
                w.redraw();
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn hit(path: &str, is_dir: bool) -> Hit {
        Hit { is_dir, path: path.as_bytes().to_vec() }
    }

    #[cfg(unix)]
    #[test]
    fn splits_and_highlights() {
        let r = Row::new(hit("/home/u/Notes.TXT", false), "notes", false);
        assert_eq!((r.name.as_str(), r.name_hl.clone()), ("Notes.TXT", Some(0..5)));
        assert_eq!(r.kind, Kind::Text);
        let r = Row::new(hit("/etc", true), "etc", false);
        assert_eq!((r.parent.as_str(), r.kind), ("/", Kind::Folder));
        // Case-sensitive: no match, no highlight.
        assert_eq!(Row::new(hit("/a/Notes", false), "notes", true).name_hl, None);
        // Non-ASCII: ranges stay on character boundaries.
        let r = Row::new(hit("/x/ÄÖÜ-Straße.md", false), "straße", false);
        assert_eq!(&r.name[r.name_hl.clone().unwrap()], "Straße");
    }

    #[test]
    fn kinds_and_labels() {
        assert_eq!(Kind::of("photo.JPG"), Kind::Image);
        assert_eq!(Kind::of(".bashrc"), Kind::Other);
        assert_eq!(Kind::of("Makefile"), Kind::Other);
        assert_eq!(ext_label("a.tar.gz").as_deref(), Some("GZ"));
        assert_eq!(ext_label("a.verylongext"), None);
        assert_eq!(ext_label(".hidden"), None);
    }
}
