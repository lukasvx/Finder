# Finder

Instant file locator for Linux and Windows: press the hotkey, type a keyword, pick a match, the file manager opens there.

## Idea

1. The hotkey opens a search bar at the top edge of the screen (like KRunner); pressing it again closes it.
2. Typing lists all files and folders on the system whose name contains the keyword; the bar grows downward to show them.
3. User picks one → bar closes, file manager opens: folder → that folder; file → its parent folder with the file selected.
4. Programs named like the keyword (`discord` → `Discord.exe`, or a shortcut `Discord.lnk` to a program; Linux: an executable file `discord`) rank first; Enter runs them, Ctrl+Enter shows them in the file manager.

File manager: Dolphin (Linux), Explorer (Windows). The UI is minimal and not configurable: light or dark like the desktop.

## Goals

- Window visible < 100 ms after launch; results while typing < 50 ms on ~2M files.
- Daemon: heap < 20 MB, index ~50 B per entry, ~0% CPU when idle.
- One codebase for Linux and Windows.
- Footprint: a download folder with `finder` + `finderd`; on the system only `settings.json`, `index.bin` and an autostart entry.

Measured (Linux, 12 cores, NVMe; `cargo test --release -p finderd -- --ignored --nocapture`):

| | Result |
|---|---|
| Window first frame after launch | ~67 ms |
| Query on 2.2M entries | 5–25 ms (single letter) |
| Full scan, 434k entries | 0.3 s |
| Catch-up pass, 35k dirs | 42 ms |
| Daemon anon RSS (idle / during queries) | ~5 MB / +9 MB |
| Index size | 59 B/entry |

## Architecture

- Rust; UI with FLTK (`fltk-rs`), chosen for startup speed.
- `finderd`: background daemon, autostarts at login. Scans the whole system on first start, caches the index on disk, keeps it current via file-change events, answers queries, registers the hotkey (starts `finder`).
- `finder`: short-lived UI. Reads settings, queries `finderd` over a local socket or named pipe, opens the file manager, exits. Starts `finderd` if it isn't running.

| | Linux | Windows |
|---|---|---|
| `settings.json`, `index.bin` | `~/.local/share/finder/` | `%LOCALAPPDATA%\Finder\` |
| Autostart | `~/.config/autostart/finderd.desktop` | `finderd` value in `HKCU\…\CurrentVersion\Run` |
| Hotkey | KDE global shortcut (kglobalaccel, D-Bus); other desktops: bind `finder` yourself | `RegisterHotKey` |
| Window | X11 (XWayland under Wayland), so it can sit at the top edge | Win32 |
| Icons | Like Dolphin: MIME type (shared-mime-info globs, content sniffing) → desktop icon theme (KDE/GTK setting, else Breeze), SVGs rendered with `resvg` in the color scheme | Like Explorer: shell icon (`SHGetFileInfoW`), so programs show their own; embedded icon or type icon as fallback |
| IPC | Unix socket in `$XDG_RUNTIME_DIR`, mode 0600 | Named pipe `\\.\pipe\finder-<user SID>`, owner-only DACL |
| Roots | `/` minus `/proc`, `/sys`, `/dev`, `/run` and non-local mounts | Fixed drives |
| Live updates | inotify on `$HOME`; rest via 15-min catch-up | `ReadDirectoryChangesW` on every drive; 15-min catch-up as safety net |

Connections start with a version check: a running `finderd` from another version (e.g. after an update) is stopped and the new one started. `finderd` logs to stderr only. Recently opened paths (ranked first) are kept in memory. On start it moves a v0.4 index over and deletes v0.4 leftovers (`settings.conf`, `recent.txt`, `finderd.log`, `--install` launchers, KRunner plugin).

## Build

```
cargo build --release   # → target/release/finder, target/release/finderd
cargo test
```

Windows build from Fedora (needs `mingw64-gcc mingw64-gcc-c++ mingw64-winpthreads-static rust-std-static-x86_64-pc-windows-gnu`; tests run under `wine`):

```
cargo build --release --target x86_64-pc-windows-gnu
CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine cargo test --release --target x86_64-pc-windows-gnu -p finder-core
```

(Most `finderd` unit tests use Unix-style paths and don't run on Windows yet; `-p finderd watch` does.)

UI checks without input devices: `cargo build --release -p finder --features uitest --target-dir target/uitest` (also build `-p finderd` there), then `FINDER_UITEST="x,y;x,y" ./target/uitest/release/finder` replays clicks (`x,y`) or drags (`x,y>x2,y2`) at window coordinates (see `crates/finder/src/uitest.rs`).

Run `finder [keyword]`. `finderd --rescan` forces a full scan; `finderd --uninstall` stops the daemon and removes the autostart entry, hotkey and both files.

Fedora deps: `rust cargo gcc-c++ libstdc++-static cmake libX11-devel libXext-devel libXft-devel libXinerama-devel libXcursor-devel libXrender-devel libXfixes-devel pango-devel cairo-devel wayland-devel wayland-protocols-devel libxkbcommon-devel dbus-devel mesa-libGL-devel`

| Crate | Contents |
|---|---|
| `crates/core` | Paths, settings, autostart, hotkey names, IPC protocol, case folding |
| `crates/finderd` | Daemon: index, scan, change listener, search, hotkey |
| `crates/finder` | UI, first-run setup, file icons |

## Settings

`settings.json`, next to `index.bin`. The first launch shows a setup in the middle of the screen (hotkey, case sensitivity) that writes it, sets up autostart and opens the search bar; after that it is changed only by editing the file. `finder` reads it on every launch; `finderd` applies hotkey edits live.

```json
{
  "hotkey": "ctrl+alt+space",
  "case_sensitive": false
}
```

| Key | Default | Meaning |
|---|---|---|
| `hotkey` | `""` (none) | Global shortcut that opens `finder`: modifiers `ctrl`, `alt`, `shift`, `win`/`meta`/`super` plus one key (`a`–`z`, `0`–`9`, `f1`–`f24`, `space`, `enter`, `tab`, `esc`, arrows, `` ` `` `-` `=` `,` `.` `/` `;` …), e.g. `win+shift+f`; F-keys alone are allowed |
| `case_sensitive` | `false` | Match case exactly |

Matching is always against names (not full paths), hidden files included. Invalid values are reported in the bar and fall back to the default.

## Progress

- [x] Idea, README, GitHub repo
- [x] Architecture, language (Rust), UI toolkit (FLTK)
- [x] Project skeleton (`finderd` + `finder`)
- [x] Index format + cache file
- [x] Initial scan: Linux, Windows (parallel directory walk)
- [x] Change listener: Linux, Windows
- [x] Startup catch-up
- [x] Delta + compaction
- [x] Search engine
- [x] Daemon ↔ UI IPC (Unix socket / named pipe)
- [x] Open Dolphin / Explorer
- [x] Benchmark RAM and latency against goals
- [x] Rework: two files (`settings.json` + `index.bin`), hotkey + case sensitivity only, first-run setup
- [x] Rework: minimal top-edge UI that grows with results, light/dark from the system
- [x] Rework: hotkey on Linux (KDE), autostart on first run, `--uninstall`
- [x] Packaging: release archives `finder-linux-x86_64.tar.gz` / `finder-windows-x86_64.zip`, a folder with `finder` + `finderd`

## Implementation notes

Planned design for features not built yet. Delete each point once implemented; after that the code is the reference.

- **Initial scan (Windows):** faster path: read the NTFS MFT (`FSCTL_ENUM_USN_DATA`, needs admin); keep the directory walk as fallback.
- **Change listener:**
  - Linux (now: inotify on every dir under `$HOME`, rest via 15-min mtime catch-up): add `fanotify` with `FAN_MARK_FILESYSTEM` when `CAP_SYS_ADMIN` is available, for instant system-wide updates (verify on btrfs subvolumes).
  - Windows (now: `ReadDirectoryChangesW` per drive, affected dirs re-read from disk): optionally the USN journal (needs admin → run as service), which also survives daemon restarts.
- **Startup catch-up (Windows):** resume USN journal from saved position.

## License

MIT, see [LICENSE](LICENSE).
