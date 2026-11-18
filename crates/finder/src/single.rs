//! One `finder` window at a time. Starting `finder` while a window is open closes that window;
//! without a keyword the new `finder` then exits too, so the hotkey toggles Finder like KRunner.
//!
//! The open window listens on a Unix socket / named pipe (current user only). A new `finder`
//! connects and sends `q`; the old one quits.

use std::io::{Read, Write};
use std::thread;
use std::time::Duration;

use fltk::app;

use crate::client::Msg;

const QUIT: &[u8] = b"q";

/// Ask an open `finder` window to close; true if there was one.
pub fn close_open() -> bool {
    #[cfg(unix)]
    let conn = std::os::unix::net::UnixStream::connect(finder_core::paths::ui_socket_path());
    #[cfg(windows)]
    let conn = finder_core::pipe::connect_ui();
    conn.and_then(|mut c| c.write_all(QUIT)).is_ok()
}

/// Listen in the background for the next `finder`; when it asks, `Msg::Quit` is sent to `tx`.
pub fn listen(tx: app::Sender<Msg>) {
    thread::spawn(move || {
        let Some(mut listener) = bind() else { return };
        loop {
            match accept(&mut listener) {
                Ok(mut c) => {
                    let mut buf = [0u8; 1];
                    if c.read_exact(&mut buf).is_ok() && buf == QUIT {
                        tx.send(Msg::Quit);
                    }
                }
                // Avoid a busy loop if accepting keeps failing.
                Err(_) => thread::sleep(Duration::from_millis(200)),
            }
        }
    });
}

#[cfg(unix)]
type Listener = std::os::unix::net::UnixListener;

#[cfg(unix)]
fn bind() -> Option<Listener> {
    let path = finder_core::paths::ui_socket_path();
    // A closing window keeps its (now unlinked) socket until it exits; it never removes the path.
    let _ = std::fs::remove_file(&path);
    Listener::bind(&path).ok()
}

#[cfg(unix)]
fn accept(l: &mut Listener) -> std::io::Result<impl Read> {
    l.accept().map(|(s, _)| s)
}

#[cfg(windows)]
type Listener = finder_core::pipe::Listener;

#[cfg(windows)]
fn bind() -> Option<Listener> {
    // The pipe name is free only once a closing window has exited: keep asking, then retry.
    for _ in 0..60 {
        match Listener::bind_ui() {
            Ok(l) => return Some(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                close_open();
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => return None,
        }
    }
    None
}

#[cfg(windows)]
fn accept(l: &mut Listener) -> std::io::Result<impl Read> {
    l.accept()
}
