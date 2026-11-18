//! Live database: the memory-mapped base index plus an in-memory delta of changes.
//!
//! Removals of base entries are recorded by id (hiding the whole subtree); additions are
//! delta nodes whose parent is a base or delta node. `compact` merges both into a new index.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::index::{FLAG_DIR, Index, IndexBuilder, NONE, View, join_path, split_path};
use finder_core::fold::fold;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    Base(u32),
    Delta(u32),
}

struct DeltaNode {
    parent: Node,
    name: Box<[u8]>,
    lower: Box<[u8]>,
    flags: u8,
    depth: u8,
    alive: bool,
}

/// Fixed-size bitset over base entry ids.
pub struct Bitset(Vec<u64>);

impl Bitset {
    pub fn new(n: u32) -> Self {
        Bitset(vec![0; (n as usize).div_ceil(64)])
    }
    pub fn get(&self, i: u32) -> bool {
        self.0[i as usize / 64] >> (i % 64) & 1 != 0
    }
    pub fn set(&mut self, i: u32) {
        self.0[i as usize / 64] |= 1 << (i % 64);
    }
}

/// Source of generation numbers, unique across `Db` instances (compaction creates a new one).
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn next_generation() -> u64 {
    GENERATION.fetch_add(1, Ordering::Relaxed) + 1
}

pub struct Db {
    base: Index,
    removed: HashSet<u32>,
    nodes: Vec<DeltaNode>,
    /// Live delta children of each node.
    kids: HashMap<Node, Vec<u32>>,
    /// Base entries hidden by a removed ancestor; computed lazily, reset on change.
    hidden: OnceLock<Bitset>,
    generation: u64,
    /// Identifies the base index; changes only when a new `Db` is created (load or compaction).
    base_id: u64,
}

impl Db {
    pub fn new(base: Index) -> Db {
        Db {
            base,
            removed: HashSet::new(),
            nodes: Vec::new(),
            kids: HashMap::new(),
            hidden: OnceLock::new(),
            generation: next_generation(),
            base_id: next_generation(),
        }
    }

    pub fn view(&self) -> View<'_> {
        self.base.view()
    }

    pub fn base(&self) -> &Index {
        &self.base
    }

    /// Changes since the base index was written (added nodes + removed base entries).
    pub fn delta_len(&self) -> usize {
        self.nodes.len() + self.removed.len()
    }

    /// Bumped on every change; used to invalidate cached search results.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn base_id(&self) -> u64 {
        self.base_id
    }

    fn changed(&mut self) {
        self.generation = next_generation();
        self.hidden = OnceLock::new();
    }

    pub fn parent(&self, n: Node) -> Option<Node> {
        match n {
            Node::Base(b) => Some(self.view().entry(b).parent).filter(|&p| p != NONE).map(Node::Base),
            Node::Delta(d) => Some(self.nodes[d as usize].parent),
        }
    }

    pub fn name(&self, n: Node) -> &[u8] {
        match n {
            Node::Base(b) => self.view().name(b),
            Node::Delta(d) => &self.nodes[d as usize].name,
        }
    }

    pub fn lower_name(&self, n: Node) -> &[u8] {
        match n {
            Node::Base(b) => self.view().lower_name(b),
            Node::Delta(d) => &self.nodes[d as usize].lower,
        }
    }

    pub fn flags(&self, n: Node) -> u8 {
        match n {
            Node::Base(b) => self.view().entry(b).flags,
            Node::Delta(d) => self.nodes[d as usize].flags,
        }
    }

    pub fn is_dir(&self, n: Node) -> bool {
        self.flags(n) & FLAG_DIR != 0
    }

    pub fn depth(&self, n: Node) -> u8 {
        match n {
            Node::Base(b) => self.view().entry(b).depth,
            Node::Delta(d) => self.nodes[d as usize].depth,
        }
    }

    pub fn path(&self, n: Node) -> Vec<u8> {
        let mut chain = vec![n];
        let mut cur = n;
        while let Some(p) = self.parent(cur) {
            chain.push(p);
            cur = p;
        }
        join_path(chain.iter().rev().map(|&c| self.name(c)))
    }

    /// Base entries hidden because they or an ancestor were removed; `None` if nothing was removed.
    pub fn hidden(&self) -> Option<&Bitset> {
        if self.removed.is_empty() {
            return None;
        }
        Some(self.hidden.get_or_init(|| {
            let v = self.view();
            let mut bits = Bitset::new(v.len());
            for &r in &self.removed {
                bits.set(r);
            }
            for i in v.root_count..v.len() {
                if bits.get(v.entry(i).parent) {
                    bits.set(i);
                }
            }
            bits
        }))
    }

    /// Whether `n` is still present (not removed itself or via an ancestor).
    pub fn is_live(&self, n: Node) -> bool {
        let mut cur = Some(n);
        while let Some(c) = cur {
            match c {
                Node::Base(b) => return self.hidden().is_none_or(|h| !h.get(b)),
                Node::Delta(d) if !self.nodes[d as usize].alive => return false,
                Node::Delta(_) => {}
            }
            cur = self.parent(c);
        }
        true
    }

    /// Live delta nodes (including those under a removed base dir; filter with `is_live`).
    pub fn delta_nodes(&self) -> impl Iterator<Item = Node> + '_ {
        self.nodes.iter().enumerate().filter(|(_, n)| n.alive).map(|(i, _)| Node::Delta(i as u32))
    }

    /// Direct child of `dir` named `name`.
    pub fn child(&self, dir: Node, name: &[u8]) -> Option<Node> {
        if let Some(kids) = self.kids.get(&dir)
            && let Some(&d) = kids.iter().find(|&&d| &*self.nodes[d as usize].name == name)
        {
            return Some(Node::Delta(d));
        }
        match dir {
            Node::Base(b) => self.view().find_child(b, name).filter(|c| !self.removed.contains(c)).map(Node::Base),
            Node::Delta(_) => None,
        }
    }

    /// Children of a directory: base children (not removed) and delta children.
    pub fn children(&self, dir: Node) -> Vec<Node> {
        let mut out = Vec::new();
        if let Node::Base(b) = dir {
            out.extend(self.view().children(b).filter(|c| !self.removed.contains(c)).map(Node::Base));
        }
        if let Some(kids) = self.kids.get(&dir) {
            out.extend(kids.iter().map(|&d| Node::Delta(d)));
        }
        out
    }

    pub fn lookup(&self, path: &[u8]) -> Option<Node> {
        let (root, comps) = split_path(path)?;
        let mut cur = Node::Base(self.view().find_root(root)?);
        for c in comps {
            cur = self.child(cur, c).or_else(|| self.child_ignore_case(cur, c))?;
        }
        Some(cur)
    }

    /// Windows names are case-insensitive: a path may arrive in a different case than on disk.
    #[cfg(windows)]
    fn child_ignore_case(&self, dir: Node, name: &[u8]) -> Option<Node> {
        let want = fold(name);
        self.children(dir).into_iter().find(|&c| self.lower_name(c) == want.as_slice())
    }

    #[cfg(not(windows))]
    fn child_ignore_case(&self, _: Node, _: &[u8]) -> Option<Node> {
        None
    }

    /// Add `name` with `flags` under `parent` (a live directory). Replaces an existing child
    /// whose dir flag differs; otherwise returns the existing child unchanged.
    pub fn insert_child(&mut self, parent: Node, name: &[u8], flags: u8) -> Node {
        if let Some(existing) = self.child(parent, name) {
            if (self.flags(existing) ^ flags) & FLAG_DIR == 0 {
                return existing;
            }
            self.remove_node(existing);
        }
        let id = self.nodes.len() as u32;
        self.nodes.push(DeltaNode {
            parent,
            name: name.into(),
            lower: fold(name).into(),
            flags,
            depth: self.depth(parent).saturating_add(1),
            alive: true,
        });
        self.kids.entry(parent).or_default().push(id);
        self.changed();
        Node::Delta(id)
    }

    /// Add a path whose parent exists. Returns `None` if the parent is missing or not a directory.
    pub fn insert(&mut self, path: &[u8], flags: u8) -> Option<Node> {
        let (parent, name) = split_last(path)?;
        let parent = self.lookup(parent)?;
        if !self.is_dir(parent) {
            return None;
        }
        Some(self.insert_child(parent, name, flags))
    }

    /// Remove a node and its subtree. Roots cannot be removed; returns whether anything changed.
    pub fn remove_node(&mut self, n: Node) -> bool {
        match n {
            Node::Base(b) if b < self.view().root_count => return false,
            Node::Base(b) => {
                self.removed.insert(b);
            }
            Node::Delta(d) => {
                let node = &mut self.nodes[d as usize];
                node.alive = false;
                let parent = node.parent;
                if let Some(kids) = self.kids.get_mut(&parent) {
                    kids.retain(|&k| k != d);
                }
                self.kids.remove(&n);
            }
        }
        self.changed();
        true
    }

    /// Remove a path and everything below it. Returns whether it existed.
    pub fn remove(&mut self, path: &[u8]) -> bool {
        self.lookup(path).is_some_and(|n| self.remove_node(n))
    }

    /// Merge base and delta into a fresh index (BFS order, children sorted by name).
    pub fn compact(&self) -> IndexBuilder {
        let v = self.view();
        let mut b = IndexBuilder::new();
        let mut src: Vec<Node> = Vec::with_capacity(v.len() as usize + self.nodes.len());
        for r in 0..v.root_count {
            b.add_root(v.name(r));
            src.push(Node::Base(r));
        }
        let mut kids: Vec<Node> = Vec::new();
        let mut i = 0;
        while i < src.len() {
            let s = src[i];
            if self.is_dir(s) {
                kids.clear();
                kids.extend(self.children(s));
                if self.kids.contains_key(&s) {
                    kids.sort_by(|&a, &b| self.name(a).cmp(self.name(b)));
                }
                for &k in &kids {
                    b.push(i as u32, self.name(k), self.flags(k));
                    src.push(k);
                }
            }
            i += 1;
        }
        b
    }
}

/// Split a path into (parent path, last component).
pub fn split_last(path: &[u8]) -> Option<(&[u8], &[u8])> {
    let is_sep = |b: &u8| *b == b'/' || *b == finder_core::paths::SEP;
    let trimmed = &path[..path.len() - path.iter().rev().take_while(|b| is_sep(b)).count()];
    let pos = trimmed.iter().rposition(is_sep)?;
    let name = &trimmed[pos + 1..];
    if name.is_empty() {
        return None;
    }
    let parent = if pos == 0 || (cfg!(windows) && pos == 2) { &trimmed[..pos + 1] } else { &trimmed[..pos] };
    Some((parent, name))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::index::tests::{build, temp_dir};

    pub fn db_from(paths: &[&str], name: &str) -> Db {
        let dir = temp_dir(name);
        let file = dir.join("index.bin");
        build(paths).write(&file, 0).unwrap();
        let idx = Index::open(&file).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        Db::new(idx)
    }

    fn all_paths(db: &Db) -> Vec<String> {
        let b = db.compact();
        let v = b.view();
        let mut out: Vec<String> = (0..v.len()).map(|i| String::from_utf8(v.path(i)).unwrap()).collect();
        out.sort();
        out
    }

    #[test]
    fn split_last_works() {
        assert_eq!(split_last(b"/a/b"), Some((b"/a".as_slice(), b"b".as_slice())));
        assert_eq!(split_last(b"/a/"), Some((b"/".as_slice(), b"a".as_slice())));
        assert_eq!(split_last(b"/"), None);
    }

    #[test]
    fn insert_remove_lookup_compact() {
        let mut db = db_from(&["/home/u/a.txt", "/home/u/src/", "/etc/x"], "db");
        assert!(db.lookup(b"/home/u/a.txt").is_some());
        assert!(db.lookup(b"/home/u/zzz").is_none());

        // Add a dir and a file inside it.
        let d = db.insert(b"/home/u/new", FLAG_DIR).unwrap();
        assert!(matches!(d, Node::Delta(_)));
        db.insert(b"/home/u/new/inner.rs", 0).unwrap();
        assert!(db.insert(b"/missing/x", 0).is_none());
        assert!(db.insert(b"/home/u/a.txt/x", 0).is_none());
        assert_eq!(db.insert(b"/home/u/a.txt", 0), db.lookup(b"/home/u/a.txt"));
        assert_eq!(db.path(db.lookup(b"/home/u/new/inner.rs").unwrap()), b"/home/u/new/inner.rs");
        assert_eq!(db.depth(db.lookup(b"/home/u/new/inner.rs").unwrap()), 4);

        // Remove a base dir: subtree hidden.
        let a = db.lookup(b"/home/u/a.txt").unwrap();
        assert!(db.remove(b"/home/u"));
        assert!(db.lookup(b"/home/u/a.txt").is_none());
        assert!(!db.is_live(a));
        assert!(!db.is_live(d));
        assert!(db.is_live(db.lookup(b"/etc/x").unwrap()));

        // Re-create it.
        db.insert(b"/home/u", FLAG_DIR).unwrap();
        db.insert(b"/home/u/b.txt", 0).unwrap();
        assert_eq!(all_paths(&db), ["/", "/etc", "/etc/x", "/home", "/home/u", "/home/u/b.txt"]);

        // Removing a delta node; file replaced by dir.
        assert!(db.remove(b"/home/u/b.txt"));
        db.insert(b"/etc/x", FLAG_DIR).unwrap();
        assert!(db.is_dir(db.lookup(b"/etc/x").unwrap()));
        assert_eq!(all_paths(&db), ["/", "/etc", "/etc/x", "/home", "/home/u"]);
        assert!(!db.remove(b"/"));
    }

    #[test]
    fn compact_roundtrip_sorted() {
        let mut db = db_from(&["/b/", "/d/"], "compact");
        db.insert(b"/c", 0).unwrap();
        db.insert(b"/a", FLAG_DIR).unwrap();
        db.insert(b"/a/z", 0).unwrap();
        let b = db.compact();
        let v = b.view();
        let names: Vec<&[u8]> = (0..v.len()).map(|i| v.name(i)).collect();
        assert_eq!(names, [b"".as_slice(), b"a", b"b", b"c", b"d", b"z"]);
        assert_eq!(v.path(5), b"/a/z");
    }
}
