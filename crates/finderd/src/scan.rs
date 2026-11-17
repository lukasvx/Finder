//! Filesystem scanning: full scan into a new index (parallel BFS) and incremental directory sync.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, FileType};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::UNIX_EPOCH;

use finder_core::paths::{SEP, from_bytes};

use crate::db::{Db, Node};
use crate::index::{FLAG_DIR, FLAG_SYMLINK, IndexBuilder};

/// Directories that are listed but never descended into.
pub struct Exclusions(HashSet<Vec<u8>>);

/// Filesystem types worth indexing; mounts of other types (tmpfs, network, fuse, ...) are skipped.
#[cfg(target_os = "linux")]
const LOCAL_FS: &[&str] = &[
    "btrfs", "ext2", "ext3", "ext4", "xfs", "f2fs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk", "zfs", "bcachefs",
    "jfs", "reiserfs", "hfsplus", "nilfs2",
];

impl Exclusions {
    #[cfg(test)]
    pub fn new(paths: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Exclusions(paths.into_iter().collect())
    }

    /// Fixed exclusions plus mount points of non-local filesystems.
    #[cfg(target_os = "linux")]
    pub fn system() -> Self {
        let mut ex: HashSet<Vec<u8>> =
            ["/proc", "/sys", "/dev", "/run", "/.snapshots"].iter().map(|p| p.as_bytes().to_vec()).collect();
        if let Ok(info) = fs::read_to_string("/proc/self/mountinfo") {
            for line in info.lines() {
                let Some((pre, post)) = line.split_once(" - ") else { continue };
                let (Some(mount), Some(fstype)) = (pre.split(' ').nth(4), post.split(' ').next()) else { continue };
                if mount != "/" && !LOCAL_FS.contains(&fstype) {
                    ex.insert(unescape_mountinfo(mount));
                }
            }
        }
        Exclusions(ex)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn system() -> Self {
        Exclusions(HashSet::new())
    }

    pub fn contains(&self, path: &[u8]) -> bool {
        self.0.contains(path)
    }
}

/// Undo mountinfo's octal escapes (`\040` = space).
#[cfg(target_os = "linux")]
fn unescape_mountinfo(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let oct = b.get(i + 1..i + 4).and_then(|d| u8::from_str_radix(std::str::from_utf8(d).ok()?, 8).ok());
        match (b[i], oct) {
            (b'\\', Some(v)) => {
                out.push(v);
                i += 4;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

pub fn flags_of(ft: FileType) -> u8 {
    if ft.is_dir() {
        FLAG_DIR
    } else if ft.is_symlink() {
        FLAG_SYMLINK
    } else {
        0
    }
}

fn os_bytes(s: OsString) -> Vec<u8> {
    #[cfg(unix)]
    return std::os::unix::ffi::OsStringExt::into_vec(s);
    #[cfg(windows)]
    return s.to_string_lossy().into_owned().into_bytes();
}

/// Directory entries as (name, flags), sorted by name. Symlinks are not followed.
pub fn read_dir_sorted(path: &Path) -> io::Result<Vec<(Vec<u8>, u8)>> {
    let mut out: Vec<(Vec<u8>, u8)> = fs::read_dir(path)?
        .filter_map(Result::ok)
        .map(|e| {
            let flags = e.file_type().map_or(0, flags_of);
            (os_bytes(e.file_name()), flags)
        })
        .collect();
    out.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub fn child_path(parent: &[u8], name: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(parent.len() + 1 + name.len());
    p.extend_from_slice(parent);
    if p.last() != Some(&SEP) {
        p.push(SEP);
    }
    p.extend_from_slice(name);
    p
}

/// Map `f` over `items` on a thread pool, preserving order.
pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let threads = thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 16).min(items.len());
    if threads <= 1 {
        return items.iter().map(f).collect();
    }
    let next = AtomicUsize::new(0);
    let mut out: Vec<Option<R>> = items.iter().map(|_| None).collect();
    thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        local.push((i, f(item)));
                    }
                    local
                })
            })
            .collect();
        for w in workers {
            for (i, r) in w.join().unwrap() {
                out[i] = Some(r);
            }
        }
    });
    out.into_iter().map(Option::unwrap).collect()
}

/// Roots of the whole system: `/` on Unix, fixed drives on Windows (no network, removable or optical drives).
fn add_roots(b: &mut IndexBuilder) {
    #[cfg(unix)]
    b.add_root(b"");
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
        const DRIVE_FIXED: u32 = 3;
        let mask = unsafe { GetLogicalDrives() };
        for (bit, letter) in (b'A'..=b'Z').enumerate() {
            let root: Vec<u16> = [letter as u16, b':' as u16, b'\\' as u16, 0].into();
            if mask & (1 << bit) != 0 && unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_FIXED {
                b.add_root(&[letter, b':']);
            }
        }
    }
}

/// Scan the whole system into a new index.
pub fn scan_system(ex: &Exclusions) -> IndexBuilder {
    let mut b = IndexBuilder::new();
    add_roots(&mut b);
    scan_tree(&mut b, 0, ex);
    b
}

/// Expand every directory in `b` from entry `start` on (BFS), reading batches of directories in parallel.
pub fn scan_tree(b: &mut IndexBuilder, start: u32, ex: &Exclusions) {
    const BATCH: u32 = 4096;
    let mut next = start;
    while next < b.len() {
        let end = (next + BATCH).min(b.len());
        let dirs: Vec<(u32, Vec<u8>)> = {
            let v = b.view();
            (next..end).filter(|&i| v.is_dir(i)).map(|i| (i, v.path(i))).filter(|(_, p)| !ex.contains(p)).collect()
        };
        let listings = par_map(&dirs, |(_, p)| read_dir_sorted(&from_bytes(p)).unwrap_or_default());
        for ((id, _), kids) in dirs.iter().zip(listings) {
            for (name, flags) in kids {
                b.push(*id, &name, flags);
            }
        }
        next = end;
    }
}

/// Add everything below the (new, empty) directory `node` at `path` to the delta.
pub fn add_subtree(db: &mut Db, node: Node, path: Vec<u8>, ex: &Exclusions) {
    let mut stack = vec![(node, path)];
    while let Some((dir, path)) = stack.pop() {
        if ex.contains(&path) {
            continue;
        }
        for (name, flags) in read_dir_sorted(&from_bytes(&path)).unwrap_or_default() {
            let child = db.insert_child(dir, &name, flags);
            if flags & FLAG_DIR != 0 {
                stack.push((child, child_path(&path, &name)));
            }
        }
    }
}

/// Make the children of directory `dir` (at `path`) match the disk; new subdirectories are scanned.
pub fn sync_dir(db: &mut Db, dir: Node, path: &[u8], ex: &Exclusions) {
    if ex.contains(path) {
        return;
    }
    let disk = match read_dir_sorted(&from_bytes(path)) {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            db.remove_node(dir);
            return;
        }
        Err(_) => return,
    };
    let on_disk: HashMap<&[u8], u8> = disk.iter().map(|(n, f)| (n.as_slice(), *f)).collect();
    let gone: Vec<Node> = db.children(dir).into_iter().filter(|&c| !on_disk.contains_key(db.name(c))).collect();
    for c in gone {
        db.remove_node(c);
    }
    for (name, flags) in &disk {
        let before = db.child(dir, name);
        let node = db.insert_child(dir, name, *flags);
        if before != Some(node) && flags & FLAG_DIR != 0 {
            add_subtree(db, node, child_path(path, name), ex);
        }
    }
}

/// Path-or-node lookup helper for syncing a path that may not be in the database yet:
/// syncs the nearest existing ancestor directory.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn sync_path(db: &mut Db, path: &[u8], ex: &Exclusions) {
    let mut p = path.to_vec();
    loop {
        if let Some(n) = db.lookup(&p).filter(|&n| db.is_dir(n)) {
            sync_dir(db, n, &p, ex);
            return;
        }
        match crate::db::split_last(&p) {
            Some((parent, _)) => p = parent.to_vec(),
            None => return,
        }
    }
}

fn mtime_secs(path: &[u8]) -> Option<u64> {
    let m = fs::symlink_metadata(from_bytes(path)).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()))
}

/// Live directories whose mtime is at or after `since` (or that vanished), with their paths.
pub fn changed_dirs(db: &Db, since: u64, ex: &Exclusions) -> Vec<(Node, Vec<u8>)> {
    let v = db.view();
    let hidden = db.hidden();
    let dirs: Vec<Node> = (0..v.len())
        .filter(|&i| v.is_dir(i) && hidden.is_none_or(|h| !h.get(i)))
        .map(Node::Base)
        .chain(db.delta_nodes().filter(|&n| db.is_dir(n) && db.is_live(n)))
        .collect();
    let mut out = Vec::new();
    for chunk in dirs.chunks(8192) {
        let paths: Vec<(Node, Vec<u8>)> =
            chunk.iter().map(|&n| (n, db.path(n))).filter(|(_, p)| !ex.contains(p)).collect();
        let changed = par_map(&paths, |(_, p)| mtime_secs(p).is_none_or(|t| t >= since));
        out.extend(paths.into_iter().zip(changed).filter(|(_, c)| *c).map(|(np, _)| np));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Index;
    use crate::index::tests::temp_dir;
    use finder_core::paths::to_bytes;

    fn paths_of(db: &Db) -> Vec<String> {
        let b = db.compact();
        let v = b.view();
        let mut out: Vec<String> = (0..v.len()).map(|i| String::from_utf8(v.path(i)).unwrap()).collect();
        out.sort();
        out
    }

    #[test]
    fn scan_and_sync_real_tree() {
        let dir = temp_dir("scan");
        let root = to_bytes(&dir);
        for p in ["a/b/c.txt", "a/d.txt", "skip/x", "e/f/g"] {
            let full = dir.join(p);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, "").unwrap();
        }
        // Scan only the temp dir: a root named after the dir path, excluding `skip`.
        let ex = Exclusions::new([child_path(&root, b"skip")]);
        let mut b = IndexBuilder::new();
        b.add_root(b"");
        for comp in root.split(|&c| c == SEP).filter(|c| !c.is_empty()) {
            let last = b.len() - 1;
            b.push(last, comp, FLAG_DIR);
        }
        let start = b.len() - 1;
        scan_tree(&mut b, start, &ex);
        let file = dir.join("index.bin");
        b.write(&file, 0).unwrap();
        let mut db = Db::new(Index::open(&file).unwrap());
        fs::remove_file(&file).unwrap();
        let rel = |db: &Db| -> Vec<String> {
            let prefix = String::from_utf8(root.clone()).unwrap();
            paths_of(db).into_iter().filter_map(|p| p.strip_prefix(&prefix).map(str::to_string)).collect()
        };
        assert_eq!(rel(&db), ["", "/a", "/a/b", "/a/b/c.txt", "/a/d.txt", "/e", "/e/f", "/e/f/g", "/skip"]);

        // Change the tree, then sync via mtime-based catch-up.
        fs::remove_dir_all(dir.join("a/b")).unwrap();
        fs::create_dir_all(dir.join("new/deep")).unwrap();
        fs::write(dir.join("new/deep/z"), "").unwrap();
        fs::remove_dir_all(dir.join("e/f")).unwrap();
        fs::write(dir.join("e/f"), "").unwrap();
        for (n, p) in changed_dirs(&db, 0, &ex) {
            if db.is_live(n) && p.starts_with(&root) {
                sync_dir(&mut db, n, &p, &ex);
            }
        }
        assert_eq!(rel(&db), ["", "/a", "/a/d.txt", "/e", "/e/f", "/new", "/new/deep", "/new/deep/z", "/skip"]);
        assert!(!db.is_dir(db.lookup(&child_path(&root, b"e/f")).unwrap()));

        // sync_path on a not-yet-indexed path syncs the nearest existing ancestor.
        fs::write(dir.join("new/deep/y"), "").unwrap();
        sync_path(&mut db, &child_path(&root, b"new/deep/y"), &ex);
        assert!(db.lookup(&child_path(&root, b"new/deep/y")).is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_unescape() {
        assert_eq!(unescape_mountinfo(r"/mnt/my\040disk"), b"/mnt/my disk");
        assert!(Exclusions::system().contains(b"/proc"));
    }
}

#[cfg(test)]
mod bench {
    /// `cargo test --release -p finderd -- --ignored --nocapture catch_up_cost`
    #[test]
    #[ignore]
    fn catch_up_cost() {
        use super::*;
        let idx = crate::index::Index::open(&finder_core::paths::index_file()).unwrap();
        let db = Db::new(idx);
        let ex = Exclusions::system();
        let t = std::time::Instant::now();
        let changed = changed_dirs(&db, u64::MAX, &ex);
        let dirs = (0..db.view().len()).filter(|&i| db.view().is_dir(i)).count();
        eprintln!("stat {dirs} dirs: {:?} ({} changed)", t.elapsed(), changed.len());
    }
}
