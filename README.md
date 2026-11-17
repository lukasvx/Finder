# Finder

Instant file locator for Linux and Windows: type a keyword, pick a match, the file manager opens there.

## Idea

1. `finder` opens and asks for a keyword.
2. It lists all files and folders on the system whose name contains the keyword.
3. User picks one → app closes, file manager opens: folder → that folder; file → its parent folder with the file selected.

File manager: Dolphin (Linux), Explorer (Windows).

## Goals

- Window visible < 100 ms after launch; results while typing < 50 ms on ~2M files.
- Daemon: heap < 20 MB, index ~50 B per entry, ~0% CPU when idle.
- One codebase for Linux and Windows.

## Architecture

- Rust; UI with FLTK (`fltk-rs`), chosen for startup speed.
- `finderd`: background daemon, autostarts at login. Scans the whole system on first start, caches the index on disk, keeps it current via file-change events, answers queries.
- `finder`: short-lived UI. Reads settings, queries `finderd` over a local socket, opens the file manager, exits.

| | Linux | Windows |
|---|---|---|
| Index cache | `~/.cache/finder/index.bin` | `%LOCALAPPDATA%\Finder\index.bin` |
| Settings | `~/.config/finder/settings.conf` | `%APPDATA%\Finder\settings.conf` |

## Settings

Plain text, `key = value`, `#` comments. Created with defaults if missing; re-read on every `finder` launch.

| Key | Default | Meaning |
|---|---|---|
| `case_sensitive` | `false` | Match case exactly |
| `match_full_path` | `false` | Match against the full path instead of the name only |

## Deferred

- Global keyboard shortcut to launch `finder`.

## Progress

- [x] Idea, README, GitHub repo
- [x] Architecture, language (Rust), UI toolkit (FLTK)
- [ ] Project skeleton (`finderd` + `finder`)
- [ ] Settings file
- [ ] Index format + cache file
- [ ] Initial scan: Linux
- [ ] Initial scan: Windows
- [ ] Change listener: Linux
- [ ] Change listener: Windows
- [ ] Startup catch-up
- [ ] Delta + compaction
- [ ] Search engine
- [ ] Daemon ↔ UI IPC
- [ ] UI: keyword prompt + result list
- [ ] Open Dolphin / Explorer
- [ ] Autostart `finderd`
- [ ] Benchmark RAM and latency against goals
- [ ] Packaging

## Implementation notes

Planned design for features not built yet. Delete each point once implemented; after that the code is the reference.

- **Index format:** fixed 12-byte record per entry (parent `u32`, name offset `u32`, name length `u16`, flags `u8`), parent-before-child order. Names packed in one buffer plus a lowercased copy. No stored full paths. Cache file is memory-mapped.
- **Initial scan:** Linux: parallel directory walk. Windows: read NTFS MFT (`FSCTL_ENUM_USN_DATA`, needs admin), fallback parallel walk.
- **Exclusions:** `/proc`, `/sys`, `/dev`, `/run`, network/other mounts, `/.snapshots`.
- **Change listener:**
  - Linux: `fanotify` with `FAN_MARK_FILESYSTEM` (needs `CAP_SYS_ADMIN`; verify it works on btrfs subvolumes). Fallback: inotify on `$HOME` + periodic dir-mtime rescan. No system-wide inotify (~1 KB kernel memory per watched dir).
  - Windows: USN journal (needs admin → run as service). Fallback: `ReadDirectoryChangesW` with subtree, per drive.
- **Startup catch-up:** Linux: re-read only dirs whose mtime changed. Windows: resume USN journal from saved position.
- **Updates:** changes go to an in-memory delta; merged into a new cache file when large or idle (temp file + atomic rename).
- **Settings:** unknown keys or invalid values → warn, use default. Options are sent with each query.
- **Search:**
  - Parallel SIMD substring scan over the (lowercased) name buffer.
  - Full-path mode: one linear pass propagating ancestor matches; keywords with separators matched component by component.
  - Cancel on each keystroke; narrow existing results when the keyword is extended.
  - Stream the first ~100 results; UI renders only visible rows.
  - Rank: exact > prefix > substring, then shorter path, then recently opened.
- **IPC:** Unix socket / named pipe, owner-only access.
- **Open file manager:** Linux: `dolphin --select <file>` / `dolphin <dir>`, fallback D-Bus `org.freedesktop.FileManager1.ShowItems`, then `xdg-open`. Windows: `explorer /select,"<file>"` / `explorer "<dir>"`.
