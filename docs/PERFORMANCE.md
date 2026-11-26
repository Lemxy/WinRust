# WinRust metrics

Measured on 2026-11-26 10:02 on the machine below. Method at the end; `scripts\metrics.ps1` reproduces every row.

| | |
|---|---|
| CPU | AMD Ryzen 5 5500 (6C / 12T) |
| RAM | 32 GB |
| GPU | Radeon RX 580 Series |
| OS | Microsoft Windows 11 Enterprise LTSC (build 26300) |
| Displays | 1360x768 (primary) + 1440x900 |
| Build | release (GNU), rtaskmgr with MSVC |

## Binaries

| Program | File | Size |
|---|---|---:|
| topbar | `topbar.exe` | 1.07 MB |
| rs-shell | `rs-shell.exe` | 0.44 MB |
| desktop | `rust-explorer.exe` | 1.34 MB |
| rs-clip | `rs-clip.exe` | 0.50 MB |
| reshell | `reshell.exe` | 0.38 MB |
| rs-calc | `rs-calc.exe` | 0.48 MB |
| rust-notepad | `rust-notepad.exe` | 0.43 MB |
| rust-settings | `rust-settings.exe` | 1.01 MB |
| rust-control | `rust-control.exe` | 0.73 MB |
| rtaskmgr | `rtaskmgr-win32.exe` | 1.15 MB |
| photoview | `photoview.exe` | 0.41 MB |
| wiztree | `wiztree.exe` | 0.50 MB |
| rs-shot | `rs-shot.exe` | 0.69 MB |
| rs-notify | `rs-notify.exe` | 0.24 MB |
| rs.exe | `rs.exe` | 0.50 MB |
| rs-launcher | `rs-launcher.exe` | 0.82 MB |
| **Total** | | **10.7 MB** |

## Idle cost of the shell

RAM: `WorkingSet64` after 5 s of no work. CPU: `TotalProcessorTime` delta over the same window, as a percentage of ONE logical core (12 cores).

| Process | RAM | CPU (1 core) |
|---|---:|---:|
| Bar | 49.9 MB | 0.00 % |
| Desktop | 21.5 MB | 0.00 % |
| Supervisor | 9.0 MB | 0.00 % |
| Clipboard (Win+V) | 20.9 MB | 0.00 % |
| Desktop hotkeys | 6.0 MB | 0.00 % |
| **Shell total** | **107.4 MB** | **0.00 %** |

## Cold start and idle cost of each app

`CreateProcess` -> `MainWindowHandle != 0`, median of 3 runs; RAM and CPU 5 s after the window appeared. Each app is closed afterwards.

| App | Window visible (ms) | RAM | CPU (1 core) |
|---|---:|---:|---:|
| rs-calc | 30 | 19.2 MB | 1.00 % |
| rust-notepad | 39 | 19.8 MB | 0.00 % |
| rust-settings | 248 | 30.4 MB | 0.00 % |
| rust-control | 38 | 19.2 MB | 0.00 % |
| Task Manager | 31 | 34.0 MB | 21.00 % |
| photoview | 30 | 18.6 MB | 0.00 % |
| File manager (new window) | 77 | 29.0 MB | 1.00 % |

## Opening the UI

Real mouse click on the bar -> first visible frame of the window it opens, median of 3. The bar dodges windows, so the pointer touches the screen edge first.

| Action | Latency (ms) |
|---|---:|
| Start menu | 32.6 |
| Bar menu (right click) | 33.2 |
| Notification centre (bell) | 28.4 |

## Notifications

`rs-notify` process start -> toast on screen: **9.2 ms** (median of 10, range 9-42 ms).

## File search

| Search | Median (ms) | Hits |
|---|---:|---:|
| `rs.exe winrust` (MFT) | 17 | 101 |
| Windows Search | not available | - |

## Recovery: the supervisor restarts the shell

`rs-shell` watches the bar and the desktop. Each number is the time from killing the process to its window being visible again, median of 2. Killing them is safe: the supervisor restarts them, which is exactly what this measures.

| Piece | Back after |
|---|---:|
| Bar (`topbar`) | 5.36 s |
| Desktop (`rust-explorer --desktop`) | 0.22 s |

## Installer

`WinRust.exe` with every plugin embedded: **13.5 MB**; the installed programs above add up to 10.7 MB.

## Method

- Binary size: the installed file on disk.
- RAM: `Process.WorkingSet64` after the process has been idle for 5 s (never during startup).
- CPU: `TotalProcessorTime` delta over 5 s of idle, as a percentage of ONE logical core (12 cores available).
- Window visible: `Stopwatch` from `CreateProcess` to `MainWindowHandle != 0`, median of 3; the first run of an app is discarded only when it fails to show a window.
- Flyouts: a real click on the bar, `Stopwatch` from the click to the flyout window (class `topbar_fly` / `topbar_notify`) being visible, median of 3.
- Notifications: `rs-notify` process start to the toast window being visible, median of 10.
- Search: `rs.exe <term>` to process exit vs the same term through Windows Search (ADO), median of 3.
- Recovery: the supervisor (`rs-shell`) is watched while the bar or the desktop is killed, median of 2.
- The bar idle CPU is the live blur: `bar-blur=0` in `%ProgramData%\WinRust\look.txt` trades it for ~0 %.

_Measured on a real desktop with the fork look (Win7-style Start, dodge windows, blur on)._
