//! `finder`: type a keyword, pick a match, the file manager opens there.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod client;
mod open;
mod results;
#[cfg(windows)]
mod activate;
mod drag;
mod single;
mod theme;
#[cfg(feature = "uitest")]
mod uitest;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use finder_core::{hotkey, paths};
use finder_core::proto::{Hit, Query};
use finder_core::settings::{Settings, Theme, split_list};
use fltk::button::{Button, CheckButton, RadioRoundButton, ToggleButton};
use fltk::enums::{Align, CallbackTrigger, Event, Font, FrameType, Key, Shortcut};
use fltk::frame::Frame;
use fltk::group::Group;
use fltk::input::{Input, MultilineInput};
use fltk::prelude::*;
use fltk::{app, draw, window::Window};

use crate::client::{Client, Msg};
use crate::results::{MatchMode, ResultList};
use crate::theme::Palette;

/// Hits requested per query; the list scrolls through these. "Show all" lifts the limit.
const LIMIT: u32 = 1000;
const W: i32 = 760;
const H: i32 = 500;
const HEADER_H: i32 = 60;
const FOOTER_H: i32 = 34;
const BODY_Y: i32 = HEADER_H + 1;
const BODY_H: i32 = H - HEADER_H - FOOTER_H - 2;
const PLACEHOLDER: &str = "Search files and folders";
/// Height of the settings panel's hotkey row; the hotkey is registered by `finderd` on Windows only.
const HOTKEY_ROW: i32 = if cfg!(windows) { 40 } else { 0 };
const HOTKEY_HINT: &str = "Opens Finder from anywhere, e.g. ctrl+alt+space, win+shift+f, f12. Empty: off.";

type Pal = Rc<Cell<Palette>>;
/// A callback bound after the widgets it needs exist.
type Slot<F> = Rc<RefCell<Option<Rc<F>>>>;

fn plural(n: u64) -> String {
    format!("{n} match{}", if n == 1 { "" } else { "es" })
}

/// A small toggle "chip" (`Aa`, `Path`, `Hidden`): filled with the accent color when on.
fn chip(x: i32, w: i32, label: &'static str, pal: &Pal) -> ToggleButton {
    let mut b = ToggleButton::new(x, 17, w, 26, None);
    b.clear_visible_focus();
    b.super_draw(false);
    let pal = pal.clone();
    b.draw(move |b| {
        let p = pal.get();
        let (x, y, w, h) = (b.x(), b.y(), b.w(), b.h());
        // Repaint the whole area: the "off" look must cover the previous "on" fill.
        draw::draw_rect_fill(x, y, w, h, p.surface);
        let fg = if b.value() {
            draw::set_draw_color(p.accent);
            draw::draw_rounded_rectf(x, y, w, h, 6);
            p.on_accent
        } else {
            draw::set_draw_color(p.border);
            draw::draw_rounded_rect(x, y, w, h, 6);
            p.muted
        };
        draw::set_font(Font::HelveticaBold, 12);
        draw::set_draw_color(fg);
        draw::draw_text2(label, x, y, w, h, Align::Center);
    });
    b
}

/// Settings button: a "sliders" icon.
fn settings_button(x: i32, pal: &Pal) -> ToggleButton {
    let mut b = ToggleButton::new(x, 15, 30, 30, None);
    b.clear_visible_focus();
    b.super_draw(false);
    let pal = pal.clone();
    b.draw(move |b| {
        let p = pal.get();
        let (x, y) = (b.x(), b.y());
        draw::draw_rect_fill(x, y, b.w(), b.h(), p.surface);
        let fg = if b.value() {
            draw::set_draw_color(p.accent);
            draw::draw_rounded_rectf(x, y, 30, 30, 7);
            p.on_accent
        } else {
            p.muted
        };
        let bg = if b.value() { p.accent } else { p.surface };
        draw::set_draw_color(fg);
        draw::set_line_style(draw::LineStyle::Solid, 2);
        for (row, knob) in [(9, 18), (15, 10), (21, 15)] {
            draw::draw_line(x + 7, y + row, x + 23, y + row);
            draw::set_draw_color(bg);
            draw::draw_circle_fill(x + knob - 3, y + row - 3, 7, bg);
            draw::set_draw_color(fg);
            draw::draw_circle(f64::from(x + knob), f64::from(y + row), 3.0);
        }
        draw::set_line_style(draw::LineStyle::Solid, 0);
    });
    b
}

/// Magnifier icon left of the search field.
fn search_icon(pal: &Pal) -> Frame {
    let mut f = Frame::new(16, 18, 24, 24, None);
    f.super_draw(false);
    let pal = pal.clone();
    f.draw(move |f| {
        let p = pal.get();
        let (x, y) = (f.x(), f.y());
        draw::set_draw_color(p.muted);
        draw::set_line_style(draw::LineStyle::Solid, 2);
        draw::draw_circle(f64::from(x + 10), f64::from(y + 10), 7.0);
        draw::draw_line(x + 15, y + 15, x + 21, y + 21);
        draw::set_line_style(draw::LineStyle::Solid, 0);
    });
    f
}

/// A flat filled strip (backgrounds, separators).
fn strip(x: i32, y: i32, w: i32, h: i32) -> Frame {
    let mut f = Frame::new(x, y, w, h, None);
    f.set_frame(FrameType::FlatBox);
    f
}

fn label(x: i32, y: i32, w: i32, h: i32, text: &str, size: i32, bold: bool) -> Frame {
    let mut f = Frame::new(x, y, w, h, None);
    f.set_label(text);
    f.set_label_size(size);
    f.set_label_font(if bold { Font::HelveticaBold } else { Font::Helvetica });
    f.set_align(Align::Left | Align::Inside | Align::Clip);
    f
}

/// A button drawn as a pill: filled (primary) or outlined.
fn pill(x: i32, y: i32, w: i32, h: i32, text: &'static str, primary: bool, pal: &Pal) -> Button {
    let mut b = Button::new(x, y, w, h, None);
    b.clear_visible_focus();
    b.super_draw(false);
    let pal = pal.clone();
    b.draw(move |b| {
        let p = pal.get();
        let (x, y, w, h) = (b.x(), b.y(), b.w(), b.h());
        draw::draw_rect_fill(x, y, w, h, p.bg);
        let fg = if primary {
            draw::set_draw_color(p.accent);
            draw::draw_rounded_rectf(x, y, w, h, h / 2);
            p.on_accent
        } else {
            draw::set_draw_color(p.accent);
            draw::draw_rounded_rect(x, y, w, h, h / 2);
            p.accent
        };
        draw::set_font(Font::HelveticaBold, 12);
        draw::set_draw_color(fg);
        draw::draw_text2(text, x, y, w, h, Align::Center);
    });
    b
}

/// Draw a check box (or radio button, `round`) in the theme's colors, with its label after it.
fn toggle_look<B: ButtonExt + WidgetBase + 'static>(b: &mut B, round: bool, pal: &Pal) {
    b.super_draw(false);
    let pal = pal.clone();
    b.draw(move |b| {
        let p = pal.get();
        let (x, y, w, h) = (b.x(), b.y(), b.w(), b.h());
        let (s, on) = (18, b.value());
        let (bx, by) = (x + 4, y + (h - s) / 2);
        draw::draw_rect_fill(x, y, w, h, p.bg);
        if round {
            draw::draw_circle_fill(bx, by, s, if on { p.accent } else { p.border });
            if on {
                draw::draw_circle_fill(bx + 5, by + 5, s - 10, p.on_accent);
            } else {
                draw::draw_circle_fill(bx + 2, by + 2, s - 4, p.surface);
            }
        } else if on {
            draw::set_draw_color(p.accent);
            draw::draw_rounded_rectf(bx, by, s, s, 4);
            draw::set_draw_color(p.on_accent);
            draw::set_line_style(draw::LineStyle::Solid, 2);
            draw::draw_line(bx + 4, by + 9, bx + 8, by + 13);
            draw::draw_line(bx + 8, by + 13, bx + 14, by + 5);
            draw::set_line_style(draw::LineStyle::Solid, 0);
        } else {
            draw::set_draw_color(p.border);
            draw::draw_rounded_rectf(bx, by, s, s, 4);
            draw::set_draw_color(p.surface);
            draw::draw_rounded_rectf(bx + 2, by + 2, s - 4, s - 4, 3);
        }
        draw::set_font(Font::Helvetica, b.label_size());
        draw::set_draw_color(p.text);
        draw::draw_text2(&b.label(), x + 30, y, w - 30, h, Align::Left | Align::Inside);
    });
}

/// The settings panel, shown in place of the result list.
struct Panel {
    group: Group,
    case: CheckButton,
    path: CheckButton,
    hidden: CheckButton,
    themes: [RadioRoundButton; 3],
    exclude: MultilineInput,
    exclude_border: Frame,
    hotkey: Input,
    hotkey_border: Frame,
    hotkey_hint: Frame,
    texts: Vec<Frame>,
    hints: Vec<Frame>,
    done: Button,
}

impl Panel {
    fn new(pal: &Pal) -> Panel {
        let mut group = Group::new(1, BODY_Y, W - 2, BODY_H, None);
        group.set_frame(FrameType::FlatBox);
        let (x, y) = (28, BODY_Y);
        let mut texts = vec![label(x, y + 14, 300, 24, "Settings", 16, true)];
        let check = |dy: i32, text: &str| {
            let mut c = CheckButton::new(x - 4, y + dy, 420, 26, None);
            c.set_label(text);
            c.set_label_size(14);
            c.clear_visible_focus();
            toggle_look(&mut c, false, pal);
            c
        };
        let case = check(50, "Case-sensitive matching");
        let path = check(80, "Match the full path, not just the name");
        let hidden = check(110, "Show hidden files and folders");
        let mut hints = Vec::new();
        for (dy, key) in [(50, "Alt+C"), (80, "Alt+P"), (110, "Alt+H")] {
            let mut f = label(W - 120, y + dy, 90, 26, key, 12, false);
            f.set_align(Align::Right | Align::Inside);
            hints.push(f);
        }
        texts.push(label(x, y + 150, 80, 26, "Theme", 14, false));
        let theme = |i: i32, text: &str| {
            let mut r = RadioRoundButton::new(x + 80 + i * 90, y + 150, 86, 26, None);
            r.set_label(text);
            r.set_label_size(14);
            r.clear_visible_focus();
            toggle_look(&mut r, true, pal);
            r
        };
        let themes = [theme(0, "Auto"), theme(1, "Light"), theme(2, "Dark")];
        texts.push(label(x, y + 190, 80, 26, "Hotkey", 14, false));
        let mut hotkey_border = Frame::new(x + 79, y + 189, 182, 28, None);
        hotkey_border.set_frame(FrameType::BorderFrame);
        let mut hotkey = Input::new(x + 80, y + 190, 180, 26, None);
        hotkey.set_frame(FrameType::FlatBox);
        hotkey.set_text_size(13);
        hotkey.set_trigger(CallbackTrigger::Changed);
        let mut hotkey_hint = label(x + 272, y + 190, W - 2 * x - 272, 26, HOTKEY_HINT, 11, false);
        hotkey_hint.set_align(Align::Left | Align::Inside | Align::Wrap);
        if HOTKEY_ROW == 0 {
            texts.last_mut().unwrap().hide();
            hotkey_border.hide();
            hotkey.hide();
            hotkey_hint.hide();
        }
        let y = y + HOTKEY_ROW;
        texts.push(label(x, y + 190, 300, 22, "Excluded folders", 14, true));
        hints.push(label(
            x,
            y + 210,
            W - 2 * x,
            20,
            "One per line: a folder name such as node_modules, or a path such as ~/Downloads/old.",
            12,
            false,
        ));
        let ex_h = BODY_H - HOTKEY_ROW - 236 - 56;
        let mut exclude_border = Frame::new(x - 1, y + 235, W - 2 * x + 2, ex_h + 2, None);
        exclude_border.set_frame(FrameType::BorderFrame);
        let mut exclude = MultilineInput::new(x, y + 236, W - 2 * x, ex_h, None);
        exclude.set_frame(FrameType::FlatBox);
        exclude.set_text_size(13);
        let file = paths::settings_file();
        let y = BODY_Y;
        hints.push(label(x, y + BODY_H - 42, W - 200, 28, &file.display().to_string(), 11, false));
        let done = pill(W - x - 90, y + BODY_H - 42, 90, 28, "Done", true, pal);
        group.end();
        group.hide();
        hotkey.set_callback({
            let mut hint = hotkey_hint.clone();
            move |i| {
                let valid = hotkey::normalize(&i.value()).is_some();
                hint.set_label(if valid { HOTKEY_HINT } else { "Not a valid hotkey. Use e.g. ctrl+alt+space." });
                if let Some(mut g) = hint.parent() {
                    g.redraw();
                }
            }
        });
        Panel {
            group,
            case,
            path,
            hidden,
            themes,
            exclude,
            exclude_border,
            hotkey,
            hotkey_border,
            hotkey_hint,
            texts,
            hints,
            done,
        }
    }

    /// Copy the text fields (excluded folders, valid hotkey) into `s`; true if anything changed.
    fn store(&self, s: &mut Settings) -> bool {
        let mut changed = false;
        let exclude = split_list(&self.exclude.value());
        if s.exclude != exclude {
            s.exclude = exclude;
            changed = true;
        }
        if let Some(h) = hotkey::normalize(&self.hotkey.value()).filter(|h| *h != s.hotkey) {
            s.hotkey = h;
            changed = true;
        }
        changed
    }

    fn load(&mut self, s: &Settings) {
        self.case.set_checked(s.case_sensitive);
        self.path.set_checked(s.match_full_path);
        self.hidden.set_checked(s.show_hidden);
        for (b, t) in self.themes.iter_mut().zip([Theme::Auto, Theme::Light, Theme::Dark]) {
            b.set_value(s.theme == t);
        }
        self.exclude.set_value(&s.exclude.join("\n"));
        self.hotkey.set_value(&s.hotkey);
        self.hotkey.do_callback();
    }

    fn style(&mut self, p: &Palette) {
        self.group.set_color(p.bg);
        for f in &mut self.texts {
            f.set_label_color(p.text);
        }
        for f in &mut self.hints {
            f.set_label_color(p.muted);
        }
        for c in [&mut self.case, &mut self.path, &mut self.hidden] {
            c.set_label_color(p.text);
            c.set_color(p.surface);
            c.set_selection_color(p.accent);
        }
        for r in &mut self.themes {
            r.set_label_color(p.text);
            r.set_color(p.surface);
            r.set_selection_color(p.accent);
        }
        self.exclude.set_color(p.surface);
        self.exclude.set_text_color(p.text);
        self.exclude.set_cursor_color(p.accent);
        self.exclude.set_selection_color(p.match_bg);
        self.exclude_border.set_color(p.border);
        self.hotkey.set_color(p.surface);
        self.hotkey.set_text_color(p.text);
        self.hotkey.set_cursor_color(p.accent);
        self.hotkey.set_selection_color(p.match_bg);
        self.hotkey_border.set_color(p.border);
        self.hotkey_hint.set_label_color(p.muted);
    }
}

/// Everything that is re-colored when the theme changes.
struct Chrome {
    win: Window,
    header: Frame,
    lines: [Frame; 2],
    footer: Frame,
    input: Input,
    status: Frame,
    hints: Frame,
}

impl Chrome {
    fn style(&mut self, p: &Palette) {
        // The window shows only as the 1-px border around the inset contents.
        self.win.set_color(p.border);
        self.header.set_color(p.surface);
        for l in &mut self.lines {
            l.set_color(p.border);
        }
        self.footer.set_color(p.bg);
        self.input.set_color(p.surface);
        self.input.set_text_color(p.text);
        self.input.set_cursor_color(p.accent);
        self.input.set_selection_color(p.match_bg);
        self.status.set_label_color(p.muted);
        self.hints.set_label_color(p.muted);
    }
}

fn main() {
    let (settings, warnings) = Settings::load();
    let settings = Rc::new(RefCell::new(settings));
    let app = app::App::default().with_scheme(app::Scheme::Gtk);
    app::set_visible_focus(false);
    let pal: Pal = Rc::new(Cell::new(Palette::load(settings.borrow().theme)));
    pal.get().apply_globals();
    let (tx, rx) = app::channel::<Msg>();
    single::take_over(tx.clone());
    let client = Rc::new(Client::start(tx));

    let mut win = Window::default().with_size(W, H).with_label("Finder").center_screen();
    win.set_xclass("finder");
    win.set_border(false);

    // Header: icon, search field, toggles, settings button.
    let header = strip(1, 1, W - 2, HEADER_H - 1);
    search_icon(&pal);
    let chips_x = W - 16 - 30 - 8 - (34 + 46 + 60 + 2 * 6);
    let mut input = Input::new(46, 13, chips_x - 46 - 12, 34, None);
    input.set_frame(FrameType::FlatBox);
    input.set_text_size(18);
    input.set_trigger(CallbackTrigger::Changed);
    input.draw({
        let pal = pal.clone();
        move |i| {
            if i.value().is_empty() {
                draw::set_font(Font::Helvetica, 18);
                draw::set_draw_color(pal.get().muted);
                draw::draw_text2(PLACEHOLDER, i.x() + 3, i.y(), i.w(), i.h(), Align::Left | Align::Inside);
            }
        }
    });
    let mut case_chip = chip(chips_x, 34, "Aa", &pal);
    let mut path_chip = chip(chips_x + 40, 46, "Path", &pal);
    let mut hidden_chip = chip(chips_x + 92, 60, "Hidden", &pal);
    let mut gear = settings_button(W - 16 - 30, &pal);
    let line_top = strip(1, HEADER_H, W - 2, 1);

    // Body: results, or the settings panel.
    let open_slot: Slot<dyn Fn(&Hit)> = Rc::default();
    let mut list = ResultList::new(1, BODY_Y, W - 2, BODY_H, pal.get(), {
        let open_slot = open_slot.clone();
        move |hit| {
            let f = open_slot.borrow().clone();
            if let Some(f) = f {
                f(hit);
            }
        }
    });
    let panel = Rc::new(RefCell::new(Panel::new(&pal)));

    // Footer: status, "Show all", key hints.
    let line_bottom = strip(1, H - FOOTER_H - 1, W - 2, 1);
    let footer = strip(1, H - FOOTER_H, W - 2, FOOTER_H - 1);
    let mut status = label(18, H - FOOTER_H, 420, FOOTER_H, "", 12, false);
    let mut more = pill(0, H - FOOTER_H + 6, 70, 22, "Show all", false, &pal);
    more.hide();
    let mut hints = label(W - 320, H - FOOTER_H, 302, FOOTER_H, "", 12, false);
    hints.set_align(Align::Right | Align::Inside);
    let mut set_hints = {
        let mut hints = hints.clone();
        move |panel_open: bool| {
            hints.set_label(if panel_open {
                "Esc back"
            } else {
                "↑↓ select   Enter open   Esc close   Ctrl+, settings"
            });
        }
    };
    set_hints(false);
    win.end();

    let mut chrome = Chrome {
        win: win.clone(),
        header,
        lines: [line_top, line_bottom],
        footer,
        input: input.clone(),
        status: status.clone(),
        hints: hints.clone(),
    };
    drag::make_handle(&mut chrome.header, &win);
    drag::make_handle(&mut chrome.footer, &win);
    chrome.style(&pal.get());
    panel.borrow_mut().style(&pal.get());
    {
        let s = settings.borrow();
        case_chip.set_value(s.case_sensitive);
        path_chip.set_value(s.match_full_path);
        hidden_chip.set_value(s.show_hidden);
    }
    if let Some(w) = warnings.first() {
        status.set_label(&format!("{}: {w}", paths::settings_file().display()));
    }

    // Close when the window loses focus, so `finder` never lingers in the background.
    win.handle({
        let (mut case_chip, mut path_chip, mut hidden_chip, mut gear) =
            (case_chip.clone(), path_chip.clone(), hidden_chip.clone(), gear.clone());
        move |_, ev| match ev {
            // FLTK also sends this to the window when focus just moves between its widgets
            // (e.g. into the excluded-folders box); then a widget still has the focus.
            Event::Unfocus if app::focus().is_none() => {
                #[cfg(feature = "uitest")]
                if uitest::active() {
                    eprintln!("uitest: window would close (focus lost)");
                    return false;
                }
                app::quit();
                false
            }
            Event::Shortcut => {
                let state = app::event_state();
                let key = app::event_key();
                let toggle = |b: &mut ToggleButton| {
                    b.set_value(!b.value());
                    b.do_callback();
                    true
                };
                if state.contains(Shortcut::Alt) {
                    match key {
                        k if k == Key::from_char('c') => toggle(&mut case_chip),
                        k if k == Key::from_char('p') => toggle(&mut path_chip),
                        k if k == Key::from_char('h') => toggle(&mut hidden_chip),
                        _ => false,
                    }
                } else if state.contains(Shortcut::Ctrl) && key == Key::from_char(',') {
                    toggle(&mut gear)
                } else {
                    false
                }
            }
            _ => false,
        }
    });

    let seq = Rc::new(Cell::new(0u64));
    let match_mode = {
        let settings = settings.clone();
        move || {
            let s = settings.borrow();
            MatchMode { case_sensitive: s.case_sensitive, full_path: s.match_full_path }
        }
    };

    // Send the current keyword with `limit`; newer replies supersede older ones.
    let run_query = {
        let (client, seq, input, settings) = (client.clone(), seq.clone(), input.clone(), settings.clone());
        let (mut list, mut status, mut more) = (list.clone(), status.clone(), more.clone());
        move |limit: u32| {
            seq.set(seq.get() + 1);
            if input.value().is_empty() {
                list.clear();
                status.set_label("");
                more.hide();
                return;
            }
            let s = settings.borrow();
            client.search(
                seq.get(),
                Query {
                    text: input.value(),
                    case_sensitive: s.case_sensitive,
                    full_path: s.match_full_path,
                    limit,
                    show_hidden: s.show_hidden,
                    exclude: s.exclude_bytes(),
                },
            );
        }
    };

    let open_hit = {
        let status = status.clone();
        move |hit: &Hit| {
            client::notify_opened(&hit.path);
            match open::reveal(&paths::from_bytes(&hit.path), hit.is_dir) {
                Ok(()) => app::quit(),
                Err(e) => status.clone().set_label(&format!("Cannot open file manager: {e}")),
            }
        }
    };
    *open_slot.borrow_mut() = Some(Rc::new(open_hit.clone()));

    // Persist a settings change and re-run the search with it.
    let save = {
        let (settings, mut status) = (settings.clone(), status.clone());
        move || {
            if let Err(e) = settings.borrow().save() {
                status.set_label(&format!("Cannot save {}: {e}", paths::settings_file().display()));
            }
        }
    };

    let apply_theme = {
        let (settings, pal, mut list, panel) = (settings.clone(), pal.clone(), list.clone(), panel.clone());
        let chrome = Rc::new(RefCell::new(chrome));
        move || {
            let p = Palette::load(settings.borrow().theme);
            pal.set(p);
            p.apply_globals();
            let mut chrome = chrome.borrow_mut();
            chrome.style(&p);
            panel.borrow_mut().style(&p);
            list.set_palette(p);
            chrome.win.redraw();
        }
    };

    // Toggle chips: flip the setting, keep the panel in sync, save, search again.
    let bind_chip = |chip: &mut ToggleButton, field: fn(&mut Settings) -> &mut bool, check: CheckButton| {
        let (settings, mut save, mut run_query, check) = (settings.clone(), save.clone(), run_query.clone(), check);
        chip.set_callback(move |b| {
            *field(&mut settings.borrow_mut()) = b.value();
            check.set_checked(b.value());
            save();
            run_query(LIMIT);
        });
    };
    let mut p = panel.borrow_mut();
    bind_chip(&mut case_chip, |s| &mut s.case_sensitive, p.case.clone());
    bind_chip(&mut path_chip, |s| &mut s.match_full_path, p.path.clone());
    bind_chip(&mut hidden_chip, |s| &mut s.show_hidden, p.hidden.clone());
    // Panel check boxes drive the same chips.
    let Panel { case, path, hidden, themes, .. } = &mut *p;
    for (check, chip) in [(case, case_chip.clone()), (path, path_chip.clone()), (hidden, hidden_chip.clone())] {
        let mut chip = chip;
        check.set_callback(move |c| {
            chip.set_value(c.is_checked());
            chip.do_callback();
            chip.redraw();
        });
    }
    for (button, theme) in themes.iter_mut().zip([Theme::Auto, Theme::Light, Theme::Dark]) {
        let (settings, mut save, mut apply_theme) = (settings.clone(), save.clone(), apply_theme.clone());
        button.set_callback(move |_| {
            settings.borrow_mut().theme = theme;
            save();
            apply_theme();
        });
    }
    drop(p);

    // Open/close the settings panel. Closing saves the text fields (excluded folders, hotkey).
    let show_panel = {
        let (panel, settings, mut list_w, mut input, mut gear) =
            (panel.clone(), settings.clone(), list.widget().clone(), input.clone(), gear.clone());
        let (mut save, mut run_query, mut more) = (save.clone(), run_query.clone(), more.clone());
        let mut set_hints = set_hints;
        move |open: bool| {
            let mut p = panel.borrow_mut();
            gear.set_value(open);
            gear.redraw();
            set_hints(open);
            if open {
                p.load(&settings.borrow());
                list_w.hide();
                more.hide();
                p.group.show();
            } else {
                p.group.hide();
                list_w.show();
                let _ = input.take_focus();
                let changed = p.store(&mut settings.borrow_mut());
                drop(p);
                if changed {
                    save();
                }
                run_query(LIMIT);
            }
        }
    };
    gear.set_callback({
        let mut show_panel = show_panel.clone();
        move |b| show_panel(b.value())
    });
    panel.borrow_mut().done.set_callback({
        let mut show_panel = show_panel.clone();
        move |_| show_panel(false)
    });
    // Esc closes the panel first, then the app.
    win.set_callback({
        let (panel, mut show_panel) = (panel.clone(), show_panel.clone());
        move |_| {
            let escape = app::event() == Event::Shortcut && app::event_key() == Key::Escape;
            if escape && panel.borrow().group.visible() {
                show_panel(false);
            } else {
                app::quit();
            }
        }
    });

    input.set_callback({
        let (mut run_query, mut more) = (run_query.clone(), more.clone());
        move |_| {
            more.hide();
            run_query(LIMIT);
        }
    });

    more.set_callback({
        let (mut input, mut run_query) = (input.clone(), run_query.clone());
        move |b| {
            b.hide();
            run_query(u32::MAX);
            let _ = input.take_focus();
        }
    });

    input.handle({
        let (mut list, panel) = (list.clone(), panel.clone());
        move |_, ev| {
            if ev != Event::KeyDown || panel.borrow().group.visible() {
                return false;
            }
            let page = list.page();
            match app::event_key() {
                Key::Down => list.move_selection(1),
                Key::Up => list.move_selection(-1),
                Key::PageDown => list.move_selection(page),
                Key::PageUp => list.move_selection(-page),
                Key::Enter | Key::KPEnter => {
                    if let Some(hit) = list.selected_hit() {
                        open_hit(&hit);
                    }
                }
                _ => return false,
            }
            true
        }
    });

    win.show();
    let _ = input.take_focus();
    #[cfg(windows)]
    activate::bring_to_front(&win);
    #[cfg(feature = "uitest")]
    uitest::run(&win);

    // `finder <keyword>` starts with the keyword filled in; `finder --settings` opens the settings.
    let mut initial: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = initial.iter().position(|a| a == "--settings") {
        initial.remove(i);
        gear.set_value(true);
        gear.do_callback();
    }
    if !initial.is_empty() {
        input.set_value(&initial.join(" "));
        input.do_callback();
    }

    while app.wait() {
        let Some(msg) = rx.recv() else { continue };
        match msg {
            Msg::Results { seq: s, total, indexing, hits: new } => {
                if s != seq.get() {
                    continue;
                }
                let shown = new.len() as u64;
                list.set_hits(new, &input.value(), &match_mode());
                let mut text = plural(total);
                let truncated = total > shown;
                if truncated {
                    text += &format!(", showing best {shown}");
                }
                if indexing {
                    text = format!("Indexing… {text} so far");
                }
                status.set_label(&text);
                if truncated && !panel.borrow().group.visible() {
                    draw::set_font(status.label_font(), status.label_size());
                    let (text_w, _) = draw::measure(&text, false);
                    more.resize(status.x() + text_w + 12, more.y(), more.w(), more.h());
                    more.show();
                } else {
                    more.hide();
                }
                win.redraw();
            }
            Msg::Status(text) => {
                status.set_label(&text);
                more.hide();
            }
            Msg::Quit => app::quit(),
        }
    }

    // Closed while the settings panel was open (focus loss, Esc, another `finder`): keep edits
    // to the text fields, which are otherwise saved when the panel closes.
    let p = panel.borrow();
    if p.group.visible() && p.store(&mut settings.borrow_mut()) {
        let _ = settings.borrow().save();
    }
}
