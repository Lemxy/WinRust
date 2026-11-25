# Fork notes — WinRust custom shell

This fork builds on [Lemxy/WinRust](https://github.com/Lemxy/WinRust) and adds, on top of upstream:

## Shell look

- **Real blur behind the bar**: the strip just outside the bar is captured, blurred (three box passes) and painted as
  the bar background with a tint; refresh every 400 ms `plugins/topbar/src/blur.rs`. Settings live in
  `%ProgramData%\WinRust\look.txt` and are edited from the bar's settings window.
- **Real blur behind the context menu** (`plugins/rust-explorer/src/menu.rs`): a custom Win11-style menu replaces
  `TrackPopupMenu`, with shell icons, shortcuts, ticks, one submenu level, rounded corners and live transparency.
- **Settings window with tabs** (`plugins/topbar/src/settingswin.rs`): replaces the long dropdown with a popup that has
  a tab rail (Appearance, Start menu, Blur, Bar, Language), double-buffered, with the frost background.
- **Dodge windows** (the settings hub, Bar → Behavior, `%ProgramData%\WinRust\dodge.txt`): the bar reserves no work area,
  hides while a window of its monitor reaches it and returns at that screen's edge; a maximized window takes the whole
  screen. Independent of auto-hide. **Per monitor**: one slide per bar (`Slide`), the targets are computed from every
  window of that monitor (not the focused one), and the edge of a screen only brings back its own bar (the pointer span
  is checked, so a bar above/below another screen does not answer its edge). With dodge on, a fullscreen window is just
  another window: the edge still reveals the bar over it. `style::reread()` makes a settings change skip the one-second
  cache, and `fit_maximized` grows *and* trims already-maximized windows (they stay maximized).
- **Bar layout**: Windows button at the left end (Start), search / Task View can be hidden, the tray hugs the clock and
  there is no hairline border.
- **Start menu, three styles** (`start7.rs`, `start10.rs`, `start11.rs` behind `startctx.rs`): Windows 7 (two columns,
  pinned on top, places on the right), Windows 10 (alphabetical list + pinned tiles, independent hover) and Windows 11
  (pinned grid + recommended). Position (centred / left) and size (90 / 100 / 115 %) are adjustable, the wheel scrolls
  the list, and right-click on a pinned tile opens the app menu.
- **Windows icons instead of ours**: Explorer and Settings entries borrow `explorer.exe` / `SystemSettings.exe` icons
  (`common::icon`, jumbo 256 px assets scaled down with an alpha-aware area average) in the bar, the Start menu and the
  desktop icon list.

## Language

- **Spanish** (`common/es.tsv`, `Lang::Es`): English texts are the keys, missing entries fall back to English. The
  language list, the `lang.txt` codes, plurals and number formatting know about `es`. `tools/i18n-check.ps1` reports
  **0 errors** for the whole workspace; the texts of Notepad, Calculator, Clipboard, Run, the icons' descriptions and
  the new bar setting are in the table too.
- **`plugins/topbar/src/fly.rs` encoding**: the file had been saved double-encoded (UTF-8 read as CP1252 and saved
  again), so every Russian/Japanese/Chinese/Portuguese string and every curly quote in the network panel and the other
  flyouts of that file showed mojibake. Converted back losslessly (1,266 Cyrillic and 1,026 CJK characters restored).

## File manager

- **This PC** as the default view (all drives), clickable "This PC" node, tabs moved to the title bar, **Recycle Bin /
  Network / Removable devices** groups in the nav, and Quick Access entries that can be pinned, unpinned and reordered.
- **Deleting** now reports the failure (`%TEMP%\winrust-delete.log` with the HRESULT and the paths) and offers a
  permanent delete when the Recycle Bin refuses (e.g. some volumes/drivers).

## Shell replacement

- `rs-shell` is the per-user shell (`HKCU\...\Winlogon\Shell`); the `rs-overlay` logon task is off, so `explorer.exe` is
  never started as the shell. It starts the bar, the desktop, the Run keys and the Win+D / Win+E hotkeys, and falls back to
  Explorer if the bar will not stay up.
- The Windows pieces WinRust replaces cannot start: `StartMenuExperienceHost`, `SearchApp`, `SearchUI`, `SearchHost`,
  `Widgets`, `Cortana` and `ShellExperienceHost` (IFEO `cmd.exe /c exit`, the same mechanism as `taskmgr.exe`).
  `explorer.exe` itself stays available: folders open in rust-explorer through `HKCU\Software\Classes\Directory|Folder\shell\open\command`,
  and URL/Settings hand-offs keep working.
- Windows Search (`WSearch`) is disabled (`rs-search` replaces it); Edge is gone (phase 1).
- `tools\shell-phase2.ps1`: `-DryRun` / `-Apply` / `-Restore`, state in `tools\state\shell-phase2.backup.json`.

## Tooling

- `tools/explorer-edge.ps1`: freezes the stock Explorer (policies only, `Shell` untouched) and removes Microsoft Edge
  with its own uninstaller, blocking reinstallation; `-DryRun` / `-Apply` / `-Restore`, every original value saved in
  `tools/state/explorer-edge.backup.json`. WebView2 untouched.
- `tools/wallpaper-engine.md`: how Wallpaper Engine attaches to `Progman`/`WorkerW` and the plan to support it in a
  custom shell (deferred).
- `PENDING.md`: what is done, what is paused and what is next.
- `plugins/rust-control`: a Control Panel in Rust (Category view as home, pages for System, Programs, Services, Devices,
  Network, Users, Power, Date/Time, Accessibility, Security and admin tools).

## Build

Most crates build with the GNU toolchain:

```
cargo +stable-x86_64-pc-windows-gnu build --workspace --exclude plugin-launcher --release
```

`rtaskmgr-win32` must be built with MSVC (its GNU build fails to load with `0xC0000139`):

```
cargo +stable-x86_64-pc-windows-msvc build -p rtaskmgr-win32 --release
```

The auto-updater task (`WinRust-update`) is disabled when using these builds, otherwise it replaces them with the
official ones.

## Notifications

- **Pop-up toasts** (`plugins/topbar/src/notify.rs`): rounded, frosted with the menu capture (`menu-blur`/`menu-alpha`),
  bottom right above the bar, at most three, slide + fade, hover pauses the auto-close, progress bar variant.
- **Any program can raise one** by writing a `winrust_common::notify::NotifyWire` to `\\.\pipe\winrust-notify`; the
  `rs-notify` helper wraps that (`--app --title --body --icon --timeout --progress --id --action --cmd`).
- **Bell + notification centre**: an unread dot on the bell next to the clock, a 360x420 panel with the 50-entry history,
  "Borrar todo" and click-to-reopen.
- `notify` and `notify-timeout` live in `look.txt` and are editable from the bar settings. Native Windows/UWP
  notifications still depend on Explorer and are not captured (documented as phase 2).