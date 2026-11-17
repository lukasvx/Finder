//! Platform-specific file locations.

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

#[cfg(unix)]
fn home() -> PathBuf {
    env_dir("HOME").unwrap_or_else(|| PathBuf::from("/"))
}

/// Directory holding the index cache.
pub fn cache_dir() -> PathBuf {
    #[cfg(unix)]
    return env_dir("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache")).join("finder");
    #[cfg(windows)]
    return env_dir("LOCALAPPDATA").unwrap_or_else(env::temp_dir).join("Finder");
}

/// Directory holding `settings.conf`.
pub fn config_dir() -> PathBuf {
    #[cfg(unix)]
    return env_dir("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config")).join("finder");
    #[cfg(windows)]
    return env_dir("APPDATA").unwrap_or_else(env::temp_dir).join("Finder");
}

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.conf")
}

pub fn index_file() -> PathBuf {
    cache_dir().join("index.bin")
}

pub fn recent_file() -> PathBuf {
    cache_dir().join("recent.txt")
}

/// Unix socket the daemon listens on.
#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    env_dir("XDG_RUNTIME_DIR").unwrap_or_else(cache_dir).join("finder.sock")
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
