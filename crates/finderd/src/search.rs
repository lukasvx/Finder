//! Substring search over the name buffers, ranking, and narrowing of previous results.
//!
//! Scanning and ranking run in parallel over chunks of the index; each chunk keeps only its
//! best `limit` hits in a bounded heap, so a query allocates at most 4 bytes per match.

use std::collections::{BinaryHeap, HashSet};
use std::ops::Range;
use std::thread;

use finder_core::paths::SEP;
use memchr::memmem;

use crate::db::{Bitset, Db, Node};
use crate::fold::fold;
use crate::index::{NONE, View};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub case_sensitive: bool,
    pub full_path: bool,
}

pub struct Outcome {
    pub total: usize,
    /// Best `limit` matches, best first.
    pub hits: Vec<Node>,
}

/// Match lists larger than this are not kept for narrowing.
const CACHE_MAX: usize = 1 << 20;
/// Below this many items a scan runs on the calling thread.
const PARALLEL_MIN: usize = 1 << 16;

struct Cache {
    generation: u64,
    opts: Options,
    needle: Vec<u8>,
    base: Vec<u32>,
    delta: Vec<Node>,
}

/// Per-connection search state; remembers the last match set to narrow it when the keyword grows.
#[derive(Default)]
pub struct Searcher {
    cache: Option<Cache>,
}

/// Rank key, smaller is better: match kind (exact name, prefix, substring, path only),
/// depth, not recently opened, name length. Final ties are broken by path.
type Key = (u8, u8, bool, usize);

struct Ranker<'a> {
    db: &'a Db,
    v: View<'a>,
    needle: &'a [u8],
    opts: Options,
    recent: &'a HashSet<Node>,
}

impl Ranker<'_> {
    fn hay(&self) -> &[u8] {
        if self.opts.case_sensitive { self.v.names } else { self.v.lower }
    }

    fn base_name(&self, i: u32) -> &[u8] {
        if self.opts.case_sensitive { self.v.name(i) } else { self.v.lower_name(i) }
    }

    fn kind(&self, name: &[u8]) -> u8 {
        if name == self.needle {
            0
        } else if name.starts_with(self.needle) {
            1
        } else if !self.opts.full_path || memmem::find(name, self.needle).is_some() {
            2
        } else {
            3
        }
    }

    fn base_key(&self, i: u32) -> Key {
        let name = self.base_name(i);
        let recent = !self.recent.is_empty() && self.recent.contains(&Node::Base(i));
        (self.kind(name), self.v.entry(i).depth, !recent, name.len())
    }

    fn key(&self, n: Node) -> Key {
        match n {
            Node::Base(i) => self.base_key(i),
            Node::Delta(_) => {
                let name = name_of(self.db, n, self.opts);
                (self.kind(name), self.db.depth(n), !self.recent.contains(&n), name.len())
            }
        }
    }
}

/// The `k` smallest (key, node) pairs pushed.
struct TopK {
    k: usize,
    heap: BinaryHeap<(Key, Node)>,
}

impl TopK {
    fn new(k: usize) -> Self {
        TopK { k, heap: BinaryHeap::new() }
    }

    fn push(&mut self, key: Key, n: Node) {
        if self.heap.len() < self.k {
            self.heap.push((key, n));
        } else if self.heap.peek().is_some_and(|top| (key, n) < *top) {
            self.heap.pop();
            self.heap.push((key, n));
        }
    }

    fn merge(&mut self, other: TopK) {
        for (key, n) in other.heap {
            self.push(key, n);
        }
    }
}

/// Matches of one query: base ids (ascending), live delta nodes, and the best hits among them.
struct Found {
    base: Vec<u32>,
    delta: Vec<Node>,
    top: TopK,
}

impl Searcher {
    pub fn search(&mut self, db: &Db, query: &str, opts: Options, limit: usize, recent: &HashSet<Node>) -> Outcome {
        let raw: Vec<u8> = query.bytes().filter(|&b| b != 0).collect();
        let needle = if opts.case_sensitive { raw } else { fold(&raw) };
        let cache = self.cache.take();
        if needle.is_empty() || limit == 0 {
            return Outcome { total: 0, hits: Vec::new() };
        }
        let r = Ranker { db, v: db.view(), needle: &needle, opts, recent };
        let mut found = match cache {
            // A keyword containing the previous one matches a subset of its results.
            Some(c)
                if c.generation == db.generation()
                    && c.opts == opts
                    && !opts.full_path
                    && memmem::find(&needle, &c.needle).is_some() =>
            {
                let f = memmem::Finder::new(&needle);
                let (base, top) = rank_ids(&r, &c.base, limit, true, |i| f.find(r.base_name(i)).is_some());
                let delta = c.delta.into_iter().filter(|&n| f.find(name_of(db, n, opts)).is_some()).collect();
                Found { base, delta, top }
            }
            _ if opts.full_path => path_matches(&r, limit),
            _ => name_matches(&r, limit),
        };
        for &n in &found.delta {
            found.top.push(r.key(n), n);
        }
        let total = found.base.len() + found.delta.len();
        let mut hits: Vec<(Key, Vec<u8>, Node)> = found.top.heap.into_iter().map(|(k, n)| (k, db.path(n), n)).collect();
        hits.sort_unstable();
        if !opts.full_path && found.base.len() <= CACHE_MAX {
            self.cache = Some(Cache { generation: db.generation(), opts, needle, base: found.base, delta: found.delta });
        }
        Outcome { total, hits: hits.into_iter().map(|(_, _, n)| n).collect() }
    }
}

fn name_of(db: &Db, n: Node, opts: Options) -> &[u8] {
    if opts.case_sensitive { db.name(n) } else { db.lower_name(n) }
}

fn visible(hidden: Option<&Bitset>, i: u32) -> bool {
    hidden.is_none_or(|h| !h.get(i))
}

/// Split `0..len` into chunks processed on up to 8 threads; results in chunk order.
fn par_ranges<R: Send>(len: usize, f: impl Fn(Range<usize>) -> R + Sync) -> Vec<R> {
    let threads = if len < PARALLEL_MIN { 1 } else { thread::available_parallelism().map_or(1, |t| t.get()).min(8) };
    if threads <= 1 {
        return vec![f(0..len)];
    }
    let chunk = len.div_ceil(threads);
    thread::scope(|s| {
        let f = &f;
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let r = (t * chunk).min(len)..((t + 1) * chunk).min(len);
                s.spawn(move || f(r))
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

/// Entry whose name covers byte `pos`, searching forward from `from` (whose name starts at or before `pos`).
fn entry_from(v: View, pos: usize, from: u32) -> u32 {
    let e = v.entries;
    let (mut lo, mut hi, mut step) = (from as usize, from as usize + 1, 1);
    while hi < e.len() && e[hi].name_off as usize <= pos {
        lo = hi;
        step *= 2;
        hi += step;
    }
    let hi = hi.min(e.len());
    (lo + e[lo..hi].partition_point(|x| x.name_off as usize <= pos) - 1) as u32
}

/// Call `f(id)` once for each entry in `ids` matched by `finder`. Names are `\0`-terminated;
/// `lead` is 1 when the pattern starts with the `\0` before a name (prefix/exact patterns).
fn scan_range(v: View, hay: &[u8], finder: &memmem::Finder, lead: usize, ids: Range<u32>, mut f: impl FnMut(u32)) {
    if ids.is_empty() {
        return;
    }
    let end = v.name_start(ids.end);
    let mut pos = v.name_start(ids.start).saturating_sub(lead);
    let mut cur = ids.start;
    while pos < end {
        let Some(off) = finder.find(&hay[pos..end]) else { break };
        let id = entry_from(v, pos + off + lead, cur);
        f(id);
        cur = id + 1;
        pos = v.name_start(id + 1).saturating_sub(lead);
    }
}

/// All base entries matched by `pattern` (see `scan_range`), ascending.
fn scan_ids(r: &Ranker, pattern: &[u8], lead: usize) -> Vec<u32> {
    let finder = memmem::Finder::new(pattern);
    let hay = r.hay();
    par_ranges(r.v.len() as usize, |c| {
        let mut ids = Vec::new();
        scan_range(r.v, hay, &finder, lead, c.start as u32..c.end as u32, |id| ids.push(id));
        ids
    })
    .concat()
}

/// Rank `ids` that pass `keep`; also returns the kept ids if `collect`.
fn rank_ids(r: &Ranker, ids: &[u32], limit: usize, collect: bool, keep: impl Fn(u32) -> bool + Sync) -> (Vec<u32>, TopK) {
    let parts = par_ranges(ids.len(), |c| {
        let mut kept = Vec::new();
        let mut top = TopK::new(limit);
        for &i in &ids[c] {
            if keep(i) {
                if collect {
                    kept.push(i);
                }
                top.push(r.base_key(i), Node::Base(i));
            }
        }
        (kept, top)
    });
    let mut top = TopK::new(limit);
    let mut kept = Vec::new();
    for (k, t) in parts {
        kept.extend(k);
        top.merge(t);
    }
    (kept, top)
}

fn name_matches(r: &Ranker, limit: usize) -> Found {
    let finder = memmem::Finder::new(r.needle);
    let hay = r.hay();
    let hidden = r.db.hidden();
    let parts = par_ranges(r.v.len() as usize, |c| {
        let mut ids = Vec::new();
        let mut top = TopK::new(limit);
        scan_range(r.v, hay, &finder, 0, c.start as u32..c.end as u32, |id| {
            if visible(hidden, id) {
                ids.push(id);
                top.push(r.base_key(id), Node::Base(id));
            }
        });
        (ids, top)
    });
    let mut found = Found { base: Vec::new(), delta: Vec::new(), top: TopK::new(limit) };
    for (ids, top) in parts {
        found.base.extend(ids);
        found.top.merge(top);
    }
    found.delta = r
        .db
        .delta_nodes()
        .filter(|&n| finder.find(name_of(r.db, n, r.opts)).is_some() && r.db.is_live(n))
        .collect();
    found
}

/// Entries whose full path contains the needle. A keyword with separators is matched component
/// by component: `a/b/c` = a name ending in `a`, then a child named `b`, then one starting with `c`.
fn path_matches(r: &Ranker, limit: usize) -> Found {
    let v = r.v;
    let needle = r.needle;
    let pieces: Vec<&[u8]> = needle.split(|&b| b == b'/' || b == SEP).collect();
    let m = pieces.len() - 1;
    let trailing = m > 0 && pieces[m].is_empty();
    let mut anchor = Bitset::new(v.len());
    if m == 0 {
        for i in scan_ids(r, needle, 0) {
            anchor.set(i);
        }
    } else {
        // Anchor = entry holding the last non-empty piece (with a trailing separator: the dir before it).
        let k = if trailing { m - 1 } else { m };
        let piece_ok = |j: usize, nm: &[u8]| match j {
            0 => nm.ends_with(pieces[0]),
            j if j == m => nm.starts_with(pieces[m]),
            j => nm == pieces[j],
        };
        let candidates: Vec<u32> = if k == 0 {
            // Name ends with the piece: `piece\0`.
            if pieces[0].is_empty() { (0..v.len()).collect() } else { scan_ids(r, &[pieces[0], b"\0"].concat(), 0) }
        } else if k == m {
            // Name starts with the piece: `\0piece`.
            scan_ids(r, &[b"\0", pieces[m]].concat(), 1)
        } else {
            // Name equals the piece: `\0piece\0`.
            scan_ids(r, &[b"\0", pieces[k], b"\0"].concat(), 1)
        };
        'cand: for c in candidates {
            let mut cur = c;
            for j in (0..k).rev() {
                cur = v.entry(cur).parent;
                if cur == NONE || !piece_ok(j, r.base_name(cur)) {
                    continue 'cand;
                }
            }
            anchor.set(c);
        }
    }
    // Everything below an anchor matches; with a trailing separator the anchor itself does not.
    let hidden = r.db.hidden();
    let mut carry = Bitset::new(v.len());
    let mut base = Vec::new();
    for i in 0..v.len() {
        let p = v.entry(i).parent;
        let inherited = p != NONE && carry.get(p);
        let a = anchor.get(i);
        if a || inherited {
            carry.set(i);
            if (inherited || !trailing) && visible(hidden, i) {
                base.push(i);
            }
        }
    }
    drop((anchor, carry));
    let (_, top) = rank_ids(r, &base, limit, false, |_| true);
    let f = memmem::Finder::new(needle);
    let delta = r
        .db
        .delta_nodes()
        .filter(|&n| {
            let path = r.db.path(n);
            let path = if r.opts.case_sensitive { path } else { fold(&path) };
            f.find(&path).is_some() && r.db.is_live(n)
        })
        .collect();
    Found { base, delta, top }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::tests::db_from;
    use crate::index::FLAG_DIR;

    const TREE: &[&str] = &[
        "/home/u/Notes.txt",
        "/home/u/notes/",
        "/home/u/notes/todo.md",
        "/home/u/projects/finder/src/main.rs",
        "/home/u/projects/finder/README.md",
        "/home/u/projects/other/src/lib.rs",
        "/etc/notes.conf",
        "/usr/share/doc/mynotes",
    ];

    fn run(db: &Db, s: &mut Searcher, q: &str, cs: bool, full: bool) -> (usize, Vec<String>) {
        let out = s.search(db, q, Options { case_sensitive: cs, full_path: full }, 100, &HashSet::new());
        (out.total, out.hits.iter().map(|&n| String::from_utf8(db.path(n)).unwrap()).collect())
    }

    #[test]
    fn name_search_and_ranking() {
        let db = db_from(TREE, "search-name");
        let mut s = Searcher::default();
        let (total, hits) = run(&db, &mut s, "NOTES", false, false);
        assert_eq!(total, 4);
        assert_eq!(hits, ["/home/u/notes", "/etc/notes.conf", "/home/u/Notes.txt", "/usr/share/doc/mynotes"]);
        let (total, hits) = run(&db, &mut s, "Notes", true, false);
        assert_eq!((total, hits), (1, vec!["/home/u/Notes.txt".to_string()]));
        assert_eq!(run(&db, &mut s, "", false, false).0, 0);
        assert_eq!(run(&db, &mut s, "zzz", false, false).0, 0);
    }

    #[test]
    fn narrowing_matches_fresh_search() {
        let db = db_from(TREE, "search-narrow");
        let mut s = Searcher::default();
        for q in ["o", "ot", "note", "notes.", "notes.c", "x"] {
            let narrowed = run(&db, &mut s, q, false, false);
            let fresh = run(&db, &mut Searcher::default(), q, false, false);
            assert_eq!(narrowed, fresh, "query {q}");
        }
    }

    #[test]
    fn full_path_search() {
        let db = db_from(TREE, "search-path");
        let mut s = Searcher::default();
        let (_, hits) = run(&db, &mut s, "jects/finder/s", false, true);
        assert_eq!(hits, ["/home/u/projects/finder/src", "/home/u/projects/finder/src/main.rs"]);
        let (_, hits) = run(&db, &mut s, "src/", false, true);
        assert_eq!(hits, ["/home/u/projects/other/src/lib.rs", "/home/u/projects/finder/src/main.rs"]);
        let (total, _) = run(&db, &mut s, "/home/u/projects", false, true);
        assert_eq!(total, 8);
        let (_, hits) = run(&db, &mut s, "other", false, true);
        assert_eq!(hits, ["/home/u/projects/other", "/home/u/projects/other/src", "/home/u/projects/other/src/lib.rs"]);
        assert_eq!(run(&db, &mut s, "u/notes/t", false, true).1, ["/home/u/notes/todo.md"]);
    }

    #[test]
    fn sees_delta_changes() {
        let mut db = db_from(TREE, "search-delta");
        let mut s = Searcher::default();
        assert_eq!(run(&db, &mut s, "note", false, false).0, 4);
        db.remove(b"/home/u/notes");
        db.insert(b"/home/u/new", FLAG_DIR).unwrap();
        db.insert(b"/home/u/new/notes2", 0).unwrap();
        let (total, hits) = run(&db, &mut s, "notes", false, false);
        assert_eq!(total, 4);
        assert!(hits.contains(&"/home/u/new/notes2".to_string()));
        assert!(!hits.contains(&"/home/u/notes".to_string()));
        let (_, hits) = run(&db, &mut s, "u/new/", false, true);
        assert_eq!(hits, ["/home/u/new/notes2"]);
        assert_eq!(run(&db, &mut s, "todo", false, true).0, 0);
    }

    /// Large enough to run in parallel chunks; compares against a naive scan of full paths.
    #[test]
    fn matches_naive_on_large_tree() {
        use crate::index::{Index, IndexBuilder};
        let parts = [b"Ab".as_slice(), b"a", b"ab", b"abc", b"b", b"ba", b"c"]; // sorted
        let mut b = IndexBuilder::new();
        b.add_root(b"");
        let mut i = 0;
        while b.len() < 150_000 {
            if b.view().is_dir(i) {
                let depth = b.view().entry(i).depth;
                for (k, p) in parts.iter().enumerate() {
                    let dir = depth < 7 && (k + i as usize) % 3 != 0;
                    b.push(i, &[*p, &[b'0' + (i % 7) as u8]].concat(), if dir { FLAG_DIR } else { 0 });
                }
            }
            i += 1;
        }
        let dir = crate::index::tests::temp_dir("search-large");
        let file = dir.join("index.bin");
        b.write(&file, 0).unwrap();
        let db = Db::new(Index::open(&file).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
        let v = db.view();
        let paths: Vec<Vec<u8>> = (0..v.len()).map(|i| v.path(i)).collect();
        for (q, full) in [("ab", false), ("c3", false), ("a", true), ("ab1/b", true), ("/a", true), ("c2/", true), ("b2/ba", true)] {
            let out = Searcher::default().search(&db, q, Options { case_sensitive: false, full_path: full }, usize::MAX, &HashSet::new());
            let mut got: Vec<u32> = out.hits.iter().map(|&n| match n { Node::Base(i) => i, Node::Delta(_) => unreachable!() }).collect();
            got.sort();
            let needle = q.to_lowercase();
            let want: Vec<u32> = (0..v.len())
                .filter(|&i| {
                    let hay = if full { fold(&paths[i as usize]) } else { v.lower_name(i).to_vec() };
                    memmem::find(&hay, needle.as_bytes()).is_some()
                })
                .collect();
            assert_eq!(out.total, want.len(), "query {q}");
            assert_eq!(got, want, "query {q}");
        }
    }

    #[test]
    fn limit_keeps_best() {
        let db = db_from(TREE, "search-limit");
        let out = Searcher::default().search(&db, "s", Options::default(), 2, &HashSet::new());
        assert!(out.total > 2);
        assert_eq!(out.hits.len(), 2);
        assert_eq!(db.path(out.hits[0]), b"/usr/share");
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::index::{Index, IndexBuilder};
    use std::time::Instant;

    fn rss_anon_kb() -> u64 {
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        s.lines().find(|l| l.starts_with("RssAnon:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()).unwrap_or(0)
    }

    /// Five copies of the real cached index under `/copyN`: ~2M entries.
    /// `cargo test --release -p finderd -- --ignored --nocapture search_2m`
    #[test]
    #[ignore]
    fn search_2m() {
        let real = Index::open(&finder_core::paths::index_file()).unwrap();
        let v = real.view();
        let mut b = IndexBuilder::new();
        b.add_root(b"");
        let mut queue: Vec<(u32, u32)> = Vec::new();
        for k in 0..5 {
            let id = b.push(0, format!("copy{k}").as_bytes(), crate::index::FLAG_DIR);
            queue.push((0, id));
        }
        let mut i = 0;
        while i < queue.len() {
            let (orig, new) = queue[i];
            for c in v.children(orig) {
                let id = b.push(new, v.name(c), v.entry(c).flags);
                if v.is_dir(c) {
                    queue.push((c, id));
                }
            }
            i += 1;
        }
        drop(queue);
        let file = std::env::temp_dir().join("finderd-bench-2m.bin");
        b.write(&file, 0).unwrap();
        let n = b.len();
        drop(b);
        let size = std::fs::metadata(&file).unwrap().len();
        let db = Db::new(Index::open(&file).unwrap());
        eprintln!("{n} entries, {} MB file, {:.1} B/entry", size >> 20, size as f64 / n as f64);
        let anon0 = rss_anon_kb();
        for (q, full) in [("a", false), ("e", false), ("mai", false), ("main.rs", false), ("readme", false),
                          ("zzzzqq", false), ("src/", true), ("finder/c", true), ("lib", false), ("README.md", false)] {
            let mut times = Vec::new();
            let mut total = 0;
            for _ in 0..5 {
                let t = Instant::now();
                let out = Searcher::default().search(&db, q, Options { case_sensitive: false, full_path: full }, 1000, &HashSet::new());
                times.push(t.elapsed());
                total = out.total;
            }
            times.sort();
            eprintln!("{q:>10} full={full:<5} median {:>7.2?}  total {total}", times[2]);
        }
        // Typing "readme" one key at a time on one connection (narrowing).
        let mut s = Searcher::default();
        let t = Instant::now();
        for k in 1..=6 {
            s.search(&db, &"readme"[..k], Options::default(), 1000, &HashSet::new());
        }
        eprintln!("typing 'readme' (6 queries, narrowing): {:?}", t.elapsed());
        eprintln!("anon RSS grew {} MB during queries (incl. narrowing cache)", (rss_anon_kb().saturating_sub(anon0)) >> 10);
        std::fs::remove_file(&file).unwrap();
    }
}
