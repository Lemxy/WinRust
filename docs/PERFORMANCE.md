# Performance

Measured on the machine this fork runs on:

| | |
|---|---|
| CPU | AMD Ryzen 5 5500 (6C / 12T) |
| RAM | 32 GB |
| GPU | Radeon RX 580 |
| OS | Windows 11 Enterprise LTSC (build 26300) |
| Displays | 1360×768 (primary) + 1440×900, 100 % DPI |

Everything is a `--release` build with the GNU toolchain, except `rtaskmgr-win32` (MSVC).

## Method

- **Binary size**: `target/release/<exe>` on disk.
- **RAM**: `Process.WorkingSet64` after the process has been idle for five seconds (no measurements during startup).
- **CPU**: `TotalProcessorTime` delta over five seconds of idle, as a percentage of **one** logical core (12 cores
  available; 0.9 % here is ≈ 0.08 % of the whole CPU).
- **Window visible**: three cold starts per app, `Stopwatch` from `CreateProcess` to `MainWindowHandle != 0`, median.
  The bar cannot be measured this way (single instance, it is part of the shell start); the desktop figure is a new
  `rust-explorer.exe <folder>` window.

## Results

| Component | Binary | RAM (idle) | CPU (idle) | Window visible |
|---|---|---|---|---|
| Bar (`topbar.exe`) | 1.03 MB | 43.2 MB | 0.94 % | — |
| Desktop (`rust-explorer.exe --desktop`) | 1.21 MB | 20.5–33.1 MB | 0 % | 78 ms (new window) |
| Shell supervisor (`rs-shell.exe`) | 0.43 MB | 6.6 MB | 0 % | — |
| Control Panel (`rust-control.exe`) | 0.74 MB | 13.9 MB | 0 % | 17 ms |
| Task Manager (`rtaskmgr-win32.exe`) | 1.11 MB | 29.9 MB | 1.25 % | 28 ms |
| Settings (`rust-settings.exe`) | 0.84 MB | — | — | — |
| `rs-notify.exe` | 0.28 MB | one shot | — | — |

Notes:

- The bar's CPU is the **live blur**: it captures the strip behind the bar, blurs it and repaints while it is visible.
  Setting `bar-blur=0` in `look.txt` removes it (the bar then uses the flat tint).
- The desktop holds the wallpaper bitmap for the virtual screen (2800×900 here), which is most of its memory.
- `rust-control` pays for its GDI+ surfaces on first paint; the window itself appears in ~17 ms.
- The whole shell (bar + desktop + supervisor) sits at **≈ 70 MB** and under 1 % of one core in normal use.

## Startup of the session

`rs-shell` is the logon shell. It frees the console, starts the Run keys and the Startup folder through `cmd /c start`
(without waiting for them), paints the wallpaper and starts the bar and the desktop. On this machine the bar is on screen
before the autostart programs finish appearing; the supervisor then keeps the bar and the desktop alive.
