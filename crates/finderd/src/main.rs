//! `finderd`: background daemon that indexes all files, answers name queries and registers the
//! global hotkey that opens `finder`.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod db;
#[cfg(windows)]
mod hotkey;
#[cfg(target_os = "linux")]
#[path = "hotkey_kde.rs"]
mod hotkey;
mod index;
mod indexer;
mod migrate;
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
use std::{env, fs, io, process, thread};

use finder_core::{autostart, paths};

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
Usage: finderd [--rescan | --uninstall]

  --rescan     ignore the cached index and scan everything again
  --uninstall  stop finderd and remove everything Finder put on this system (autostart
               entry, hotkey, settings.json, index.bin); then delete the program folder
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
            "--uninstall" => {
                if let Err(e) = uninstall() {
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
        // Socket and index are private to the user.
        libc::umask(0o077);
    }
    let _ = fs::create_dir_all(paths::data_dir());

    let listener = server::bind().unwrap_or_else(|e| {
        eprintln!("finderd: cannot listen: {e}");
        // Another instance is not a failure.
        process::exit(if e.kind() == io::ErrorKind::AddrInUse { 0 } else { 1 });
    });
    migrate::remove_old_files();
    let shared = Arc::new(Shared {
        db: RwLock::new(None),
        indexing: AtomicBool::new(false),
        recent: Mutex::new(Recent::default()),
    });
    let s = shared.clone();
    thread::Builder::new().name("indexer".into()).spawn(move || indexer::run(s, rescan)).unwrap();
    hotkey::spawn();
    server::serve(listener, shared);
}

fn uninstall() -> io::Result<()> {
    if let Ok(mut c) = finder_core::proto::connect() {
        let _ = finder_core::proto::exchange(&mut c, &finder_core::proto::Request::Quit);
        println!("stopped the running finderd");
    }
    autostart::disable()?;
    println!("removed the autostart entry");
    #[cfg(target_os = "linux")]
    hotkey::unregister();
    let dir = paths::data_dir();
    for f in [paths::settings_file(), paths::index_file()] {
        match fs::remove_file(&f) {
            Ok(()) => println!("removed {}", f.display()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    indexer::remove_stale_tmp_files();
    let _ = fs::remove_dir(&dir);
    println!("Done. Delete the program folder to finish.");
    Ok(())
}

/// finderd is a GUI-subsystem program on Windows so autostart opens no console window.
#[cfg(windows)]
mod win {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};

    /// Print to the console of the shell that ran us (for `--help`, `--uninstall`, errors).
    pub fn attach_console() {
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}
