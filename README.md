# Finder

Instant file locator for Linux and Windows: type a keyword, pick a match, the file manager opens there.

## Idea

1. `finder` opens and asks for a keyword.
2. It lists all files and folders on the system whose name contains the keyword.
3. User picks one → app closes, file manager opens: folder → that folder; file → its parent folder with the file selected.
4. Programs named like the keyword (`discord` → `Discord.exe`; Linux: an executable file `discord`) rank first; Enter runs them, Ctrl+Enter shows them in the file manager.

File manager: Dolphin (Linux), Explorer (Windows).

## Goals

- Window visible < 100 ms after launch; results while typing < 50 ms on ~2M files.
- Daemon: heap < 20 MB, index ~50 B per entry, ~0% CPU when idle.
- One codebase for Linux and Windows.

Measured (Linux, 12 cores, NVMe; `cargo test --release -p finderd -- --ignored --nocapture`):

| | Result |
|---|---|
| Window first frame after launch | ~67 ms |
| Query on 2.2M entries | 5–25 ms (single letter, full path included) |
| Full scan, 434k entries | 0.3 s |
| Catch-up pass, 35k dirs | 42 ms |
| Daemon anon RSS (idle / during queries) | ~5 MB / +9 MB |
| Index size | 59 B/entry |

## Architecture

- Rust; UI with FLTK (`fltk-rs`), chosen for startup speed.
- `finderd`: background daemon, autostarts at login. Scans the whole system on first start, caches the index on disk, keeps it current via file-change events, answers queries.
- `finder`: short-lived UI. Reads settings, queries `finderd` over a local socket or named pipe, opens the file manager, exits.

| | Linux | Windows |
|---|---|---|
| Index cache | `~/.cache/finder/index.bin` | `%LOCALAPPDATA%\Finder\index.bin` |
| Settings | `~/.config/finder/settings.conf` | `%APPDATA%\Finder\settings.conf` |
| IPC | Unix socket in `$XDG_RUNTIME_DIR`, mode 0600 | Named pipe `\\.\pipe\finder-<user SID>`, owner-only DACL |
| Roots | `/` minus `/proc`, `/sys`, `/dev`, `/run` and non-local mounts | Fixed drives |
| Live updates | inotify on `$HOME`; rest via 15-min catch-up | `ReadDirectoryChangesW` on every drive; 15-min catch-up as safety net |

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

UI checks without input devices: `cargo build --release -p finder --features uitest --target-dir target/uitest`, then `FINDER_UITEST="x,y;x,y" ./target/uitest/release/finder` replays clicks (`x,y`) or drags (`x,y>x2,y2`) at window coordinates (see `crates/finder/src/uitest.rs`).

Run `finder [keyword]` (`finder --settings` opens the settings panel); it starts `finderd` if needed (log: `~/.cache/finder/finderd.log`). `finderd --install` starts the daemon at login (systemd user service / Windows `Run` key) and adds a Finder app launcher; `--uninstall` undoes it. `finderd --rescan` forces a full scan.

Fedora deps: `rust cargo gcc-c++ libstdc++-static cmake libX11-devel libXext-devel libXft-devel libXinerama-devel libXcursor-devel libXrender-devel libXfixes-devel pango-devel cairo-devel wayland-devel wayland-protocols-devel libxkbcommon-devel dbus-devel mesa-libGL-devel`

### KRunner (Linux/KDE only)

`integrations/krunner/install.sh` adds a KRunner entry `Search "…" on Finder` to every query; Enter runs `finder <query>`. Needs `python3-dbus` and `python3-gobject`; `--uninstall` removes it. Windows has no equivalent plugin: `finderd --install` adds a Start-menu launcher there.

| Crate | Contents |
|---|---|
| `crates/core` | Paths, settings, IPC protocol, case folding |
| `crates/finderd` | Daemon: index, scan, change listener, search |
| `crates/finder` | UI |

## Settings

Plain text, `key = value`, no comments (older files are rewritten). Created with defaults if missing; re-read on every `finder` launch. Changed in the app with the toggles next to the search field (`Aa`, `Path`, `Hidden`; Alt+C/P/H), the settings panel (sliders button or Ctrl+,), or Ctrl +/- / Ctrl+0 for zoom; the app rewrites the file.

| Key | Default | Meaning |
|---|---|---|
| `case_sensitive` | `false` | Match case exactly |
| `match_full_path` | `false` | Match against the full path instead of the name only |
| `show_hidden` | `false` | Show hidden entries (names starting with `.`; on Windows also the hidden attribute) and everything below hidden folders |
| `exclude` | (empty) | Comma-separated folder names (`node_modules`, any depth, case-insensitive) or paths (`~/Downloads/old`) left out of results with everything below them |
| `theme` | `auto` | `auto` follows the desktop (KDE color scheme and accent / GTK dark preference; Windows app mode and accent), or `light` / `dark` |
| `zoom` | `100` | Window zoom in percent on top of the display scaling; saved when `finder` closes |
| `hotkey` | (empty) | Windows: global shortcut that opens `finder`, e.g. `ctrl+alt+space`, `win+shift+f`, `f12`; also in the settings panel. Registered by `finderd` (picks up edits live), so it opens instantly; a `.lnk` shortcut key goes through Explorer, which can stall ~3 s per hung window. Linux: bind `finder` in the desktop's shortcut settings |

Hidden files and exclusions are filtered at query time, so they stay in the index and changes apply immediately.

## Progress

- [x] Idea, README, GitHub repo
- [x] Architecture, language (Rust), UI toolkit (FLTK)
- [x] Project skeleton (`finderd` + `finder`)
- [x] Settings file
- [x] Index format + cache file
- [x] Initial scan: Linux
- [x] Initial scan: Windows (parallel directory walk)
- [x] Change listener: Linux
- [x] Change listener: Windows
- [x] Startup catch-up
- [x] Delta + compaction
- [x] Search engine
- [x] Daemon ↔ UI IPC (Unix socket / named pipe)
- [x] UI: keyword prompt + result list; launcher look, match highlighting, file-type icons, light/dark theme
- [x] In-app settings: toggles, settings panel, hidden files, excluded folders
- [x] Open Dolphin / Explorer
- [x] Autostart `finderd`
- [x] Benchmark RAM and latency against goals
- [ ] Packaging

## Implementation notes

Planned design for features not built yet. Delete each point once implemented; after that the code is the reference.

- **Initial scan (Windows):** faster path: read the NTFS MFT (`FSCTL_ENUM_USN_DATA`, needs admin); keep the directory walk as fallback.
- **Change listener:**
  - Linux (now: inotify on every dir under `$HOME`, rest via 15-min mtime catch-up): add `fanotify` with `FAN_MARK_FILESYSTEM` when `CAP_SYS_ADMIN` is available, for instant system-wide updates (verify on btrfs subvolumes).
  - Windows (now: `ReadDirectoryChangesW` per drive, affected dirs re-read from disk): optionally the USN journal (needs admin → run as service), which also survives daemon restarts.
- **Startup catch-up (Windows):** resume USN journal from saved position.

## License

MIT, see [LICENSE](LICENSE).
