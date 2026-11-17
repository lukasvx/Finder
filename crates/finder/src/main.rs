//! `finder`: type a keyword, pick a match, the file manager opens there.

mod client;
mod open;

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use finder_core::paths::{self, SEP};
use finder_core::proto::{Hit, Query};
use finder_core::settings::Settings;
use fltk::browser::BrowserScrollbar;
use fltk::enums::{Align, CallbackTrigger, Color, Event, Font, Key};
use fltk::prelude::*;
use fltk::{app, browser::HoldBrowser, button::Button, draw, frame::Frame, input::Input, window::Window};

use crate::client::{Client, Msg};

/// Hits requested per query; the list scrolls through these. "Show All" lifts the limit.
const LIMIT: u32 = 1000;
const W: i32 = 780;
const H: i32 = 480;
const PAD: i32 = 8;
const NAME_COL: i32 = 280;
const STATUS_H: i32 = 26;
const MORE_W: i32 = 64;
/// Format char for browser lines; `@` (the default) can appear in file names.
const FMT: char = '\x01';

/// Browser line: bold name for folders, then the parent folder in grey.
fn row(hit: &Hit) -> String {
    let path = String::from_utf8_lossy(&hit.path);
    let (parent, name) = match path.rfind(SEP as char) {
        Some(i) if i + 1 < path.len() => (&path[..i.max(1)], &path[i + 1..]),
        _ => ("", &*path),
    };
    let clean = |s: &str| s.replace(['\t', FMT], " ");
    let bold = if hit.is_dir { format!("{FMT}b") } else { String::new() };
    let grey = Color::Dark3.bits();
    format!("{bold}{FMT}.{}\t{FMT}C{grey}{FMT}.{}", clean(name), clean(parent))
}

fn plural(n: u64) -> String {
    format!("{n} match{}", if n == 1 { "" } else { "es" })
}

/// Put the "Show All" button right after the status text, in a window of height `win_h`.
fn place_more(more: &mut Button, status: &Frame, win_h: i32) {
    draw::set_font(status.label_font(), status.label_size());
    let (text_w, _) = draw::measure(&status.label(), false);
    more.resize(PAD + text_w + 12, win_h - STATUS_H - PAD + 3, MORE_W, STATUS_H - 6);
}

/// Move the list selection by `delta` rows, keeping it visible.
fn move_selection(list: &mut HoldBrowser, delta: i32) {
    let n = list.size();
    if n == 0 {
        return;
    }
    let target = (list.value() + delta).clamp(1, n);
    list.select(target);
    if !list.displayed(target) {
        if delta > 0 { list.bottom_line(target) } else { list.top_line(target) }
    }
}

fn main() {
    let (settings, warnings) = Settings::load();
    let app = app::App::default().with_scheme(app::Scheme::Gtk);
    app::set_visible_focus(false);
    let (tx, rx) = app::channel::<Msg>();
    let client = Rc::new(Client::start(tx));

    let mut win = Window::default().with_size(W, H).with_label("Finder");
    win.set_xclass("finder");
    let mut input = Input::new(PAD, PAD, W - 2 * PAD, 34, None);
    input.set_text_size(17);
    input.set_trigger(CallbackTrigger::Changed);
    let mut list = HoldBrowser::new(PAD, 34 + 2 * PAD, W - 2 * PAD, H - 34 - STATUS_H - 4 * PAD, None);
    list.set_text_size(14);
    list.set_format_char(FMT);
    list.set_column_char('\t');
    list.set_column_widths(&[NAME_COL]);
    list.set_has_scrollbar(BrowserScrollbar::Vertical);
    list.set_selection_color(Color::from_rgb(0x3d, 0xae, 0xe9));
    let mut status = Frame::new(PAD, H - STATUS_H - PAD, W - 2 * PAD, STATUS_H, None);
    status.set_align(Align::Left | Align::Inside);
    status.set_label_size(12);
    status.set_label_font(Font::Helvetica);
    if let Some(w) = warnings.first() {
        status.set_label(&format!("{}: {w}", paths::settings_file().display()));
    }
    let mut more = Button::new(0, 0, MORE_W, STATUS_H - 6, "Show All");
    more.set_label_size(12);
    more.hide();
    win.resizable(&list);
    win.end();
    win.resize_callback({
        let (mut more, status) = (more.clone(), status.clone());
        move |_, _, _, _, h| place_more(&mut more, &status, h)
    });
    // Close when the window loses focus, so `finder` never lingers in the background.
    win.handle(|_, ev| {
        if ev == Event::Unfocus {
            app::quit();
        }
        false
    });
    win.show();
    let _ = input.take_focus();

    let hits: Rc<RefCell<Vec<Hit>>> = Rc::default();
    let seq = Rc::new(Cell::new(0u64));

    // Send the current keyword with `limit`; newer replies supersede older ones.
    let run_query = {
        let (client, seq, input) = (client.clone(), seq.clone(), input.clone());
        move |limit: u32| {
            seq.set(seq.get() + 1);
            client.search(
                seq.get(),
                Query {
                    text: input.value(),
                    case_sensitive: settings.case_sensitive,
                    full_path: settings.match_full_path,
                    limit,
                },
            );
        }
    };

    let open_selected = {
        let (hits, list, mut status) = (hits.clone(), list.clone(), status.clone());
        move || {
            let hits = hits.borrow();
            let i = if list.value() > 0 { list.value() } else { 1 };
            let Some(hit) = hits.get(i as usize - 1) else { return };
            client::notify_opened(&hit.path);
            match open::reveal(&paths::from_bytes(&hit.path), hit.is_dir) {
                Ok(()) => app::quit(),
                Err(e) => status.set_label(&format!("Cannot open file manager: {e}")),
            }
        }
    };

    input.set_callback({
        let (seq, mut list, mut status, hits, mut more) =
            (seq.clone(), list.clone(), status.clone(), hits.clone(), more.clone());
        let run_query = run_query.clone();
        move |i| {
            more.hide();
            if i.value().is_empty() {
                seq.set(seq.get() + 1);
                hits.borrow_mut().clear();
                list.clear();
                status.set_label("");
                return;
            }
            run_query(LIMIT);
        }
    });

    more.set_callback({
        let mut input = input.clone();
        move |b| {
            b.hide();
            run_query(u32::MAX);
            let _ = input.take_focus();
        }
    });

    input.handle({
        let (mut list, mut open) = (list.clone(), open_selected.clone());
        move |_, ev| {
            if ev != Event::KeyDown {
                return false;
            }
            let page = (list.h() / 18).max(1);
            match app::event_key() {
                Key::Down => move_selection(&mut list, 1),
                Key::Up => move_selection(&mut list, -1),
                Key::PageDown => move_selection(&mut list, page),
                Key::PageUp => move_selection(&mut list, -page),
                Key::Enter | Key::KPEnter => open(),
                _ => return false,
            }
            true
        }
    });

    list.set_callback({
        let mut open = open_selected.clone();
        move |_| {
            let double_click = app::event() == Event::Released && app::event_clicks();
            let enter = app::event() == Event::KeyDown && matches!(app::event_key(), Key::Enter | Key::KPEnter);
            if double_click || enter {
                open();
            }
        }
    });

    // `finder <keyword>` starts with the keyword filled in.
    let initial: Vec<String> = std::env::args().skip(1).collect();
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
                list.clear();
                for h in &new {
                    list.add(&row(h));
                }
                if !new.is_empty() {
                    list.select(1);
                }
                let mut text = plural(total);
                let truncated = total > new.len() as u64;
                if truncated {
                    text += &format!(", showing best {}", new.len());
                }
                if indexing {
                    text = format!("Indexing… {text} so far");
                }
                status.set_label(&text);
                if truncated {
                    place_more(&mut more, &status, win.h());
                    more.show();
                } else {
                    more.hide();
                }
                *hits.borrow_mut() = new;
            }
            Msg::Status(text) => {
                status.set_label(&text);
                more.hide();
            }
        }
    }
}
