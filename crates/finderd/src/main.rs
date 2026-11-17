//! `finderd`: background daemon that indexes all files and answers name queries.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod db;
mod index;
mod indexer;
mod install;
mod recent;
mod scan;
mod search;
mod server;
#[cfg(target_os = "linux")]
mod watch;
#[cfg(windows)]
#[path = "watch_windows.rs"]
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
    #[cfg(windows)]
    if env::args_os().len() > 1 {
        win::attach_console();
    }
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
    #[cfg(windows)]
    win::log_to_file_if_detached();

    let listener = server::bind().unwrap_or_else(|e| {
        eprintln!("finderd: cannot listen: {e}");
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

/// finderd is a GUI-subsystem program on Windows so autostart opens no console window.
#[cfg(windows)]
mod win {
    use std::os::windows::io::IntoRawHandle;

    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, SetStdHandle,
    };

    /// Print to the console of the shell that ran us (for `--help`, `--install`, errors).
    pub fn attach_console() {
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }

    /// Without a stderr handle (started at login, not by `finder`), log to `finderd.log`.
    pub fn log_to_file_if_detached() {
        if !unsafe { GetStdHandle(STD_ERROR_HANDLE) }.is_null() {
            return;
        }
        let log = finder_core::paths::cache_dir().join("finderd.log");
        if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
            unsafe { SetStdHandle(STD_ERROR_HANDLE, f.into_raw_handle()) };
        }
    }
}
