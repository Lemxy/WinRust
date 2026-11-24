# Multi-monitor review — desktop and taskbar

Static review of `C:\winrust-build` on the test machine: primary **1360×768 @ (0,0)**, secondary **1440×900 @ (1360,0)**,
virtual desktop 2800×900, 100 % DPI. Line numbers are from the reviewed revision.

## Summary

1. **The fork is single-monitor by design.** One `topbar` window (`plugins/topbar/src/main.rs:3105`) guarded by a mutex and
   `FindWindowW("topbar")` (`:3072`), and every screen metric comes from `GetSystemMetrics(SM_CXSCREEN/SM_CYSCREEN)`,
   which is always the primary (`:88, :96, :100, :108, :280, :748, :1629, :1679, :3105`).
2. **The desktop icons are single-monitor too.** `place_desk_list()` puts the only `SysListView32` on the primary
   (`plugins/rust-explorer/src/main.rs:46-51`). The `Progman` window does cover the virtual screen (`:2466`), so the
   wallpaper is right, but there is no icon grid on the secondary.
3. **What already handles multiple monitors**: full-screen capture (`shot.rs:7-23`), per-monitor wallpaper fill
   (`rust-explorer:1207-1244`), snapping by monitor (`topbar:2671`), optional per-monitor Alt+Tab (`:557-604`), the
   launcher on the cursor's monitor (`rs-launcher.rs:215-224`), and dialog clamping (`rust-explorer:1502`).
4. **DPI risk**: `rust-explorer` is PerMonitorV2 in its manifest (`app.manifest:10`) but never handles `WM_DPICHANGED`
   and keeps a single scale read from the primary (`ui.rs:76-84`). `topbar` is system-DPI-aware (`main.rs:3091`) with the
   same single `SCALE`.
5. **Without Explorer, `SHAppBarMessage` is a no-op**: `ABM_*` is forwarded to `Shell_TrayWnd`'s owner (`tray.rs:83-97`);
   with no explorer that is `NULL`. The real reservation is `SPI_SETWORKAREA` + `fit_maximized` (`main.rs:779-826`).

## Works / broken / missing

| Area | Works | Broken | Missing |
|---|---|---|---|
| Bar | Re-placed on `WM_DISPLAYCHANGE` of the primary (`:3021`) | Position/size always the primary (`:88, :748`); single instance (`:3074`) | Bar per monitor, monitor picker, `WM_DPICHANGED` |
| AppBar | `ABM_*` with Explorer; `SPI_SETWORKAREA` without it | One `hWnd` and one `rc`, always the primary (`:749-775, :2862`); `SPI_SETWORKAREA` is global | Per-monitor appbar, work-area broker |
| Flyouts | Frost/blur on any rect (`blur.rs:224`) | `pop_y`, `side_x`, `fly::toggle` use the primary (`main.rs:95-123, 279`; `fly.rs:1086`) | Anchoring to the bar's monitor, re-anchor on hotplug |
| Toasts / centre | Stack, animations, history | `slot()` uses `SPI_GETWORKAREA` (primary) (`notify.rs:406-421`); centre at `sw-360` (`:766`) | Active-monitor placement, per-monitor stack |
| Desktop | `Progman` covers the virtual screen; per-monitor wallpaper | Icons only on the primary; no re-layout on `WM_DISPLAYCHANGE` | Icon grids per monitor |
| Wallpaper | Per-monitor "fill", black gaps (`:1198-1244`) | Not reloaded on `WM_SETTINGCHANGE` (only on restart, `:18, :2073`) | Hot wallpaper change |
| DPI | `sc()` everywhere | Scale read once, from the primary; no `WM_DPICHANGED`; `Ctx.dpi` unused (`fly.rs:834` passes 1.0) | Per-monitor scale |
| Hotkeys | Win+D across all monitors (`common/src/desktop.rs:13`) | Win+D/Win+E registered by both `rs-shell` and ReShell (`rs-shell:251`, `hotkeys.rs:78`) | "show desktop of this monitor", Win+E on the cursor's monitor |

## Concrete reproductions

- **R1 — ghost auto-hide**: with auto-hide on, put the cursor at `(2000, 800)` on the secondary. `fs_update` checks
  `pt.y >= SM_CYSCREEN-1` (`:1629`) → it treats the primary's bar as hovered although the cursor is 700 px away.
- **R2 — moving the bar**: no setting; forcing it needs `bar_x/bar_x_w/bar_top/place` (`:87-123, 279, 746`) and the
  single-instance guard still allows only one window.
- **R3 — toast on the wrong monitor**: trigger `rs-notify` from an app on the secondary; the toast lands at
  `(972, 630)` (primary).
- **R5 — quick settings**: always `x = 1360 - w - 8`, `y = 734 - 8 - h` (`fly.rs:1086`).
- **R9 — dragging an icon to the secondary**: the list is 1360 wide (`:47`); the icon sticks to the right edge.
- **R10 — changing the primary**: the bar and the wallpaper follow, but the icons are not re-laid out; icons outside the
  new grid only find a free cell on the next start (`:1763`).
- **R14 — explorer context menu on the secondary**: `menu.rs:690` clamps against `SM_CXSCREEN`, so a menu at
  `(1500, 400)` opens at the primary's right edge.

## Plan by phases

**Phase 0 — foundation (P0).** New `common/src/monitors.rs` (`Mon { h, rc, work, dpi, primary, dev }`, `all()`, `at()`,
`of()`, `primary()`, `bar_monitor()`, `cursor_monitor()`) and `common/src/dpi.rs`; export them in `common/src/lib.rs`.
Replace the hand-written `GetMonitorInfoW` calls. Always guard `set_work_area`: **only the primary** may call
`SPI_SETWORKAREA` (it is a single global rect).

**Phase 1 — one bar per monitor (P1).** `BAR: AtomicIsize` → `Vec<Bar>`; geometry helpers take the bar/monitor; create and
destroy windows on `WM_DISPLAYCHANGE`; one `APPBARDATA.hWnd` per bar when Explorer is alive; secondary bars reserve nothing
(auto-hide/overlay) because `SPI_SETWORKAREA` cannot carve a secondary monitor; one tray host, clock and buttons on each;
the keyboard hook targets the leader bar; `--restore` removes every bar's reservation.

**Phase 2 — popups on their bar's monitor (P1).** `fly::toggle` and `menu` take the origin bar; `tip_cx`, `pop_y`,
`side_x`, `show_tip`, `thumbs::layout` and `settingswin` use the bar's `rc`; close flyouts on `WM_DISPLAYCHANGE`.

**Phase 3 — toasts on the active monitor (P1).** Target monitor: foreground window → cursor → the bar with the bell →
primary. Keep the stack per monitor; open the centre on the bell's monitor; re-anchor on hotplug.

**Phase 4 — desktop icons (P2).** First the low-risk version: pin the list to the primary explicitly, include left/right
bar insets in `desk_top/bottom`, and re-lay out icons on `WM_DISPLAYCHANGE` instead of leaving them off-screen. Then, as a
separate project, one `SysListView32` per monitor (items assigned per monitor, one `LVSIL_NORMAL` shared).

**Phase 5 — per-monitor DPI (P2).** PerMonitorV2 in `topbar` plus `WM_DPICHANGED` in every window proc; replace the global
`SCALE` with a per-window scale; recreate fonts and regions; `rust-explorer` must keep `Progman` on the virtual rect and
ignore the suggested rect.

**Phase 6 — hotplug (P2).** Debounce `WM_DISPLAYCHANGE`/`WM_DPICHANGED`, rebuild the bar set, close popups, re-anchor
toasts, re-lay out icons, reload the wallpaper; version `desktop-icons.txt` with the layout it was saved for.

## Do not touch

`Progman` + `WorkerW` + `0x052C` (Wallpaper Engine contract), the `rs-desktop` title (`rs-shell` finds the desktop by it),
the tray protocol (`NOTIFYICONDATA`, `Shell_NotifyIcon` v4), the `winrust-notify` pipe and `NotifyWire`, the
`SetTaskmanWindow`/`RegisterShellHookWindow` handshake, `SPI_SETMINIMIZEDMETRICS`, and the file formats under
`%ProgramData%\WinRust`. Keep the `ABM_*` path even though it is inert without Explorer: it is what works in overlay mode.

## Priority

| Priority | Work | Why |
|---|---|---|
| P0 | `common::monitors` + guard `set_work_area` to the primary | Avoids the catastrophic global work-area bug and unblocks everything |
| P1 | Bar per monitor + per-`hWnd` appbar | The core request |
| P1 | Flyouts/Start on their bar's monitor | Small change, big visual impact |
| P1 | Toasts/centre on the active monitor | Small, today's obvious bug |
| P1 | Hotplug without losing icons | Perceived data loss |
| P2 | Per-monitor DPI | Needed for 125/150 % |
| P2 | Icon grids per monitor | Biggest change; after the bar is stable |
