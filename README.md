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

Run `finder [keyword]`; it starts `finderd` if needed (log: `~/.cache/finder/finderd.log`). `finderd --install` starts the daemon at login (systemd user service / Windows `Run` key) and adds a Finder app launcher; `--uninstall` undoes it. `finderd --rescan` forces a full scan.

Fedora deps: `rust cargo gcc-c++ libstdc++-static cmake libX11-devel libXext-devel libXft-devel libXinerama-devel libXcursor-devel libXrender-devel libXfixes-devel pango-devel cairo-devel wayland-devel wayland-protocols-devel libxkbcommon-devel dbus-devel mesa-libGL-devel`

### KRunner (Linux/KDE only)

`integrations/krunner/install.sh` adds a KRunner entry `Search "…" on Finder` to every query; Enter runs `finder <query>`. Needs `python3-dbus` and `python3-gobject`; `--uninstall` removes it. Windows has no equivalent plugin: `finderd --install` adds a Start-menu launcher there.

| Crate | Contents |
|---|---|
| `crates/core` | Paths, settings, IPC protocol |
| `crates/finderd` | Daemon: index, scan, change listener, search |
| `crates/finder` | UI |

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
- [x] UI: keyword prompt + result list
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
