//! Local socket (Unix) / named pipe (Windows) server: one thread per UI connection, each with its own `Searcher`.

use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

use finder_core::proto::{Hit, Query, Request, Response};

use crate::Shared;
use crate::db::Node;
use crate::search::{Options, Searcher};

#[cfg(unix)]
pub type Listener = std::os::unix::net::UnixListener;

/// Bind the daemon socket. Fails if another daemon is already listening.
#[cfg(unix)]
pub fn bind() -> io::Result<Listener> {
    use std::os::unix::net::UnixListener;
    let path = finder_core::paths::socket_path();
    // Also stops a running finderd from another version.
    if finder_core::proto::connect().is_ok() {
        return Err(io::Error::new(io::ErrorKind::AddrInUse, "another finderd is already running"));
    }
    let _ = std::fs::remove_file(&path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    UnixListener::bind(&path)
}

#[cfg(windows)]
pub type Listener = finder_core::pipe::Listener;

/// Bind the daemon pipe. Fails if another daemon is already listening.
#[cfg(windows)]
pub fn bind() -> io::Result<Listener> {
    // Also stops a running finderd from another version, which frees the pipe name.
    if finder_core::proto::connect().is_ok() {
        return Err(io::Error::new(io::ErrorKind::AddrInUse, "another finderd is already running"));
    }
    Listener::bind()
}

#[cfg(unix)]
pub fn serve(listener: Listener, shared: Arc<Shared>) {
    for stream in listener.incoming() {
        spawn_handler(stream, &shared);
    }
}

#[cfg(windows)]
pub fn serve(mut listener: Listener, shared: Arc<Shared>) {
    loop {
        let stream = listener.accept();
        if stream.is_err() {
            // Avoid a busy loop if pipe creation keeps failing.
            thread::sleep(std::time::Duration::from_millis(100));
        }
        spawn_handler(stream, &shared);
    }
}

fn spawn_handler<S: io::Read + io::Write + Send + 'static>(stream: io::Result<S>, shared: &Arc<Shared>) {
    match stream {
        Ok(mut s) => {
            let shared = shared.clone();
            thread::spawn(move || {
                let _ = handle(&mut s, &shared);
            });
        }
        Err(e) => eprintln!("finderd: accept failed: {e}"),
    }
}

fn handle<S: io::Read + io::Write>(stream: &mut S, shared: &Shared) -> io::Result<()> {
    let mut searcher = Searcher::default();
    loop {
        let resp = match Request::recv(stream)? {
            Request::Search(q) => search(shared, &mut searcher, &q),
            Request::Opened(path) => {
                shared.recent.lock().unwrap().push(path);
                Response::Ok
            }
            Request::Version => Response::Version(finder_core::proto::VERSION),
            Request::Quit => {
                Response::Ok.send(stream)?;
                std::process::exit(0);
            }
        };
        let big = matches!(&resp, Response::Results { hits, .. } if hits.len() > TRIM_AFTER);
        resp.send(stream)?;
        if big {
            drop(resp);
            trim_heap();
        }
    }
}

/// Responses with more hits than this are followed by returning freed memory to the OS.
const TRIM_AFTER: usize = 10_000;

fn trim_heap() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        libc::malloc_trim(0);
    }
}

fn search(shared: &Shared, searcher: &mut Searcher, q: &Query) -> Response {
    let indexing = shared.indexing.load(Ordering::Relaxed);
    let guard = shared.db.read().unwrap();
    let Some(db) = guard.as_ref() else {
        return Response::Results { total: 0, indexing: true, hits: Vec::new() };
    };
    let recent: HashSet<Node> = shared.recent.lock().unwrap().nodes(db);
    let opts = Options { case_sensitive: q.case_sensitive };
    let out = searcher.search(db, &q.text, opts, q.limit as usize, &recent);
    let hits = out.hits.iter().map(|&n| Hit { is_dir: db.is_dir(n), path: db.path(n) }).collect();
    Response::Results { total: out.total as u64, indexing, hits }
}
