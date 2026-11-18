# WinRust: Roadmap

Русская версия: [ROADMAP.ru.md](ROADMAP.ru.md) | Português: [ROADMAP.ptbr.md](ROADMAP.ptbr.md)

Each item says how far it was checked: **tests** (unit tests), **built** (compiles, never run), **ran** (run by hand on a live system), **e2e** (full install/uninstall on a live system).

## Done

### Installer

- One `plugin-launcher.exe` with every plugin embedded (unpacked into Program Files, not %TEMP%), static CRT, dark Win32/GDI window. _ran_
- "Install WinRust" installs what is missing in order (topbar before rust-explorer), updates when everything is there; "Remove all" removes in reverse order. _built_
- A failed install undoes its own changes (not when updating a working copy); original values (WSearch, search box, Explorer settings, services) are saved under `HKLM\SOFTWARE\plugin-launcher\<plugin>` and put back on uninstall; non-fatal errors go to `%ProgramData%\plugin-launcher\install.log`. _built_
- Restore point before install. _built_
- One button installs what is missing **and** updates what is installed, so running a newer `WinRust.exe` brings its fixes to every plugin (v0.2.1). _built_
- Command line: `list | install <plugin> --yes | update <plugin> | uninstall <plugin> | rollback --yes`, exit codes 0/1/2; the window and the command line call the same functions. Only the read-only commands were run. _ran (partly)_
- `update` and `scripts\update-plugin.ps1`: reinstall from `payload\` on disk without rebuilding the installer. _ran_
- English by default, Русский/English switch in the installer and in the bar menu; the choice is stored in `%ProgramData%\WinRust\lang.txt`. _tests_
- Review fixes (v0.3.0): rs-du keeps its quarantine list next to its exe (admin-only), `payload\` two levels up only from `target\release`, rtaskmgr and rust-explorer save and restore another program's Task Manager / folder settings, rust-search brings Windows Search back before removing its files, DiagTrack is left to debloat, one `reg query` parser with a test. _tests, built_
- Code layout: one cargo workspace; the installer split into `install/` (one file per plugin), `cli.rs`, `ui.rs`; shared `winrust-common` crate (language switch, MFT reader); one `payload.txt` list for the build and the scripts; CI on GitHub Actions. _tests_

### Plugins

- **rust-search**: NTFS search service + Alt+Space; disables WSearch, blocks SearchHost.exe, hides the taskbar search box. Right-click a result: rename, recycle bin, "delete permanently + clean the registry" (scan → confirm → delete; registry saved to `%LOCALAPPDATA%\rs-search\registry-backups\*.reg` first). _tests, ran_
- **rtaskmgr-win32**: replaces Task Manager through an IFEO Debugger (`taskmgr.exe` untouched); on a panic writes `%APPDATA%\rtaskmgr\crash.log` and opens the real Taskmgr. _tests, ran_
- **topbar**: Windows 11-style bottom bar instead of the taskbar; `topbar.exe --restore` brings the taskbar back. Installs with it: reshell hotkeys, rs-du, the PowerShell profile block (`src/install/profile.ps1`, Windows PowerShell 5.1 and pwsh 7). _ran; the profile block only on a temporary profile_
- **reshell safety net**: if topbar is not running while the taskbar is hidden, twice in a row (~10 s), it runs `topbar.exe --restore`. _tests; killing topbar was not tried_
- **reshell hotkeys**: Win+D (show desktop) and Win+I (Settings) work without Explorer, like Win+E (v0.2.1). _ran_
- **Styles** (v0.3.0): a gear at the left end of the bar switches Normal / Terminal / Floating dock / Tiling; the bar, rust-explorer and the search window repaint at once (a broadcast message, `%ProgramData%\WinRust\style.txt`). The dock reserves no screen space and slides away from maximized windows; Tiling puts the bar at the top and can tile the windows. _ran_
- **topbar look** (v0.3.0): the network icon is drawn (Wi-Fi arcs by signal, a cross when offline, "!" when there is no internet, per Windows' connectivity hint); new Wi-Fi panel (status, connected card with its own Disconnect button, saved/open networks, rows fade in); text-only menus with key hints; input language picker (left click on ENG/RUS); the tray flyout fades in and its chevron turns. _ran_
- **rust-explorer**: file manager (tabs, icons, IFileOperation copy/paste/delete, shell context menu, drag and drop), draws the desktop, turns off Explorer as the shell with `rs-shell --overlay` (90 s after sign-in at the latest: Windows 26300 no longer writes the autostart markers it used to wait for, v0.2.1), trims services. _tests, ran_
- **rs-du**: disk cleaner (MFT analysis, caches, `target`/`node_modules`, recycle bin, 30-day quarantine), started from the bar menu. _tests_
- **wiztree**: disk-usage treemap from the MFT. _tests (layout only); not run on a live disk_
- **photoview**: WIC photo viewer, EXIF rotation, "Open with" entry. _ran; EXIF orientations 5 and 7 not checked_
- **debloat**: services and policies (telemetry, Copilot, Recall, ads, Edge background), old values in `%ProgramData%\winrust-debloat\undo.txt`, `--undo`. _dry run ran; `--apply`/`--undo` not run_

## Next (in order)

1. **Test on a clean Windows 11 VM**: install, uninstall, rollback, reboot, for each plugin. Nothing has been checked on another machine yet.
2. **One undo journal for every plugin**: each change (registry value, service, file) written as "before → after" before it is made; uninstall and rollback replay it backwards. Replaces the hand-written uninstall of each plugin. Needs step 1 first, and a migration for installs made by the current version.
3. **Registry through the Win32 API** (`RegGetValueW`/`RegSetValueExW`) instead of running `reg.exe` and parsing its output.
4. **Releases**: SHA-256 next to the exe, version in the window, update check.
5. **Code signing** (removes the SmartScreen warning and part of the antivirus false positives).
6. **PowerShell profile**: optionally install rg/fd/bat/eza/zoxide through winget (a checkbox in the topbar settings).
7. **chrome-rs**: a real install once the browser is ready (see `plugins/chrome-rs/ROADMAP.md`).
8. **Win+S / Start button** open rust-search instead of Windows Search (risky, a separate decision).
9. **Non-NTFS drives** in rust-search (skipped today).

## Known limitations

- System components are disabled or bypassed (reversibly), not removed: deleting `taskmgr.exe` or Windows Search outright would break system protection.
- Administrator rights are required.
- Win+S and Start still open Windows Search.
