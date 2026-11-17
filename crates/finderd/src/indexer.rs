//! The indexer thread: the only writer of the database. Loads or builds the index,
//! catches up on changes, and compacts the delta into a new cache file.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use finder_core::paths;

use crate::db::Db;
use crate::index::Index;
use crate::scan::{self, Exclusions};
use crate::Shared;

/// Interval between dir-mtime catch-up passes (until a change listener is in place).
const CATCH_UP_EVERY: Duration = Duration::from_secs(15 * 60);

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
    let ex = Exclusions::system();
    let file = paths::index_file();
    let cached = if rescan { None } else { Index::open(&file).map_err(|e| eprintln!("finderd: no usable cache: {e}")).ok() };
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
    loop {
        let start = now_secs();
        catch_up(&shared, since, &ex);
        compact(&shared, start);
        since = start;
        thread::sleep(CATCH_UP_EVERY);
    }
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
