//! Linux change listener: inotify watches on every directory under `$HOME`.
//! Events are forwarded to the indexer thread; everything else is covered by periodic catch-up.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::FromRawFd;
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::thread;

use finder_core::paths::from_bytes;

use crate::Shared;
use crate::index::FLAG_DIR;
use crate::scan::{Exclusions, child_path, read_dir_sorted};

pub enum Change {
    Created {
        path: Vec<u8>,
        is_dir: bool,
    },
    Removed {
        path: Vec<u8>,
    },
    /// Events were lost; re-sync from disk.
    Overflow,
}

const MASK: u32 = libc::IN_CREATE
    | libc::IN_DELETE
    | libc::IN_MOVED_FROM
    | libc::IN_MOVED_TO
    | libc::IN_ONLYDIR
    | libc::IN_DONT_FOLLOW
    | libc::IN_EXCL_UNLINK;

struct Watcher {
    inotify: File,
    /// Watch descriptor → directory path.
    dirs: HashMap<i32, Vec<u8>>,
    ex: Arc<Exclusions>,
    limit_hit: bool,
}

impl Watcher {
    fn add(&mut self, path: &[u8]) -> bool {
        if self.limit_hit || self.ex.contains(path) {
            return false;
        }
        let Ok(c) = CString::new(path) else { return false };
        // SAFETY: valid fd and NUL-terminated path.
        let wd = unsafe { libc::inotify_add_watch(std::os::fd::AsRawFd::as_raw_fd(&self.inotify), c.as_ptr(), MASK) };
        if wd < 0 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::ENOSPC) {
                eprintln!(
                    "finderd: inotify watch limit reached after {} dirs; raise fs.inotify.max_user_watches",
                    self.dirs.len()
                );
                self.limit_hit = true;
            }
            return false;
        }
        // Re-adding an already watched inode (e.g. a moved dir) returns its wd: the path is updated.
        self.dirs.insert(wd, path.to_vec());
        true
    }

    /// Watch `path` and all directories below it, reading them from disk.
    fn add_tree(&mut self, path: Vec<u8>) {
        let mut stack = vec![path];
        while let Some(dir) = stack.pop() {
            if !self.add(&dir) {
                continue;
            }
            for (name, flags) in read_dir_sorted(&from_bytes(&dir)).unwrap_or_default() {
                if flags & FLAG_DIR != 0 {
                    stack.push(child_path(&dir, &name));
                }
            }
        }
    }

    fn run(&mut self, tx: &Sender<Change>) -> io::Result<()> {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = self.inotify.read(&mut buf)?;
            let mut off = 0;
            while off + size_of::<libc::inotify_event>() <= n {
                // SAFETY: the kernel writes whole events; read_unaligned copes with any alignment.
                let ev: libc::inotify_event = unsafe { std::ptr::read_unaligned(buf[off..].as_ptr().cast()) };
                let name_start = off + size_of::<libc::inotify_event>();
                let raw = &buf[name_start..name_start + ev.len as usize];
                off = name_start + ev.len as usize;
                let name = &raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())];
                if ev.mask & libc::IN_Q_OVERFLOW != 0 {
                    tx.send(Change::Overflow).ok();
                    continue;
                }
                if ev.mask & libc::IN_IGNORED != 0 {
                    self.dirs.remove(&ev.wd);
                    continue;
                }
                let Some(dir) = self.dirs.get(&ev.wd) else { continue };
                if name.is_empty() {
                    continue;
                }
                let path = child_path(dir, name);
                let is_dir = ev.mask & libc::IN_ISDIR != 0;
                let change = if ev.mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
                    if is_dir {
                        self.add_tree(path.clone());
                    }
                    Change::Created { path, is_dir }
                } else if ev.mask & (libc::IN_DELETE | libc::IN_MOVED_FROM) != 0 {
                    Change::Removed { path }
                } else {
                    continue;
                };
                if tx.send(change).is_err() {
                    return Ok(());
                }
            }
        }
    }
}

/// Watch every indexed directory below `root` and forward changes to `tx`.
pub fn spawn(shared: Arc<Shared>, root: Vec<u8>, ex: Arc<Exclusions>, tx: Sender<Change>) -> io::Result<()> {
    // SAFETY: plain syscall; the returned fd is owned by `File`.
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut w = Watcher { inotify: unsafe { File::from_raw_fd(fd) }, dirs: HashMap::new(), ex, limit_hit: false };
    // Directory list from the index (no disk walk), parents first.
    let dirs: Vec<Vec<u8>> = {
        let guard = shared.db.read().unwrap();
        let Some(db) = guard.as_ref() else { return Ok(()) };
        let Some(start) = db.lookup(&root) else { return Ok(()) };
        let mut queue = vec![start];
        let mut i = 0;
        while i < queue.len() {
            let n = queue[i];
            queue.extend(db.children(n).into_iter().filter(|&c| db.is_dir(c)));
            i += 1;
        }
        queue.into_iter().map(|n| db.path(n)).collect()
    };
    thread::Builder::new().name("watcher".into()).spawn(move || {
        for d in &dirs {
            w.add(d);
        }
        eprintln!("finderd: watching {} dirs under {}", w.dirs.len(), String::from_utf8_lossy(&root));
        drop(dirs);
        if let Err(e) = w.run(&tx) {
            eprintln!("finderd: watcher stopped: {e}");
        }
    })?;
    Ok(())
}
