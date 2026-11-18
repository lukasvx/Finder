//! Programs named like the keyword: `discord` finds `Discord.exe` or a shortcut `Discord.lnk` to a
//! program on Windows, or an executable file named `discord` on Linux. Such hits rank first, and
//! Enter runs them instead of showing them in the file manager. Shared so the daemon (ranking)
//! and the UI (Enter) agree.

use std::path::Path;

use crate::fold::fold;
use crate::paths::{self, SEP};

/// `name` is the program name for `keyword`. Compared as given: fold both for case-insensitive.
pub fn name_matches(name: &[u8], keyword: &[u8]) -> bool {
    name_matches_on(cfg!(windows), name, keyword)
}

fn name_matches_on(windows: bool, name: &[u8], keyword: &[u8]) -> bool {
    if keyword.is_empty() || keyword.contains(&SEP) {
        return false;
    }
    if !windows {
        return name == keyword;
    }
    let stem = name
        .len()
        .checked_sub(4)
        .filter(|&n| name[n..].eq_ignore_ascii_case(b".exe") || name[n..].eq_ignore_ascii_case(b".lnk"))
        .map(|n| &name[..n]);
    stem.is_some() && (stem == Some(keyword) || name == keyword)
}

/// The file can be run: an `.exe` (judged by name) or a shortcut to a program on Windows, a
/// regular file with an execute bit on Unix.
pub fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(windows)]
    {
        match path.extension() {
            Some(e) if e.eq_ignore_ascii_case("exe") => true,
            Some(e) if e.eq_ignore_ascii_case("lnk") => is_program_shortcut(path),
            _ => false,
        }
    }
}

/// A Windows shortcut that starts a program.
pub fn is_program_shortcut(path: &Path) -> bool {
    use std::io::Read;
    // Shortcuts are a few KB; don't read whatever else is named `.lnk` in full.
    let mut data = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(64 * 1024).read_to_end(&mut data));
    read.is_ok() && crate::lnk::Link::parse(&data).is_some_and(|l| l.runs_program())
}

/// The hit at `path` is the program to run for `keyword`. Touches the disk (Unix) only when the
/// name matches.
pub fn is_launch_hit(path: &[u8], is_dir: bool, keyword: &str, case_sensitive: bool) -> bool {
    if is_dir {
        return false;
    }
    let name = path.rsplit(|&b| b == SEP).next().unwrap_or(path);
    let matches = if case_sensitive {
        name_matches(name, keyword.as_bytes())
    } else {
        name_matches(&fold(name), &fold(keyword.as_bytes()))
    };
    matches && is_executable(&paths::from_bytes(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let win = |n: &str, k: &str| name_matches_on(true, n.as_bytes(), k.as_bytes());
        assert!(win("discord.exe", "discord"));
        assert!(win("discord.exe", "discord.exe"));
        assert!(win("Discord.EXE", "Discord"));
        assert!(!win("discord", "discord"));
        assert!(!win("discord.exe.lnk", "discord"));
        assert!(win("Discord.LNK", "Discord"));
        assert!(win("discord.lnk", "discord.lnk"));
        assert!(!win("discord.url", "discord"));
        assert!(!win("discordx.exe", "discord"));
        assert!(!win(".exe", ""));
        let unix = |n: &str, k: &str| name_matches_on(false, n.as_bytes(), k.as_bytes());
        assert!(unix("discord", "discord"));
        assert!(!unix("discord.exe", "discord"));
        assert!(!unix("discord", &format!("bin{}discord", SEP as char)));
    }

    #[cfg(unix)]
    #[test]
    fn launch_hits() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("finder-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Discord");
        let doc = dir.join("notes");
        std::fs::write(&exe, "").unwrap();
        std::fs::write(&doc, "").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let b = |p: &Path| paths::to_bytes(p);
        assert!(is_launch_hit(&b(&exe), false, "discord", false));
        assert!(!is_launch_hit(&b(&exe), false, "discord", true));
        assert!(is_launch_hit(&b(&exe), false, "Discord", true));
        assert!(!is_launch_hit(&b(&exe), true, "Discord", true));
        assert!(!is_launch_hit(&b(&doc), false, "notes", false));
        assert!(!is_launch_hit(&b(&exe), false, "disc", false));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn program_shortcuts() {
        let dir = std::env::temp_dir().join(format!("finder-lnk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let build = crate::lnk::tests::build;
        let app = dir.join("App.lnk");
        let doc = dir.join("Notes.lnk");
        std::fs::write(&app, build(Some(r"C:\Program Files\App\app.exe"), true, None, None)).unwrap();
        std::fs::write(&doc, build(Some(r"C:\Users\me\notes.txt"), true, None, None)).unwrap();
        assert!(is_program_shortcut(&app));
        assert!(!is_program_shortcut(&doc));
        #[cfg(windows)]
        {
            assert!(is_executable(&app) && !is_executable(&doc));
            assert!(is_launch_hit(&paths::to_bytes(&app), false, "app", false));
            assert!(!is_launch_hit(&paths::to_bytes(&doc), false, "notes", false));
        }
        assert!(!is_program_shortcut(&dir.join("missing.lnk")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
