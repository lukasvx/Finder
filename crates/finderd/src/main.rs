//! `finderd`: background daemon that indexes all files and answers name queries.

mod db;
mod fold;
mod index;
mod indexer;
mod install;
mod recent;
mod scan;
mod search;
mod server;
#[cfg(target_os = "linux")]
mod watch;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};
use std::{env, process, thread};

use finder_core::paths;

use crate::db::Db;
use crate::recent::Recent;

/// State shared between the indexer thread and connection threads.
pub struct Shared {
    /// `None` until the first index is loaded or scanned.
    pub db: RwLock<Option<Db>>,
    /// A full scan is in progress.
    pub indexing: AtomicBool,
    pub recent: Mutex<Recent>,
}

const USAGE: &str = "\
Usage: finderd [--rescan | --install | --uninstall]

  --rescan     ignore the cached index and scan everything again
  --install    start finderd at login and add the Finder app launcher
  --uninstall  undo --install
";

fn main() {
    let mut rescan = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--rescan" => rescan = true,
            "--install" | "--uninstall" => {
                let r = if arg == "--install" { install::install() } else { install::uninstall() };
                if let Err(e) = r {
                    eprintln!("finderd: {e}");
                    process::exit(1);
                }
                return;
            }
            "-h" | "--help" => {
                print!("{USAGE}");
                return;
            }
            _ => {
                eprint!("finderd: unknown argument `{arg}`\n\n{USAGE}");
                process::exit(2);
            }
        }
    }
    #[cfg(unix)]
    unsafe {
        // Socket and cache files are private to the user.
        libc::umask(0o077);
    }
    let _ = std::fs::create_dir_all(paths::cache_dir());

    #[cfg(unix)]
    {
        let listener = server::bind().unwrap_or_else(|e| {
            eprintln!("finderd: {}: {e}", paths::socket_path().display());
            // Another instance is not a failure (keeps systemd from restarting us).
            process::exit(if e.kind() == std::io::ErrorKind::AddrInUse { 0 } else { 1 });
        });
        let shared = Arc::new(Shared {
            db: RwLock::new(None),
            indexing: AtomicBool::new(false),
            recent: Mutex::new(Recent::load(paths::recent_file())),
        });
        let s = shared.clone();
        thread::Builder::new().name("indexer".into()).spawn(move || indexer::run(s, rescan)).unwrap();
        server::serve(listener, shared);
    }
    #[cfg(windows)]
    {
        let _ = (rescan, Arc::new(()), thread::current());
        eprintln!("finderd: Windows is not supported yet");
        process::exit(1);
    }
}
