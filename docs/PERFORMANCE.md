# WinRust metrics

Measured on 2026-11-26 12:05 on the machine below, with the weather/news item enabled. Method at the end; `scripts\metrics.ps1` reproduces every row.

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
| topbar | `topbar.exe` | 1.23 MB |
| rs-shell | `rs-shell.exe` | 0.44 MB |
| desktop | `rust-explorer.exe` | 1.38 MB |
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
| **Total** | | **10.9 MB** |

## Idle cost of the shell

RAM: `WorkingSet64` after 5 s of no work. CPU: `TotalProcessorTime` delta over the same window, as a percentage of ONE logical core (12 cores).

| Process | RAM | CPU (1 core) |
|---|---:|---:|
| Bar (with weather and news) | 45.9 MB | 0.13 % |
| Desktop | 21.5 MB | 0.00 % |
| Supervisor | 9.0 MB | 0.00 % |
| Clipboard (Win+V) | 20.9 MB | 0.00 % |
| Desktop hotkeys | 6.0 MB | 0.00 % |
| **Shell total** | **103.3 MB** | **0.13 %** |

## Desktop and the metrics widget

`rs-metrics` is not launched by the script because it is off by default; these are 60 s of idle with every module on
(card plus the temperature pill detached), same method as the table above.

| Piece | RAM (private) | CPU (1 core) |
|---|---:|---:|
| `rs-metrics` (all modules) | 10.2 MB | 0.05 % |

The desktop icon list lives in the existing `rust-explorer --desktop` process (21.5 MB, 0.00 % above). Its folder
watcher is one blocked `ReadDirectoryChangesW` per Desktop folder (no polling), and the drive list is read on a worker
with a 3 s deadline, so a removed USB cannot freeze the window.

## Weather and the copy toast

The bar row above was taken with the weather item on: 60 s with the flyout closed, private memory 12.1 MB, 8 threads and
590 handles. The weather worker is asleep between refreshes; it wakes once every 15 minutes (or when the flyout is opened
with data older than 5 minutes), does one HTTP request per feed and stores the result in memory. The 0.13 % includes the
bar's own frost repaint; the weather worker itself added no measurable CPU in this window.

The copy detail has no process of its own. `rust-explorer` sends one 3.7 KB `NotifyWire` every 500 ms while a job runs
(about 7 KB/s, only while the bar is listening) and the bar keeps 60 samples × 8 bytes for each of the two graph lines
(under 1 KB per open card). The record is written from the job's ticker thread, never from a copy worker, and a missing
bar just drops it, so the transfer can never wait for the notification UI.

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
