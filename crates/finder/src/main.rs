//! `finder`: type a keyword, pick a match, the file manager opens there.
//!
//! A search bar at the top edge of the screen (like KRunner) that grows downward as results come
//! in. Light or dark like the desktop; nothing to configure here besides the first-run setup.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod activate;
mod client;
mod icons;
mod open;
mod results;
mod setup;
mod single;
mod theme;
#[cfg(feature = "uitest")]
mod uitest;
mod widgets;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use finder_core::proto::{Hit, Query};
use finder_core::settings::Settings;
use finder_core::{autostart, paths};
use fltk::enums::{Align, CallbackTrigger, Event, Font, FrameType, Key, Shortcut};
use fltk::frame::Frame;
use fltk::input::Input;
use fltk::prelude::*;
use fltk::{app, draw, window::Window};

use crate::client::{Client, Msg};
use crate::icons::Icons;
use crate::results::{ROW_H, ResultList};
use crate::setup::Setup;
use crate::theme::Palette;
use crate::widgets::{fill, label, search_icon};

/// Hits requested per query; the list scrolls through these.
const LIMIT: u32 = 1000;
const W: i32 = 680;
const BAR_H: i32 = 58;
/// Line for errors and settings warnings, under the bar.
const MSG_H: i32 = 26;
/// Rows shown before the list scrolls.
const MAX_ROWS: i32 = 8;
const LIST_PAD: i32 = 4;
const PLACEHOLDER: &str = "Search files and folders";

/// The parts that move when the window grows or shrinks.
#[derive(Clone)]
struct Layout {
    win: Window,
    bg: Frame,
    msg: Frame,
    line: Frame,
    list: ResultList,
}

impl Layout {
    /// Size the window for the message line (if any) and the result rows; the top edge stays put.
    fn update(&mut self) {
        let mut y = BAR_H;
        if self.msg.label().is_empty() {
            self.msg.hide();
        } else {
            self.msg.resize(18, y - 6, W - 36, MSG_H);
            self.msg.show();
            y += MSG_H - 6;
        }
        let rows = (self.list.len() as i32).min(MAX_ROWS);
        if rows > 0 {
            self.line.resize(1, y, W - 2, 1);
            self.list.widget().resize(1, y + 1 + LIST_PAD, W - 2, rows * ROW_H);
            self.line.show();
            self.list.widget().show();
            y += 1 + 2 * LIST_PAD + rows * ROW_H;
        } else {
            self.line.hide();
            self.list.widget().hide();
        }
        let h = y + 1;
        self.bg.resize(1, 0, W - 2, h - 1);
        if self.win.h() != h {
            let (x, top) = (self.win.x(), self.win.y());
            self.win.resize(x, top, W, h);
        }
        self.win.redraw();
    }
}

/// Put the window on the screen the mouse is on, horizontally centered: at the top edge (below a
/// top panel, if any), or for the first-run setup in the middle.
fn place(win: &mut Window, middle: bool) {
    let (mx, my) = app::get_mouse();
    let (sx, sy, sw, sh) = app::screen_work_area(app::screen_num(mx, my));
    let y = if middle { sy + (sh - win.h()) / 2 } else { sy };
    win.set_pos(sx + (sw - W) / 2, y);
}

/// Opens a hit; the flag says it is a program to run.
type OpenFn = dyn Fn(&Hit, bool);

/// Colors for icons that follow the color scheme (Linux icon themes).
fn icon_colors(p: &Palette) -> icons::Colors {
    #[cfg(target_os = "linux")]
    {
        let hex = |c: fltk::enums::Color| {
            let (r, g, b) = c.to_rgb();
            format!("#{r:02x}{g:02x}{b:02x}")
        };
        icons::Colors {
            text: hex(p.text),
            background: hex(p.surface),
            highlight: hex(p.accent),
            highlighted_text: hex(p.on_accent),
        }
    }
    #[cfg(windows)]
    {
        let _ = p;
        icons::Colors
    }
}

fn plural(n: u64) -> String {
    format!("{n} match{}", if n == 1 { "" } else { "es" })
}

fn main() {
    // Wayland does not let a window place itself; X11 (XWayland) does.
    #[cfg(target_os = "linux")]
    if std::env::var_os("FLTK_BACKEND").is_none() {
        // SAFETY: no other threads yet.
        unsafe { std::env::set_var("FLTK_BACKEND", "x11") };
    }
    // `finder <keyword>` starts with the keyword filled in. Without one, starting `finder` while
    // a window is open just closes it (the hotkey toggles).
    let keyword = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    if single::close_open() && keyword.is_empty() {
        return;
    }
    let loaded = Settings::load();
    let first_run = loaded.is_none();
    let (settings, warnings) = loaded.unwrap_or_default();
    let settings = Rc::new(RefCell::new(settings));

    let app = app::App::default().with_scheme(app::Scheme::Gtk);
    app::keyboard_screen_scaling(false);
    app::set_visible_focus(false);
    let p = Palette::load();
    p.apply_globals();
    let (tx, rx) = app::channel::<Msg>();
    single::listen(tx);
    let client = Rc::new(Client::start(tx));
    let icons = Rc::new(RefCell::new(Icons::new(tx, icon_colors(&p))));

    let mut win = Window::default().with_size(W, if first_run { setup::H } else { BAR_H + 1 }).with_label("Finder");
    win.set_xclass("finder");
    win.set_border(false);
    win.make_resizable(false);
    // The window shows only as the 1-px border around the contents (none at the top edge).
    win.set_color(p.border);
    let bg = fill(1, 0, W - 2, win.h() - 1, p.surface);

    // Search bar: icon, field, match count.
    let mut bar = fltk::group::Group::new(1, 0, W - 2, BAR_H, None);
    search_icon(18, (BAR_H - 24) / 2, p);
    let count_w = 150;
    let mut input = Input::new(50, (BAR_H - 36) / 2, W - 50 - count_w - 24, 36, None);
    input.set_frame(FrameType::FlatBox);
    input.set_color(p.surface);
    input.set_text_color(p.text);
    input.set_cursor_color(p.accent);
    input.set_selection_color(p.match_bg);
    input.set_text_size(19);
    input.set_trigger(CallbackTrigger::Changed);
    input.draw(move |i| {
        if i.value().is_empty() {
            draw::set_font(Font::Helvetica, 19);
            draw::set_draw_color(p.muted);
            draw::draw_text2(PLACEHOLDER, i.x() + 3, i.y(), i.w(), i.h(), Align::Left | Align::Inside);
        }
    });
    let mut count = label(W - count_w - 18, 0, count_w, BAR_H, "", 12, false);
    count.set_align(Align::Right | Align::Inside | Align::Clip);
    count.set_label_color(p.muted);
    bar.end();

    let mut msg = label(18, BAR_H, W - 36, MSG_H, "", 12, false);
    msg.set_label_color(p.muted);
    msg.hide();
    let mut line = fill(1, BAR_H, W - 2, 1, p.border);
    line.hide();
    let open_slot: Rc<RefCell<Option<Rc<OpenFn>>>> = Rc::default();
    let mut list = ResultList::new((1, BAR_H + 1, W - 2, ROW_H), p, icons.clone(), {
        let open_slot = open_slot.clone();
        move |hit, program| {
            let f = open_slot.borrow().clone();
            if let Some(f) = f {
                f(hit, program);
            }
        }
    });
    list.widget().hide();
    let mut setup = first_run.then(|| Setup::new(W, p));
    if first_run {
        bar.hide();
    }
    win.end();

    let layout = Layout { win: win.clone(), bg, msg: msg.clone(), line, list: list.clone() };
    // Settings warnings stay; errors from finderd replace each other.
    let warning = warnings.join("; ");
    let error: Rc<RefCell<String>> = Rc::default();
    let show_msg = {
        let (msg, error, layout) = (msg.clone(), error.clone(), layout.clone());
        move || {
            let (mut msg, mut layout) = (msg.clone(), layout.clone());
            let e = error.borrow();
            let text = [warning.as_str(), e.as_str()].iter().filter(|t| !t.is_empty()).copied().collect::<Vec<_>>();
            msg.set_label(&text.join(" · "));
            layout.update();
        }
    };

    // Close when the window loses focus, so `finder` never lingers in the background; but not
    // while the first-run setup is open.
    let in_setup = Rc::new(Cell::new(first_run));
    win.handle({
        let in_setup = in_setup.clone();
        move |_, ev| {
            // FLTK also sends this to the window when focus just moves between its widgets.
            if ev == Event::Unfocus && app::focus().is_none() && !in_setup.get() {
                #[cfg(feature = "uitest")]
                if uitest::active() {
                    eprintln!("uitest: window would close (focus lost)");
                    return false;
                }
                app::quit();
            }
            false
        }
    });
    // Esc closes (the window callback runs on Esc and on a close request).
    win.set_callback(|_| app::quit());

    let seq = Rc::new(Cell::new(0u64));
    let run_query = {
        let (client, seq, input, settings) = (client.clone(), seq.clone(), input.clone(), settings.clone());
        let (mut list, mut count, mut layout) = (list.clone(), count.clone(), layout.clone());
        move || {
            seq.set(seq.get() + 1);
            if input.value().is_empty() {
                list.clear();
                count.set_label("");
                layout.update();
                return;
            }
            let q = Query { text: input.value(), case_sensitive: settings.borrow().case_sensitive, limit: LIMIT };
            client.search(seq.get(), q);
        }
    };
    input.set_callback({
        let mut run_query = run_query.clone();
        move |_| run_query()
    });

    // Run a program hit, or (other hits, or Ctrl held) show it in the file manager.
    *open_slot.borrow_mut() = Some(Rc::new({
        let (error, show_msg) = (error.clone(), show_msg.clone());
        move |hit: &Hit, program: bool| {
            client::notify_opened(&hit.path);
            let path = paths::from_bytes(&hit.path);
            let r = if program && !app::event_state().contains(Shortcut::Ctrl) {
                open::run(&path).map_err(|e| format!("Cannot run {}: {e}", path.display()))
            } else {
                open::reveal(&path, hit.is_dir).map_err(|e| format!("Cannot open the file manager: {e}"))
            };
            match r {
                Ok(()) => app::quit(),
                Err(e) => {
                    *error.borrow_mut() = e;
                    show_msg();
                }
            }
        }
    }));

    input.handle({
        let (mut list, open_slot) = (list.clone(), open_slot.clone());
        move |_, ev| {
            if ev != Event::KeyDown {
                return false;
            }
            match app::event_key() {
                Key::Down => list.move_selection(1),
                Key::Up => list.move_selection(-1),
                Key::PageDown => list.move_selection(MAX_ROWS),
                Key::PageUp => list.move_selection(-MAX_ROWS),
                Key::Enter | Key::KPEnter => {
                    let open = open_slot.borrow().clone();
                    if let (Some((hit, program)), Some(open)) = (list.selected_hit(), open) {
                        open(&hit, program);
                    }
                }
                _ => return false,
            }
            true
        }
    });

    // First run: save the choices, start finderd at login, then switch to the search bar.
    if let Some(s) = &setup {
        s.start.clone().set_callback({
            let (settings, in_setup, client, error) =
                (settings.clone(), in_setup.clone(), client.clone(), error.clone());
            let (setup, mut bar, mut input, show_msg) = (s.clone(), bar.clone(), input.clone(), show_msg.clone());
            let mut win = win.clone();
            move |_| {
                let new = setup.settings();
                let mut problems = Vec::new();
                if let Err(e) = new.save() {
                    problems.push(format!("Cannot save {}: {e}", paths::settings_file().display()));
                }
                if let Err(e) = autostart::enable() {
                    problems.push(format!("Cannot set up autostart: {e}"));
                }
                *settings.borrow_mut() = new;
                in_setup.set(false);
                setup.group.clone().hide();
                bar.show();
                let _ = input.take_focus();
                // Start finderd now: indexing begins and the hotkey gets registered.
                client.connect();
                *error.borrow_mut() = problems.join(" · ");
                show_msg();
                // From the middle of the screen to the search bar's place at the top.
                place(&mut win, false);
            }
        });
    }

    place(&mut win, first_run);
    win.show();
    match &mut setup {
        Some(s) => s.focus(),
        None => {
            let _ = input.take_focus();
            client.connect();
        }
    }
    #[cfg(windows)]
    activate::bring_to_front(&win);
    #[cfg(feature = "uitest")]
    uitest::run(&win);
    if !first_run {
        show_msg();
        if !keyword.is_empty() {
            input.set_value(&keyword);
            input.do_callback();
        }
    }

    while app.wait() {
        let Some(m) = rx.recv() else { continue };
        match m {
            Msg::Results { seq: s, total, indexing, hits } => {
                if s != seq.get() || input.value().is_empty() {
                    continue;
                }
                list.set_hits(hits, &input.value(), settings.borrow().case_sensitive);
                let mut text = plural(total);
                if indexing {
                    text = format!("Indexing… {text}");
                }
                count.set_label(&text);
                if !error.borrow().is_empty() {
                    error.borrow_mut().clear();
                }
                show_msg();
            }
            Msg::Status(text) => {
                *error.borrow_mut() = text;
                show_msg();
            }
            Msg::Quit => app::quit(),
            Msg::Icon { path, found } => {
                icons.borrow_mut().loaded(path, found);
                list.widget().redraw();
            }
        }
    }
}
