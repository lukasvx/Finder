//! The indexer thread: the only writer of the database. Loads or builds the index,
//! applies watcher events, catches up on changes, and compacts the delta into a new cache file.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use finder_core::paths;

use crate::Shared;
use crate::db::Db;
use crate::index::Index;
use crate::scan::{self, Exclusions};
#[cfg(target_os = "linux")]
use crate::watch::{self, Change};

#[cfg(not(target_os = "linux"))]
pub enum Change {}

/// Interval between dir-mtime catch-up passes (covers everything the watcher does not).
const CATCH_UP_EVERY: Duration = Duration::from_secs(15 * 60);
/// Collect watcher events this long before applying them in one go.
const BATCH_WINDOW: Duration = Duration::from_millis(200);
/// Compact as soon as the delta has this many changes (bounds daemon heap).
const COMPACT_AT: usize = 20_000;

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Lower CPU and I/O priority of the calling thread and the threads it spawns.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let tid = libc::gettid() as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
        // ioprio_set(IOPRIO_WHO_PROCESS, tid, best-effort class, lowest level)
        libc::syscall(libc::SYS_ioprio_set, 1, tid, (2 << 13) | 7);
    }
}

pub fn run(shared: Arc<Shared>, rescan: bool) {
    lower_priority();
    let ex = Arc::new(Exclusions::system());
    let file = paths::index_file();
    let cached =
        if rescan { None } else { Index::open(&file).map_err(|e| eprintln!("finderd: no usable cache: {e}")).ok() };
    let mut since = match cached {
        Some(idx) => {
            eprintln!("finderd: loaded {} entries from {}", idx.view().len(), file.display());
            let since = idx.scan_time();
            *shared.db.write().unwrap() = Some(Db::new(idx));
            since
        }
        None => {
            shared.indexing.store(true, Ordering::Relaxed);
            let since = full_scan(&shared, &ex);
            shared.indexing.store(false, Ordering::Relaxed);
            since
        }
    };

    let (tx, rx) = mpsc::channel::<Change>();
    let mut tx = Some(tx);
    let mut next_catch_up = Instant::now();
    loop {
        if Instant::now() >= next_catch_up {
            let start = now_secs();
            catch_up(&shared, since, &ex);
            compact(&shared, start);
            since = start;
            next_catch_up = Instant::now() + CATCH_UP_EVERY;
            // Start watching after the first catch-up, so the watch list matches the disk.
            #[cfg(target_os = "linux")]
            if let (Some(tx), Some(home)) = (tx.take(), std::env::var_os("HOME")) {
                let home = paths::to_bytes(home.as_ref());
                if let Err(e) = watch::spawn(shared.clone(), home, ex.clone(), tx) {
                    eprintln!("finderd: cannot start watcher: {e}");
                }
            }
            #[cfg(not(target_os = "linux"))]
            drop(tx.take());
        }
        let first = match rx.recv_timeout(next_catch_up.saturating_duration_since(Instant::now())) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(next_catch_up.saturating_duration_since(Instant::now()));
                continue;
            }
        };
        let mut batch = vec![first];
        let deadline = Instant::now() + BATCH_WINDOW;
        while let Ok(c) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            batch.push(c);
        }
        if apply(&shared, batch, &ex) {
            catch_up(&shared, since, &ex);
        }
        if shared.db.read().unwrap().as_ref().is_some_and(|db| db.delta_len() >= COMPACT_AT) {
            compact(&shared, since);
        }
    }
}

/// Apply watcher events. Returns true if events were lost and a catch-up is needed.
#[cfg(target_os = "linux")]
fn apply(shared: &Shared, batch: Vec<Change>, ex: &Exclusions) -> bool {
    let mut guard = shared.db.write().unwrap();
    let Some(db) = guard.as_mut() else { return false };
    let mut overflow = false;
    for change in batch {
        match change {
            Change::Created { path, is_dir } => {
                // Re-check the disk: the entry may already be gone again.
                let Ok(meta) = std::fs::symlink_metadata(paths::from_bytes(&path)) else { continue };
                let flags = scan::flags_of(meta.file_type());
                let before = db.lookup(&path);
                match db.insert(&path, flags) {
                    Some(n) if is_dir && before != Some(n) => scan::add_subtree(db, n, path, ex),
                    Some(_) => {}
                    None => scan::sync_path(db, &path, ex),
                }
            }
            Change::Removed { path } => {
                db.remove(&path);
            }
            Change::Overflow => overflow = true,
        }
    }
    overflow
}

#[cfg(not(target_os = "linux"))]
fn apply(_: &Shared, batch: Vec<Change>, _: &Exclusions) -> bool {
    batch.into_iter().next().is_some_and(|c| match c {})
}

/// Scan everything into a new cache file; returns the scan start time.
fn full_scan(shared: &Shared, ex: &Exclusions) -> u64 {
    let start = now_secs();
    let t = Instant::now();
    let b = scan::scan_system(ex);
    eprintln!("finderd: scanned {} entries in {:.1?}", b.len(), t.elapsed());
    let file = paths::index_file();
    match b.write(&file, start).and_then(|_| Index::open(&file)) {
        Ok(idx) => *shared.db.write().unwrap() = Some(Db::new(idx)),
        Err(e) => eprintln!("finderd: cannot write {}: {e}", file.display()),
    }
    start
}

/// Re-read directories modified since `since`.
fn catch_up(shared: &Shared, since: u64, ex: &Exclusions) {
    let t = Instant::now();
    let changed = match &*shared.db.read().unwrap() {
        Some(db) => scan::changed_dirs(db, since.saturating_sub(1), ex),
        None => return,
    };
    if changed.is_empty() {
        return;
    }
    let mut guard = shared.db.write().unwrap();
    let db = guard.as_mut().unwrap();
    for (n, p) in &changed {
        if db.is_live(*n) {
            scan::sync_dir(db, *n, p, ex);
        }
    }
    eprintln!("finderd: synced {} changed dirs in {:.1?}, delta {}", changed.len(), t.elapsed(), db.delta_len());
}

/// Merge the delta into a new cache file. Safe under a read lock: this thread is the only writer.
fn compact(shared: &Shared, scan_time: u64) {
    let builder = match &*shared.db.read().unwrap() {
        Some(db) if db.delta_len() > 0 => db.compact(),
        _ => return,
    };
    let file = paths::index_file();
    match builder.write(&file, scan_time).and_then(|_| Index::open(&file)) {
        Ok(idx) => {
            eprintln!("finderd: compacted to {} entries", idx.view().len());
            *shared.db.write().unwrap() = Some(Db::new(idx));
        }
        Err(e) => eprintln!("finderd: cannot write {}: {e}", file.display()),
    }
}
