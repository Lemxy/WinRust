# Wallpaper Engine on the WinRust desktop — manual test plan

What this build does, and how to check it on a machine that has Wallpaper Engine (WE) installed. The desktop is
`rust-explorer.exe --desktop` (class `Progman`, title `"Program Manager"`, property `WinRustDesktop`).

- **Native route (phase 1, always on)**: the desktop exposes `Progman` → `SHELLDLL_DefView` + one
  `WinRustWallpaper` host per monitor + the `WorkerW` that `0x052C` spawns (bottom-most). WE can find that hierarchy and
  put its own desktop window in the `WorkerW`. Nothing has to be enabled for this; it is what WE does on Explorer.
- **Bridge route (phase 2, `we=1`, default off)**: `we.rs` starts WE's documented pop-out per monitor
  (`-control openWallpaper -file … -playInWindow "WinRust Wallpaper <n>" … -borderless`) and `SetParent`s it into that
  monitor's host, below the icon list. Used when WE does not attach by itself (or for a scene on every monitor while WE
  is configured for one).
- **Fallback**: with WE absent, stopped, not installable, or the flag off, the hosts paint
  `%LOCALAPPDATA%\rs-shell\wallpaper.bmp` exactly as before. Every failure is logged; the picture never depends on WE.

Everything is logged to `%LOCALAPPDATA%\rs-shell\desktop.log` (one line per state change, not per tick).

## 1. What to install / launch

1. Install WE from Steam (any current 2.x) and make sure the **Wallpaper Engine process is running**
   (`wallpaper64.exe` or `wallpaper32.exe`; the UI may be open or in the tray). `-control` does not start the app.
2. Have at least one local/Workshop wallpaper. A default scene under
   `<steam>\steamapps\common\wallpaper_engine\projects\defaultprojects\<name>\project.json` is enough.
3. The shell must not be elevated (WE refuses Steamworks elevated, and UIPI would block `SetParent`); run both as the
   same user, same integrity level.
4. Keep DWM on. Do not use this on the secure desktop (UAC/lock): the wallpaper is expected to disappear there.

## 2. Native route: let WE attach by itself

1. Start the WinRust desktop (`rs-shell` does it; or run `rust-explorer.exe --desktop`).
2. In WE's settings, choose a wallpaper **for the monitor you are testing** (display 1). WE sends `0x052C` to Progman
   and looks for the host; within a few seconds the window tree should show:

   ```text
   Progman "Program Manager"                         (rust-explorer.exe)
     SysListView32                                   (the icons, primary monitor)
     SHELLDLL_DefView                                (0x0 marker, WS_CHILD)
     WinRustWallpaper                                (one per monitor)
     WorkerW                                         (bottom-most)
       WPELiveWallpaper / WPEDesktop...              (wallpaper64.exe — WE attached)
   ```

   Dump it with `powershell -ExecutionPolicy Bypass -File tools\window-tree.ps1` (read-only).
3. Expected: the WE wallpaper is visible behind the icons on that monitor; icons are clickable and on top; no WE entry
   in Alt+Tab; the static picture is still painted on monitors WE did not attach to.

If WE does not attach (no child under `WorkerW`):

- WE may be looking for the title `Program Manager` — this build uses it; confirm with
  `FindWindowW(L"Progman", L"Program Manager")` from a test program (the window tree shows the title).
- Try `0x052C` by hand (this is exactly what WE sends):
  `SendMessageTimeoutW(progman, 0x052C, 0xD, 0x1, SMTO_NORMAL, 2000, &r)`.
- If it still does not attach, or the attached window is black, use the bridge route below; a black attached window is
  the `WS_EX_NOREDIRECTIONBITMAP`/DirectComposition case and GDI cannot fix it (see §5).

## 3. Bridge route: `we=1`

`%ProgramData%\WinRust\look.txt` (same file as the bar's look settings), one key per line:

```ini
we=1
we-file=G:\SteamLibrary\steamapps\common\wallpaper_engine\projects\defaultprojects\neon_sunset\project.json
# optional: one window across the whole virtual screen instead of one per monitor
we-span=0
```

- `we=1` — enable the bridge. **Default is off**: with no `we` line (an old look.txt, a fresh install) the desktop
  behaves exactly as before.
- `we-file` — `project.json`, `scene.pkg`, `.mp4`, `index.html` or a folder. If empty, the bridge asks WE once with
  `-control getWallpaper` (off the UI thread); if that returns nothing it logs and keeps the picture.
- `we-span=1` — a single pop-out over the virtual screen (good for scenes), attached under the icon list; `0` (default)
  opens one per monitor, in the same order as `common::monitors::all()` and the per-monitor bars.

The desktop re-reads look.txt every 2 s: no restart needed. Within ~2–4 s the log should show `found …`, `opening …`,
`found WinRust Wallpaper n`. Expected window tree:

```text
WinRustWallpaper  (1360x768 at 0,0)       ← host 0 (rust-explorer)
  WPEOverlappedWallpaper "WinRust Wallpaper 0"   ← wallpaper64.exe, WS_CHILD, WS_EX_NOACTIVATE
    WPEDesktopDX11Window "WPELiveWallpaper"      ← the renderer (may be NRB/WS_EX_LAYERED)
WinRustWallpaper  (1440x900 at 1360,0)    ← host 1
  WPEOverlappedWallpaper "WinRust Wallpaper 1"
```

Expected behaviour:

- the WE wallpaper on every monitor, behind the icons; icons, labels, double-click, drag & drop, rename and the
  right-click menu keep working (they did in the live test on 2026-11-26);
- no pop-out in Alt+Tab and no focus stealing (`WS_EX_NOACTIVATE`, no `WS_EX_APPWINDOW`);
- turning `we` back to `0` (or removing the key) closes the pop-outs with
  `-control closeWallpaper -location "WinRust Wallpaper n"` and the picture is back immediately. WE itself is **not**
  killed: the user may have it for other displays;
- if WE already attached natively into `WorkerW`, the bridge closes its own pop-outs and lets WE's desktop wallpaper
  show (`wallpaper engine attached itself to WorkerW` in the log). This is the preferred route (playlists, profiles,
  pause rules).
- if any monitor's pop-out never appears (6 s × 3 tries), only that monitor keeps the picture; the others keep WE.

Toggle in the UI: **Settings → Desktop → Live wallpaper → Wallpaper Engine** (same `we` key). The wallpaper path is the
`we-file` key in look.txt; the toggle does not open a file picker on purpose (WE projects are folders).

## 4. Multi-monitor / DPI / lifecycle to try

1. 1, 2 and 3 monitors; one monitor left of the primary (negative `-x`); different resolutions and 125/150/200 %.
   Expect one host per monitor (same left-to-right order as the bars) and one pop-out per host.
2. Change the resolution or unplug a monitor: hosts are recreated (`WM_DISPLAYCHANGE`, `WM_DPICHANGED`), the old pop-out
   dies with its host and the bridge opens a new one sized to the new rectangle.
3. Lock/unlock, suspend/resume: the wallpaper may not be visible on the secure desktop; it must come back without a
   restart. Signing out: `WM_ENDSESSION` closes the pop-outs (`-control closeWallpaper`).
4. Kill WE while `we=1`: the bridge notices the pop-outs are gone, retries 3 times, then leaves the picture and logs
   `pop-out … disappeared` / `keeps closing`.
5. Toggle `we` while WE is not installed: the log says `not found: no wallpaper_engine under the Steam libraries` and
   the picture stays. `WINRUST_WE_EXE=<path>` overrides the discovery for a non-Steam install.

## 5. Known limits (from `tools/wallpaper-engine.md`)

- Pop-outs do **not** support WE playlists; use the native route for those.
- A pop-out (or an attached native window) that renders **black** is a DirectComposition /
  `WS_EX_NOREDIRECTIONBITMAP` surface; GDI cannot composite it. The log prints a warning with the window name. Do not
  fight it: use WE's native route if it works, or turn `we` off.
- WE must run at the same integrity level as the shell; `SetParent` is refused otherwise (logged once).
- HDR + the GDI colour key (`0x000101`) is not tested; if the wallpaper or icons look wrong with HDR, turn `we` off.
- While a live wallpaper covers the desktop, mouse input over **empty** desktop belongs to the wallpaper window: the
  icon list is `WS_EX_LAYERED` + `LWA_COLORKEY` (the key pixels are click-through), which is what lets WE receive mouse
  input for interactive wallpapers and keeps OLE drops working. The desktop right-click menu therefore works with `we=0`
  (or when no wallpaper is live); with a live wallpaper, right-click an icon or turn `we` off. This is the existing
  colour-key design, not something the bridge changes.
- `GetShellWindow()` is still `NULL` (we do not call the undocumented `SetShellWindow`): the live test shows WE 2.8
  does not need it. If a future WE version does, `GetShellWindow` is the first thing to check.
- 24H2+ with desktop icons hidden at logon has a known WE bug; this shell can start with icons hidden
  (`%APPDATA%\WinRust\desktop-icons-hidden.txt`). Show the icons once before testing.

## 6. How to recover (no log off needed)

1. `we=0` (or delete the `we` key) in `%ProgramData%\WinRust\look.txt` and wait 2 s: pop-outs close, the picture is
   back. This does **not** stop WE's own desktop wallpaper if WE attached natively; close it in WE or stop WE.
2. If a pop-out is stuck: `wallpaper64.exe -control closeWallpaper -location "WinRust Wallpaper 0"` (and `…1`, `…2`).
3. If the desktop itself is wrong: kill `rust-explorer.exe`; `rs-shell` respawns `--desktop` (`--desktop` has a
   single-instance guard, so an extra respawn cannot create a second Progman). Never kill `rs-shell`.
4. Logs: `%LOCALAPPDATA%\rs-shell\desktop.log` (bridge), `%LOCALAPPDATA%\rs-shell\log.txt` (shell supervisor).
   Window tree: `tools\window-tree.ps1`.

## 7. What was verified on the development machine (2026-11-26)

The machine turned out to have WE 2.8.42 installed at
`G:\SteamLibrary\steamapps\common\wallpaper_engine\wallpaper64.exe` (the earlier analysis said it was absent). With
`we=1` and `we-file=…\defaultprojects\neon_sunset\project.json` the bridge opened two `WPEOverlappedWallpaper` windows,
one per monitor, both attached under the icons (`WinRust Wallpaper 0/1`), and the scene was visible on both monitors
with the icons on top; `we=0` closed both and the picture came back. WE also attached its own
`WPELiveWallpaper`/`WPEDesktopCEFWindow` into the `WorkerW` by itself on the primary monitor, which is the native route.
With `we-file=…\dino_run\project.json` and `we-span=1` a single 2800×900 pop-out was attached under the icon list.

The window-tree dump and the build/run evidence are in the task report; the scripts are
`tools\window-tree.ps1` and `tools\shots.ps1`.
