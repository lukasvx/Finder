//! Show a path in the system file manager (folders are opened, files are selected in their
//! folder), or run a program.

use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Spawn detached so the file manager outlives this process.
fn spawn(cmd: &mut Command) -> io::Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(cmd, 0);
    cmd.spawn().map(drop)
}

#[cfg(unix)]
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

#[cfg(unix)]
pub fn reveal(path: &Path, is_dir: bool) -> io::Result<()> {
    // 1. Dolphin.
    let mut dolphin = Command::new("dolphin");
    if !is_dir {
        dolphin.arg("--select");
    }
    if spawn(dolphin.arg(path)).is_ok() {
        return Ok(());
    }
    // 2. Any file manager implementing org.freedesktop.FileManager1.
    let method = if is_dir { "ShowFolders" } else { "ShowItems" };
    let dbus = Command::new("dbus-send")
        .args(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "--type=method_call"])
        .arg("/org/freedesktop/FileManager1")
        .arg(format!("org.freedesktop.FileManager1.{method}"))
        .arg(format!("array:string:{}", file_uri(path)))
        .arg("string:")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if dbus.is_ok_and(|s| s.success()) {
        return Ok(());
    }
    // 3. Open the folder with the default handler.
    let dir = if is_dir { path } else { path.parent().unwrap_or(path) };
    spawn(Command::new("xdg-open").arg(dir))
}

#[cfg(windows)]
pub fn reveal(path: &Path, is_dir: bool) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new("explorer");
    if is_dir {
        cmd.arg(path);
    } else {
        cmd.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    spawn(&mut cmd)
}

/// Run the program at `path`, detached, in its own folder.
#[cfg(unix)]
pub fn run(path: &Path) -> io::Result<()> {
    let mut cmd = Command::new(path);
    if let Some(dir) = path.parent() {
        cmd.current_dir(dir);
    }
    spawn(&mut cmd)
}

/// Run the program at `path` in its own folder (a shortcut: as set in the shortcut, like Explorer
/// does). `ShellExecuteW` rather than `CreateProcess`, so a program that requires admin rights
/// gets the elevation prompt, and a shortcut gets its arguments.
#[cfg(windows)]
pub fn run(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let file = wide(path);
    let shortcut = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
    let dir = path.parent().filter(|_| !shortcut).map(wide);
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    // SAFETY: all strings are NUL-terminated and outlive the call.
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()),
            SW_SHOWNORMAL,
        )
    };
    // Values up to 32 are errors.
    if r as usize > 32 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn uri_escapes() {
        assert_eq!(super::file_uri("/a b/ü#.txt".as_ref()), "file:///a%20b/%C3%BC%23.txt");
    }
}
