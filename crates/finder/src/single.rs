//! One `finder` window at a time: a new `finder` closes the one that is already open.
//!
//! `finder` normally closes when its window loses keyboard focus. A window that never got the
//! focus (Windows' foreground lock, or focus-stealing prevention) never loses it either, so it
//! would stay open behind other windows; each launch could leave another one behind.
//!
//! The open window listens on a Unix socket / named pipe (current user only). A new `finder`
//! connects and sends `q`; the old one quits.

use std::io::{Read, Write};
use std::thread;
use std::time::Duration;

use fltk::app;

use crate::client::Msg;

const QUIT: &[u8] = b"q";

/// Close any open `finder`, then listen for the next one in the background; when it asks,
/// `Msg::Quit` is sent to `tx`.
pub fn take_over(tx: app::Sender<Msg>) {
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
    use std::os::unix::net::UnixStream;
    let path = finder_core::paths::ui_socket_path();
    if let Ok(mut c) = UnixStream::connect(&path) {
        let _ = c.write_all(QUIT);
    }
    // The old window keeps its (now unlinked) socket until it exits; it never removes the path.
    let _ = std::fs::remove_file(&path);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
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
    use finder_core::pipe;
    // The pipe name is free only once the old window has exited: ask it to quit, then retry.
    for _ in 0..60 {
        match Listener::bind_ui() {
            Ok(l) => return Some(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                if let Ok(mut c) = pipe::connect_ui() {
                    let _ = c.write_all(QUIT);
                }
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
