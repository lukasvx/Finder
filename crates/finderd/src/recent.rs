//! Recently opened paths (most recent first), persisted as one path per line.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use crate::db::{Db, Node};

const MAX: usize = 100;

pub struct Recent {
    file: PathBuf,
    paths: Vec<Vec<u8>>,
}

impl Recent {
    pub fn load(file: PathBuf) -> Recent {
        let paths = fs::read(&file)
            .map(|b| b.split(|&c| c == b'\n').filter(|l| !l.is_empty()).take(MAX).map(<[u8]>::to_vec).collect())
            .unwrap_or_default();
        Recent { file, paths }
    }

    pub fn push(&mut self, path: Vec<u8>) {
        if path.is_empty() || path.contains(&b'\n') {
            return;
        }
        self.paths.retain(|p| *p != path);
        self.paths.insert(0, path);
        self.paths.truncate(MAX);
        let mut out = self.paths.join(&b'\n');
        out.push(b'\n');
        if let Err(e) = fs::write(&self.file, out) {
            eprintln!("finderd: cannot write {}: {e}", self.file.display());
        }
    }

    pub fn nodes(&self, db: &Db) -> HashSet<Node> {
        self.paths.iter().filter_map(|p| db.lookup(p)).collect()
    }
}
