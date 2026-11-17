# Finder

Fast, keyboard-driven file locator for Windows and Unix (Linux).

## Idea

1. App opens and prompts for a keyword.
2. User enters the keyword; the app lists every file and folder on the system whose name contains it.
3. User selects an entry.
4. App closes and opens the system file manager:
   - selected **folder** → open that folder
   - selected **file** → open its parent folder with the file highlighted

File managers: **Dolphin** on Linux (KDE), **Explorer** on Windows.

## Goals

- Speed first: results appear while typing (< 50 ms on ~2M files).
- Low footprint: the background part must stay small (see RAM budget).
- Minimal UI: type → pick → done.
- Cross-platform: one codebase for Windows and Linux.

## Architecture (proposed)

Language: **Rust** (no GC, low RAM, fast startup). Two processes:

- **`finderd`** — headless background daemon, autostarts at login. Owns the index, listens for file changes, answers queries. Loads no GUI toolkit, which keeps it small.
- **`finder`** — the UI. Launched by a global hotkey, talks to `finderd` over a local socket (Unix socket / named pipe, restricted to the owning user), shows results, opens the file manager, exits.

### Index lifecycle

1. **First start:** scan the whole system → write the index to a cache file
   (`~/.cache/finder/index.bin`, `%LOCALAPPDATA%\Finder\index.bin`).
2. **Later starts:** memory-map the cache file (loads in ms), then catch up on changes made while not running:
   - Windows: resume the USN journal from the saved position.
   - Linux: re-read only directories whose mtime changed (adds/removes/renames inside a dir update its mtime).
3. **While running:** the change listener records adds/removes in a small in-memory delta. When the delta grows or the system is idle, it is merged into a new cache file (write to a temp file, then atomic rename).

### Scanning and change listening

| OS | Initial scan | Change listener | Fallback (no admin/root) |
|---|---|---|---|
| Windows | Read NTFS MFT (`FSCTL_ENUM_USN_DATA`) | USN change journal | Parallel walk + `ReadDirectoryChangesW` (subtree) per drive |
| Linux | Parallel directory walk | `fanotify` (`FAN_MARK_FILESYSTEM`): one mark per filesystem | inotify on `$HOME` only + periodic dir-mtime rescan |

- MFT/USN need admin: run `finderd` as a Windows service.
- `fanotify` needs `CAP_SYS_ADMIN` (set it on the binary at install or run as a system service). Risk: verify it works on Fedora's btrfs subvolumes (`/` and `/home` are separate subvolumes).
- Avoid watching the whole system with inotify: each watch costs ~1 KB of kernel memory, one per directory.
- Excluded by default: `/proc`, `/sys`, `/dev`, `/run`, network/other mounts, btrfs snapshots (`/.snapshots`). Configurable.

### Index format (RAM budget)

- One fixed-size record per file/dir: parent index `u32`, name offset `u32`, name length `u16`, flags `u8` → 12 bytes. No per-file heap allocations.
- Names packed into one byte buffer, plus a lowercased copy that is the only part scanned during search.
- Full paths are never stored; they are rebuilt from the parent chain only for displayed results.
- The index lives in the memory-mapped cache file, so the OS loads pages on demand and can reclaim them under memory pressure.
- **Budget:** ~50 bytes per entry in total (≈ 100 MB for 2M files, mostly reclaimable), daemon heap < 20 MB, ~0% CPU when idle (change events are batched).

### Search

- Parallel SIMD substring scan over the lowercased name buffer: < 10 ms for 2–3M names. No trigram index needed below ~10M files.
- Each keystroke cancels the running query. Extending the query only re-filters the current results.
- Stream the first ~100 results; the UI renders only visible rows.
- Ranking: exact name > prefix > substring, then shorter path, then recently opened.

### Opening the result

- Linux: `dolphin --select <file>` / `dolphin <dir>`; fallback: D-Bus `org.freedesktop.FileManager1.ShowItems` (any file manager), then `xdg-open <dir>`.
- Windows: `explorer /select,"<file>"` / `explorer "<dir>"`.

## Open decisions

- UI toolkit
- Match rules: case sensitivity (default: insensitive), filename only (default) vs. full path
- Global hotkey mechanism (KDE custom shortcut running `finder` vs. daemon-registered hotkey on Windows)

## Progress

- [x] Project idea and README
- [x] GitHub repo created and linked
- [x] Search/indexing architecture proposed
- [ ] Confirm Rust, choose UI toolkit, set up project skeleton
- [ ] Index format + cache file (write, mmap load, atomic replace)
- [ ] Initial scan: Linux (parallel walk + exclusions)
- [ ] Initial scan: Windows (MFT; fallback parallel walk)
- [ ] Change listener: Linux (fanotify; inotify fallback)
- [ ] Change listener: Windows (USN journal; `ReadDirectoryChangesW` fallback)
- [ ] Startup catch-up (dir-mtime diff / USN resume)
- [ ] In-memory delta + compaction into cache file
- [ ] Search engine (parallel scan, cancel, incremental narrowing, ranking)
- [ ] Daemon ↔ UI IPC
- [ ] Keyword prompt UI + result list (keyboard navigation)
- [ ] Open Dolphin / Explorer at selected folder / file
- [ ] Global hotkey + autostart
- [ ] Benchmark RAM and latency on ~2M files against the budget
- [ ] Packaging (Linux + Windows)
