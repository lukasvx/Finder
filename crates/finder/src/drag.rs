//! Moving the borderless window by dragging an empty part of the header or footer.
//!
//! X11 and Windows: the window follows the pointer. Wayland apps can't place their windows, but
//! FLTK hands a move during a left-button drag to the compositor (`xdg_toplevel_move`), which
//! then moves the window itself.

use fltk::enums::Event;
use fltk::prelude::*;
use fltk::{app, window::Window};
use std::cell::Cell;
use std::rc::Rc;

pub fn make_handle<W: WidgetBase + 'static>(w: &mut W, win: &Window) {
    let win = win.clone();
    // Pointer position relative to the window at the press.
    let grab = Rc::new(Cell::new((0, 0)));
    w.handle(move |_, ev| match ev {
        Event::Push if app::event_mouse_button() == app::MouseButton::Left => {
            grab.set((app::event_x_root() - win.x(), app::event_y_root() - win.y()));
            true
        }
        Event::Drag => {
            let (gx, gy) = grab.get();
            let (x, y) = (app::event_x_root() - gx, app::event_y_root() - gy);
            if (x, y) != (win.x(), win.y()) {
                move_window(&mut win.clone(), x, y);
            }
            true
        }
        Event::Released => true,
        _ => false,
    });
}

#[cfg(target_os = "linux")]
fn move_window(win: &mut Window, x: i32, y: i32) {
    // FLTK's Wayland driver starts the compositor move only if the button state is exactly
    // "button 1", which fails while Num Lock or another modifier is active.
    unsafe extern "C" {
        #[link_name = "_ZN2Fl7e_stateE"]
        static mut E_STATE: std::os::raw::c_int;
    }
    const FL_BUTTON1: std::os::raw::c_int = 0x0100_0000;
    // SAFETY: a plain int that FLTK reads and writes on this (the UI) thread.
    unsafe {
        let saved = E_STATE;
        E_STATE = FL_BUTTON1;
        win.set_pos(x, y);
        // Wayland clears the state once the compositor takes over; otherwise restore it.
        if E_STATE == FL_BUTTON1 {
            E_STATE = saved;
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn move_window(win: &mut Window, x: i32, y: i32) {
    win.set_pos(x, y);
}
