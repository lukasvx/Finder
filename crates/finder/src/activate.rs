//! Windows: make sure the new window is in front with keyboard focus.
//!
//! Windows may refuse to activate a newly shown window (the "foreground lock"): the window then
//! appears without focus and typing goes to the previous app. Sharing the input state with the
//! current foreground window's thread for a moment lets `SetForegroundWindow` succeed.

use fltk::prelude::*;
use fltk::{app, window::Window};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow,
};

/// Bring `win` to the front now, and check again shortly after (activation can be undone by the
/// launching shell, such as the closing Start menu).
pub fn bring_to_front(win: &Window) {
    activate(win);
    for delay in [0.1, 0.3] {
        let win = win.clone();
        app::add_timeout3(delay, move |_| activate(&win));
    }
}

fn activate(win: &Window) {
    if !win.shown() {
        return;
    }
    let hwnd = win.raw_handle() as HWND;
    // SAFETY: plain Win32 calls on our own, live window handle.
    unsafe {
        let fg = GetForegroundWindow();
        if fg == hwnd || SetForegroundWindow(hwnd) != 0 {
            return;
        }
        let me = GetCurrentThreadId();
        let other = GetWindowThreadProcessId(fg, std::ptr::null_mut());
        if other == 0 || other == me || AttachThreadInput(me, other, 1) == 0 {
            return;
        }
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
        AttachThreadInput(me, other, 0);
    }
}
