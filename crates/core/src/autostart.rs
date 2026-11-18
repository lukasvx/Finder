//! Start `finderd` at login, so the hotkey works right away: Linux `~/.config/autostart/finderd.desktop`,
//! Windows the `finderd` value under `HKCU\…\CurrentVersion\Run`. Set up by the first-run setup.

use std::io;
#[cfg(unix)]
use std::path::Path;

use crate::paths;

fn daemon() -> io::Result<std::path::PathBuf> {
    paths::sibling_exe("finderd")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "finderd not found next to finder"))
}

#[cfg(unix)]
fn desktop_file() -> std::path::PathBuf {
    paths::xdg_config_home().join("autostart/finderd.desktop")
}

/// Quote a path for a desktop entry `Exec=` line.
#[cfg(unix)]
fn quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/-_.+".contains(c)) {
        s.into_owned()
    } else {
        format!(
            "\"{}\"",
            s.replace('\\', "\\\\\\\\").replace('"', "\\\\\"").replace('$', "\\\\$").replace('`', "\\\\`")
        )
    }
}

#[cfg(unix)]
fn entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Finder\nComment=File index for Finder\nExec={}\nNoDisplay=true\n",
        quote(exe)
    )
}

/// Start the `finderd` next to this executable at login.
#[cfg(unix)]
pub fn enable() -> io::Result<()> {
    let file = desktop_file();
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(file, entry(&daemon()?))
}

#[cfg(unix)]
pub fn disable() -> io::Result<()> {
    match std::fs::remove_file(desktop_file()) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(windows)]
mod reg {
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "finderd";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn check(r: u32) -> std::io::Result<()> {
        if r == ERROR_SUCCESS { Ok(()) } else { Err(std::io::Error::from_raw_os_error(r as i32)) }
    }

    pub fn set(command: &str) -> std::io::Result<()> {
        let data = wide(command);
        check(unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(KEY).as_ptr(),
                wide(VALUE).as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        })
    }

    pub fn delete() -> std::io::Result<()> {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(KEY).as_ptr(), wide(VALUE).as_ptr()) } {
            ERROR_FILE_NOT_FOUND => Ok(()),
            r => check(r),
        }
    }
}

/// Start the `finderd` next to this executable at login.
#[cfg(windows)]
pub fn enable() -> io::Result<()> {
    reg::set(&format!("\"{}\"", daemon()?.display()))
}

#[cfg(windows)]
pub fn disable() -> io::Result<()> {
    reg::delete()
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn quoting() {
        assert_eq!(super::quote("/usr/bin/finderd".as_ref()), "/usr/bin/finderd");
        // Desktop entry: quoted-string escapes, then `\` itself escaped once more.
        assert_eq!(super::quote("/a b/$x\"".as_ref()), r#""/a b/\\$x\\"""#);
        assert!(super::entry("/x/finderd".as_ref()).contains("\nExec=/x/finderd\n"));
    }
}
