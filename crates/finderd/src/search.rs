//! Substring search over the name buffers, ranking, and narrowing of previous results.

use std::collections::HashSet;
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

struct Cache {
    generation: u64,
    opts: Options,
    needle: Vec<u8>,
    matches: Vec<Node>,
}

/// Per-connection search state; remembers the last match set to narrow it when the keyword grows.
#[derive(Default)]
pub struct Searcher {
    cache: Option<Cache>,
}

impl Searcher {
    pub fn search(&mut self, db: &Db, query: &str, opts: Options, limit: usize, recent: &HashSet<Node>) -> Outcome {
        let raw: Vec<u8> = query.bytes().filter(|&b| b != 0).collect();
        let needle = if opts.case_sensitive { raw } else { fold(&raw) };
        if needle.is_empty() {
            self.cache = None;
            return Outcome { total: 0, hits: Vec::new() };
        }
        let matches = match self.cache.take() {
            // A keyword containing the previous one matches a subset of its results.
            Some(c)
                if c.generation == db.generation()
                    && c.opts == opts
                    && !opts.full_path
                    && memmem::find(&needle, &c.needle).is_some() =>
            {
                let f = memmem::Finder::new(&needle);
                c.matches.into_iter().filter(|&n| f.find(name_of(db, n, opts)).is_some()).collect()
            }
            _ if opts.full_path => path_matches(db, &needle, opts),
            _ => name_matches(db, &needle, opts),
        };
        let total = matches.len();
        let hits = rank(db, &matches, &needle, opts, limit, recent);
        self.cache = Some(Cache { generation: db.generation(), opts, needle, matches });
        Outcome { total, hits }
    }
}

fn name_of(db: &Db, n: Node, opts: Options) -> &[u8] {
    if opts.case_sensitive { db.name(n) } else { db.lower_name(n) }
}

fn visible(hidden: Option<&Bitset>, i: u32) -> bool {
    hidden.is_none_or(|h| !h.get(i))
}

/// Ids of base entries whose name in `hay` contains `needle`, ascending. Runs in parallel.
pub fn scan(v: View, hay: &[u8], needle: &[u8]) -> Vec<u32> {
    let n = v.len();
    let threads = thread::available_parallelism().map_or(1, |t| t.get()).min(8) as u32;
    if threads <= 1 || hay.len() < (1 << 20) {
        return scan_range(v, hay, needle, 0..n);
    }
    let chunk = n.div_ceil(threads);
    thread::scope(|s| {
        let handles: Vec<_> = (0..threads)
            .map(|t| {
                let ids = (t * chunk).min(n)..((t + 1) * chunk).min(n);
                s.spawn(move || scan_range(v, hay, needle, ids))
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

fn scan_range(v: View, hay: &[u8], needle: &[u8], ids: Range<u32>) -> Vec<u32> {
    let mut out = Vec::new();
    if ids.is_empty() {
        return out;
    }
    let finder = memmem::Finder::new(needle);
    let end = v.name_start(ids.end);
    let mut pos = v.name_start(ids.start);
    // Names are `\0`-terminated and the needle has no `\0`, so a match lies within one name.
    while pos < end {
        let Some(off) = finder.find(&hay[pos..end]) else { break };
        let id = v.entry_at(pos + off);
        out.push(id);
        pos = v.name_start(id + 1);
    }
    out
}

fn name_matches(db: &Db, needle: &[u8], opts: Options) -> Vec<Node> {
    let v = db.view();
    let hay = if opts.case_sensitive { v.names } else { v.lower };
    let hidden = db.hidden();
    let mut out: Vec<Node> = scan(v, hay, needle).into_iter().filter(|&i| visible(hidden, i)).map(Node::Base).collect();
    let f = memmem::Finder::new(needle);
    out.extend(db.delta_nodes().filter(|&n| f.find(name_of(db, n, opts)).is_some() && db.is_live(n)));
    out
}

/// Entries whose full path contains `needle`. A keyword with separators is matched component
/// by component: `a/b/c` = a name ending in `a`, then a child named `b`, then one starting with `c`.
fn path_matches(db: &Db, needle: &[u8], opts: Options) -> Vec<Node> {
    let v = db.view();
    let cs = opts.case_sensitive;
    let hay = if cs { v.names } else { v.lower };
    let name = |i: u32| if cs { v.name(i) } else { v.lower_name(i) };
    let pieces: Vec<&[u8]> = needle.split(|&b| b == b'/' || b == SEP).collect();
    let m = pieces.len() - 1;
    let trailing = m > 0 && pieces[m].is_empty();
    let mut anchor = Bitset::new(v.len());
    if m == 0 {
        for i in scan(v, hay, needle) {
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
        let candidates: Vec<u32> = if pieces[k].is_empty() { (0..v.len()).collect() } else { scan(v, hay, pieces[k]) };
        'cand: for c in candidates {
            if !piece_ok(k, name(c)) {
                continue;
            }
            let mut cur = c;
            for j in (0..k).rev() {
                cur = v.entry(cur).parent;
                if cur == NONE || !piece_ok(j, name(cur)) {
                    continue 'cand;
                }
            }
            anchor.set(c);
        }
    }
    // Everything below an anchor matches; with a trailing separator the anchor itself does not.
    let hidden = db.hidden();
    let mut carry = Bitset::new(v.len());
    let mut out = Vec::new();
    for i in 0..v.len() {
        let p = v.entry(i).parent;
        let inherited = p != NONE && carry.get(p);
        let a = anchor.get(i);
        if a || inherited {
            carry.set(i);
            if (inherited || !trailing) && visible(hidden, i) {
                out.push(Node::Base(i));
            }
        }
    }
    let f = memmem::Finder::new(needle);
    for n in db.delta_nodes() {
        let path = db.path(n);
        let path = if cs { path } else { fold(&path) };
        if f.find(&path).is_some() && db.is_live(n) {
            out.push(n);
        }
    }
    out
}

/// Best `limit` matches: exact name > prefix > substring, then shallower, then recently opened,
/// then shorter name, then path.
fn rank(db: &Db, matches: &[Node], needle: &[u8], opts: Options, limit: usize, recent: &HashSet<Node>) -> Vec<Node> {
    let key = |n: Node| {
        let name = name_of(db, n, opts);
        let kind = if name == needle {
            0u8
        } else if name.starts_with(needle) {
            1
        } else if !opts.full_path || memmem::find(name, needle).is_some() {
            2
        } else {
            3
        };
        (kind, db.depth(n), !recent.contains(&n), name.len())
    };
    let mut keyed: Vec<_> = matches.iter().map(|&n| (key(n), n)).collect();
    if keyed.len() > limit {
        keyed.select_nth_unstable(limit);
        keyed.truncate(limit);
    }
    let mut with_path: Vec<_> = keyed.into_iter().map(|(k, n)| (k, db.path(n), n)).collect();
    with_path.sort_unstable();
    with_path.into_iter().map(|(_, _, n)| n).collect()
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

    #[test]
    fn limit_keeps_best() {
        let db = db_from(TREE, "search-limit");
        let out = Searcher::default().search(&db, "s", Options::default(), 2, &HashSet::new());
        assert!(out.total > 2);
        assert_eq!(out.hits.len(), 2);
        assert_eq!(db.path(out.hits[0]), b"/usr/share");
    }
}
