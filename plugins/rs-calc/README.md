# rs-calc

The WinRust calculator for the ISO plan in `tools/iso-replacement.md` (the `calc.exe` / Calculator row).
A frameless Win32 window drawn with the shared WinRust style, plus a command line that evaluates an
expression. No dependencies beyond `windows-sys` and `winrust-common`.

```
rs-calc                  the window
rs-calc "2+2*3"          evaluate and print (exit 0, or 1 on a bad expression)
rs-calc --rad "sin(pi/2)"
rs-calc --help
```

## Covered

| Area | What works |
| --- | --- |
| Window | Own chrome: 360x560 (minimum 320x480), drag by the header, resize from every edge and corner, close button, mouse-wheel history scrolling, the style palette (`common::style::pal()`), the style-change broadcast, high DPI, rounded region, optional frost (`look.txt`: `win-blur=1`, off by default). The display has a focus ring and the whole window takes typing. |
| Standard | Digits, `+ − × ÷`, `%`, `±`, decimal key (`.` or `,` according to the language), `C`, `CE`, `⌫`, `1/x`, `x²`, `√`, and `%` with the calculator meaning (`200 + 10%` = 220, `200 × 10%` = 20). |
| Scientific | `fx` chip: `sin cos tan asin acos atan ln log`, `xʸ`, `e`, `π`, `n!`, `EXP`, `(` `)` and a `DEG`/`RAD` chip in the display. The chip switches the unit of the trigonometric functions. |
| Memory | `MC MR M+ M− MS`; an `M` mark in the display while the memory is not zero. |
| History | The last 20 results (`=` commits): expression on the left, result on the right; a click loads the result into the display. The clock button opens it, `Clear` empties it. History, memory, angle and mode are kept in `%APPDATA%\WinRust\rs-calc.txt`. |
| Keyboard | `0-9` and the numpad, `+ - * /`, `Enter` or `=`, `Backspace`, `Delete` (C), `%`, `^`, `!`, `(`, `)`, `,` / `.`, `e` (EXP); `Esc` closes the history panel first and then the window. `Ctrl+C` / `Ctrl+V` copy and paste the display (a pasted `2+2 = 4` keeps the right-hand side). |
| CLI | Recursive-descent parser (`calc.rs`): `+ - * / % ^`, parentheses, unary signs, factorial, functions and constants. `%` after a value is a percent (`50%` = 0.5); between two values it is the remainder (`7%3` = 1). The IFEO victim path (`...\calc.exe`) is ignored, so a redirect opens the window. |
| Precision | `f64`, at most 12 significant digits: fixed notation in `1e-6 .. 1e12`, scientific outside it, trailing zeros removed, `-0` shown as `0`. A non-finite result (division by zero, `ln(0)`, `sqrt(-1)`, overflow) is an error, never `inf`/`NaN`. |

## Not covered

- Unit conversion, a paper tape / export, programmable keys, a keypad-only RTL layout.
- Typing function names letter by letter (`s`, `i`, `n`): use the scientific panel; pasted expressions may contain them.
- A window per calculation: a second launch brings the existing window to the front.
- Locale-aware thousands separators; the parser also accepts `,` as the decimal separator.
- `cmd` does not wait for a GUI-subsystem program, so `%ERRORLEVEL%` right after `rs-calc "2+2"` is cmd's,
  not the calculator's. For scripts: `start /wait rs-calc "2+2"` (cmd) or `Start-Process -Wait` (PowerShell).
  The output itself is written to the parent console (or to the redirected handle), so `rs-calc "2+2" > out.txt`
  works; CLI output always uses `.` as the decimal separator.

## Install

`plugin-launcher` copies `rs-calc.exe` into `Program Files\rs-calc`, adds a **WinRust Calculator** Start menu
shortcut and lists the entry next to `rust-settings` (`src/install/tools.rs`, `src/main.rs`). Uninstalling
removes the file and the shortcut. The installer does **not** redirect `calc.exe`.

## Redirecting calc.exe (not done by the installer)

`calc.exe` is a stub and a system binary: replacing the file is not an option (hardlink/store, WRP, servicing).
Redirect the *name* instead, per machine, from an elevated shell:

```
reg add "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\calc.exe" /v Debugger /t REG_SZ /d "\"C:\Program Files\rs-calc\rs-calc.exe\"" /f
```

Undo:

```
reg delete "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\calc.exe" /v Debugger /f
```

IFEO runs `<debugger> <victim path> [arguments]`; `rs-calc` drops a first argument that is a `calc.exe`
path, so the window opens normally. The key read depends on the bitness of the caller (a 32-bit process uses
`HKLM\SOFTWARE\WOW6432Node\...`), and EDR may flag IFEO as T1546.012 — journal it and keep it reversible.
An `App Paths\calc.exe` entry is the softer, per-user alternative where `ShellExecute` is the caller.

## i18n

Every text goes through `t!`/`tf!`; Spanish comes from `common/es.tsv` (English is the key). The strings of this
program:

```
Calculator
History
Clear
Nothing yet
Error
rs-calc — the WinRust calculator
Usage: ... (the full usage block, one key)
Operators: ... / Functions: ... / % after a value ... (one key)
Hint: cmd does not wait for a GUI program. ... (one key)
```

`--print-strings` prints them (and the usage) as `key<TAB>text`, which is handy for checking a `lang.txt`
without a screen. The keys on the keypad (`C`, `CE`, `MC`, `MR`, `M+`, `M−`, `MS`, `DEG`, `RAD`, `fx`, the
digits and the operators) are the universal calculator notation and are not translated, as in Windows'
own calculator.

## Build

```
cargo check -p rs-calc
cargo test -p rs-calc
cargo build --release -p rs-calc
```

`app.ico` is a generated 256x256 icon (a graphite calculator with the system-blue display); replace it if the
image needs a different one.
