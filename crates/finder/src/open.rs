//! Show a path in the system file manager: folders are opened, files are selected in their folder.

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

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn uri_escapes() {
        assert_eq!(super::file_uri("/a b/ü#.txt".as_ref()), "file:///a%20b/%C3%BC%23.txt");
    }
}
