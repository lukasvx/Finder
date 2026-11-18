//! Recently opened paths (most recent first), ranked above other matches. Kept in memory only, so
//! Finder writes no file besides `settings.json` and `index.bin`; forgotten when `finderd` restarts.

use std::collections::HashSet;

use crate::db::{Db, Node};

const MAX: usize = 100;

#[derive(Default)]
pub struct Recent {
    paths: Vec<Vec<u8>>,
}

impl Recent {
    pub fn push(&mut self, path: Vec<u8>) {
        if path.is_empty() {
            return;
        }
        self.paths.retain(|p| *p != path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX);
    }

    pub fn nodes(&self, db: &Db) -> HashSet<Node> {
        self.paths.iter().filter_map(|p| db.lookup(p)).collect()
    }
}
