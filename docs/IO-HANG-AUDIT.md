# I/O hang audit — WinRust shell

**Snapshot:** 2026-11-26 ~08:50 local. The tree is dirty and `plugins/rust-explorer/src/{main,ui,jobs,drives}.rs` were being edited while this audit ran (`main.rs` mtime 08:43:56, `ui.rs` 08:44:34, `jobs.rs` 08:44:27, `drives.rs` 08:44:30). Line numbers are from that snapshot; the symbol names in each finding are the stable reference if a line moved.

**Scope:** every workspace member plus `src/` and `common/`, with priority on the file manager, copy/move/delete, task manager refresh, settings opening, topbar/Start, and search/du/wiztree. `chrome-rs` and `rust-search` are not workspace members (excluded in `Cargo.toml`); their UI/client code is still covered where the shell calls it.

**Severity**
- `FREEZE-AT-START` — blocks before/while the first window is usable.
- `FREEZE-ON-REFRESH` — blocks the UI thread on a timer, paint, refresh, or device-change path.
- `HANG-FOREVER` — an unbounded kernel/shell wait with no cancel; the app (or the operation) never returns.
- `SLOW-ONLY` — no indefinite wait proven, but it can add seconds on the UI thread.

**Status of the exact reported incident (important):** the original `WM_CREATE` → `ui::nav_refresh` → `ui::drive_items` → `GetVolumeInformationW`/`GetDiskFreeSpaceExW` freeze **has been fixed in this working tree**. `drive_items` now only reads `drives::cached()` (`plugins/rust-explorer/src/ui.rs:1282`), and `nav_refresh` starts a worker scan (`ui.rs:1271-1275`); `drives::probe` owns all volume calls on per-drive threads with a 3 s deadline (`plugins/rust-explorer/src/drives.rs:44,77-149`). The remaining boot/freeze vectors are `quick_items()` (still stats every Quick-access path), the synchronous folder listing in `load()`, and `start_target()` when launched with `/select` on a dead drive. They are findings F1–F3 below. Everything else in the report is still open.

---

## Summary table

| # | Severity | file:line (snapshot) | API / operation | Thread | Recommended fix |
|---|----------|----------------------|-----------------|--------|-----------------|
| F1 | HANG-FOREVER | `plugins/rust-explorer/src/main.rs:1160` (`load`→`rows_of:1126`) | `fs::read_dir`, `DirEntry::metadata` | UI: WM_CREATE:2556, tab/nav/back, `activate` | Worker listing + generation + `PostMessage`; keep old rows; volume-ready gate; cancel via `CancelSynchronousIo` |
| F2 | FREEZE-AT-START | `plugins/rust-explorer/src/ui.rs:1100` (`quick_items`), called `ui.rs:1313` | `Path::is_dir` on every Quick-access pin | UI: `WM_CREATE`→`nav_refresh:2487`; also `WM_DEVICECHANGE:2886` | No stat on UI: use drive cache + per-pin state; check on worker lazily |
| F3 | FREEZE-AT-START / HANG-FOREVER | `main.rs:1030` `start_target` (`p.is_file()`:1039); `main.rs:2937` WM_GO (`p.is_dir():2950`); `main.rs:742`/`811` (`is_file:822,828`) | `Path::is_file/is_dir` | UI: WM_CREATE, address Enter, right-click | Parse `/select` without stat; volume-ready gate; do existence checks on worker |
| F4 | FREEZE-ON-REFRESH | `ui.rs:361` (`list_draw`), `ui.rs:262` (`drive_cell`), `main.rs:2698` (`icon_of:1094`) | `GetDriveTypeW`, `SHGetFileInfoW`, `icon::best` | UI: `NM_CUSTOMDRAW` and `LVN_GETDISPINFO` | Use cached `Drive.ty`/icon; async icon service with negative cache; never call shell in paint |
| F5 | FREEZE-AT-START | `main.rs:2487`→`ui.rs:1313` (same as F2) plus `main.rs:2556` `load` | `is_dir` + `read_dir` in one WM_CREATE | UI: WM_CREATE | Fix F1/F2; defer `load` until a worker answer |
| F6 | HANG-FOREVER | `main.rs:650` `paste`, preflight at `:653` → `jobs.rs:178/183` | `is_dir`, `symlink_metadata`, `File::create` on destination | UI: Ctrl+V / menu paste | Move preflight into the worker; volume-ready gate; never create a probe file on the destination from the UI |
| F7 | HANG-FOREVER | `main.rs:1502` `IFileOperation::PerformOperations` (worker `:1531`) | Shell copy/move/delete, no Abort | Worker; UI stays alive but operation never ends | Keep `IFileOperation`, add `IFileOperationProgressSink`/`Abort`; `CancelSynchronousIo` on the worker thread; prefer `jobs::copy` for plain paths |
| F8 | HANG-FOREVER | `jobs.rs:303` `walk` + `:302` `free_name`; registration only at `:322` | `read_dir`, `metadata` before `io_threads` is populated | Job worker (not UI) | Register the job thread before `walk`; cancel checks inside `walk`; source/dest volume gate |
| F9 | FREEZE-ON-REFRESH | `plugins/rust-explorer/src/we.rs:387` (`file.exists()`), `:188`/`:194` (Steam library `is_file`) | `Path::exists`/`is_file` | UI: desktop `WM_TIMER` 2 s (`main.rs:2949`) | Cache existence per drive; check only on change; never every tick |
| F10 | HANG-FOREVER | `main.rs:2825` (`LVN_ENDLABELEDIT`), `main.rs:2250` (`drag_end`), `main.rs:640/643` (`new_entry`) | `fs::rename`, synchronous `sh::fileop`, `exists`/`create` | UI | `rename_timeout` (`main.rs:360`) on a worker; `fileop_async`; new-name search on worker |
| F11 | HANG-FOREVER | `main.rs:1753` `sh::show`, `:1610` `clip_set`, `:1823` `drag` → `ui()` `:1646` (`SHParseDisplayName:1651`, `QueryContextMenu:1758`) | Shell item creation + context-menu extensions | UI: right-click / Ctrl+C / drag | Volume-ready gate for shell items; build menus on an STA worker with owner window; bounded shell calls |
| F12 | HANG-FOREVER | `main.rs:1203` `search` (`ask:1190`, `fs::metadata:1212`, `local_search:1250`) | Pipe read with no deadline; per-hit `metadata`; `read_dir` walk | UI: address-bar search | Worker + generation + `PostMessage` (like `net_scan`); pipe deadline; don't stat hits |
| F13 | HANG-FOREVER (feature) | `main.rs:273` `trash_scan`; `:389` `trash_empty`; `:418` `trash_delete` | `fs::read_dir`/delete on every `$Recycle.Bin`, no timeout | Worker; `TRASH_LOADING` stays true if stuck | Skip drives not in `drives::cached()`; per-item `rename_timeout`; reset flag in all paths; cancel token |
| F14 | FREEZE-ON-REFRESH | `plugins/rtaskmgr-win32/src/sys.rs:444` (`hung_windows`) and `:289` (`window_titles`), called `data.rs:271,302` | `GetWindowTextW` on foreign windows | UI: 1 s/2 s timer | `SendMessageTimeoutW(WM_GETTEXT, SMTO_ABORTIFHUNG|SMTO_BLOCK, 50 ms)`; skip `IsHungAppWindow` |
| F15 | FREEZE-ON-REFRESH | `rtaskmgr/src/data.rs:315` → `suspect.rs:106` | `fs::metadata` on every process exe | UI: 1 s refresh | Worker existence check; drive-ready cache; negative cache |
| F16 | FREEZE-ON-REFRESH | `data.rs:264` (`perf.rs:178,151,157`), `:293` (disk capacity), `:296` (`sys.rs:56` services), `:417/418` (`hw.rs:174-186`) | PDH wildcard collect, sysinfo disks, `EnumServicesStatusExW`, `GetDiskFreeSpaceExW` | UI: 1 s timer | One background sampler thread publishes a snapshot; skip non-ready volumes; don't enumerate services on UI |
| F17 | FREEZE-AT-START | `rtaskmgr/src/main.rs:191` `Data::new` | `DXGI`/`D3D12CreateDevice`, first PDH collect, SMBIOS, icon load | UI before message loop | Create window first; initialise data on worker; incremental first paint |
| F18 | SLOW-ONLY | `rtaskmgr/src/startup.rs:124` (`:251` read_dir, `:205` command output), `:394-401` `wevtutil` | Task XML walk, `sc`/`schtasks`, event log | UI: Startup page every 10 ticks; Insights paint (`data.rs:582`, `draw.rs:249`) | Cache, refresh on worker, never `.output()` on UI |
| F19 | FREEZE-AT-START | `plugins/topbar/src/main.rs:1390` `load_pins` (call `:3820`) → `:1233/1235` `exists`, `:1416` `lnk_icon` → `:1120-1135` `item_icon` | `Path::exists`, `SHCreateItemFromParsingName`, `GetImage`, `icon::best` | UI main thread before window/message loop | Load pins + icons on worker; placeholders; drive cache |
| F20 | FREEZE-ON-REFRESH | `topbar/src/main.rs:1442` `refresh_buttons` → `:1498` `win_meta` → `:1165` `win_icon` → `:1120` `item_icon` | Shell icon for every new window | UI: 1 s/5 s timer (`:3630-3634`) | Async icon queue keyed by exe; negative cache; never shell in timer |
| F21 | FREEZE-ON-REFRESH | `topbar/src/fly.rs:2078-2082` (`row_icon:451`, `exe_icon:419`, `lnk_icon_px:1088`) | Icon extraction in Start panel paint | UI: flyout `WM_PAINT` (`fly.rs:2149`) | Async icon worker like `rs-launcher.rs:1306-1376`; draw placeholder |
| F22 | FREEZE-ON-REFRESH | `topbar/src/notify.rs:262` (`icon::best`) | Notification icon path | UI: bar thread `POST_MSG` (`notify.rs:141`) | Async icon / placeholder |
| F23 | SLOW-ONLY | `topbar/src/fly.rs:1155,1159,2175` → `:617`; `topbar/src/status.rs:53` | `WlanScan`, `WlanGetAvailableNetworkList`, WLAN query | UI: panel open and 1 s timer | Cache + worker; don't query WLAN in paint/timer |
| F24 | FREEZE-AT-START | `plugins/rust-settings/src/hub.rs:1580` `pages()` → `main.rs:700` `power_rows` (`:600` `ext::bin_bytes`, `:611` `power::drives`) | `SHQueryRecycleBinW(NULL)` + `GetDriveTypeW` on every letter | UI before window | Build window first; lazy pages; workers; drive cache; bin only on ready drives |
| F25 | FREEZE-ON-REFRESH | `rust-settings/src/main.rs:1397/1400` (`dynamic`) from `:1860-1874` WM_TIMER 2 | Same `SHQueryRecycleBinW`/`GetDriveTypeW`; BT WinRT joins | UI every 1.5–4 s | Cache the values; per-page worker; stop refreshing the Power page on every tick |
| F26 | HANG-FOREVER | `rust-settings/src/main.rs:1491-1492` → `ext.rs:251-253` | `SHEmptyRecycleBinW(NULL)` | UI: “Empty Recycle Bin” click | Worker + `IFileOperation` per bin / ready drives; progress + cancel |
| F27 | FREEZE-AT-START | `rust-settings/src/main.rs:623` (`bt.rs:54,117`), `:509` (`wifi.rs:171`), `:577` (`audio.rs:136`) | WinRT `.join()`, WLAN list, audio session COM | UI before window | Lazy page load + worker; never `.join()` on UI |
| F28 | HANG-FOREVER | `rust-notepad/src/main.rs:327,368` | `fs::read`, `fs::write` | UI: open/save | Worker + cancel; temp + rename; status line |
| F29 | HANG-FOREVER | `photoview/src/main.rs:56`, `:158` | `read_dir`, `SHFileOperationW` | UI: open / Delete | Worker folder scan with generation; delete through worker shell op |
| F30 | HANG-FOREVER | `rs-shot/src/editor.rs:635-639` (`save.rs:95-105,195`), `record.rs:131` | `Path::exists` loop + GDI+ save / default video path | UI: editor save / record start | Save on worker; bounded unique-name loop; volume gate |
| F31 | FREEZE-AT-START | `rs-shell/src/main.rs:110,123`, `:393-401`, `:256` | `read_dir` Startup, `Path::exists`, wallpaper BMP worker + `join` | Shell bootstrap thread | Drive cache; no `exists` for Run entries on dead letters; don't join before desktop |
| F32 | HANG-FOREVER (app) | `wiztree/src/main.rs:138` (`scan`), `:128` (`GetDriveTypeW`) | Raw volume MFT scan; drive probe on Tab | Worker (scan) / UI (Tab) | Cancellable scan + generation; drive cache; one scan at a time |
| F33 | SLOW-ONLY | `rust-control/src/pages/security.rs:49-53` | `netsh advfirewall ... .output()` | UI: Security page navigation | Run on worker with timeout; cache |
| F34 | SLOW-ONLY | `rs-clip/src/main.rs:158-164` | Clipboard capture writes image files | UI: `WM_CLIPBOARDUPDATE` | Queue capture to worker |
| F35 | FREEZE-ON-REFRESH / HANG | `common/src/icon.rs:54-92` (`best:63,65,200-235`) | `SHGetFileInfoW`/`ExtractIconExW`, no negative cache | Any caller (often UI) | Split into `request_async` (worker + `PostMessage` + negative cache) and blocking `best` for workers only |
| F36 | SLOW-ONLY / HANG (worker) | `common/src/search.rs:20-39` | Pipe read until EOF, no deadline | Worker in topbar/rs-launcher; explorer has its own copy (F12) | Overlapped/`PeekNamedPipe` deadline or a reader thread with timeout |
| F37 | SLOW-ONLY | `rust-explorer/src/openwith.rs:1241,1314` | `icon::best` for every registered app | UI: Open-with dialog | Async icons / placeholder |

---

## Findings

### F1 — `rust-explorer` folder listing runs on the UI thread — HANG-FOREVER
`main.rs:1160` `load()` calls `rows_of()` (`main.rs:1126`), which does `fs::read_dir` (`:1145`) and `e.metadata()` (`:1147`) for every entry. It is called from `WM_CREATE` (`:2556`), from every tab switch (`tab_select`, `tab_open`), from Back/Up (`WM_NAV`), from `activate()` on a folder double-click, and after every copy/delete/rename (`reload:1277`). There is no worker, deadline, or generation.

Why a stalled device blocks: `NtCreateFile`/`NtQueryDirectoryFile`/`NtQueryInformationFile` wait in the volume/redirector stack. A removed USB with a stale letter or a dead SMB share leaves those calls in the kernel for minutes. The UI thread is inside `read_dir`/`metadata`, so it cannot paint, time out, or process WM_CLOSE.

Proof of thread: the only message loop is `GetMessageW` at `main.rs:3100+`; `WM_CREATE` and `tab_select` run under `proc` (`main.rs:2470`); `rows_of` is called directly from there.

Fix: mirror the `net_scan` pattern (`ui.rs:1167-1246`): a worker with a generation counter, the result posted as `WM_FOLDER` (`lp = Box<FolderResult>`), and a `drive_ready()` pre-check. Keep the old rows while loading; if the volume is not `Ready` in `drives::cached()`, show “drive not available” without touching it. Register the listing thread in the cancellable-I/O registry (F8 helper) so Esc/WM_CLOSE can `CancelSynchronousIo` it. `zipx::list_dir` on a `.zip` on a dead drive must use the same worker.

### F2 — Quick-access pins are stat'ed during `WM_CREATE` — FREEZE-AT-START
`ui.rs:1100` `quick_items()` filters the pin file with `p.is_dir()`. `nav_init` calls it at `ui.rs:1313`; `nav_refresh` (`ui.rs:1271`) is called from `WM_CREATE` (`main.rs:2487`), from `WM_DEVICECHANGE` (`main.rs:2886`), from F5 (`main.rs:2800`) and from `WM_NET` (`main.rs:2902`).

If the user pinned `E:\Photos` (or a shortcut’s target folder) and `E:` is the dead USB, the window never appears: `is_dir` waits in the volume driver before the first paint. Even without a dead `E:`, a disconnected network pin does the same. The new `drives` cache does not help here because Quick access is stat'ed directly.

Fix: never call `is_dir` from `nav_init`. Keep a `QuickPin { path, state }` cache keyed by path, and resolve each pin on the same drive worker (or a tiny per-pin worker) with a 2–3 s deadline. Draw the pin immediately; mark it with a warning glyph/“unavailable” when the reply is `NoRoot`/timeout. `holds()`/`ready()` from `drives.rs` already gives the fast answer when the letter is in the cache.

### F3 — Arbitrary-path stats on startup, address bar, and right-click — FREEZE-AT-START / HANG-FOREVER
- `start_target()` (`main.rs:1030`) calls `p.is_file()` (`:1039`) while handling `/select,"E:\..."` in `WM_CREATE`. This is exactly what `winrust_common::reveal()` passes when another app says “show this file”; a process whose image was on the unplugged drive makes the new Explorer window die on the launch path. The `Task Manager → Open file location` action (`rtaskmgr app.rs:1133-1139`) calls `reveal()`, which starts `rust-explorer /select,...`, so one dead path can freeze both apps.
- `WM_GO` (`main.rs:2937`) does `p.is_dir()` (`:2950`) before `load`; typing `E:\` in the address bar blocks the UI thread.
- `activate()` (`main.rs:742`) and `item_menu()` (`main.rs:811`) call `p.is_file()` per selected row (`:754`, `:822`, `:828`). Search results keep absolute paths from a drive that may have died since the search; right-clicking or opening one blocks.

Fix: make `start_target` path-only for `/select` (no stat; `load` will show “not available” from the worker). In `WM_GO`/`item_menu`, consult `drives::ready()`/`holds()` first and dispatch any remaining stat to a worker; never call `is_file` on a row whose drive is not in the `Ready` cache.

### F4 — Drive/type/icon queries in list paint and `GETDISPINFO` — FREEZE-ON-REFRESH
`ui.rs:315` `list_draw` handles `NM_CUSTOMDRAW`; at `:361` it calls `GetDriveTypeW` for the row’s path on **every item paint** (not the cached `Drive.ty`). `ui.rs:262` (`drive_cell`, This PC icon grid) calls `winrust_common::icon::best(&path, px)` for the drive root. `main.rs:2698` `LVN_GETDISPINFO` calls `icon_of()` (`:1094`) when `r.icon < 0`; for `.exe/.lnk/.ico/.url/.scr` it calls `SHGetFileInfoW` without `SHGFI_USEFILEATTRIBUTES` (`:1094-1102`), so it touches the file. `desk_icon` uses `icon::best` too (`:842-849`).

Why: `GetDriveTypeW` and `SHGetFileInfoW` go through the shell/volume providers; a stale letter or dead share blocks inside the provider. Because `r.icon` is assigned only after the call returns, a timeout/failure is retried on the next paint — the freeze repeats.

Fix: use the cached `Drive.ty`/`Drive.icon` from `drives.rs` in `list_draw` and `drive_cell` (no live calls). Move `icon_of`/`desk_icon` to the existing thumbnail worker pattern (`sh::start_thumbs`, `WM_THUMB`, `GEN`), or to a shared async icon service (F35); draw `I_IMAGENONE` while pending. Negative-cache the result immediately so one dead path is asked once.

### F5 — `WM_CREATE` still does sync work after `nav_refresh` — FREEZE-AT-START
`WM_CREATE` (`main.rs:2479`) calls `nav_refresh` (`:2487`) and later `load(h, start)` (`:2556`). Even after the drive cache fix, `quick_items` (F2) runs in `nav_refresh`, and `load` (F1) runs synchronously. A startup with `/select,E:\...`, a pinned dead drive, or a desktop redirected to a dead share freezes before the window is shown.

Fix: F1/F2, plus make `load` return immediately and post the rows later; show the window with a “Loading…” status. This also makes the first paint independent of any device.

### F6 — Paste preflight touches the destination on the UI thread — HANG-FOREVER
`paste()` (`main.rs:650`) runs `jobs::can_fast_copy(&paths, dir)` and `jobs::writable(dir)` at `:653` before spawning anything.
- `can_fast_copy` (`jobs.rs:178`) calls `to.is_dir()` and `fs::symlink_metadata(p)` for every source.
- `writable` (`jobs.rs:183-188`) does `fs::File::create(dir/.winrust-<pid>.tmp)`, i.e. a real create on the destination.

Pasting to a dead `E:` therefore blocks in `CreateFileW` **on the UI thread before the copy job exists**. There is no Esc handling because `jobs::busy()` is still false, and no watchdog/`CancelSynchronousIo` because no `Progress` was created. This is the clearest reproduction of “copy to E: hung forever with no way to cancel”.

Fix: spawn the job first. Inside the worker: volume-ready check for source and destination, source existence, destination writability (create/delete the probe there), then the copy. `jobs::can_fast_copy`/`writable` must never be called from a message handler. If the destination is not `Ready` in the drive cache, fail immediately with “drive not available” and keep the source untouched. Esc must set the cancel flag even during preflight.

### F7 — Shell file operations on a worker have no abort — HANG-FOREVER
`spawn_fileop` (`main.rs:1531`) runs `fileop_ex` on its own STA; `fileop_ex` (`:1483`) does `PerformOperations` (`:1502`). The UI stays responsive, but the operation has no `Abort`/progress sink and no thread registration for `CancelSynchronousIo`. A copy/move to a removed USB can sit in the shell’s copy engine forever; the shell progress dialog (owner is the main window) offers no working cancel, and closing the app does not stop the worker.

Fix: hold the `IFileOperation` and a progress sink; on cancel call `IFileOperation::Abort` and `CancelSynchronousIo` on the worker thread. For plain `PathBuf` sources/destinations route through `jobs::copy` instead (it already has a stall watchdog and `CancelSynchronousIo`); keep `IFileOperation` only for shell items/elevation. Add a deadline to the worker and report “operation did not answer” rather than silently hanging.

### F8 — Copy `walk` runs before the cancel registry is populated — HANG-FOREVER
`copy_now` (`jobs.rs:298`) calls `free_name` (`:302`) and `walk` (`:303`) before any copy thread is registered. Only the per-file copy threads push their ids into `p.io_threads` (`:322`). The 45 s stall watchdog (`STALL`, `:32`) measures bytes; during `walk` no bytes move, but `abort_io` (`:56-68`) has an empty thread list, and `cancel()` only sets a flag that `walk` never checks. A copy **from** a dead `E:` (source unplugged after the copy was queued) therefore blocks forever with no Esc effect. `free_name`’s `symlink_metadata` and `seek_penalty`’s `CreateFileW(\\.\E:)` add the same risk on the worker.

Fix: register the job thread (and any `walk` thread) before the first filesystem call; check `p.cancelled()` in `walk`/`free_name`; add the drive-ready gate before walking; use `CancelSynchronousIo` for the registered thread when Esc/stall fires. `walk` should return `Interrupted` and let the existing “Cancelled” path run. Also delete the partial destination on cancel (the current `copy_file` does, but `walk` failures happen before any destination exists).

### F9 — Wallpaper Engine tick stats paths every 2 s — FREEZE-ON-REFRESH
`we::tick` (`we.rs:296`) runs on the desktop’s `WM_TIMER` id 7 (`main.rs:2949`). Every tick with a configured wallpaper does `if !file.exists()` (`we.rs:387`) on `we-file`. `find_exe` (`we.rs:182-195`) reads Steam’s `libraryfolders.vdf` and calls `p.is_file()` for every library/name every 30 s. If the wallpaper project or a Steam library is on the dead `E:`/dead share, the desktop thread freezes every 2 s/30 s even with no file window open.

Fix: resolve `we-file` and the WE exe through `drives::cached()`; cache the existence result and refresh it on `WM_DEVICECHANGE` or when the user changes `we-file`, not on every tick. `find_exe` should run once on a worker and retry with backoff.

### F10 — Rename, desktop drag-move and “New folder” on the UI thread — HANG-FOREVER
- `LVN_ENDLABELEDITW` (`main.rs:2819`) calls `fs::rename` at `:2825` directly. A rename on `E:` blocks the list while the user is still in the edit box.
- `desk::drag_end` (`main.rs:2223`) calls `sh::fileop(..., Op::Move, ...)` synchronously at `:2250` (“a rename: instant” — only if the target volume answers).
- `new_entry` (`main.rs:640`) searches `!p.exists()` (`:643`) and calls `create_dir`/`File::create` on the current folder.

Fix: use the now-existing `rename_timeout()` helper (`main.rs:360`) on a worker for label edits and desktop moves; the synchronous `fileop` in `drag_end` must become `fileop_async` or `jobs::copy` with `then_delete`. New-folder creation should be a worker operation with a “could not create” notice.

### F11 — Shell context menu / clipboard / drag create shell items on the UI thread — HANG-FOREVER
`sh::show` (`main.rs:1753`) calls `ui()` (`:1646`), which does `SHParseDisplayName` for every selected path (`:1651`), then `QueryContextMenu` (`:1758`). `sh::clip_set` (`:1610`) and `sh::drag` (`:1823`) use the same `ui()`. For a path on a dead device, `SHParseDisplayName`/the context-menu extensions may read the target (and network locations can stall). Right-click (`NM_RCLICK`, `main.rs:2734`), Ctrl+C/X (`clip_selected:606`) and drag (`LVN_BEGINDRAG:2754`) all run on the UI thread.

Fix: gate with `drives::holds(path)`/`ready`; for shell items that pass, build the menu on an STA worker (with `SetOwnerWindow` and a private message loop) or at least wrap the call with a watchdog + `CancelSynchronousIo` on the worker. For plain files where our own menu is enough, avoid shell item creation entirely.

### F12 — Search blocks the address-bar handler — HANG-FOREVER
`search()` (`main.rs:1203`) loops A–Z calling `ask()` (`:1190`): `OpenOptions` on `\\.\pipe\rs-search-<d>`, `write_all`, then `read` until EOF with **no deadline** (`:1196-1198`). It then calls `fs::metadata(line)` for every hit (`:1212`) and, without a service, `local_search()` (`:1250`), which does `read_dir`/`metadata` in a 1.5 s (wall-clock-checked but still synchronous) walk. All of this is in the `WM_GO` handler (`:2937`).

A rust-search service that is rebuilding a dead drive’s journal, or a hit path on dead `E:`, freezes the file window for minutes. `ask()` can return an error only at EOF; a wedged server never hangs up.

Fix: move the whole search to a worker with a generation counter; post `WM_SEARCH` with the rows. Read the pipe with `PeekNamedPipe` polling + a 300–500 ms deadline (or overlapped I/O with `CancelIoEx`). Do not stat hits on the UI; either show them with unknown size/type (resolved lazily on selection) or stat on the worker with the drive-ready gate.

### F13 — Recycle Bin scan/delete workers have no timeout or generation — HANG-FOREVER (feature)
`trash_scan` (`main.rs:273`) sets `TRASH_LOADING` and spawns a worker that calls `trash_rows` (`:249`), which reads `$Recycle.Bin` of every drive (`drives::trash_bins`). If one bin is on dead `E:`, the worker blocks; `TRASH_LOADING` is cleared only after `trash_rows` returns (`:278`), so later openings of the Recycle Bin never retry. `trash_empty` (`:389`) and `trash_delete` (`:418`) delete payloads without the new `rename_timeout`/drive guard; `trash_restore` was fixed with `drives::ready()/holds()` (`:322`) and `rename_timeout` (`:335,360`).

Fix: use `drives::cached()` to skip bins on non-ready volumes; reset `TRASH_LOADING` in a guard/finally; add a generation so a late worker cannot overwrite a newer view; use `rename_timeout`/`CancelSynchronousIo` for deletes as well.

### Task-manager note: no WMI, but the same class of wait
`rtaskmgr` was checked specifically for WMI: there is **no WMI/CIM call** in `plugins/rtaskmgr-win32` (no `CoCreateInstance` of `WbemLocator`, no `wmic`/`Get-CimInstance`). Its per-process data comes from `sysinfo`; per-process disk bytes come from the process I/O counters; disk/GPU/network counters come from PDH. `QueryFullProcessImageNameW` (`topbar/main.rs:994-1010`) and `GetProcessImageFileName`-class calls only read the kernel’s recorded image path — they do not touch the device — but the resulting path is then passed to `suspect::file_missing` (F15) and `item_icon`/`SHGetFileInfo` (F20), which do. So the task-manager hazard is not WMI: it is `GetWindowTextW`, `fs::metadata`, PDH wildcard collection, `EnumServicesStatusEx`, and shell icon extraction.

### F14 — `GetWindowTextW` on foreign windows freezes the task manager — FREEZE-ON-REFRESH
`sys::window_titles` (`sys.rs:279`) calls `GetWindowTextW` for every visible top-level window (`:289`) every other tick; `sys::hung_windows` (`:436`) calls it for every hung window (`:444`) every tick. `GetWindowTextW` on another process’s window sends `WM_GETTEXT` synchronously; if the owner is hung (exactly the rust-explorer window waiting in `E:`), it blocks for the hung-window timeout (about 5 s) per window. `Data::refresh` holds `&mut Data` during this, so paint cannot run: the task manager appears hung too. This is the most likely reason the task manager froze even though none of its own paths were on `E:`.

Fix: use `SendMessageTimeoutW(hwnd, WM_GETTEXT, ..., SMTO_ABORTIFHUNG | SMTO_BLOCK, 50, &mut r)` and fall back to `GetWindowTextW` only for this process’s windows. Skip `IsHungAppWindow` windows in `window_titles`; cache titles by `(hwnd, pid)` for a few seconds. The shared helper should live in `common` and replace both functions.

### F15 — Per-process `fs::metadata` on the task-manager UI thread — FREEZE-ON-REFRESH
`Data::refresh` (`data.rs:315`) calls `suspect::file_missing(exe)` (`suspect.rs:106`: `std::fs::metadata`) for each process on a rotating schedule (every 3–60 ticks, and immediately for a newly seen process). A process launched from `E:` keeps its exe path in the kernel; `metadata` opens it and blocks in the volume driver while the task manager’s UI thread waits.

Fix: do the existence check on a worker (or through the volume cache: a path whose letter is not in `Ready` is “unknown”, not “missing”). Negative-cache `NotFound`; never call `metadata` inside `refresh`. Keep `SigCache` on its worker (`suspect.rs:139-171`) — same queue discipline.

### F16 — PDH/disk/service enumeration on the task-manager UI thread — FREEZE-ON-REFRESH
`Data::refresh` runs on the 1 s `nwg::Timer` (`main.rs:161,317` → `app.rs:774/782`):
- `self.perf.sample()` (`data.rs:264`) → `PdhCollectQueryData` (`perf.rs:178`) plus four `PdhGetFormattedCounterArrayW` calls for `PhysicalDisk(*)`, GPU engine/memory and network (`perf.rs:151,157`). These talk to the PDH providers; a removed USB or a wedged GPU/disk driver can hold the provider for seconds (and PDH has no caller-visible timeout).
- `disk_capacity()` (`data.rs:293`, `:618`) builds `sysinfo::Disks::new_with_refreshed_list()`, which asks every mounted volume for space (`GetDiskFreeSpaceExW`-class calls).
- `self.svcs = sys::services()` every 5 ticks (`data.rs:296`) → `OpenSCManagerW`/`EnumServicesStatusExW` (`sys.rs:40-68`). A service stuck in a start/stop transition can hold the SCM call for its timeout.
- `hw::disk_info_for`/`hw::volumes_total` (`data.rs:417/418`; `hw.rs:174-186,204-254`) open `\\.\PhysicalDriveN` and call `GetDiskFreeSpaceExW` for the sample’s letters when a new disk appears.

Fix: one background sampler thread produces an immutable `Sample`/`SysSnapshot`; the UI timer only swaps the reference and repaints. Skip volumes not in the shared `Ready` cache. Bound PDH by keeping it on the sampler and, if it stalls, skip that sample rather than queueing more. Services and disk capacity belong on the same sampler with their own intervals.

### F17 — Task-manager startup does GPU/PDH/hardware work before the loop — FREEZE-AT-START
`main.rs:191` constructs `Data::new` before `app.tick()` (`:209`) and the message loop. `Data::new` (`data.rs:167-175`) calls `gpu::adapters()` (`gpu.rs:151`), which creates a D3D12 device once per feature level per adapter (`gpu.rs:71-91`), `perf::Perf::new()` (adds wildcard counters and does the first collect, `perf.rs:96-131`), `hw::memory_hw()` (SMBIOS), `ResCache::new()` (`res.rs:104-108`, `ExtractIconExW("shell32.dll")`) and possibly `net::start()` (ETW). A hung GPU driver, a corrupt PDH registry, or a stalled device makes the window appear but never paint.

Fix: create/show the window first; initialise hardware data on a worker and post a `WM_DATA_READY` (or let `tick` see “not ready”). `Perf::new` should be fallible asynchronously; keep the CPU/memory basics available immediately.

### F18 — Startup page and boot log scan on the UI thread — SLOW-ONLY
`startup::list()` (`startup.rs:124`) walks `System32\Tasks` recursively (`:246-263`) and reads every task XML (`:251,257`), plus the Run keys and Startup folders. It is called from `go_page` (`app.rs:732`) and from the tick every 10 ticks while the Startup page is open (`app.rs:800-802`). `startup::boot_log()` (`startup.rs:394-403`) spawns `wevtutil.exe ... .output()` on first use; `Data::hints()` calls it (`data.rs:582`), and `hints` is called from `draw_insights` (`draw.rs:249`) and `content_height` (`app.rs:286`) during paint.

`StartupEnable/Disable` (`app.rs:1200+` → `startup::set_enabled`) runs `schtasks`/`sc` with `.output()` (`startup.rs:203-209`) on the UI thread; an unresponsive SCM/scheduler freezes the click handler.

Fix: cache `startup::list()` and refresh it on a worker; load the boot log once at startup on a worker; run `set_enabled` on a worker and report via a toast. Avoid `.output()` in any message handler.

### F19 — Topbar pins/icons resolve at shell startup — FREEZE-AT-START
`load_pins()` (`topbar/main.rs:1390`) runs on the main thread at `:3820`, before `bars_sync()`/`GetMessageW`. For every taskbar/Superbar pin it:
- calls `fix_msix_pin` (`:1233`), which does `Path::new(target).exists()` (`:1235`), `GetPackagesByPackageFamily`/`GetPackagePathByFullName` (`:1240-1257`), then loads/saves the `.lnk`;
- builds the icon via `lnk_icon` (`:1416`) → `sys_pin_icon`/`lnk_icon_px` (`:1088`) → `item_icon` (`:1120`, `SHCreateItemFromParsingName` + `IShellItemImageFactory::GetImage`) → `icon::best` (`:1095`) → `ExtractIconExW` (`:1101`);
- `taskband_links` (`:1333`) also does `real.is_file()` (`:1371`) for pinned shortcuts.

If a pin points to an app on the unplugged `E:` (a portable app on a USB stick), the shell has no bar until those calls return. This is a genuine “shell does not start after boot” scenario.

Fix: load the pin list from its files first (no target resolution), show placeholder icons, and resolve targets/icons on a worker with `WM_PIN_ICON`. Gate `exists` with the drive cache; cache negative results. Never run `IShellItemImageFactory`/`ExtractIconEx` on the startup thread.

### F20 — Taskbar button icons are built on the bar timer — FREEZE-ON-REFRESH
`refresh_buttons` (`topbar/main.rs:1442`) runs on the 1 s status tick and the 5-tick safety refresh (`:3630-3634`). For a new window it calls `win_meta` (`:1498`) → `win_icon` (`:1165`) → `item_icon(&exe)` (`:1187`). `item_icon` uses `SHCreateItemFromParsingName` + `GetImage`; for an exe on a dead device this blocks in the shell icon provider. The icon cache is filled only after a successful call, so a failed/timeout lookup is retried.

Fix: keep a per-exe icon state (`Pending/Ready/None`), insert `Pending` immediately, and do the shell extraction on an icon worker that posts `WM_BTN_ICON`. Use the cached drive state to mark exes on non-ready letters as `None` without calling the shell.

### F21 — Start panel paint loads icons synchronously — FREEZE-ON-REFRESH
`fly::paint` (`fly.rs:1577`) builds the rows on `WM_PAINT` (`:2149`). The Windows 10/7 style paths call `start_rows()`/`start_pins()` (`:878-879`, `:2006-2007`, `:2214`), which call `row_icon` (`:451`) → `exe_icon` (`:419`: `icon::best`, `ExtractIconExW`, `SHGetFileInfoW`) or `lnk_icon_px` (`:467`). The built-in panel repeats the same at `:2078-2082`. A shortcut/app installed on `E:` freezes the Start panel on open; the icon cache (`ICONS`) is only populated after success.

Fix: use the `rs-launcher` pattern (`rust-search/src/bin/rs-launcher.rs:1306-1376`): mark the path as pending in the icon map, spawn a worker that calls `SHGetFileInfoW`/`ExtractIconEx`, post `WM_ICONS`, and redraw. Add a 30–60 s negative TTL for a failed path; on a non-ready drive, don’t ask at all.

### F22 — Notification icons are extracted on the bar thread — FREEZE-ON-REFRESH
`notify::show()` (`notify.rs:195`) calls `winrust_common::icon::best(Path::new(&n.icon), sc(32))` at `:262` before creating the toast window. `show` is reached from `pump()` (`:141`), which runs in the bar’s `POST_MSG` handler (`topbar/main.rs:3604`). A notification whose icon path is on the dead `E:` (an app that was installed there) freezes the whole bar, not just the toast.

Fix: create the toast with a generic icon, request the real icon on a worker, and update the toast when it arrives. `icon::best` must not be called on the bar thread.

### F23 — WLAN scan/enumeration on the bar and flyout UI threads — SLOW-ONLY
`fly::toggle` on open calls `WlanScan` and `read_nets()` (`fly.rs:1153-1156`, `:1159`), and the flyout’s 1 s `WM_TIMER` calls `read_nets()` again (`:2175`). `read_nets` → `read_one` → `WlanGetAvailableNetworkList` (`:617`) and `WlanQueryInterface` (`:637`). `status.rs::read_wifi` (`:53-90`) runs on the bar’s 1 s timer and calls `WlanEnumInterfaces`/`WlanQueryInterface`. If `wlansvc` is wedged or the adapter is resetting, these calls can take seconds; because the bar and its flyouts share one thread, the bar stops repainting.

Fix: move WLAN enumeration to a worker that updates a cached `Vec<Net>` and posts a message; the panel paints from the cache. Add a 1–2 s deadline per refresh and keep the last list on timeout.

### F24 — Settings builds every page (including drive/bin/BT/audio probes) before the window — FREEZE-AT-START
`hub::run` calls `crate::pages()` (`hub.rs:1580`) before `create(page)`. `pages()` (`main.rs:690-731`) eagerly calls:
- `power_rows()` (`:700`) → `ext::bin_bytes()` (`:600`) → `SHQueryRecycleBinW(NULL, ...)` (`ext.rs:243-248`), which asks **every drive’s** Recycle Bin, and `power::drives()` (`main.rs:611`) → `GetDriveTypeW`/`GetDiskFreeSpaceExW` for every logical drive (`power.rs:38-50`);
- `bt_rows()` (`:623`) → `bt::state()` (`bt.rs:44`), including `Radio::GetRadiosAsync().join()` (`bt.rs:54`) and `DeviceInformation::FindAllAsyncAqsFilter(...).join()` (`bt.rs:117`);
- `network_rows()` (`:509`) → `wifi::list()` (`wifi.rs:166-190`, `WlanGetAvailableNetworkList`);
- `sound_rows()` (`:577`) → `audio::apps()` (`audio.rs:136`, audio session COM).

Any of these can take seconds; `SHQueryRecycleBinW` can wait on the dead USB for minutes. The user sees no Settings window at all. This is the second most likely “settings opening hangs” report after the drive cache fix.

Fix: create/show the window first, then load the current page on a worker; keep the page’s rows as `Option<Vec<Row>>` with a spinner. `power::drives` and bin size must come from a cached/guarded volume snapshot; `bt::state`/WLAN/audio must be worker-only and never called from `pages()`. Prefer the shared `drives` cache from F35/helper 1.

### F25 — Settings refresh timer re-runs drive/bin/BT probes — FREEZE-ON-REFRESH
`refresh_page()` (`main.rs:1397`) computes `dynamic(page)` at `:1400`. It is called from many places, notably the 1.5–4 s `WM_TIMER` 2 (`:1860-1874`: `refresh_page` for every page except Apps). On the Power page this re-runs `power_rows`: `SHQueryRecycleBinW(NULL)` + `GetDriveTypeW`/`GetDiskFreeSpaceExW` every few seconds. On the Bluetooth page it re-runs `bt::state()` with WinRT `.join()`. A dead `E:` therefore produces a repeating multi-second freeze while Settings is open, not just at launch.

Fix: cache the values (`bin_bytes`, `drives`, BT state) and refresh them on a worker with a TTL (e.g. 30 s for disk/bin, on demand for BT). `refresh_page` should read the cached values only; the timer should not perform I/O.

### F26 — “Empty Recycle Bin” runs `SHEmptyRecycleBinW` on the UI thread — HANG-FOREVER
`Cmd::EmptyBin` (`main.rs:1491-1492`) calls `ext::empty_bin(hwnd)` (`ext.rs:251-253`), which calls `SHEmptyRecycleBinW(hwnd, NULL, SHERB_NOSOUND)`. With a `NULL` root it processes all drives; the shell shows its own confirmation and then deletes. On the dead `E:` it can wait in the shell/volume stack indefinitely, with no cancel and no progress. The Settings window is unresponsive for the duration.

Fix: run it on a worker (the existing `measure_temp`/`clean_temp` worker pattern, `ext.rs:219-233`), and either enumerate ready bins and call `SHEmptyRecycleBinW("E:\\")` per ready drive, or use `IFileOperation` on each bin with `Abort`. Show progress/cancel. Never call a shell operation from a `WM_COMMAND` handler.

### F27 — Settings startup calls WinRT `join`, WLAN and audio COM on the UI thread — FREEZE-AT-START / SLOW
Same `pages()` path as F24: `bt::state()` uses `Radio::GetRadiosAsync().and_then(|a| a.join())` (`bt.rs:53-54`) and `FindAllAsyncAqsFilter(...).join()` (`bt.rs:117`); `network_rows` uses `wifi::list` (`wifi.rs:171`); `sound_rows` uses `audio::apps()` (`audio.rs:136`). There is no timeout; a wedged Bluetooth stack or audio service blocks the Settings window before it is created. `bt::refresh` on the BT page (`main.rs:1868-1870`) repeats some of this on the timer.

Fix: make page rows lazy and load them on a worker; for WinRT use `IAsyncOperation` with a timeout (or a worker + `recv_timeout`), never `.join()` on the UI. Cache BT paired devices for 10–30 s.

### F28 — Notepad open/save on the UI thread — HANG-FOREVER
`open_path` (`main.rs:326-345`) does `fs::read` at `:327`; `open_initial` (`:347`) does `is_dir`/`exists` (`:348,352`); `save` (`:360`) does `fs::write` at `:368`. All run in the window procedure. Opening a file from `E:` (double-click in a still-open view) or saving to `E:` blocks the editor with no cancel; a failed save also cannot be retried while the write is stuck.

Fix: read/write on a worker with a `WM_OPENED`/`WM_SAVED` message; keep the buffer in memory; use `write temp + rename` and `CancelSynchronousIo` on the worker. Disable the editor’s close path until the worker answers or the user confirms discard.

### F29 — Photoview folder scan and delete on the UI thread — HANG-FOREVER
`open()` (`main.rs:54-64`) does `std::fs::read_dir(dir)` at `:56` to build the sibling list, on the UI thread (called from `WM_DROPFILES` and startup). `delete_current` (`:152-166`) uses `SHFileOperationW` with `FOF_ALLOWUNDO` on the UI thread at `:158`; the shell’s confirmation/progress is modal to the caller. A folder on a dead share blocks opening; a delete on dead `E:` can hang forever. (`load()` decoding is already correct: worker + `GEN`.)

Fix: build the sibling list on a worker with a generation counter (show the requested image immediately); delete through `IFileOperation` on a worker with `Abort`/`CancelSynchronousIo`.

### F30 — Screenshot save/record paths on the UI thread — HANG-FOREVER
`editor::save_it` (`editor.rs:618-648`) calls `save::auto_path` (`save.rs:95`) when the file has no path; `auto_path` does `create_dir_all` and `while p.exists()` (`save.rs:101`) in `Pictures\Screenshots`, then `save::save` (`:639`) encodes via GDI+ (`save.rs:195-205`). `record.rs:131` computes `default_video_path` (`save.rs:108-118`, same `exists` loop) on the UI start path. If Pictures/Videos are redirected to a disconnected network share or the dead `E:`, the editor or recorder freezes with no cancel and no screenshot.

Fix: choose the path and write on a worker; show “Saving…” with a cancel; if the target volume is not `Ready`, save to `%TEMP%` and offer “Save as…”. The unique-name loop should be bounded (e.g. 1000 tries) and use a single `CreateFile(CREATE_NEW)` rather than `exists` + create.

### F31 — Shell bootstrap stats autostart entries and joins wallpaper conversion — FREEZE-AT-START
`main()` (`rs-shell/main.rs:240+`) calls `autostart()` at `:249`. `autostart` reads the Startup folders (`:110`) and filters Run entries with `Path::new(&p).exists()` (`:123`). A Run entry pointing at `E:\...` (portable app, old USB install) blocks the shell before topbar/desktop are started. `overlay()` starts `wallpaper_bmp` on a worker (`:393`) but immediately `join()`s it at `:401` before starting the desktop; `set_wallpaper()` is also called synchronously at `:256`. A wallpaper file on `E:` blocks the same bootstrap; `wallpaper_bmp` uses `exists`/`canonicalize`/`metadata` (`:195-209`).

Fix: never stat a Run target on the startup thread; use the drive cache and skip entries on non-ready letters (Explorer would just fail to start them). Do not `join` the wallpaper worker; let the desktop paint a solid colour until the BMP is ready. Cache `wallpaper_bmp` by source path + mtime.

### F32 — Wiztree scan has no cancel and probes drives on the UI — HANG-FOREVER (app)
`rescan()` (`wiztree/main.rs:131-142`) spawns `scan(drive)` on a worker; `scan` opens the raw volume/MFT and can block for minutes on a removed USB. There is no cancel token, no `CancelSynchronousIo`, no generation, and F5/Tab can start additional scans (`:288,292`) while the old one is stuck, piling up threads. `fixed_drives()` (`:126-129`) calls `GetDriveTypeW` for every letter on the UI thread when Tab is pressed; a dead network mapping can block the keyboard handler. `WM_DONE` (`:237`) accepts whichever result arrives, so a late scan can overwrite a newer one.

Fix: one scan at a time with a generation; register the scan thread for `CancelSynchronousIo`; on Esc/F5 cancel and start the new scan; use the shared drive cache for `fixed_drives`; ignore `WM_DONE` results with an old generation.

### F33 — Control Panel firewall page runs `netsh` on the UI thread — SLOW-ONLY
`pages/security.rs::firewall()` (`:49-53`) runs `netsh advfirewall show allprofiles` with `.output()`; `security::rows()` is called by `pages::load` from `load_current` (`main.rs:289-298`) and `goto` (`:327`) on the UI thread. If the firewall service (or WMI behind `netsh`) is slow, navigating to Security freezes for seconds; it is not on the 2 s timer list (`main.rs:1660-1664` only reloads System/Services/Power/DateTime), so it is a one-time cost per navigation.

Fix: run on a worker with a 2 s timeout; cache the parsed profile states and show “checking…” until the reply.

### F34 — Clipboard capture writes images on the UI thread — SLOW-ONLY
`WM_CLIPBOARDUPDATE` (`rs-clip/main.rs:158-164`) calls `store.capture(h)` synchronously; the store writes the payload and thumbnail under `%APPDATA%\WinRust\clipboard` (`store.rs:261,287-295`). For a large CF_DIB this is several MB of disk I/O in the message handler. Normally local and fast; if the profile is redirected to a dead share, the clipboard watcher blocks on every copy. Not an `E:` path unless `%APPDATA%` is redirected.

Fix: copy the clipboard data under the window handler (must be quick) and hand the bytes to a worker queue that writes/encodes; the popup already opens on a worker (`main.rs:142`), which is the right pattern.

### F35 — `common::icon::best` is a synchronous shell call with no negative cache — FREEZE root cause
`common/src/icon.rs:54` `best()` tries `SHGetFileInfoW` (`:200-219`, `:205`), `ExtractIconExW` (`:226`), then `SHGetFileInfoW` again (`:230`). It caches only on success (`:86-90`); a failed/timeout path is retried on every call. Every UI caller that draws a path uses it directly: `rust-explorer/ui.rs:262` (drive cell), `openwith.rs:1241,1314`, `topbar/fly.rs:420`, `topbar/main.rs:1095/1174`, `topbar/notify.rs:262`. A `SHGetFileInfoW` on a dead device can block for minutes, so this helper turns any list/paint into a freeze.

Fix: split the API:
- `request(path, px, hwnd, msg) -> HICON` — returns a placeholder immediately, marks the key `Pending` (0) in the cache, and queues a worker that loads the icon and posts `WM_ICON`. Negative results stay cached for a TTL (30–60 s) so a dead path is asked once. This is exactly the pattern in `rs-launcher.rs:1306-1376`.
- `best_blocking(path, px)` — for workers only; rename the current function so UI code cannot call it by accident.
Keep `for_window` (`icon.rs:121-145`) as is: it already uses `SendMessageTimeoutW(..., SMTO_ABORTIFHUNG, 40)`.

### F36 — Search pipe client has no read deadline — SLOW-ONLY / HANG (worker)
`common/src/search.rs:20-39` `ask()` writes the query and loops `read` until EOF/BROKEN_PIPE, with no timeout. `topbar/find_loose_exes` (worker, `fly.rs:378-400`) and `rs-launcher` (worker, `rs-launcher.rs:292-310`) use it; the UI stays alive, but one wedged `rs-search` service blocks the worker forever, so later queries/drives never return. `rust-explorer` has a private copy with the same problem plus UI-thread use (F12).

Fix: add `ask_with_deadline(d, q, 300ms)` (overlapped `ReadFile` + `CancelIoEx`, or `PeekNamedPipe` + `Sleep(10)` loop with a deadline). Return `None`/timeout and let the caller show “search unavailable”. Keep the worker + `WM_RESULTS` pattern.

### F37 — Open-with dialog builds every app icon on the UI thread — SLOW-ONLY
`openwith::dialog` (`openwith.rs:1297`) loads `icon::best` for all registered apps at `:1314` before showing; `browse` also reloads icons at `:1241`. An association whose exe is on the dead `E:` (portable app registered on the USB) makes “Open with” hang while the dialog is being created.

Fix: show the dialog with generic icons; request each icon asynchronously (F35 helper) and repaint as they arrive.

### CLI-only crates (no shell UI freeze, but they hang themselves)
`rs-du` (`plugins/rs-du/src/main.rs:40` MFT `scan`, `clean.rs:15-36` directory walks), `debloat` (`main.rs:185` command `.output()`), `purge` (`main.rs:66` command `.output()`), and `rs-notify`/`rs-notify-bridge` are console/background programs. They cannot freeze a WinRust window, but:
- `rs-du` on dead `E:` blocks in `CreateFile(\\.\E:)`/MFT reads or `read_dir` and has no cancel; `--clean` may then be interrupted by the user with files half-quarantined. It is launched from the topbar menu as a separate console, so the bar is safe.
- `rs-notify` has a bounded 10 × 50 ms connect loop (`rs-notify/main.rs:132-147`), which is good. `rs-notify-bridge` blocks in WinRT `.join()` (`main.rs:268,293`) on its own console thread; a stuck notification platform only stops notifications.
- `debloat`/`purge` call `.output()` on their own console and only touch app/system paths; no `E:` volume logic.

---

## Already-correct patterns (reuse checklist)

- **Worker + `PostMessage` for network enumeration:** `ui::net_enum`/`net_scan` (`rust-explorer/src/ui.rs:1167-1276`) runs `WNetOpenEnum`/`WNetEnumResource` on a worker with a 4 s budget, caches the list, and rebuilds the pane on `WM_NET`. Caveat: `WNetEnumResourceW` itself has no deadline; only the 4 s check between calls.
- **Recycle Bin scan on a worker:** `TRASH_LOADING` + `trash_scan` (`main.rs:218,273-280`) and `WM_TRASH`; the new `trash_restore` guard (`drives::ready()/holds()` at `:322`) and `rename_timeout` (`:360-385`) are the right direction. Missing: skip non-ready bins and reset the flag in all paths (F13).
- **Background thumbnails:** `sh::start_thumbs` (`main.rs:1586-1602` region) uses `IShellItemImageFactory::GetImage` on a worker, a `GEN` generation, and `WM_THUMB`; late images for an old folder are dropped.
- **Cancellable copy:** `jobs::Progress` (`jobs.rs:15-68`) with `io_threads`, `CancelSynchronousIo` (`abort_io`), the 45 s stall watchdog (`STALL`), and the per-file `ReadFile`/`WriteFile` loop that checks `cancelled()` (`jobs.rs:222-284`). The missing piece is registering the walk/preflight thread (F8).
- **Drive cache (new):** `plugins/rust-explorer/src/drives.rs` — `cached()` never touches a device (`:55`), `holds()` is a string/cache check (`:65`), `probe()` runs on a per-drive thread (`:77`), `scan_now()` waits at most `DEADLINE = 3 s` (`:44,108-136`), `scan()` is single-flight and posts `WM_DRIVES` (`:139-149`). It should become the shared implementation for all apps (helper 1). Note: at snapshot time `scan()` is wired from `ui::nav_refresh` (`ui.rs:1275`); verify the final edit keeps that call.
- **Bounded cross-process window icon:** `common::icon::for_window` uses `SendMessageTimeoutW(..., SMTO_ABORTIFHUNG, 40 ms)` (`icon.rs:121-145`). The same pattern should be used for `GetWindowTextW` (F14).
- **Async icon loader with negative cache:** `rs-launcher::ensure_icons` (`rust-search/src/bin/rs-launcher.rs:1306-1376`) inserts `0` before spawning, so a stuck `SHGetFileInfo` is asked once; `WM_ICONS` updates each icon. This is the model for helper 2.
- **Search on a worker:** `rs-launcher` query worker + `WM_RESULTS` (`rs-launcher.rs:292-310`); the file manager should copy it (F12).
- **Task-manager resource caches:** `ResCache` (`rtaskmgr/src/res.rs:91-124`) and `SigCache` (`suspect.rs:139-171`) do shell/work on a worker and return `None`/`Unknown` immediately. They are single-threaded and have no timeout, so one dead exe starves the queue; a second worker or per-request deadline is advisable.
- **Settings cleanup workers:** `ext::measure_temp`/`clean_temp` (`ext.rs:219-233`) already run the temp-folder walk on a worker; `rust-settings/src/wu.rs:56-70` runs Windows Update on a worker; `hub::launch_app` (`hub.rs:1504-1523`) spawns apps off the UI thread.
- **Settings broadcast with timeout:** `sys::notify_setting_change` uses `SendMessageTimeoutW(HWND_BROADCAST, ..., SMTO_ABORTIFHUNG, 2000)` (`rust-settings/src/sys.rs:32`) — good model for cross-process sends.
- **Topbar async loads:** `load_apps` is spawned at startup (`topbar/main.rs:3822`) and on Start open (`fly.rs:1165`); `read_brightness` runs on a worker (`fly.rs:560-575`). The remaining problem is icons, not the app list.
- **rs-clip popups:** `open()` spawns history/emoji on a worker (`rs-clip/main.rs:142-149`), keeping the watcher’s message loop free.
- **USEFILEATTRIBUTES where no disk is wanted:** `common/popup.rs:159`, `rust-explorer/menu.rs:162` use `SHGFI_USEFILEATTRIBUTES`; these are safe. `icon_of` should do the same for non-own icons (F4).

---

## Recommended shared helpers

### 1. `common::volumes` — async, cached volume state (highest leverage)
Move `plugins/rust-explorer/src/drives.rs` into `common` and use it everywhere:
```text
VolumeState { letter, root, ty, label, free, total, icon, state: Ready|NoRoot|Network|Unknown }
cached() -> Vec<Drive>          // never blocks
state(path) -> VolumeState     // cached answer by letter/UNC; Unknown if not scanned
ready(path) -> bool             // Ready only
holds(path) -> bool             // letter/UNC present in the cache
scan(hwnd, msg)                // single-flight worker; per-drive probe threads; DEADLINE 3 s
```
Rules:
- `GetDriveTypeW`, `GetVolumeInformationW`, `GetDiskFreeSpaceExW`, `SHGetFileInfoW` for a root live only in `probe()` on its own thread.
- On timeout, keep the previous entry marked `Unknown` instead of dropping it; `WM_DEVICECHANGE` invalidates and rescans.
- Use it for: explorer Quick access and This PC, topbar pins/Start icons, settings power/bin, task-manager disk page, `rs-shot` save target, wiztree Tab, notepad/photoview startup arguments, `rs-shell` autostart/wallpaper.
- A path whose letter is `NoRoot`/`Unknown` is a **fast failure**: show “drive not available”, do not call any filesystem API.

### 2. `common::icon` — async icon service with negative cache
```text
request(path, px, hwnd, msg) -> HICON // placeholder now; worker later; post WM_ICON
best_blocking(path, px) -> HICON      // workers only; rename current best()
```
- Insert `0` (pending) before the worker starts; keep failures cached for a TTL.
- One STA worker (or a small pool) with a bounded queue; a dead path must not block the next request.
- Convert topbar Start/taskbar/notify, explorer `list_draw`/`drive_cell`/`openwith`, and any other paint path.
- `for_window` keeps its `SendMessageTimeoutW(40 ms)` implementation.

### 3. `common::wintext` — bounded foreign-window text
```text
fn text(hwnd) -> String
```
Use `SendMessageTimeoutW(hwnd, WM_GETTEXT, ..., SMTO_ABORTIFHUNG | SMTO_BLOCK, 50, &mut r)` for foreign windows; fall back to `GetWindowTextW` for this process; skip `IsHungAppWindow` windows. Replace `rtaskmgr/src/sys.rs:window_titles` and `hung_windows` (F14); use in any topbar title lookup if added.

### 4. `common::iojob` — cancellable I/O worker registry
```text
IoJob { cancel: AtomicBool, threads: Mutex<Vec<u32>>, stalled: AtomicBool, generation: u32 }
spawn_iobounded(hwnd, msg, f)  // registers the thread before f runs
cancel(job)                    // sets the flag + CancelSynchronousIo on every registered thread
```
- Register **before** the first FS call (`walk`, `free_name`, `read_dir`, `exists`, raw-volume open).
- Add a cancellation check in every directory-file loop; return `ErrorKind::Interrupted`.
- Use for `jobs::copy_now` walk, `trash_scan`/`trash_delete`, wiztree `scan`, rs-du `--clean`, and any future recursive scan.
- Keep the existing `jobs::copy_file` chunk loop and stall watchdog; this registry is the missing preflight half.

### 5. `common::shellop` — abortable shell file operations
Wrap `IFileOperation` on a worker:
- `Advise` an `IFileOperationProgressSink`; `Abort` from the UI thread; `CancelSynchronousIo` on the worker thread as a last resort.
- Owner window only for modality; the UI thread never calls `PerformOperations`.
- For plain `PathBuf` sources/destinations prefer `jobs::copy`; for `SHEmptyRecycleBinW`/`SHFileOperationW` either avoid them or run them in the same abortable worker.
- Add a “drive not available” pre-check from helper 1.

### 6. `common::pipe` — deadlined search client
`ask_with_deadline(drive, query, 300 ms)` with `PeekNamedPipe` polling or overlapped `ReadFile` + `CancelIoEx`. Use in `common::search::ask` (F36), the explorer copy (F12), and any future client. Keep the one-request-per-connection protocol.

### 7. `common::asyncpage` — page/row loader with generation
For explorer folder rows and settings pages: `load_async(key, generation, worker -> T, post WM_ROWS)`, keep the old rows, drop stale results. This is the same shape as `net_scan`, `trash_scan`, `WM_THUMB`; one implementation prevents each screen from inventing its own.

### 8. `common::winrt` — `join` with timeout
A small helper that runs an `IAsyncOperation` on a worker and waits with `recv_timeout` (or uses `IAsyncOperation::Completed`); never call `.join()` from a message handler. Use for `bt.rs`/`wifi.rs`/`rs-notify-bridge` (F27, F31).

---

## Top 10 fixes, in order

1. **Explorer: make `load()` asynchronous** (`main.rs:1160`, `rows_of:1126`). Worker + generation + `WM_FOLDER`, keep the old rows, volume-ready gate, cancellable thread. This removes the folder-navigation `HANG-FOREVER` and the last synchronous part of `WM_CREATE`.
2. **Explorer: stop stat'ing Quick access in `nav_init`** (`ui.rs:1100`, `ui.rs:1313`). Cache per-pin state and resolve on the drive worker. This removes the remaining `FREEZE-AT-START` from `WM_CREATE` when a pin points at `E:`.
3. **Explorer paste: move the preflight into the worker** (`main.rs:650-653`; `jobs.rs:178/183`). No `is_dir`/`symlink_metadata`/`File::create` on the UI; volume gate; Esc cancels during preflight. This is the direct “copy to E: hangs with no cancel” fix.
4. **Explorer/`IFileOperation`: add Abort** (`main.rs:1502`, `1531`). Progress sink + `Abort` + `CancelSynchronousIo`; prefer `jobs::copy` for plain files. Then the shell copy/move/delete can be cancelled even when the shell engine is stuck.
5. **Explorer `jobs`: register the walk/preflight thread** (`jobs.rs:303`, registration `:322`). Check `cancelled()` inside `walk`/`free_name`, gate source/destination by volume state. This closes the no-cancel source-side copy hang.
6. **Task manager: bounded window text** (`sys.rs:289,444`). `SendMessageTimeoutW` with `SMTO_ABORTIFHUNG` and a 50 ms deadline; skip hung windows. This is the likely reason the task manager froze when explorer was stuck.
7. **Task manager: move all device/PDH/service probes off the tick** (`data.rs:264,293,296,315,417/418`). One sampler thread publishes a snapshot; `file_missing` on worker + volume cache; no `GetDiskFreeSpaceEx`/`EnumServicesStatusEx`/PDH on the UI timer.
8. **Topbar: async icons for pins, taskbar buttons, Start and notifications** (`main.rs:1390-1427,1442-1512`; `fly.rs:451-469,2078-2082`; `notify.rs:262`). Load pins/icons on a worker, placeholders first, negative cache. This removes the shell-start and bar-timer freezes for apps on a dead USB.
9. **Settings: lazy pages + no shell/drive calls before the window** (`hub.rs:1580`; `main.rs:600,611,623,700-702`; `:1397-1400,1860-1874`). `SHQueryRecycleBinW`, `power::drives`, BT/WLAN/audio on workers, cached values for the timer; empty bin on an abortable worker.
10. **Shared helpers, then the remaining apps:** `common::volumes` (move `drives.rs`), `common::icon` async/negative cache, `common::wintext`, `common::iojob`, `common::shellop`, `common::pipe`. Then apply them to notepad/photoview/rs-shot/rs-shell/wiztree/openwith (F28–F32, F37) and convert the task-manager/settings screens to “worker snapshot → cached rows”.

---

## Quick verification checklist for the next agent
- `rg "drives::scan" plugins/rust-explorer/src` — it is wired from `ui::nav_refresh` in this snapshot (`ui.rs:1275`); keep that call through the final edit and also call it from any new `WM_DEVICECHANGE`/refresh path.
- `rg "\.is_dir\(\)|\.is_file\(\)|\.exists\(\)|fs::read_dir|SHGetFileInfo" plugins/rust-explorer/src` — every hit in a `proc`, `paint`, `nav_init`, `GETDISPINFO`, `load`, `paste`, `search` is a suspect.
- `rg "GetWindowTextW|PdhCollect|EnumServicesStatus|GetDiskFreeSpaceEx|SHQueryRecycleBin|SHEmptyRecycleBin|SHFileOperation" plugins` — each must be on a registered worker, not in a timer/paint.
- `rg "\.join\(\)" plugins/rust-settings plugins/rust-search` — none may be in `pages()`, `rows()`, `paint`, or `WM_TIMER`.
- `rg "WaitForSingleObject|recv\(\)" plugins` — no unbounded waits on the UI thread; the only allowed unbounded wait is inside a worker that the UI can cancel.
- In Task Manager, hang a test process with a window (a suspended console is enough) and watch whether the task manager still repaints every second — it should, after F14/F16.
