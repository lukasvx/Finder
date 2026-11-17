//! UI test mode (cargo feature `uitest`, never in normal builds).
//!
//! `FINDER_UITEST="x,y;x,y>x2,y2;…"` replays left clicks (or a drag from x,y to x2,y2) at window
//! coordinates through FLTK's normal event dispatch, one every `FINDER_UITEST_STEP` seconds
//! (default 1), and keeps the window open on focus loss (logging that it would have closed).
//! Clicks bypass the OS input stack, so no input-control permission is needed.

use std::os::raw::c_int;

use fltk::enums::Event;
use fltk::prelude::*;
use fltk::{app, window::Window};

// FLTK's current-event state (`Fl::e_x` etc.), which event handlers read.
unsafe extern "C" {
    #[link_name = "_ZN2Fl3e_xE"]
    static mut E_X: c_int;
    #[link_name = "_ZN2Fl3e_yE"]
    static mut E_Y: c_int;
    #[link_name = "_ZN2Fl8e_x_rootE"]
    static mut E_X_ROOT: c_int;
    #[link_name = "_ZN2Fl8e_y_rootE"]
    static mut E_Y_ROOT: c_int;
    #[link_name = "_ZN2Fl8e_clicksE"]
    static mut E_CLICKS: c_int;
    #[link_name = "_ZN2Fl10e_is_clickE"]
    static mut E_IS_CLICK: c_int;
    #[link_name = "_ZN2Fl7e_stateE"]
    static mut E_STATE: c_int;
    #[link_name = "_ZN2Fl8e_keysymE"]
    static mut E_KEYSYM: c_int;
}

/// `FL_Button + 1`: what `app::event_mouse_button()` reports as the left button.
const LEFT_BUTTON: c_int = 0xfee8 + 1;

/// Place the pointer at window coordinates (with the root position following the window).
fn at(win: &Window, x: i32, y: i32) {
    // SAFETY: plain ints read by FLTK on this (the UI) thread.
    unsafe {
        E_X = x;
        E_Y = y;
        E_X_ROOT = win.x_root() + x;
        E_Y_ROOT = win.y_root() + y;
        E_KEYSYM = LEFT_BUTTON;
    }
}

fn drag(win: &Window, (x, y): (i32, i32), (x2, y2): (i32, i32)) {
    let start = (win.x(), win.y());
    at(win, x, y);
    let _ = app::handle_main(Event::Push);
    // The root position is what moves; window coordinates stay relative to the original spot.
    unsafe {
        E_STATE = 0x0100_0000;
        E_X_ROOT = start.0 + x2;
        E_Y_ROOT = start.1 + y2;
    }
    let _ = app::handle_main(Event::Drag);
    unsafe { E_STATE = 0 };
    let _ = app::handle_main(Event::Released);
    // The window manager applies the move asynchronously.
    let win = win.clone();
    app::add_timeout3(0.3, move |_| {
        eprintln!("uitest: dragged {x},{y} -> {x2},{y2}; window {start:?} -> ({}, {})", win.x(), win.y());
    });
}

fn point(s: &str) -> Option<(i32, i32)> {
    let (x, y) = s.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

pub fn active() -> bool {
    std::env::var_os("FINDER_UITEST").is_some()
}

fn click(win: &Window, x: i32, y: i32) {
    at(win, x, y);
    // SAFETY: plain ints read by FLTK on this (the UI) thread.
    unsafe {
        E_CLICKS = 0;
        E_IS_CLICK = 1;
    }
    let _ = app::handle_main(Event::Move);
    let _ = app::handle_main(Event::Push);
    let _ = app::handle_main(Event::Released);
    eprintln!("uitest: clicked {x},{y}");
}

pub fn run(win: &Window) {
    let Ok(script) = std::env::var("FINDER_UITEST") else { return };
    let step: f64 = std::env::var("FINDER_UITEST_STEP").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    for (i, pt) in script.split(';').filter(|s| !s.is_empty()).enumerate() {
        let when = step * (i + 1) as f64;
        let win = win.clone();
        if let Some((a, b)) = pt.split_once('>').and_then(|(a, b)| Some((point(a)?, point(b)?))) {
            app::add_timeout3(when, move |_| drag(&win, a, b));
        } else if let Some((x, y)) = point(pt) {
            app::add_timeout3(when, move |_| click(&win, x, y));
        } else {
            eprintln!("uitest: bad step `{pt}`");
        }
    }
}
