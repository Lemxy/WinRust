# WinRust — my custom shell

<p align="center">
  <img src="assets/logo.svg" alt="WinRust" width="120">
</p>

<p align="center">
  <b>A Windows shell replacement, tuned for one machine.</b><br>
  Real blur, three Start menu styles, a file manager, a Control Panel, notifications and a shell that never starts Explorer.
</p>

<p align="center">
  <a href="#screenshots">Screenshots</a> ·
  <a href="#what-this-fork-adds">What this fork adds</a> ·
  <a href="#replacing-windows-explorer">Replacing Explorer</a> ·
  <a href="#build">Build</a> ·
  <a href="#performance">Performance</a> ·
  <a href="#roadmap">Roadmap</a>
</p>

---

This is a personal fork of [**Lemxy/WinRust**](https://github.com/Lemxy/WinRust): the upstream project already rebuilds the
taskbar, the file manager, the file search and Task Manager in Rust. On top of it this fork adds the look and the pieces I
wanted for a daily-driver machine — every change is documented in [FORK-CHANGES.md](FORK-CHANGES.md), and what is still
missing lives in [PENDING.md](PENDING.md).

> Screenshots are from a real session (1360×768 + 1440×900, Spanish UI, `rs-shell` as the shell, no `explorer.exe`).

## Screenshots

### The bar

![The bar](docs/screenshots/bar.png)

A real blurred backdrop (screen capture + box blur + tint, refreshed while it is visible), the tray hugging the clock, the
bell for notifications and no hairline border. Right-click it for the bar menu:

![Bar menu](docs/screenshots/bar-menu.png)

### One bar per monitor

![Two bars](docs/screenshots/two-bars.png)

`bar-monitor=all` in `look.txt` (or **Settings → Bar → Monitor → All screens**) puts a bar on every monitor: each one
paints its own blur, buttons, pins and clock; the tray icons and the keyboard hook stay on the leader (the primary bar),
and only the primary reserves work area (Windows' `SPI_SETWORKAREA` is a single global rect). Popups open on the monitor
of the bar that was clicked.

### Start

![Start menu](docs/screenshots/start.png)

Windows 7, Windows 10 and Windows 11 styles (this is the Windows 10 one), pinned apps, independent hover per column, wheel
scrolling, right-click on a tile (open, run as administrator, **uninstall**, pin), adjustable position and size.

### Notifications

![Notification centre](docs/screenshots/notifications.png)

![Toasts](docs/screenshots/toasts.png)

Toasts with the same frost as the menus, a three-toast stack, a progress variant and a centre with the history. Any program
can raise one: write a `NotifyWire` to `\\.\pipe\winrust-notify`, or use the `rs-notify` helper:

```bat
rs-notify --app "Backup" --title "Copy finished" --body "Documents backed up" --icon "C:\Program Files\App\app.exe"
rs-notify --app "Download" --title "Downloading" --body "Windows.iso" --progress 0.42 --id download
```

### File manager

![File manager](docs/screenshots/explorer.png)

![This PC](docs/screenshots/this-pc.png)

Tabs in the title bar, **This PC** with every drive as its own cell (letter tag + free space), a 48 px icon grid as the
default view, a scrollable left pane with Quick access / This PC / Removable devices / Network / Recycle Bin, Quick access
that can be pinned, unpinned, reordered by drag and drop, a Recycle Bin view with Restore and Empty, and a delete path that
reports the real error when the shell cannot recycle.

### Control Panel

![Control Panel](docs/screenshots/control-panel.png)

A Control Panel written in Rust, following the project's look: the classic **Category** home page, the same eight
categories, each link opening a real page in the app or delegating to `ms-settings:` when Windows owns it. Pages for
System, Programs, Services, Devices, Network, Users, Power, Date/Time, Accessibility, Security and admin tools.

### Task Manager

![Task Manager](docs/screenshots/task-manager.png)

Half the size of the Windows one, smaller fonts, Mica-style frost and opaque rows.

## What this fork adds

- **Real blur** behind the bar, the menus and the panels (not translucency): the desktop behind the rect is captured,
  blurred and painted with a tint; `bar-blur`, `bar-radius`, `menu-alpha`, `menu-blur`, `menu-radius` in
  `%ProgramData%\WinRust\look.txt`.
- **Three Start menu styles** (`start7.rs`, `start10.rs`, `start11.rs`), position (left / centred) and size (90 / 100 / 115 %).
- **Windows 11 style context menus** with icons, shortcuts, ticks, one submenu level and live transparency, instead of
  `TrackPopupMenu`.
- **Tabbed settings window** instead of a long dropdown, double-buffered, with the frost background.
- **Spanish** across the shell (`common/es.tsv`, English is the key, missing entries fall back to English) plus the
  existing EN/RU/JA/PT/ZH UI; `tools/i18n-check.ps1` audits coverage, placeholders and duplicates.
- **Windows' own icons** for Explorer/Settings and sharp shell icons everywhere: `common::icon` takes the jumbo 256 px
  asset and scales it down with an alpha-aware area average.
- **Notifications** with a named pipe and `rs-notify`, a bell with an unread dot and a notification centre.
- **Control Panel** in Rust (new crate, `plugins/rust-control`).

## Replacing Windows Explorer

Phase 1 (`tools/explorer-edge.ps1`) froze the stock Explorer and removed Edge. Phase 2 (`tools/shell-phase2.ps1`) makes
the replacement real:

- `rs-shell.exe` is this user's shell (`HKCU\...\Winlogon\Shell`); the `rs-overlay` logon task is disabled, so
  **Explorer never starts at logon**. `rs-shell` starts the bar, the desktop, the Run keys and Win+D / Win+E, and hands the
  session back to Explorer if the bar will not stay up.
- The shell pieces WinRust replaces cannot start: `StartMenuExperienceHost`, `SearchApp`, `SearchUI`, `SearchHost`,
  `Widgets`, `Cortana` and `ShellExperienceHost` (`Image File Execution Options` → `cmd.exe /c exit`).
- Windows Search is disabled (`rs-search` replaces it); folders open in `rust-explorer` through
  `HKCU\Software\Classes\Directory|Folder\shell\open\command`; `explorer.exe` itself stays available for
  "show in folder" and URL/Settings hand-offs.
- Everything is reversible: both scripts keep their original values under `tools\state\` and support `-Restore`.

## Build

Most crates build with the GNU toolchain:

```bat
cargo +stable-x86_64-pc-windows-gnu build --workspace --exclude plugin-launcher --release
```

`rtaskmgr-win32` must be built with MSVC (its GNU build fails to load with `0xC0000139`):

```bat
cargo +stable-x86_64-pc-windows-msvc build -p rtaskmgr-win32 --release
```

The installer (`plugin-launcher`) embeds the built exes listed in `payload.txt`; populate `payload\` with
`scripts\build-plugins.ps1` before building it.

## Configuration

`%ProgramData%\WinRust\look.txt` (edited from the bar's settings window):

```ini
bar-blur=1
bar-radius=14
menu-alpha=67
menu-blur=1
menu-radius=18
start-left=1
start-scale=0
start-style=1
notify=1
notify-timeout=5
```

`lang.txt` holds the language code (`es` for Spanish). Window frost (`win-blur`) stays off by default: on bright wallpapers
the tint needed for readable text washes the window out.

## Performance

Measured on this machine; method and numbers in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

| Component | Binary | RAM (idle) | CPU (idle) | Window visible |
|---|---|---|---|---|
| Bar (`topbar`) | 1.03 MB | 43 MB | ~0.9 % | — |
| Desktop (`rust-explorer`) | 1.21 MB | 20–33 MB | 0 % | 78 ms |
| Shell (`rs-shell`) | 0.43 MB | 6.6 MB | 0 % | — |
| Control Panel | 0.74 MB | 14 MB | 0 % | 17 ms |
| Task Manager | 1.11 MB | 30 MB | 1.2 % | 28 ms |
| `rs-notify` | 0.28 MB | — (one shot) | — | — |

The bar's idle CPU is the live blur refresh; set `bar-blur=0` (or `menu-blur=0`) to trade the blur for ~0 %.

## Roadmap

- **Multi-monitor**: bar per monitor, flyouts/toasts anchored to their monitor, per-monitor DPI, hotplug. Full review in
  [tools/multimonitor-review.md](tools/multimonitor-review.md).
- **Native notifications bridge**: Discord/Steam/Serein post WinRT toasts, which need Explorer's toast host; a small
  package-identity listener (`UserNotificationListener`) forwarding them to `\\.\pipe\winrust-notify` is the next step.
- **Wallpaper Engine**: how WE attaches to `Progman`/`WorkerW` and the plan to support it in a custom shell:
  [tools/wallpaper-engine.md](tools/wallpaper-engine.md).

## Credits and license

Built on [Lemxy/WinRust](https://github.com/Lemxy/WinRust) (the original README is kept as
[README.upstream.md](README.upstream.md)). Same license as upstream (MIT / Apache-2.0, see `LICENSE-MIT` and
`LICENSE-APACHE`).
