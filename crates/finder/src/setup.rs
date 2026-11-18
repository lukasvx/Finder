//! First-run setup: pick the hotkey and case sensitivity once. Saving writes `settings.json` and
//! starts `finderd` at login; from then on the settings are changed only in that file.

use std::cell::RefCell;
use std::rc::Rc;

use finder_core::hotkey;
use finder_core::settings::Settings;
use fltk::button::Button;
use fltk::enums::{Align, Event, Font, Key, Shortcut};
use fltk::group::Group;
use fltk::prelude::*;
use fltk::{app, draw, widget::Widget};

use crate::theme::Palette;
use crate::widgets::{check, label, pill};

pub const H: i32 = 236;
const SUGGESTED: &str = "ctrl+alt+space";
const WIN_KEY: &str = if cfg!(windows) { "win" } else { "meta" };

#[derive(Clone)]
pub struct Setup {
    pub group: Group,
    pub start: Button,
    capture: Widget,
    state: Rc<RefCell<Capture>>,
    case: fltk::button::CheckButton,
}

#[derive(Default)]
struct Capture {
    /// Normalized hotkey (`ctrl+alt+space`), empty for none.
    hotkey: String,
    /// Message under the field when the last key press was not usable.
    problem: Option<&'static str>,
}

impl Setup {
    pub fn new(w: i32, p: Palette) -> Setup {
        let group = Group::new(1, 0, w - 2, H, None);
        let x = 28;
        let mut f = label(x, 18, w - 2 * x, 28, "Welcome to Finder", 18, true);
        f.set_label_color(p.text);
        let mut f =
            label(x, 46, w - 2 * x, 22, "Two settings, chosen once. Later, change them in settings.json.", 13, false);
        f.set_label_color(p.muted);

        let mut f = label(x, 90, 190, 34, "Shortcut to open Finder", 14, false);
        f.set_label_color(p.text);
        let state = Rc::new(RefCell::new(Capture { hotkey: SUGGESTED.into(), problem: None }));
        let fx = x + 200;
        let mut capture = Widget::new(fx, 90, 220, 34, None);
        let mut hint = label(fx + 2, 126, w - fx - x, 20, "Click to change", 11, false);
        hint.set_label_color(p.muted);
        capture.draw({
            let state = state.clone();
            move |c| draw_capture(c, &state.borrow(), p)
        });
        capture.handle({
            let state = state.clone();
            move |c, ev| {
                let handled = handle_capture(c, &state, ev);
                if handled {
                    // During Focus/Unfocus, FLTK's focus widget is not updated yet.
                    let focused = match ev {
                        Event::Focus => true,
                        Event::Unfocus => false,
                        _ => is_focused(c),
                    };
                    hint.set_label(hint_text(&state.borrow(), focused));
                    c.redraw();
                    hint.redraw_label();
                }
                handled
            }
        });
        let case = check(x - 2, 138, 300, "Case-sensitive search", p);

        let file = finder_core::paths::settings_file().display().to_string();
        #[cfg(unix)]
        let file = match file.strip_prefix(&*finder_core::paths::home().to_string_lossy()) {
            Some(rest) if rest.starts_with('/') => format!("~{rest}"),
            _ => file,
        };
        let mut f = label(x, H - 52, w - 2 * x - 120, 28, &file, 11, false);
        f.set_label_color(p.muted);
        let mut start = pill(w - x - 100, H - 52, 100, 30, "Start", p);
        start.set_shortcut(Shortcut::None | Key::Enter);
        group.end();
        Setup { group, start, capture, state, case }
    }

    pub fn focus(&mut self) {
        let _ = self.capture.take_focus();
    }

    pub fn settings(&self) -> Settings {
        Settings { hotkey: self.state.borrow().hotkey.clone(), case_sensitive: self.case.is_checked() }
    }
}

/// `ctrl+alt+space` → `Ctrl + Alt + Space`.
fn display(hotkey: &str) -> String {
    hotkey
        .split('+')
        .map(|p| {
            let mut c = p.chars();
            c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn draw_capture(c: &Widget, st: &Capture, p: Palette) {
    let (x, y, w, h) = (c.x(), c.y(), c.w(), c.h());
    let focused = is_focused(c);
    draw::draw_rect_fill(x, y, w, h, p.surface);
    draw::set_draw_color(if focused { p.accent } else { p.border });
    draw::draw_rounded_rectf(x, y, w, h, 7);
    draw::set_draw_color(p.surface);
    draw::draw_rounded_rectf(x + 2, y + 2, w - 4, h - 4, 5);
    let (text, color, font) = if st.hotkey.is_empty() {
        ("None".to_string(), p.muted, Font::Helvetica)
    } else {
        (display(&st.hotkey), p.text, Font::HelveticaBold)
    };
    draw::set_font(font, 14);
    draw::set_draw_color(color);
    draw::draw_text2(&text, x + 12, y, w - 24, h, Align::Left | Align::Inside);
}

fn is_focused(c: &Widget) -> bool {
    app::focus().is_some_and(|f| f.as_widget_ptr() == c.as_widget_ptr())
}

fn hint_text(st: &Capture, focused: bool) -> &'static str {
    match (st.problem, focused) {
        (Some(problem), _) => problem,
        (None, true) => "Press a key combination; Backspace for none",
        (None, false) => "Click to change",
    }
}

fn handle_capture(c: &mut Widget, st: &Rc<RefCell<Capture>>, ev: Event) -> bool {
    match ev {
        Event::Push => {
            let _ = c.take_focus();
            true
        }
        Event::Focus | Event::Unfocus => true,
        Event::KeyDown => {
            let key = app::event_key();
            let state = app::event_state();
            let mods: Vec<&str> = [
                (Shortcut::Ctrl, "ctrl"),
                (Shortcut::Alt, "alt"),
                (Shortcut::Shift, "shift"),
                (Shortcut::Meta, WIN_KEY),
            ]
            .into_iter()
            .filter(|(m, _)| state.contains(*m))
            .map(|(_, n)| n)
            .collect();
            // Plain Enter / Esc / Tab keep their usual meaning (start, close, next field).
            if mods.is_empty() && matches!(key, Key::Enter | Key::KPEnter | Key::Escape | Key::Tab) {
                return false;
            }
            let mut s = st.borrow_mut();
            if mods.is_empty() && matches!(key, Key::BackSpace | Key::Delete) {
                s.hotkey.clear();
                s.problem = None;
            } else if let Some(name) = key_name(key) {
                let combo = [mods.as_slice(), &[name.as_str()]].concat().join("+");
                match hotkey::normalize(&combo) {
                    Some(h) => {
                        s.hotkey = h;
                        s.problem = None;
                    }
                    None => s.problem = Some("Add Ctrl, Alt, Shift or Win (or use F1–F24)"),
                }
            } else if !is_modifier(key) {
                s.problem = Some("That key can't be used; try another");
            }
            true
        }
        _ => false,
    }
}

fn is_modifier(k: Key) -> bool {
    [
        Key::ShiftL,
        Key::ShiftR,
        Key::ControlL,
        Key::ControlR,
        Key::AltL,
        Key::AltR,
        Key::MetaL,
        Key::MetaR,
        Key::CapsLock,
    ]
    .contains(&k)
}

/// The hotkey name (see `finder_core::hotkey`) of an FLTK key.
fn key_name(k: Key) -> Option<String> {
    let b = k.bits();
    let f1 = Key::F1.bits();
    let name = match b {
        0x30..=0x39 | 0x61..=0x7a => (b as u8 as char).to_string(),
        0x41..=0x5a => (b as u8 as char).to_ascii_lowercase().to_string(),
        0x20 => "space".into(),
        0x60 | 0x2d | 0x3d | 0x2c | 0x2e | 0x2f | 0x3b => (b as u8 as char).to_string(),
        _ if (f1..f1 + 24).contains(&b) => format!("f{}", b - f1 + 1),
        _ => {
            let named = [
                (Key::Enter, "enter"),
                (Key::KPEnter, "enter"),
                (Key::Tab, "tab"),
                (Key::Escape, "esc"),
                (Key::BackSpace, "backspace"),
                (Key::Insert, "insert"),
                (Key::Delete, "delete"),
                (Key::Home, "home"),
                (Key::End, "end"),
                (Key::PageUp, "pageup"),
                (Key::PageDown, "pagedown"),
                (Key::Left, "left"),
                (Key::Up, "up"),
                (Key::Right, "right"),
                (Key::Down, "down"),
                (Key::Pause, "pause"),
            ];
            named.iter().find(|(key, _)| *key == k)?.1.into()
        }
    };
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_display() {
        assert_eq!(key_name(Key::from_char('a')).as_deref(), Some("a"));
        assert_eq!(key_name(Key::from_char(' ')).as_deref(), Some("space"));
        assert_eq!(key_name(Key::F12).as_deref(), Some("f12"));
        assert_eq!(key_name(Key::PageDown).as_deref(), Some("pagedown"));
        assert_eq!(key_name(Key::ShiftL), None);
        assert_eq!(display("ctrl+alt+space"), "Ctrl + Alt + Space");
        assert!(hotkey::normalize(SUGGESTED).is_some());
    }
}
