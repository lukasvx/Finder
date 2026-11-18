//! Connection to `finderd` on a worker thread. Only the latest query is sent; queries typed
//! while one is in flight replace each other, so stale keystrokes are dropped.

use std::io;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use finder_core::proto::{Hit, Query, Request, Response, Stream, connect, exchange};
use fltk::app;

pub enum Msg {
    Results {
        seq: u64,
        total: u64,
        indexing: bool,
        hits: Vec<Hit>,
    },
    Status(String),
    /// A newer `finder` was started: close this one.
    Quit,
    /// The icon thread's result for a path.
    Icon {
        path: Vec<u8>,
        found: crate::icons::Found,
    },
}

#[derive(Default)]
struct Slot {
    pending: Option<(u64, Query)>,
    /// Connect (starting `finderd` if needed) without a query.
    connect: bool,
}

pub struct Client {
    slot: Arc<(Mutex<Slot>, Condvar)>,
}

/// While the daemon is still indexing, repeat the current query this often.
const REPOLL: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Start `finderd` from next to this executable (or from `PATH`), detached.
fn spawn_daemon() -> io::Result<()> {
    use std::process::{Command, Stdio};
    let exe = finder_core::paths::sibling_exe("finderd");
    let mut cmd = Command::new(exe.as_deref().unwrap_or("finderd".as_ref()));
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
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

    /// Make sure `finderd` runs, so indexing starts and the hotkey is registered.
    pub fn connect(&self) {
        let (lock, cv) = &*self.slot;
        lock.lock().unwrap().connect = true;
        cv.notify_one();
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
        let job = {
            let mut g = lock.lock().unwrap();
            loop {
                if let Some(p) = g.pending.take() {
                    break Some(p);
                }
                if std::mem::take(&mut g.connect) {
                    break None;
                }
                match &repoll {
                    Some(_) => {
                        let (g2, timeout) = cv.wait_timeout(g, REPOLL).unwrap();
                        g = g2;
                        if timeout.timed_out() && g.pending.is_none() {
                            break repoll.take();
                        }
                    }
                    None => g = cv.wait(g).unwrap(),
                }
            }
        };
        repoll = None;
        if conn.is_none() {
            match connect_or_spawn(tx) {
                Ok(c) => {
                    conn = Some(c);
                    // Clears "Starting finderd…" or an earlier error.
                    tx.send(Msg::Status(String::new()));
                }
                Err(e) => {
                    tx.send(Msg::Status(format!("Cannot reach finderd: {e}")));
                    continue;
                }
            }
        }
        let Some((seq, q)) = job else { continue };
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
            Ok(Response::Ok | Response::Version(_)) => {}
            Err(e) => {
                conn = None;
                tx.send(Msg::Status(format!("Lost connection to finderd: {e}")));
            }
        }
    }
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

/// Tell the daemon a path was opened (for ranking). Best effort: waits at most 300 ms.
pub fn notify_opened(path: &[u8]) {
    let req = Request::Opened(path.to_vec());
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    thread::spawn(move || {
        if let Ok(mut c) = connect() {
            let _ = exchange(&mut c, &req);
        }
        let _ = done_tx.send(());
    });
    let _ = done_rx.recv_timeout(Duration::from_millis(300));
}
