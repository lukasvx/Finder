//! Windows change listener: `ReadDirectoryChangesW` with subtree watching, one thread per drive.
//!
//! Events only name the changed path. The indexer re-reads the parent directories from disk
//! instead of trusting the names: Windows reports a name in the case the caller used, which can
//! differ from the case on disk.

use std::io;
use std::ptr::{null, null_mut};
use std::sync::mpsc::Sender;
use std::thread;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ACTION_ADDED, FILE_ACTION_REMOVED, FILE_ACTION_RENAMED_NEW_NAME, FILE_ACTION_RENAMED_OLD_NAME,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};

use crate::scan::child_path;

pub enum Change {
    /// An entry was added, removed or renamed at this path.
    Changed { path: Vec<u8> },
    /// Events were lost; re-sync from disk.
    Overflow,
}

/// Bytes of change records buffered per drive between reads. Local volumes allow more than 64 KB.
const BUF_SIZE: usize = 256 * 1024;

struct Dir(HANDLE);

// The handle is only used by the thread that owns it.
unsafe impl Send for Dir {}

impl Drop for Dir {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open_dir(path: &[u8]) -> io::Result<Dir> {
    let mut wide: Vec<u16> = String::from_utf8_lossy(path).encode_utf16().collect();
    if wide.last() != Some(&(b'\\' as u16)) {
        wide.push(b'\\' as u16);
    }
    wide.push(0);
    let h = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(Dir(h)) }
}

/// Block until changes arrive and append them (as full paths below `root`) to `out`;
/// `Change::Overflow` if the buffer overflowed and events were lost.
fn read_changes(dir: &Dir, root: &[u8], buf: &mut [u32], out: &mut Vec<Change>) -> io::Result<()> {
    let mut len = 0u32;
    let ok = unsafe {
        ReadDirectoryChangesW(
            dir.0,
            buf.as_mut_ptr().cast(),
            (buf.len() * 4) as u32,
            1,
            FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME,
            &mut len,
            null_mut(),
            None,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if len == 0 {
        out.push(Change::Overflow);
        return Ok(());
    }
    parse(&bytemuck::cast_slice::<u32, u8>(buf)[..len as usize], root, out);
    Ok(())
}

/// Parse a chain of `FILE_NOTIFY_INFORMATION` records.
fn parse(bytes: &[u8], root: &[u8], out: &mut Vec<Change>) {
    let u32_at = |i: usize| bytes.get(i..i + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let mut off = 0;
    loop {
        let (Some(next), Some(action), Some(name_len)) = (u32_at(off), u32_at(off + 4), u32_at(off + 8)) else {
            return;
        };
        let Some(name) = bytes.get(off + 12..off + 12 + name_len as usize) else { return };
        let name: Vec<u16> = name.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        if matches!(
            action,
            FILE_ACTION_ADDED | FILE_ACTION_REMOVED | FILE_ACTION_RENAMED_OLD_NAME | FILE_ACTION_RENAMED_NEW_NAME
        ) {
            let rel = String::from_utf16_lossy(&name);
            out.push(Change::Changed { path: child_path(root, rel.as_bytes()) });
        }
        if next == 0 {
            return;
        }
        off += next as usize;
    }
}

/// Watch `root` (a drive like `C:` or any directory) and everything below it.
fn watch(root: Vec<u8>, tx: Sender<Change>) -> io::Result<()> {
    let dir = open_dir(&root)?;
    thread::Builder::new().name("watcher".into()).spawn(move || {
        let mut buf = vec![0u32; BUF_SIZE / 4];
        let mut changes = Vec::new();
        loop {
            if let Err(e) = read_changes(&dir, &root, &mut buf, &mut changes) {
                eprintln!("finderd: watcher for {} stopped: {e}", String::from_utf8_lossy(&root));
                return;
            }
            for c in changes.drain(..) {
                if tx.send(c).is_err() {
                    return;
                }
            }
        }
    })?;
    Ok(())
}

/// Watch every root (drive) of the index and forward changes to `tx`.
pub fn spawn(roots: Vec<Vec<u8>>, tx: Sender<Change>) {
    let mut watched = Vec::new();
    for root in roots {
        let shown = String::from_utf8_lossy(&root).into_owned();
        match watch(root, tx.clone()) {
            Ok(()) => watched.push(shown),
            Err(e) => eprintln!("finderd: cannot watch {shown}: {e}"),
        }
    }
    eprintln!("finderd: watching {}", watched.join(" "));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn record(path: &str) -> Vec<u8> {
        let name: Vec<u16> = path.encode_utf16().collect();
        let mut r = Vec::new();
        r.extend_from_slice(&0u32.to_le_bytes());
        r.extend_from_slice(&FILE_ACTION_ADDED.to_le_bytes());
        r.extend_from_slice(&(name.len() as u32 * 2).to_le_bytes());
        r.extend(name.iter().flat_map(|c| c.to_le_bytes()));
        r
    }

    #[test]
    fn parses_record_chain() {
        let mut a = record(r"dir\ü.txt");
        let b = record("x");
        let next = a.len().next_multiple_of(4);
        a.resize(next, 0);
        a[..4].copy_from_slice(&(next as u32).to_le_bytes());
        a.extend_from_slice(&b);
        let mut out = Vec::new();
        parse(&a, b"C:", &mut out);
        let paths: Vec<String> = out
            .into_iter()
            .map(|c| match c {
                Change::Changed { path } => String::from_utf8(path).unwrap(),
                Change::Overflow => "overflow".into(),
            })
            .collect();
        assert_eq!(paths, [r"C:\dir\ü.txt", r"C:\x"]);
        // Truncated input is ignored, not a panic.
        let mut out = Vec::new();
        parse(&a[..a.len() - 3], b"C:", &mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn reports_changes_below_root() {
        let dir = crate::index::tests::temp_dir("watch-win");
        let root = finder_core::paths::to_bytes(&dir);
        let (tx, rx) = mpsc::channel();
        watch(root.clone(), tx).unwrap();
        // Changes are buffered only from the watcher thread's first read on.
        std::thread::sleep(Duration::from_millis(300));
        std::fs::create_dir_all(dir.join(r"a\b")).unwrap();
        std::fs::write(dir.join(r"a\b\f.txt"), "").unwrap();
        std::fs::rename(dir.join(r"a\b\f.txt"), dir.join(r"a\g.txt")).unwrap();
        let mut seen = Vec::new();
        while let Ok(Change::Changed { path }) = rx.recv_timeout(Duration::from_secs(2)) {
            seen.push(String::from_utf8(path).unwrap());
            if seen.iter().any(|p| p.ends_with(r"a\g.txt")) {
                break;
            }
        }
        let rel = |s: &str| format!("{}\\{s}", dir.display());
        for want in [r"a", r"a\b\f.txt", r"a\g.txt"] {
            assert!(seen.contains(&rel(want)), "missing {want} in {seen:?}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
