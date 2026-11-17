//! Connection to `finderd` on a worker thread. Only the latest query is sent; queries typed
//! while one is in flight replace each other, so stale keystrokes are dropped.

use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use finder_core::proto::{Hit, Query, Request, Response};
use fltk::app;

pub enum Msg {
    Results { seq: u64, total: u64, indexing: bool, hits: Vec<Hit> },
    Status(String),
}

#[derive(Default)]
struct Slot {
    pending: Option<(u64, Query)>,
}

pub struct Client {
    slot: Arc<(Mutex<Slot>, Condvar)>,
}

/// While the daemon is still indexing, repeat the current query this often.
const REPOLL: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;

#[cfg(unix)]
fn connect() -> io::Result<Stream> {
    Stream::connect(finder_core::paths::socket_path())
}

#[cfg(windows)]
type Stream = std::fs::File;

#[cfg(windows)]
fn connect() -> io::Result<Stream> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Windows IPC not implemented yet"))
}

/// Start `finderd` from next to this executable (or from `PATH`), detached, logging to the cache dir.
fn spawn_daemon() -> io::Result<()> {
    use std::process::{Command, Stdio};
    let exe = if cfg!(windows) { "finderd.exe" } else { "finderd" };
    let sibling = std::env::current_exe().ok().and_then(|p| Some(p.parent()?.join(exe))).filter(|p| p.is_file());
    let mut cmd = Command::new(sibling.as_deref().unwrap_or(exe.as_ref()));
    let cache = finder_core::paths::cache_dir();
    let _ = std::fs::create_dir_all(&cache);
    let log = std::fs::OpenOptions::new().create(true).append(true).open(cache.join("finderd.log"));
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(log.map_or_else(|_| Stdio::null(), Stdio::from));
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    cmd.spawn().map(drop)
}

impl Client {
    pub fn start(tx: app::Sender<Msg>) -> Client {
        let slot = Arc::new((Mutex::new(Slot::default()), Condvar::new()));
        let s = slot.clone();
        thread::spawn(move || worker(&s, &tx));
        Client { slot }
    }

    pub fn search(&self, seq: u64, q: Query) {
        let (lock, cv) = &*self.slot;
        lock.lock().unwrap().pending = Some((seq, q));
        cv.notify_one();
    }
}

fn worker(slot: &(Mutex<Slot>, Condvar), tx: &app::Sender<Msg>) {
    let (lock, cv) = slot;
    let mut conn: Option<Stream> = None;
    let mut repoll: Option<(u64, Query)> = None;
    loop {
        let (seq, q) = {
            let mut g = lock.lock().unwrap();
            loop {
                if let Some(p) = g.pending.take() {
                    break p;
                }
                match &repoll {
                    Some(_) => {
                        let (g2, timeout) = cv.wait_timeout(g, REPOLL).unwrap();
                        g = g2;
                        if timeout.timed_out() && g.pending.is_none() {
                            break repoll.take().unwrap();
                        }
                    }
                    None => g = cv.wait(g).unwrap(),
                }
            }
        };
        repoll = None;
        if conn.is_none() {
            match connect_or_spawn(tx) {
                Ok(c) => conn = Some(c),
                Err(e) => {
                    tx.send(Msg::Status(format!("Cannot reach finderd: {e}")));
                    continue;
                }
            }
        }
        let req = Request::Search(q.clone());
        let mut result = exchange(conn.as_mut().unwrap(), &req);
        if result.is_err() {
            // Daemon restarted: reconnect once.
            conn = connect().ok();
            result = match conn.as_mut() {
                Some(c) => exchange(c, &req),
                None => result,
            };
        }
        match result {
            Ok(Response::Results { total, indexing, hits }) => {
                if indexing {
                    repoll = Some((seq, q));
                }
                tx.send(Msg::Results { seq, total, indexing, hits });
            }
            Ok(Response::Error(e)) => tx.send(Msg::Status(format!("finderd: {e}"))),
            Ok(Response::Ok) => {}
            Err(e) => {
                conn = None;
                tx.send(Msg::Status(format!("Lost connection to finderd: {e}")));
            }
        }
    }
}

fn exchange(c: &mut Stream, req: &Request) -> io::Result<Response> {
    req.send(c)?;
    Response::recv(c)
}

fn connect_or_spawn(tx: &app::Sender<Msg>) -> io::Result<Stream> {
    if let Ok(c) = connect() {
        return Ok(c);
    }
    tx.send(Msg::Status("Starting finderd…".into()));
    spawn_daemon()?;
    let start = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(50));
        match connect() {
            Ok(c) => return Ok(c),
            Err(e) if start.elapsed() > CONNECT_TIMEOUT => return Err(e),
            Err(_) => {}
        }
    }
}

/// Tell the daemon a path was opened (for ranking). Best effort, short timeout.
pub fn notify_opened(path: &[u8]) {
    let Ok(mut c) = connect() else { return };
    #[cfg(unix)]
    let _ = c.set_read_timeout(Some(Duration::from_millis(300)));
    let _ = exchange(&mut c, &Request::Opened(path.to_vec()));
}
