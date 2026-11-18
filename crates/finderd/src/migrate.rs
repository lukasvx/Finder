//! Up to v0.4, Finder put more files on the system: the index, `recent.txt` and `finderd.log` in
//! a cache folder, `settings.conf` in a config folder, and (`finderd --install`, KRunner plugin)
//! launchers and service files. Move the index over (saves a full scan) and delete the rest, so
//! only `settings.json`, `index.bin` and the autostart entry remain.

use std::fs;
use std::path::PathBuf;

use finder_core::paths;

fn var(v: &str) -> Option<PathBuf> {
    std::env::var_os(v).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// (old cache folder, old config folder, other old files and folders).
fn old_locations() -> Option<(PathBuf, PathBuf, Vec<PathBuf>)> {
    #[cfg(unix)]
    {
        let config = paths::xdg_config_home();
        let data = var("XDG_DATA_HOME").unwrap_or_else(|| paths::home().join(".local/share"));
        let cache = var("XDG_CACHE_HOME").unwrap_or_else(|| paths::home().join(".cache")).join("finder");
        let other = vec![
            config.join("systemd/user/default.target.wants/finderd.service"),
            config.join("systemd/user/finderd.service"),
            data.join("applications/finder.desktop"),
            data.join("krunner/dbusplugins/finder.desktop"),
            data.join("dbus-1/services/org.finder.krunner.service"),
            data.join("finder/krunner/finder-runner.py"),
            data.join("finder/krunner"),
        ];
        Some((cache, config.join("finder"), other))
    }
    #[cfg(windows)]
    {
        let appdata = var("APPDATA")?;
        let other = vec![appdata.join(r"Microsoft\Windows\Start Menu\Programs\Finder.lnk")];
        Some((paths::data_dir(), appdata.join("Finder"), other))
    }
}

pub fn remove_old_files() {
    let Some((cache, config, other)) = old_locations() else { return };
    let old_index = cache.join("index.bin");
    if old_index != paths::index_file() && old_index.is_file() {
        if !paths::index_file().exists() && fs::rename(&old_index, paths::index_file()).is_ok() {
            eprintln!("finderd: moved {} to {}", old_index.display(), paths::index_file().display());
        }
        let _ = fs::remove_file(&old_index);
    }
    let files = [cache.join("recent.txt"), cache.join("finderd.log"), config.join("settings.conf")];
    for f in files.into_iter().chain(other) {
        // `symlink_metadata`: the systemd "wants" link may point at the already removed unit.
        let removed = match fs::symlink_metadata(&f) {
            Ok(m) if m.is_dir() => fs::remove_dir(&f).is_ok(),
            Ok(_) => fs::remove_file(&f).is_ok(),
            Err(_) => false,
        };
        if removed {
            eprintln!("finderd: removed old {}", f.display());
        }
    }
    // Only if empty now.
    for d in [cache, config] {
        if d != paths::data_dir() {
            let _ = fs::remove_dir(d);
        }
    }
}
