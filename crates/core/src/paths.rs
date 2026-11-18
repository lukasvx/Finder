//! Platform-specific file locations.
//!
//! Finder keeps exactly two files, side by side in `data_dir()`: `settings.json` and `index.bin`.

use std::env;
use std::path::PathBuf;

/// Path separator byte used in index paths.
#[cfg(unix)]
pub const SEP: u8 = b'/';
#[cfg(windows)]
pub const SEP: u8 = b'\\';

fn env_dir(var: &str) -> Option<PathBuf> {
    env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The user's home folder.
pub fn home() -> PathBuf {
    #[cfg(unix)]
    return env_dir("HOME").unwrap_or_else(|| PathBuf::from("/"));
    #[cfg(windows)]
    return env_dir("USERPROFILE").unwrap_or_else(|| PathBuf::from(r"C:\"));
}

/// `$XDG_CONFIG_HOME` (default `~/.config`).
#[cfg(unix)]
pub fn xdg_config_home() -> PathBuf {
    env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"))
}

/// Directory holding `settings.json` and `index.bin`: `$XDG_DATA_HOME/finder`
/// (`~/.local/share/finder`) or `%LOCALAPPDATA%\Finder`.
pub fn data_dir() -> PathBuf {
    #[cfg(unix)]
    return env_dir("XDG_DATA_HOME").unwrap_or_else(|| home().join(".local/share")).join("finder");
    #[cfg(windows)]
    return env_dir("LOCALAPPDATA").unwrap_or_else(env::temp_dir).join("Finder");
}

pub fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn index_file() -> PathBuf {
    data_dir().join("index.bin")
}

/// Private directory for sockets: `$XDG_RUNTIME_DIR` (a tmpfs, cleared at logout).
#[cfg(unix)]
fn runtime_dir() -> PathBuf {
    env_dir("XDG_RUNTIME_DIR").unwrap_or_else(data_dir)
}

/// Unix socket the daemon listens on.
#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    runtime_dir().join("finder.sock")
}

/// Unix socket of the open `finder` window, so a new one can close it.
#[cfg(unix)]
pub fn ui_socket_path() -> PathBuf {
    runtime_dir().join("finder-ui.sock")
}

/// The other executable (`finder` or `finderd`) from next to the running one.
pub fn sibling_exe(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    Some(env::current_exe().ok()?.with_file_name(file)).filter(|p| p.is_file())
}

/// Convert raw index path bytes to a `PathBuf`.
pub fn from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    return PathBuf::from(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(bytes));
    #[cfg(windows)]
    return PathBuf::from(String::from_utf8_lossy(bytes).into_owned());
}

/// Convert a path to raw index path bytes.
pub fn to_bytes(path: &std::path::Path) -> Vec<u8> {
    #[cfg(unix)]
    return std::os::unix::ffi::OsStrExt::as_bytes(path.as_os_str()).to_vec();
    #[cfg(windows)]
    return path.to_string_lossy().into_owned().into_bytes();
}
