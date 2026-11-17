//! `finderd --install` / `--uninstall`: autostart the daemon at login and register the `finder` launcher.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use finder_core::paths::{xdg_config_home, xdg_data_home};

fn exe_paths() -> io::Result<(PathBuf, PathBuf)> {
    let daemon = std::env::current_exe()?;
    let ui = daemon.with_file_name(if cfg!(windows) { "finder.exe" } else { "finder" });
    Ok((daemon, ui))
}

#[cfg(unix)]
fn unit_file() -> PathBuf {
    xdg_config_home().join("systemd/user/finderd.service")
}

#[cfg(unix)]
fn desktop_file() -> PathBuf {
    xdg_data_home().join("applications/finder.desktop")
}

/// Quote a path for a systemd `ExecStart=` / desktop entry `Exec=` line.
#[cfg(unix)]
fn quote(p: &Path) -> String {
    let s = p.to_string_lossy();
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "/-_.+".contains(c)) {
        s.into_owned()
    } else {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('$', "\\$"))
    }
}

fn write(path: &Path, content: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, content)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn run(cmd: &mut Command) {
    let shown = format!("{cmd:?}").replace('"', "");
    match cmd.status() {
        Ok(s) if s.success() => println!("ran {shown}"),
        Ok(s) => eprintln!("finderd: `{shown}` failed ({s})"),
        Err(e) => eprintln!("finderd: cannot run `{shown}`: {e}"),
    }
}

#[cfg(unix)]
pub fn install() -> io::Result<()> {
    let (daemon, ui) = exe_paths()?;
    write(
        &unit_file(),
        &format!(
            "[Unit]\nDescription=Finder file index daemon\n\n[Service]\nExecStart={}\nRestart=on-failure\nRestartSec=5\n\n[Install]\nWantedBy=default.target\n",
            quote(&daemon)
        ),
    )?;
    run(Command::new("systemctl").args(["--user", "daemon-reload"]));
    run(Command::new("systemctl").args(["--user", "enable", "--now", "finderd.service"]));
    if ui.is_file() {
        write(
            &desktop_file(),
            &format!(
                "[Desktop Entry]\nType=Application\nName=Finder\nComment=Find files and folders by name\nExec={}\nIcon=system-search\nTerminal=false\nCategories=Utility;\nStartupWMClass=finder\n",
                quote(&ui)
            ),
        )?;
    }
    Ok(())
}

#[cfg(unix)]
pub fn uninstall() -> io::Result<()> {
    run(Command::new("systemctl").args(["--user", "disable", "--now", "finderd.service"]));
    for f in [unit_file(), desktop_file()] {
        match fs::remove_file(&f) {
            Ok(()) => println!("removed {}", f.display()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    run(Command::new("systemctl").args(["--user", "daemon-reload"]));
    Ok(())
}

#[cfg(windows)]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[cfg(windows)]
pub fn install() -> io::Result<()> {
    let (daemon, _) = exe_paths()?;
    let value = format!("\"{}\"", daemon.display());
    run(Command::new("reg").args(["add", RUN_KEY, "/v", "finderd", "/t", "REG_SZ", "/d", &value, "/f"]));
    Ok(())
}

#[cfg(windows)]
pub fn uninstall() -> io::Result<()> {
    run(Command::new("reg").args(["delete", RUN_KEY, "/v", "finderd", "/f"]));
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn quoting() {
        assert_eq!(super::quote("/usr/bin/finderd".as_ref()), "/usr/bin/finderd");
        assert_eq!(super::quote("/a b/$x\"".as_ref()), r#""/a b/\$x\"""#);
    }
}
