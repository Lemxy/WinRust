# rust-notepad

A small Notepad for the WinRust ISO (see `tools/iso-replacement.md`): a Win32 window drawn with the shared
WinRust style (title bar with the file name and the `*` of unsaved changes, menu bar, status bar, rounded
cards), and a native multiline `EDIT` control as the editor. No new dependencies beyond `windows-sys` and
`winrust-common`.

```
rust-notepad [file]
```

With no file (or with `--help`, `/?`, `-h`, `-?`, which are just skipped) it opens an empty document; `-` is
not stdin either, it is skipped like the other flags. A path that does not exist is opened as an empty
document under that name and created on the first save.

The menus are native popup menus (`TrackPopupMenu`) for the same reason `rust-control` uses them: a
hand-drawn drop-down would be hidden behind the editor, which is a real child window. The title bar, the menu
bar, the find bar and the status bar are drawn with the shared style.

## Covered

| Area | What works |
| --- | --- |
| Window | Own chrome: title with the file name and `*` when dirty, drag, double-click to maximize, minimize / maximize / close, resize from the frame, DPI change, the WinRust style palette and the style-change broadcast. |
| Editor | `EDIT` with `ES_MULTILINE \| ES_AUTOVSCROLL \| ES_AUTOHSCROLL \| ES_NOHIDESEL \| WS_VSCROLL \| WS_HSCROLL`, monospace face (Cascadia Mono if installed, else Consolas), no automatic wrap, wheel scrolling, dark scrollbar where the theme supports it. |
| File | New (`Ctrl+N`), Open (`Ctrl+O`, `GetOpenFileNameW`), Save (`Ctrl+S`), Save as (`Ctrl+Shift+S`, `GetSaveFileNameW`), Exit (`Alt+F4`). All of them ask **Save changes?** (Yes / No / Cancel) when there are unsaved changes. |
| Clipboard and undo | `Ctrl+A`, `Ctrl+C`, `Ctrl+V`, `Ctrl+X`; `Ctrl+Z` / `Ctrl+Shift+Z` / `Ctrl+Y` are a multi-level undo of our own (one step per typing pause, 80 snapshots at most). |
| Encoding | UTF-8 with and without BOM, UTF-16 LE/BE (with BOM) and, for text that is neither valid UTF-8 nor has a BOM, the ANSI code page of the system (`MultiByteToWideChar(CP_ACP)`). The status bar shows what was read. |
| Line ends | CRLF, LF or CR are detected and kept: the editor always holds CRLF, the file gets its own ending back on save. New files are CRLF. |
| Find | `Ctrl+F` opens a bar above the status bar (single-line field, **Find next** button, close). `Enter` in the field and `F3` find the next match, case-insensitive, wrapping at the end, and the match stays selected in the editor while the field has the focus (`ES_NOHIDESEL`). `Esc` closes the bar. |
| Status | `Line X, Column Y`, the document length in characters, the encoding and the line ending. Short messages (Saved, Not found) appear there for four seconds. |
| Drag and drop | A dropped file is opened (with the save question first), over the window or over the editor. |
| CLI | `rust-notepad [file]`; IFEO's victim path as argv[1] is tolerated (see below). |

## Not covered

- Printing and page setup; there is no `notepad.exe /p` equivalent.
- Automatic line wrap, word wrap and a wrap toggle (the editor always scrolls horizontally).
- Replace / Replace all, Go to line, Insert time/date, a font and size chooser, zoom.
- Encodings other than the ones above (no UTF-32, no Big5/Shift-JIS picker); saving always writes UTF-8
  (with a BOM only when the file already had a UTF-8 one), so a UTF-16 or ANSI file is converted on its
  first save.
- A window per file and session restore: one process, one file, like Notepad.
- Very large files: every typing pause copies the whole text into the undo stack (capped at 80 copies) and
  the status bar recounts the characters, so a multi-megabyte file gets slower than a small one.
- Syntax highlighting, multiple cursors, word count, spell check, auto-save.
- Mica/frost behind the window: the palette of the style is solid (frost is optional in this project and
  the editor wanted a calm background).

## Install

`plugin-launcher` copies `rust-notepad.exe` into `Program Files\rust-notepad`, adds a **WinRust Notepad**
Start menu shortcut and lists the entry next to `rust-settings` (`src/install/tools.rs`, `src/main.rs`).
Uninstalling removes the file and the shortcut. The installer does **not** redirect `notepad.exe`.

## Redirecting notepad.exe (not done by the installer)

`notepad.exe` is a system binary: replacing the file is not an option (hardlink into WinSxS, WRP, servicing).
Redirect the *name* instead. Both of these are per-machine, need an elevated shell, and are reversible by
deleting the key or value.

IFEO (the same mechanism as `taskmgr.exe`, see `tools/iso-replacement.md`):

```
reg add "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\notepad.exe" /v Debugger /t REG_SZ /d "\"C:\Program Files\rust-notepad\rust-notepad.exe\"" /f
```

IFEO starts the debugger as `<debugger> <victim> [original arguments]`, so the editor receives
`C:\Windows\System32\notepad.exe` as `argv[1]` and the user's file as `argv[2]`. `rust-notepad` drops a first
argument that is a `notepad.exe` path (or, when a second argument exists, one that does not exist), trims
quotes left by a hand-written command line, uses the second argument when the first was dropped, ignores
`/p`, `/pt`, `/?`, `--help`, `-h`, and opens an empty document when only the victim path arrives. Undo:

```
reg delete "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\notepad.exe" /v Debugger /f
```

Shell `Applications` handler (per-user variant under `HKCU\Software\Classes`), with the `%1` the shell
substitutes for the file:

```
reg add "HKLM\SOFTWARE\Classes\Applications\notepad.exe\shell\open\command" /ve /t REG_SZ /d "\"C:\Program Files\rust-notepad\rust-notepad.exe\" \"%1\"" /f
```

Notes, from `tools/iso-replacement.md`:

- IFEO resolves by image name and depends on the bitness of the process that calls `CreateProcess`; an app
  execution alias (`%LOCALAPPDATA%\Microsoft\WindowsApps\notepad.exe`) may win over the name. Test both.
- EDR may flag IFEO as T1546.012; keep the change reversible and journaled.
- `OpenWithProgids` / the Default apps UI is the supported route for the *file* associations (`.txt`); the
  installer does not touch `UserChoice` or `UCPD.sys`-protected associations.

## i18n

Every text goes through `t!`/`tf!`. Spanish comes from `common/es.tsv` (English is the key), so the keys
below still need their Spanish rows:

```
Untitled
File
Edit
New
Open...
Save
Save as...
Exit
Undo
Redo
Cut
Copy
Paste
Select all
Find...
Find next
Save changes to "{}"?
Cannot open the file: {}
Cannot save the file: {}
Line {}, Column {}
Length: {}
Saved
Not found
Search
Text files
All files
Open
Save as
Cannot open a folder.
```

## Build

```
cargo check -p rust-notepad
cargo build --release -p rust-notepad
```

`app.ico` is a placeholder (the WinRust app icon copied from `rust-settings`); replace it with a notepad
icon before shipping the image.
