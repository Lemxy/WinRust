# WinRust (fork) — pending work

Everything done so far lives in this fork; this file tracks what is **not** done yet, so nothing gets lost.
Toolchain: mingw-w64 (GNU) for most crates, **MSVC** for `rtaskmgr-win32` (its GNU build fails to load with `0xC0000139`).

## 1. Wallpaper Engine (deferred — next phase)

Full report: `tools/wallpaper-engine.md` (mechanism, phases, snippets, risks, 24 sources).

- [ ] **Phase 1 (short term)**: launch WE through its CLI (`wallpaper64.exe -control openWallpaper -file <pkg> -playInWindow "<title>" -borderless`) and `SetParent` the window inside the `WorkerW` that `rust-explorer` already creates.
  - Needs WE installed and running; no playlists inside the window.
- [ ] **Phase 2 (medium)**: rebuild the desktop as the classic topology (`Progman` + top-level `WorkerW` with `SHELLDLL_DefView`/`SysListView32` + host `WorkerW` below) behind a feature flag, so WE attaches by itself (playlists, profiles, pause rules).
- [ ] Rename/settle the desktop window title: `rs-desktop` breaks code that looks for `FindWindow("Progman", "Program Manager")`.
- Do **not**: mimic the raised desktop without DirectComposition (black screen) or inject into WE (`weshim`, anti-cheat/DRM risk).

## 2. Window frost (Mica) — paused

- [ ] `common::frost` works (capture + blur + tint) and is wired in `rust-explorer`, `rust-settings`, `rtaskmgr-win32`,
      but it is **off by default** (`look.txt` → `win-blur=0`): on bright wallpapers the tint needed for readable text
      makes the window look washed out.
- [ ] Decide: fixed ~70 % tint of a neutral colour, stronger blur, or real DWM acrylic per window.

## 3. In progress / to verify

- [ ] `rust-explorer`: default **This PC** view listing all drives (not only `C:`), clickable "This PC" nav node,
      and tabs moved into the title bar (one row, no duplicated title).
- [ ] `common::icon`: jumbo (256 px) shell icons scaled down with quality; wired in bar, Start and the file list.
- [ ] `plugins/rust-control`: new Control Panel crate (Category view as the home page, Icon view toggle,
      pages: System, Programs, Services, Devices, Network, Users, Power, Date/Time, Accessibility, Security, Admin tools).
- [ ] Bluetooth: `DeviceWatcher` (classic + LE) with live results — verify the controller appears while pairing.
- [x] Spanish entries added for every new `t!` key: `tools/i18n-check.ps1` reports **0 errors** (the 32 remaining
      duplicate aliases are historical, only one of each pair is used). `plugins/topbar/src/fly.rs` was double-encoded
      and has been converted back (RU/JA/ZH/PT and curly quotes in the flyouts).

## 4. Applied, do not undo

- **Phase 2 (shell replacement)**: `rs-shell.exe` is now this user's shell (HKCU Winlogon\Shell); the `rs-overlay` logon
  task is disabled, so Explorer never starts at logon. IFEO blocks (`cmd.exe /c exit`) for StartMenuExperienceHost, SearchApp,
  SearchUI, SearchHost, Widgets, Cortana and ShellExperienceHost; WSearch disabled. `explorer.exe` stays available for
  hand-offs, and folders open in rust-explorer through HKCU Classes\Directory|Folder\shell\open\command.
  Undo: `tools\shell-phase2.ps1 -Restore` (state in `tools\state\shell-phase2.backup.json`).
  Pending: sign out/in once and confirm bar + desktop icons + Win+E + Start + tray come up.

- `WinRust-update` scheduled task is **disabled** (it would replace the custom binaries with the official ones).
- Explorer policies `NoDesktop`, `NoTrayContextMenu`, `NoViewContextMenu` and EdgeUpdate install blocks
  (backup + revert: `tools/explorer-edge.ps1 -Restore`). Edge was uninstalled; WebView2 untouched.
- `AuthRestartShell`/`Winlogon\Shell` left as they were (`explorer.exe`) — WinRust relies on `rs-shell --overlay`.

## 5. Restore points

- `Antes de instalar WinRust` (before the shell replacement)
- `Antes de Explorer/Edge` (before the debloat script)

## 6. Icon quality (implemented, partial rollout)

- [x] `common::icon` (jumbo 256 → area-averaged downscale with alpha) wired in `topbar` (bar, Start rows/tiles, tray, menus)
      and in the explorer's desktop icon list.
- [ ] The bar button of `rust-explorer` still shows its own logo on some setups: the pin's `.lnk` target does not always
      resolve, so `sys_pin_icon` now also matches the shortcut's file name — verify after the next restart.
- [ ] `start7.rs` / `start10.rs` / `start11.rs` still call `DrawIconEx` directly (one line each to switch to `icon::draw`).
- [ ] `thumbs.rs` (alt-tab previews) still uses `icon_of`; switch it when DPI changes are handled.
## 7. File manager: icon grid and navigation (done)

- [x] The icon grid is the default view of every window (the toolbar button / Ctrl+M switch to the details list); cells are
      48 px (half of the old 96) with 11 px labels, and wider in This PC.
- [x] This PC draws its own cells: the volume name, and under it the drive letter as a small tag plus "free of total".
      The nav pane and the details list use the same letter tag instead of "Name (C:)".
- [x] The left pane scrolls with the wheel (three rows per notch, slim thumb on the right edge), also while the list holds
      the focus.
- [x] "+" and Ctrl+T open a new tab on This PC.
- [ ] Screenshots + performance table for the README (`docs/screenshots`, metrics to be measured).

## 8. Notification centre (phase 2)

- [x] Phase 1 (this fork): `topbar` shows `rs-notify` / named-pipe toasts (`\\.\pipe\winrust-notify`), a 3-toast stack,
      a bell with an unread dot and a 50-entry in-memory history panel; `notify` and `notify-timeout` live in
      `look.txt`, with a switch in the bar settings.
- [ ] Phase 2: capture **native** Windows notifications (the ones Apps/UWP raise through Explorer's
      `Windows.UI.Notifications` / `ToastNotificationManager`). They depend on Explorer being the shell and are **not**
      captured in phase 1; this needs a toast listener/COM server or a hook into the notification platform, plus a
      decision on the app identity to register under.
- [ ] Optional: persist the history in `%APPDATA%\WinRust\notifications.txt` (phase 1 keeps it in memory only).
- [ ] Phase 1 limits: one notification per pipe connection; the pipe thread reads one record and disconnects, so a
      program that wants live progress must reopen the pipe (or keep a connection open in a later protocol version).
## 9. Translation audit (done) and what is left

- [x] `tools/i18n-check.ps1`: audits every `t!`/`tf!` call (languages, placeholders, `es.tsv` keys, duplicates).
- [x] `common/es.tsv`: 61 duplicates removed (last value kept, historical values restored for Settings/Disabled/Start/
      Nothing here yet), malformed lines dropped, CRLF normalised, 26 missing entries added (installer, purge, tools,
      notifications, bar menu). Checker: 0 malformed, 0 mismatched placeholders, 0 missing keys.
- [ ] `topbar` still has 65 calls without `ja`/`pt`/`zh` (44 in `settingswin.rs`, 16 in `start7.rs`, 3 in `start10.rs`) and
      `rust-settings`/`common` a few more; fill them from the existing terminology.
- [ ] Portuguese is pt-BR in practice (`arquivo`, `tela`, `registro`); moving to pt-PT would be a full pass, not a fix.
- [ ] 18 remaining "duplicates" are case variants of distinct keys (`installed`/`Installed`); harmless, keep both or align
      the source literals.

## 10. GitHub page

- [x] `README.md` in English with screenshots (`docs/screenshots`: bar, bar menu, Start, notifications, toasts, file
      manager, This PC, Control Panel, Task Manager) and `docs/PERFORMANCE.md` with the measured numbers.
- [x] `tools/multimonitor-review.md`: multi-monitor review and phase plan (not implemented yet).
- [ ] PR upstream is intentionally on hold; the branch is committed locally only.
## 11. After the first shell-mode reboot (validated)

- [x] `rs-shell` started as the logon shell (no `explorer.exe` as shell), autostart ran (Rainmeter, Firefox, Serein), bar and
      desktop came up. Only `/factory -Embedding` explorer hosts remain, which is normal.
- [x] Context menus clamp to the monitor they open on (secondary included) and toasts follow the pointer's monitor.
- [x] `rs-shell` shell mode now respawns `rust-explorer --desktop` if it dies (the overlay watcher already did this; shell
      mode only watched the bar). Built and queued for the next boot through PendingFileRenameOperations, because the
      running `rs-shell.exe` cannot be overwritten.
- [ ] Next reboot: confirm the new `rs-shell.exe` (desktop respawn) is in place; `Get-Item` date 24/11 10:18.