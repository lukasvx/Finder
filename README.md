# Finder

Fast, keyboard-driven file locator for Windows and Unix (Linux).

## Idea

1. App opens and prompts for a keyword.
2. User enters the keyword; the app lists every file and folder on the system whose name contains it.
3. User selects an entry.
4. App closes and opens the system file manager:
   - selected **folder** → open that folder
   - selected **file** → open its parent folder (ideally with the file highlighted)

File managers: **Dolphin** on Linux (KDE), **Explorer** on Windows. Fall back to the system default (`xdg-open`) where Dolphin isn't available.

## Goals

- Speed first: results should appear near-instantly, even on large disks.
- Minimal UI: type → pick → done.
- Cross-platform: one codebase for Windows and Linux.

## Open decisions

- Language / UI toolkit
- Search strategy: live filesystem walk vs. prebuilt index (or OS index: `locate`/`plocate` on Linux, Everything/NTFS MFT on Windows)
- Match rules: case sensitivity, filename only vs. full path
- Which roots to search and which to exclude (`/proc`, `/sys`, `node_modules`, …)

## Progress

- [x] Project idea and README
- [x] GitHub repo created and linked
- [ ] Choose language / UI toolkit
- [ ] Choose search strategy (walk vs. index)
- [ ] Keyword prompt UI
- [ ] File search + result list
- [ ] Result selection (keyboard navigation)
- [ ] Open Dolphin in the selected folder / parent folder (Linux)
- [ ] Open Explorer in the selected folder / parent folder (Windows)
- [ ] Performance tuning on large filesystems
- [ ] Packaging / global hotkey launch
