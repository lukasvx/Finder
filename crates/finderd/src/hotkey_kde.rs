//! Linux (KDE Plasma): the `hotkey` setting as a KDE global shortcut, registered with
//! kglobalaccel over D-Bus; pressing it starts `finder`. `settings.json` is the source of truth:
//! the key is set (overriding System Settings) at start and whenever the file changes.
//!
//! Other desktops have no such service; there, bind `finder` in the desktop's shortcut settings.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use dbus::blocking::{Connection, Proxy};
use dbus::channel::{BusType, Channel};
use dbus::message::MatchRule;
use finder_core::hotkey;
use finder_core::paths;
use finder_core::settings::Settings;

const COMPONENT: &str = "finder";
const ACTION: &str = "open";
const TIMEOUT: Duration = Duration::from_secs(5);
/// kglobalaccel `setShortcut` flags: SetPresent (we are running) | NoAutoloading (use the
/// given key, not the one stored in KDE's config).
const SET_PRESENT_NO_AUTOLOADING: u32 = 2 | 4;

/// [component, action, component display name, action display name].
fn action_id() -> Vec<&'static str> {
    vec![COMPONENT, ACTION, "Finder", "Open Finder"]
}

fn kglobalaccel(conn: &Connection) -> Proxy<'_, &Connection> {
    conn.with_proxy("org.kde.kglobalaccel", "/kglobalaccel", TIMEOUT)
}

pub fn spawn() {
    let r = thread::Builder::new().name("hotkey".into()).spawn(|| {
        if let Err(e) = run() {
            eprintln!("finderd: hotkey: {e}");
        }
    });
    if let Err(e) = r {
        eprintln!("finderd: cannot start hotkey thread: {e}");
    }
}

/// Remove the shortcut from KDE (`finderd --uninstall`).
pub fn unregister() {
    let Ok(conn) = Connection::new_session() else { return };
    let r: Result<(bool,), _> =
        kglobalaccel(&conn).method_call("org.kde.KGlobalAccel", "unregister", (COMPONENT, ACTION));
    if let Ok((true,)) = r {
        println!("removed the KDE global shortcut");
    }
}

/// The hotkey in `settings.json`, as Qt key codes (empty: none).
fn wanted() -> Vec<i32> {
    let s = Settings::load().map(|(s, _)| s).unwrap_or_default();
    hotkey::parse(&s.hotkey).map(|h| h.qt()).into_iter().collect()
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    // A channel with watch tracking, so its socket can be polled together with inotify.
    let mut channel = Channel::get_private(BusType::Session)?;
    channel.set_watch_enabled(true);
    let conn = Connection::from(channel);
    let kga = kglobalaccel(&conn);
    if let Err(e) = kga.method_call::<(), _, _, _>("org.kde.KGlobalAccel", "doRegister", (action_id(),)) {
        // Not KDE Plasma (or no session bus): nothing to do here.
        eprintln!("finderd: no KDE global shortcuts ({e}); bind `finder` in the desktop's shortcut settings");
        return Ok(());
    }
    let rule = MatchRule::new_signal("org.kde.kglobalaccel.Component", "globalShortcutPressed")
        .with_path(format!("/component/{COMPONENT}"));
    conn.add_match(rule, |(_, action, _): (String, String, i64), _, _| {
        if action == ACTION {
            launch();
        }
        true
    })?;

    let ino = watch_settings()?;
    let mut current = None;
    loop {
        let want = wanted();
        if current.as_ref() != Some(&want) {
            let (got,): (Vec<i32>,) = kga.method_call(
                "org.kde.KGlobalAccel",
                "setShortcut",
                (action_id(), want.clone(), SET_PRESENT_NO_AUTOLOADING),
            )?;
            if got.iter().filter(|&&k| k != 0).ne(want.iter()) {
                eprintln!("finderd: hotkey is taken by another KDE shortcut; free it in System Settings → Shortcuts");
            }
            current = Some(want);
        }
        // Handle queued signals, then sleep until D-Bus or the settings folder has news.
        while conn.process(Duration::ZERO)? {}
        let mut fds = [
            libc::pollfd { fd: conn.channel().watch().fd, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: ino.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() != std::io::ErrorKind::Interrupted {
                return Err(e.into());
            }
        }
        if fds[1].revents != 0 {
            // Drain; any change in the folder just means "read settings.json again".
            let mut buf = [0u8; 4096];
            while unsafe { libc::read(ino.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
        }
    }
}

/// inotify on the data folder: catches both in-place saves and editors that save via rename.
fn watch_settings() -> std::io::Result<OwnedFd> {
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let dir = std::ffi::CString::new(paths::to_bytes(&paths::data_dir())).map_err(std::io::Error::other)?;
    let mask = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_DELETE;
    if unsafe { libc::inotify_add_watch(fd.as_raw_fd(), dir.as_ptr(), mask) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

/// Start `finder` from next to this executable.
fn launch() {
    let Some(ui) = paths::sibling_exe("finder") else {
        return eprintln!("finderd: finder not found next to finderd");
    };
    let mut cmd = Command::new(&ui);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        // Reap it in the background so it does not linger as a zombie.
        Ok(mut child) => drop(thread::spawn(move || child.wait())),
        Err(e) => eprintln!("finderd: cannot start {}: {e}", ui.display()),
    }
}
