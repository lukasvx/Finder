//! On-disk index: a memory-mapped file of fixed-size entries plus packed name buffers.
//!
//! Layout: 64-byte header, `entry_count` × 12-byte [`Entry`], names, lowercased names.
//! Each name is followed by a `\0` so substring matches never span two names; the lowercased
//! buffer has identical offsets. Entries are in BFS order: roots first, then children grouped
//! by parent in parent order, sorted by name. So `parent` is non-decreasing after the roots,
//! and children of a directory are found by binary search.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use bytemuck::{Pod, Zeroable};
use finder_core::paths::SEP;
use memmap2::Mmap;

use finder_core::fold::fold_into;

pub const NONE: u32 = u32::MAX;
pub const FLAG_DIR: u8 = 1;
pub const FLAG_SYMLINK: u8 = 2;
/// Marked hidden by the filesystem (Windows attribute). Dotfiles are recognized by name instead.
pub const FLAG_HIDDEN: u8 = 4;

const MAGIC: [u8; 8] = *b"FINDRIDX";
/// 2: `FLAG_HIDDEN`. A cache with another version is rebuilt by a full scan.
const VERSION: u32 = 2;
const HEADER_LEN: usize = size_of::<Header>();

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct Entry {
    pub parent: u32,
    pub name_off: u32,
    pub name_len: u16,
    pub flags: u8,
    /// Distance from the root, saturating at 255.
    pub depth: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Header {
    magic: [u8; 8],
    version: u32,
    root_count: u32,
    entry_count: u64,
    names_len: u64,
    /// Unix time (seconds) when the scan that produced this index started.
    scan_time: u64,
    _reserved: [u8; 24],
}

const _: () = assert!(size_of::<Entry>() == 12 && HEADER_LEN == 64);

/// Read-only accessors shared by [`Index`] and [`IndexBuilder`].
#[derive(Clone, Copy)]
pub struct View<'a> {
    pub entries: &'a [Entry],
    pub names: &'a [u8],
    pub lower: &'a [u8],
    pub root_count: u32,
}

impl<'a> View<'a> {
    pub fn len(&self) -> u32 {
        self.entries.len() as u32
    }

    pub fn entry(&self, i: u32) -> Entry {
        self.entries[i as usize]
    }

    fn range(e: &Entry) -> Range<usize> {
        e.name_off as usize..e.name_off as usize + e.name_len as usize
    }

    pub fn name(&self, i: u32) -> &'a [u8] {
        &self.names[Self::range(&self.entries[i as usize])]
    }

    pub fn lower_name(&self, i: u32) -> &'a [u8] {
        &self.lower[Self::range(&self.entries[i as usize])]
    }

    pub fn is_dir(&self, i: u32) -> bool {
        self.entries[i as usize].flags & FLAG_DIR != 0
    }

    /// Ids of the children of `dir`.
    pub fn children(&self, dir: u32) -> Range<u32> {
        let rc = self.root_count as usize;
        let tail = &self.entries[rc..];
        let lo = tail.partition_point(|e| e.parent < dir);
        let hi = lo + tail[lo..].partition_point(|e| e.parent <= dir);
        (rc + lo) as u32..(rc + hi) as u32
    }

    pub fn find_child(&self, dir: u32, name: &[u8]) -> Option<u32> {
        let r = self.children(dir);
        let kids = &self.entries[r.start as usize..r.end as usize];
        kids.binary_search_by(|e| self.names[Self::range(e)].cmp(name)).ok().map(|k| r.start + k as u32)
    }

    /// Roots are `""` (Unix) or drive names such as `C:`, which may be typed in lowercase.
    pub fn find_root(&self, name: &[u8]) -> Option<u32> {
        (0..self.root_count).find(|&r| self.name(r).eq_ignore_ascii_case(name))
    }

    /// Id of the entry whose name (or trailing `\0`) covers byte `pos` of the name buffer.
    pub fn entry_at(&self, pos: usize) -> u32 {
        (self.entries.partition_point(|e| e.name_off as usize <= pos) - 1) as u32
    }

    /// Start of entry `i`'s name in the name buffer; `names.len()` for `i == len()`.
    pub fn name_start(&self, i: u32) -> usize {
        self.entries.get(i as usize).map_or(self.names.len(), |e| e.name_off as usize)
    }

    pub fn path(&self, i: u32) -> Vec<u8> {
        let mut ids = Vec::new();
        let mut cur = i;
        while cur != NONE {
            ids.push(cur);
            cur = self.entries[cur as usize].parent;
        }
        join_path(ids.iter().rev().map(|&id| self.name(id)))
    }
}

/// Join a root name and components into a path. Linux root is `""` (→ `/a/b`), Windows `C:` (→ `C:\a\b`).
pub fn join_path<'a>(mut components: impl Iterator<Item = &'a [u8]>) -> Vec<u8> {
    let mut out = components.next().unwrap_or_default().to_vec();
    let mut any = false;
    for c in components {
        out.push(SEP);
        out.extend_from_slice(c);
        any = true;
    }
    if !any {
        out.push(SEP);
    }
    out
}

/// Split an absolute path into (root name, components). `None` for relative paths.
pub fn split_path(path: &[u8]) -> Option<(&[u8], impl Iterator<Item = &[u8]>)> {
    #[cfg(unix)]
    let (root, rest) = (b"".as_slice(), path.strip_prefix(b"/")?);
    #[cfg(windows)]
    let (root, rest) = {
        if path.len() < 2 || path[1] != b':' {
            return None;
        }
        (&path[..2], &path[2..])
    };
    let is_sep = |b: &u8| *b == b'/' || *b == SEP;
    Some((root, rest.split(is_sep).filter(|c| !c.is_empty())))
}

/// A memory-mapped index file.
pub struct Index {
    map: Mmap,
    root_count: u32,
    entries: Range<usize>,
    names: Range<usize>,
    lower: Range<usize>,
    scan_time: u64,
}

fn corrupt(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("corrupt index: {msg}"))
}

impl Index {
    pub fn open(path: &Path) -> io::Result<Index> {
        let file = File::open(path)?;
        // SAFETY: index files are only ever replaced by atomic rename, never modified in place.
        let map = unsafe { Mmap::map(&file)? };
        let h: Header = bytemuck::pod_read_unaligned(map.get(..HEADER_LEN).ok_or_else(|| corrupt("short header"))?);
        if h.magic != MAGIC || h.version != VERSION {
            return Err(corrupt("bad magic or version"));
        }
        let n = usize::try_from(h.entry_count).map_err(|_| corrupt("size"))?;
        let names_len = usize::try_from(h.names_len).map_err(|_| corrupt("size"))?;
        let entries = HEADER_LEN..HEADER_LEN + n * size_of::<Entry>();
        let names = entries.end..entries.end + names_len;
        let lower = names.end..names.end + names_len;
        if lower.end != map.len() || h.root_count as usize > n || h.root_count == 0 {
            return Err(corrupt("size mismatch"));
        }
        let idx = Index { map, root_count: h.root_count, entries, names, lower, scan_time: h.scan_time };
        idx.validate()?;
        Ok(idx)
    }

    fn validate(&self) -> io::Result<()> {
        let v = self.view();
        let mut prev_parent = 0;
        for (i, e) in v.entries.iter().enumerate() {
            let is_root = (i as u32) < v.root_count;
            let parent_ok = if is_root { e.parent == NONE } else { e.parent < i as u32 && e.parent >= prev_parent };
            let end = e.name_off as usize + e.name_len as usize;
            if !parent_ok || end >= v.names.len() || v.names[end] != 0 {
                return Err(corrupt("bad entry"));
            }
            if !is_root {
                prev_parent = e.parent;
            }
        }
        Ok(())
    }

    pub fn view(&self) -> View<'_> {
        View {
            entries: bytemuck::cast_slice(&self.map[self.entries.clone()]),
            names: &self.map[self.names.clone()],
            lower: &self.map[self.lower.clone()],
            root_count: self.root_count,
        }
    }

    pub fn scan_time(&self) -> u64 {
        self.scan_time
    }
}

/// Builds an index in memory, in BFS order: all roots first, then `push` children
/// with non-decreasing parent ids, each parent's children sorted by name.
#[derive(Default)]
pub struct IndexBuilder {
    entries: Vec<Entry>,
    names: Vec<u8>,
    lower: Vec<u8>,
    root_count: u32,
}

impl IndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> u32 {
        self.entries.len() as u32
    }

    fn add(&mut self, parent: u32, name: &[u8], flags: u8, depth: u8) -> u32 {
        let id = self.len();
        let name = &name[..name.len().min(u16::MAX as usize)];
        let name_off = u32::try_from(self.names.len()).expect("index name buffer exceeds 4 GiB");
        self.entries.push(Entry { parent, name_off, name_len: name.len() as u16, flags, depth });
        self.names.extend_from_slice(name);
        self.names.push(0);
        fold_into(name, &mut self.lower);
        self.lower.push(0);
        id
    }

    pub fn add_root(&mut self, name: &[u8]) -> u32 {
        assert_eq!(self.len(), self.root_count, "roots must come first");
        self.root_count += 1;
        self.add(NONE, name, FLAG_DIR, 0)
    }

    pub fn push(&mut self, parent: u32, name: &[u8], flags: u8) -> u32 {
        debug_assert!(parent < self.len());
        debug_assert!(self.entries.last().is_none_or(|e| e.parent == NONE || e.parent <= parent));
        let depth = self.entries[parent as usize].depth.saturating_add(1);
        self.add(parent, name, flags, depth)
    }

    pub fn view(&self) -> View<'_> {
        View { entries: &self.entries, names: &self.names, lower: &self.lower, root_count: self.root_count }
    }

    /// Write to `path` atomically (temp file + rename).
    pub fn write(&self, path: &Path, scan_time: u64) -> io::Result<()> {
        let tmp = self.write_tmp(path, scan_time)?;
        fs::rename(&tmp, path)
    }

    /// Write to a new temporary file next to `path` and return its name; renaming it to `path` is up to the caller.
    /// Each call uses a fresh name, so a still-mapped earlier file is never truncated.
    pub fn write_tmp(&self, path: &Path, scan_time: u64) -> io::Result<PathBuf> {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("{}-{seq}.tmp", std::process::id()));
        let header = Header {
            magic: MAGIC,
            version: VERSION,
            root_count: self.root_count,
            entry_count: self.entries.len() as u64,
            names_len: self.names.len() as u64,
            scan_time,
            _reserved: [0; 24],
        };
        let file = File::create(&tmp)?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        w.write_all(bytemuck::bytes_of(&header))?;
        w.write_all(bytemuck::cast_slice(&self.entries))?;
        w.write_all(&self.names)?;
        w.write_all(&self.lower)?;
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        Ok(tmp)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Unique temp directory for a test.
    pub fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("finderd-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Build an index from `(path, is_dir)` pairs (Linux-style paths, parents listed or implied).
    pub fn build(paths: &[&str]) -> IndexBuilder {
        use std::collections::BTreeMap;
        // Tree: dir path -> sorted children (name, is_dir).
        let mut tree: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
        tree.insert(String::new(), BTreeMap::new());
        for p in paths {
            let is_dir = p.ends_with('/');
            let p = p.trim_end_matches('/');
            let comps: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect();
            let mut cur = String::new();
            for (k, c) in comps.iter().enumerate() {
                let last = k + 1 == comps.len();
                let dir = !last || is_dir;
                let kid_is_dir = tree.get_mut(&cur).unwrap().entry(c.to_string()).or_insert(dir);
                *kid_is_dir |= dir;
                cur = format!("{cur}/{c}");
                if dir {
                    tree.entry(cur.clone()).or_default();
                }
            }
        }
        let mut b = IndexBuilder::new();
        b.add_root(b"");
        let mut queue = vec![Some(String::new())];
        let mut i = 0;
        while i < queue.len() {
            if let Some(dir) = queue[i].clone() {
                for (name, &is_dir) in &tree[&dir] {
                    b.push(i as u32, name.as_bytes(), if is_dir { FLAG_DIR } else { 0 });
                    queue.push(is_dir.then(|| format!("{dir}/{name}")));
                }
            }
            i += 1;
        }
        b
    }

    #[test]
    fn write_open_lookup() {
        let b = build(&["/home/user/Notes.txt", "/home/user/docs/", "/etc/hosts", "/a"]);
        let dir = temp_dir("index");
        let file = dir.join("index.bin");
        b.write(&file, 42).unwrap();
        let idx = Index::open(&file).unwrap();
        let v = idx.view();
        assert_eq!(idx.scan_time(), 42);
        assert_eq!(v.len(), b.len());
        let home = v.find_child(0, b"home").unwrap();
        let user = v.find_child(home, b"user").unwrap();
        let notes = v.find_child(user, b"Notes.txt").unwrap();
        assert!(!v.is_dir(notes));
        assert!(v.is_dir(v.find_child(user, b"docs").unwrap()));
        assert_eq!(v.find_child(user, b"nope"), None);
        assert_eq!(v.path(notes), b"/home/user/Notes.txt");
        assert_eq!(v.path(0), b"/");
        assert_eq!(v.lower_name(notes), b"notes.txt");
        assert_eq!(v.entry(notes).depth, 3);
        assert_eq!(v.entry_at(v.name_start(notes) + 3), notes);
        assert!(v.children(v.find_child(0, b"a").unwrap()).is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rejects_corrupt_file() {
        let dir = temp_dir("corrupt");
        let file = dir.join("index.bin");
        build(&["/x/y"]).write(&file, 0).unwrap();
        let mut bytes = fs::read(&file).unwrap();
        bytes.pop();
        fs::write(&file, &bytes).unwrap();
        assert!(Index::open(&file).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn split_and_join() {
        let (root, comps) = split_path(b"/a//b/").unwrap();
        assert_eq!(root, b"");
        assert_eq!(comps.collect::<Vec<_>>(), [b"a", b"b"]);
        assert!(split_path(b"rel/x").is_none());
        assert_eq!(join_path([b"".as_slice(), b"a", b"b"].into_iter()), b"/a/b");
    }
}
