# rs-shot — Spectacle-like capture and recording for WinRust

Reference study for `plugins/rs-shot`. **No GPL code was copied**: this file records how Spectacle behaves so the Rust
implementation can reproduce the UX with its own GDI / Win32 code.

## 1. Sources read

Fetched on 2026-11-24 into `%TEMP%\spectacle-ref` (outside the repository, so no GPL file enters this tree):

| Source | Result |
|---|---|
| `https://github.com/KDE/spectacle` (mirror of `invent.kde.org/graphics/spectacle`), `--depth 1` | cloned; `README.md`, `src/Gui/*.qml`, `src/ExportManager.cpp`, `src/ExportMenu.cpp`, `src/SpectacleCore.cpp` read |
| `https://invent.kde.org/graphics/kimageannotator`, `--depth 1` | **not available**: KDE Invent asks for a KDE Identity login for anonymous clones here (Git Credential Manager prompt; `GIT_TERMINAL_PROMPT=0` → “could not read Username”). GitHub has no `KDE/kimageannotator` mirror. |
| `kimageannotator` behaviour | taken from Spectacle's own annotation toolbar/options QML, which exposes the KImageAnnotator tool set; the library source itself was not read. |

Relevant Spectacle files:

- `src/Gui/AnnotationsToolBarContents.qml` — tool list: crop, select, freehand, highlighter, line, arrow, rectangle,
  ellipse, pixelate, blur, text, number; undo/redo group; Shift/Ctrl modifiers (45° lines, square/circle, resize from
  centre).
- `src/Gui/AnnotationOptionsToolBarContents.qml` — stroke width, stroke colour, fill colour, effect strength, font
  family/size/colour, number, shadow.
- `src/Gui/CaptureOverlay.qml` — full-screen dimmed overlay, selection rectangle with handles, live size label,
  magnifier, click-a-window selection, Enter/Esc.
- `src/Gui/SizeLabel.qml` — “width × height” label under/above the selection.
- `src/ExportManager.cpp` / `ExportMenu.cpp` — autosave filename (`Screenshot_…` style, date placeholders), preferred
  image format, quality/compression, save/copy/open/print/share/OCR, screenshot folder.
- `src/SpectacleCore.cpp` — global shortcuts: PrintScreen (open), Shift+Print (full screen), Meta+Print (active
  window), Meta+Ctrl+Print (window under cursor), Meta+Shift+Print (region); recording shortcuts.

## 2. Spectacle behaviour reproduced (and what was not)

### Capture modes

| Spectacle | rs-shot |
|---|---|
| Full screen (all monitors) | ✅ `--full`, virtual desktop via `BitBlt` + `CAPTUREBLT` |
| Current screen / screen under cursor | ✅ `--monitor` (cursor monitor), `--monitor N` (1-based) |
| Active window | ✅ `--window` (foreground, `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS)` + `PrintWindow`) |
| Window under cursor | ✅ clicking a window in the region overlay picks it and captures it with `PrintWindow` |
| Choose a window from a list | ❌ not yet (foreground window or click in the overlay only) |
| Rectangular region | ✅ full-virtual-desktop overlay, one window across all monitors |
| Delay 0/2/5/10 s | ✅ `--delay N` (any seconds; sleeps before the overlay/capture, no countdown UI yet) |
| Include/exclude pointer | ✅ `--cursor` / default excluded |
| Magnifier in the overlay | ❌ pending (size label, thirds guides, handles and window hover outline are present) |

Overlay behaviour: desktop frozen and dimmed, dragged rectangle stays bright; accent 2 px border, eight handles, live
“W × H” badge, thirds guide lines, hint pill; Esc or right click cancels, Enter confirms; a click without a drag picks
the window under the pointer (or the monitor when the desktop is clicked). Coordinates are physical pixels; the process
is per-monitor DPI aware v2, so monitors at different scales line up.

### Editor (Spectacle tool set)

| Tool / action | rs-shot |
|---|---|
| Crop | ✅ drag and release; non-destructive crop rect, undo restores |
| Select / move / delete | ✅ click a shape, drag to move, Delete removes it |
| Arrow, line | ✅ 3 px accent, Shift snaps to 45° |
| Rectangle, ellipse | ✅ outline, Shift makes a square/circle |
| Text | ✅ click, type, Enter commits (single line; BMP-only characters) |
| Highlighter | ✅ freehand with translucent yellow blending |
| Pixelate / blur | ✅ rectangle, block / box-blur pixel operations |
| Number | ✅ auto-incrementing numbered discs |
| Freehand pen | ❌ (only the highlighter is freehand) |
| Undo / redo / clear all | ✅ snapshot undo stack, Ctrl+Z / Ctrl+Y, clear button |
| Zoom | ✅ Ctrl+wheel, 10 %–800 %; “fit” on open; no pan yet |
| Stroke/fill colour, width, font, shadow, effect strength, number editor | ❌ fixed accent colour, 3 px strokes, 24 px text, yellow highlighter, 14 px pixel blocks, radius 10 blur |
| Save (Ctrl+S) | ✅ auto file + clipboard together |
| Copy (Ctrl+C) | ✅ clipboard only |
| Save-as dialog / open with / print / OCR / share | ❌ not in this pass |
| Notification after copy | status line in the editor (no toast) |

### Output

- Default folder `%USERPROFILE%\Pictures\Screenshots` (created), name
  `Screenshot_2026-11-24_15-30-00.png` (a `-2` suffix is added if two land in the same second).
- `--file PATH` overrides the name; the extension chooses PNG (default), JPEG (`.jpg`/`.jpeg`) or BMP.
- Clipboard: 24-bit `CF_DIB` (top-down buffer converted to bottom-up rows, 4-byte aligned), accepted by Paint,
  Word, browsers and `System.Windows.Forms.Clipboard` (verified).
- The editor's **Save and copy** writes the file and the clipboard; **Copy** only the clipboard.

### Recording (new in this pass)

Spectacle records screen/window/region to a video file and offers audio. rs-shot implements the source choice, the
floating stop UI, duration/fps and an output path; WGC + Media Foundation is the primary encoder, with ffmpeg as the
documented bridge for sources/builds WGC cannot take.

| Spectacle | rs-shot |
|---|---|
| Record region / monitor / window | ✅ region (default), monitor (cursor or `N`), foreground window |
| Choose a window from a list | ❌ same as capture |
| Floating stop button, border, timer | ✅ topmost recording bar (pulsing red dot, elapsed mm:ss, source size, fps, Stop), 3 px accent frame around the source |
| Stop with Esc | ✅ temporary low-level keyboard hook (swallows Esc while recording, catches a quick tap) |
| Duration, fps, output path | ✅ `--duration`, `--fps`, `--out` |
| H.264/MP4 | ✅ WGC + Media Foundation (window verified; monitor where the OS allows it), ffmpeg bridge otherwise |
| Audio | ❌ `--audio` accepted and ignored; pending |
| Pause / resume, hardware encoding | ❌ |

## 3. Technology decisions

### Images: GDI+ encoders, no new crate

The spec allowed “PNG via `common::wall`, else BMP + WIC”. `winrust-common` has no PNG encoder, and WIC would add COM
machinery. The repository already ships GDI+ through `windows-sys` (`Win32_Graphics_GdiPlus`, used by
`plugins/topbar/src/shot.rs`), so rs-shot uses:

- PNG encoder CLSID `557CF406-…`, JPEG `557CF401-…`, BMP `557CF400-…` via `GdipSaveImageToFile`;
- a 24-bit DIB copy before encoding (screen bitmaps carry alpha = 0, which would make a 32-bit PNG transparent);
- `GdiplusStartup` once in `main`, before capture/recording;
- no image, png, jpeg or WIC dependency.

This choice is documented here because the brief asked to decide and document it.

### Video: Windows.Graphics.Capture + Media Foundation, with an ffmpeg bridge

**Primary encoder (implemented and tested):** Windows.Graphics.Capture + Media Foundation. A free-threaded WGC frame
pool captures a **window** or **monitor**, each D3D11 texture is copied to a CPU-readable staging texture and turned
into an MF sample, and an H.264 `IMFSinkWriter` writes the MP4. No ffmpeg is needed for a window recording (verified),
and for a monitor or a region inside one monitor on Windows builds where `CreateForMonitor` works.

**Why the CPU copy:** feeding DXGI texture samples straight to the H.264 MFT returned `E_INVALIDARG` on every
`WriteSample`; the staging copy to an RGB32 memory sample is the portable path. It costs ~4 MB per 1360×768 frame
(≈120 MB/s at 30 fps), acceptable for this pass.

**Known OS bug on this machine:** Windows 11 build **26300** rejects
`IGraphicsCaptureItemInterop::CreateForMonitor` with `E_INVALIDARG` (“Could not capture the given monitor”), a reported
Windows regression in 26100.7623+. Window capture works. rs-shot probes WGC first and, when the probe fails, falls back
to ffmpeg for monitor/region sources. The long-term fix is `GraphicsCaptureItem.TryCreateFromDisplayId(DisplayId)` with
`Microsoft.UI.Interop.GetDisplayIdFromMonitor` (Windows App SDK) or a fixed Windows build; a diagnostic line is written
to `%TEMP%\rs-shot-wgc.log` when the probe/record fails.

**Encoder choice:** `--encoder auto` (default) probes WGC and uses it when the source is one window or one monitor and
the probe succeeds; otherwise it uses ffmpeg. `--encoder wgc` forces WGC and reports the error; `--encoder ffmpeg` forces
the frame bridge. A region crossing monitors and the full multi-monitor desktop have no single WGC item, so they always
use ffmpeg.

**ffmpeg fallback (optional today, required for monitor/region on builds hit by the OS bug):**

1. `ffmpeg.exe` beside `rs-shot.exe`;
2. `%ProgramFiles%\rs-shot\ffmpeg.exe`;
3. `%ProgramFiles%\ffmpeg\*\bin\ffmpeg.exe` — the versioned Gyan build, e.g.
   `C:\Program Files\ffmpeg\ffmpeg-9.1-essentials_build\bin\ffmpeg.exe` (glob, survives version upgrades);
4. every directory on `PATH`;
5. WinGet fallbacks: `%LOCALAPPDATA%\Microsoft\WinGet\Links\ffmpeg.exe` and
   `%LOCALAPPDATA%\Microsoft\WinGet\Packages\Gyan.FFmpeg*\**\bin\ffmpeg.exe`.

It captures JPEG frames (`%TEMP%\rs-shot-rec-<date>\frame-000001.jpg`, at `--fps`) and runs:

```text
ffmpeg -hide_banner -loglevel error -y -framerate <measured> -start_number 1 \
       -i "<dir>\frame-%06d.jpg" -c:v libx264 -preset veryfast -pix_fmt yuv420p "<out>.mp4"
```

JPEG frames keep a 1080p30 take small enough for `%TEMP%`; the sequence shape is the requested
`-framerate N -i frame%06d.<ext> -c:v libx264 -pix_fmt yuv420p`. The measured rate is `frames / elapsed`, so a slow
encoder keeps real time.

**No partial or orphan recordings:**

- the encoder is resolved/probed *after* the source is chosen but *before* the recording bar appears; if neither WGC
  nor ffmpeg can take the source, a dialog explains ffmpeg installation and the command exits 4 — nothing is recorded;
- WGC writes directly to the final MP4; a failed WGC recording deletes the partial file;
- the ffmpeg bridge uses one temp folder per take, deleted **on success and on failure**; a hard-killed process is
  cleaned by the next run (folders older than 12 h);
- the WGC probe writes a temporary MP4 under `%TEMP%` and deletes it immediately.

**Installing the optional encoder today** (already present on this machine):

```powershell
winget install --id Gyan.FFmpeg -e
# installed here: C:\Program Files\ffmpeg\ffmpeg-9.1-essentials_build\bin\ffmpeg.exe
ffmpeg -version
```

A machine-PATH update is picked up by a *new* rs-shot process; the explicit `Program Files\ffmpeg\*\bin` lookup works
even when the shell was started before the PATH change.

**Next WGC work:**

1. monitor capture once the Windows build regression is fixed or via `TryCreateFromDisplayId`;
2. GPU-side colour conversion (Video Processor MFT) instead of the CPU staging copy;
3. `--audio` through WASAPI loopback;
4. pause/resume, hardware encoder preferences.

**Audio** is not recorded. `--audio` parses and is ignored; the spec marks it pending.

## 4. Exact commands

```powershell
# Default: region overlay, then the editor (no arguments is the same as --region)
rs-shot.exe

# Direct capture, no editor: save + clipboard
rs-shot.exe --full --no-ui --file "%TEMP%\full.png"
rs-shot.exe --monitor --cursor --no-ui --file "%TEMP%\monitor.png"
rs-shot.exe --monitor 2 --delay 5 --no-ui --file "%TEMP%\monitor2.jpg"
rs-shot.exe --window --delay 2 --no-ui --file "%TEMP%\window.png"
rs-shot.exe --region --no-ui --file "%TEMP%\region.png"

# Region capture with the editor, pre-set destination
rs-shot.exe --region --file "%USERPROFILE%\Pictures\Screenshots\demo.png"

# Recording (WGC + Media Foundation first; ffmpeg bridge for monitor/region on this Windows build)
rs-shot.exe --record                                          # region overlay, 30 fps, until Stop/Esc
rs-shot.exe --record --window --duration 10 --fps 30 --out "%TEMP%\clip.mp4"
rs-shot.exe --record --window --encoder wgc --duration 5
rs-shot.exe --record --monitor 1 --duration 10 --fps 30 --out "%TEMP%\clip.mp4"
rs-shot.exe --record --monitor 1 --encoder ffmpeg --duration 10
rs-shot.exe --record --region --delay 2 --fps 60 --out "%TEMP%\region.mp4"

# Help (message box: the binary is windows-subsystem, no console flash)
rs-shot.exe --help
```

Editor keys: `V` select, `C` crop, `A` arrow, `L` line, `R` rectangle, `E` ellipse, `T` text, `H` highlighter,
`P` pixelate, `B` blur, `N` number, Ctrl+Z / Ctrl+Y undo/redo, Delete removes the selected shape, Ctrl+S save+copy,
Ctrl+C copy, Ctrl+wheel zoom, Esc close/cancel.

## 5. Shell integration

`plugins/topbar/src/run.rs` (the leader bar's low-level keyboard hook) and `plugins/topbar/src/main.rs`:

- PrintScreen seen by the low-level keyboard hook starts `rs-shot --region` **when rs-shot is installed**, also while
  Explorer runs (Windows' own snip setting may own the key; the hook runs before the system hotkey). Without rs-shot the
  hook keeps the old restriction (PrintScreen only without Explorer).
- Win+Shift+S (already intercepted by the hook) now starts `rs-shot --region`; if rs-shot is missing it posts the old
  `SNIP` message, so the previous topbar clipper runs unchanged.
- PrintScreen seen by the hook without Explorer also starts `rs-shot --region`; missing rs-shot falls back to the old
  screenshot notice.
- As a backstop outside the hook, the bar tries `RegisterHotKey(PrintScreen)` when rs-shot exists; it may fail because
  the system already owns the key. The hook is the primary path.
- Win+Alt+R starts `rs-shot --record` (region) only when rs-shot exists; otherwise the key is left to Windows/Game Bar.
- Lookup order for the binary: `%ProgramFiles%\rs-shot\rs-shot.exe`, beside the bar `topbar.exe`, then
  `%LOCALAPPDATA%\plugin-launcher\bin\rs-shot.exe`. The installer still needs a `Program Files\rs-shot` entry (out of
  scope here); `payload.txt` and the workspace `members` list are updated.

## 6. Files

- `plugins/rs-shot/Cargo.toml` — `windows-sys` (GDI, GDI+, Dwm, DataExchange, Memory, Xps for `PrintWindow`,
  Shell, WindowsAndMessaging, Controls, HiDpi, KeyboardAndMouse, SystemInformation, Threading) + `windows` (WGC,
  Direct3D11/DXGI, Media Foundation, WinRT interop) + `winrust-common`.
- `plugins/rs-shot/src/main.rs` — CLI, delay, dispatch.
- `capture.rs` — `BitBlt`/`PrintWindow`, cursor, window at point, DWM bounds.
- `img.rs` — top-down BGRA image, DIB readers/writers.
- `save.rs` — GDI+ PNG/JPEG/BMP, CF_DIB clipboard, auto names, frame folders.
- `overlay.rs` — virtual-desktop region overlay.
- `editor.rs` — toolbar, canvas, tools, undo/redo, crop, save/copy.
- `annot.rs` — annotation model and pixel/GDI renderer.
- `record.rs` — frame recorder, recording bar, stop hook, ffmpeg bridge.
- `wgc.rs` — Windows.Graphics.Capture + Media Foundation H.264/MP4 encoder (probe, D3D11, SinkWriter, region crop).
- `ui.rs` — DPI, fonts, fills, dashed rectangles, Segoe MDL2 text.
- `Cargo.toml` root — added `plugins/rs-shot` to `members`; `payload.txt` — `rs-shot.exe`; installer entry in
  `src/main.rs` + `src/install/tools.rs` (`%ProgramFiles%\rs-shot`); installer height in `src/ui.rs`.

## 7. What was verified

- `cargo check -p rs-shot` and `cargo check -p topbar` pass with the local toolchain (`cargo 1.100.0`); rs-shot adds
  no warnings of its own.
- `--full --no-ui` produced a 2800 × 900 PNG; the same to `.jpg` produced a valid JPEG (394 KB).
- `--window --no-ui` produced a 504 × 576 PNG.
- Clipboard was read back with `System.Windows.Forms.Clipboard.GetImage()` (1360 × 768).
- The editor was drawn into with a synthetic arrow drag; Ctrl+S wrote a 1.8 MB PNG containing the arrow.
- A regional drag from (200, 200) to (700, 500) produced a 500 × 300 crop.
- `--record --monitor --duration 3 --fps 10` wrote 31 JPEG frames; Esc after ~2 s stopped at 21 frames; a window
  recording wrote 11 frames at 662 × 696. The stop bar and accent frame were absent from the frames.
- **WGC + Media Foundation**: `--record --window --encoder wgc` produced a 133 KB MP4; `ffprobe` reports `h264`,
  662 × 696, 15 frames, 1.875 s. The staged CPU→MF sample path is the one that works (texture samples failed with
  `E_INVALIDARG` on `WriteSample`).
- **WGC monitor on this build**: `CreateForMonitor` returns `0x80070057` on Windows 11 build 26300 (reported Windows
  regression). `--encoder auto --record --monitor` probes, logs to `%TEMP%\rs-shot-wgc.log`, and falls back to ffmpeg:
  1440 × 900, 17 frames, 2.13 s MP4, temp frames removed.
- **ffmpeg bridge**: `--record --monitor --duration 3 --fps 10 --out …mp4` produced a 102 KB file; `ffprobe` reports
  `h264`, 1360 × 768, 31 frames, 3.109 s (~9.97 fps). A forced ffmpeg failure (`.txt` output) left neither an output
  file nor a frame folder.
- **Not executed:** the WGC monitor/region crop path (blocked by the OS bug above), WGC on Windows 10, and audio.

## 8. Review list before shipping

1. **Installer**: a `rs-shot` plugin entry now copies `rs-shot.exe` to `%ProgramFiles%\rs-shot\rs-shot.exe`
   (`src/install/tools.rs`, `PLUGINS` in `src/main.rs`; the installer window grew one row in `src/ui.rs`). The payload
   must be refreshed with `scripts\build-plugins.ps1` before building the launcher, or `payload\rs-shot.exe` is absent
   and the launcher's build script stops with its usual message.
2. **PrintScreen ownership**: if Windows' screen-capture setting owns the key, `RegisterHotKey` fails by design; check
   Win+Shift+S and Win+Alt+R on a machine with Explorer and with the WinRust shell.
3. **WGC monitor**: `CreateForMonitor` fails on this Windows 11 build (26300, a reported regression). Until it is fixed
   or `TryCreateFromDisplayId` is wired, monitor/region recordings fall back to ffmpeg; window recordings already use
   WGC. Test on a build below 26100.7623 and on Windows 10 1903+.
4. **WGC performance/limits**: the CPU staging copy is used because texture samples were rejected; move to a Video
   Processor MFT next. WGC window items change size when the window resizes; the current pool keeps the initial size.
   Minimized/protected windows produce no frames and the error is reported.
5. **ffmpeg dependency**: the lookup covers beside-the-exe, `Program Files\rs-shot`, versioned
   `Program Files\ffmpeg\*\bin`, PATH and WinGet. Check the versioned glob after a Gyan upgrade and on a machine where
   PATH is stale in an already-running topbar.
6. **PrintWindow** can return a black frame for some GPU/composited apps and does not capture hardware overlays; the
   fallback is a screen `BitBlt` only when PrintWindow fails, not when it returns a black bitmap.
6. **DPI/multi-monitor**: test 100 % + 150 % monitors side by side, mixed heights (virtual desktop has uncovered black
   areas), and moving the editor between monitors (WM_DPICHANGED).
7. **Editor limits**: fixed colours/widths, single-line text, no pan, no save-as dialog; text input handles BMP
   characters only (surrogate pairs become spaces).
8. **Recording cost**: JPEG frames are kept for the length of the capture; a long 4K take fills `%TEMP%` until the
   encode ends (success or failure both clean it). `--duration` is the guard; WGC will remove the frame folder entirely.
9. **GDI+**: confirm `gdiplus.dll` startup succeeds on the target Windows versions; every encoder call is checked and
   returns false rather than writing a broken file.
10. **Clipboard**: 24-bit CF_DIB has no alpha; that is intentional for compatibility. Transparent annotations are
    flattened onto the screenshot.
11. **Release build**: `cargo build --release -p rs-shot` (workspace `panic = "abort"`, `opt-level = "s"`); the icon and
    manifest come from the shared `app.ico` / `app.rc` copied from photoview.
