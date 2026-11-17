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

- Speed first: window visible < 100 ms after launch, results while typing (< 50 ms on ~2M files).
- Low footprint: the background part must stay small (see RAM budget).
- Minimal UI: type → pick → done.
- Cross-platform: one codebase for Windows and Linux.

## Architecture

- Language: **Rust** (no GC, low RAM, fast startup).
- UI toolkit: **FLTK** via [`fltk-rs`](https://github.com/fltk-rs/fltk-rs). Native Win32 / X11 / Wayland drawing, no GPU or webview startup cost, small static binary. Toolkits like egui, iced or Tauri pay a graphics/webview init cost on every launch, which matters because the UI is started fresh each time.

Two processes:

- **`finderd`**: headless background daemon, autostarts at login. Owns the index, listens for file changes, answers queries. Loads no GUI toolkit, which keeps it small.
- **`finder`**: the UI. Started manually for now (app launcher or terminal; no global shortcut yet). Reads the settings file, talks to `finderd` over a local socket (Unix socket / named pipe, restricted to the owning user), shows results, opens the file manager, exits.

### Settings

Plain-text file, `key = value` per line, `#` for comments. Created with defaults and comments on first start if missing.

- Linux: `~/.config/finder/settings.conf` (respects `$XDG_CONFIG_HOME`)
- Windows: `%APPDATA%\Finder\settings.conf`

```
# Match upper/lower case exactly (true/false)
case_sensitive = false

# Match the keyword against the full path instead of only the file/folder name (true/false)
match_full_path = false
```

`finder` reads the file on every launch and sends the options with each query, so changes apply without restarting `finderd`. Unknown keys or invalid values: warn and use the default.

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
- Records ordered parent-before-child (needed for full-path matching).
- Names packed into one byte buffer, plus a lowercased copy for case-insensitive search.
- Full paths are never stored; they are rebuilt from the parent chain only for displayed results.
- The index lives in the memory-mapped cache file, so the OS loads pages on demand and can reclaim them under memory pressure.
- **Budget:** ~50 bytes per entry in total (≈ 100 MB for 2M files, mostly reclaimable), daemon heap < 20 MB, ~0% CPU when idle (change events are batched).

### Search

- Parallel SIMD substring scan over the name buffer (lowercased buffer + lowercased keyword when `case_sensitive = false`, original buffer otherwise): < 10 ms for 2–3M names. No trigram index needed below ~10M files.
- `match_full_path = true`: an entry matches if its own name or any ancestor's name matches. Thanks to parent-before-child order this is one linear pass over a bitset. Keywords containing a path separator are matched component by component along the parent chain.
- Each keystroke cancels the running query. Extending the keyword only re-filters the current results.
- Stream the first ~100 results; the UI renders only visible rows.
- Ranking: exact name > prefix > substring, then shorter path, then recently opened.

### Opening the result

- Linux: `dolphin --select <file>` / `dolphin <dir>`; fallback: D-Bus `org.freedesktop.FileManager1.ShowItems` (any file manager), then `xdg-open <dir>`.
- Windows: `explorer /select,"<file>"` / `explorer "<dir>"`.

## Deferred

- Global keyboard shortcut to launch `finder`.

## Progress

- [x] Project idea and README
- [x] GitHub repo created and linked
- [x] Search/indexing architecture proposed
- [x] Language (Rust) and UI toolkit (FLTK) chosen
- [ ] Project skeleton (`finderd` + `finder` binaries)
- [ ] Settings file (create with defaults, parse, pass options with queries)
- [ ] Index format + cache file (write, mmap load, atomic replace)
- [ ] Initial scan: Linux (parallel walk + exclusions)
- [ ] Initial scan: Windows (MFT; fallback parallel walk)
- [ ] Change listener: Linux (fanotify; inotify fallback)
- [ ] Change listener: Windows (USN journal; `ReadDirectoryChangesW` fallback)
- [ ] Startup catch-up (dir-mtime diff / USN resume)
- [ ] In-memory delta + compaction into cache file
- [ ] Search engine (parallel scan, case modes, full-path mode, cancel, incremental narrowing, ranking)
- [ ] Daemon ↔ UI IPC
- [ ] Keyword prompt UI + result list (keyboard navigation)
- [ ] Open Dolphin / Explorer at selected folder / file
- [ ] Autostart `finderd` at login
- [ ] Benchmark RAM and latency on ~2M files against the budget
- [ ] Packaging (Linux + Windows)
