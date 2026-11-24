//! Our own "Open with" picker.
//!
//! Why it exists: without explorer.exe the modern picker (`OpenWith.exe`) never shows - it starts at (-32000, -32000)
//! with a 6x28 window - and `rundll32 shell32.dll,OpenAs_RunDLL` ends up in that same surface, so a file with no
//! association cannot be opened at all. A plain Win32 dialog of our own is the only picker that renders, so it is what
//! a double click on an unassociated file gets, and what the context menu's "Open with..." opens.
//!
//! The list comes from the registry, the same places Windows itself reads:
//!   1. `...\.<ext>\OpenWithProgids` (HKCU, then HKLM) -> ProgID -> `<ProgID>\shell\open\command` (the exe is parsed
//!      out of the command line);
//!   2. `...\Applications\*.exe` whose `SupportedTypes` names the extension;
//!   3. a reasonable fallback: Notepad for text, the default browser for .html/.htm/.url, Paint for pictures.
//! "Always use this app" writes the type's default ProgID under HKCU - never UserChoice, which Windows protects.
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    mem::{size_of, zeroed},
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HWND, RECT, ERROR_MORE_DATA, ERROR_SUCCESS, S_OK},
    Graphics::Gdi::*,
    System::{
        LibraryLoader::GetModuleHandleW,
        Registry::{
            RegCloseKey, RegCreateKeyExW, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
            HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_NONE,
            REG_OPTION_NON_VOLATILE, REG_SZ,
        },
        Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW, STARTF_USESHOWWINDOW},
    },
    UI::{
        Input::KeyboardAndMouse::{
            SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_SPACE, VK_UP,
        },
        Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE},
        WindowsAndMessaging::*,
    },
};
use winrust_common::t;

use crate::ui::{self, sc};

const CLASS: &str = "WinRustOpenWith";
/// Rows of applications: past this the list stops being a picker and becomes a wall.
const MAX_APPS: usize = 8;
const WIN_W: i32 = 460;
const ROW_H: i32 = 46;
const ICON: i32 = 26;
const HEAD: i32 = 70;
const WM_MOUSELEAVE: u32 = 0x02A3; // not exported under the enabled features

/// `HKCU`, `HKLM` then the merged view, for anything a ProgID may be registered in.
const CLASS_ROOTS: [(HKEY, &str); 3] = [
    (HKEY_CLASSES_ROOT, ""),
    (HKEY_CURRENT_USER, r"Software\Classes\"),
    (HKEY_LOCAL_MACHINE, r"Software\Classes\"),
];

thread_local! {
    /// The dialog that is up right now: a second request brings it forward instead of opening another one.
    static OPEN: std::cell::Cell<HWND> = const { std::cell::Cell::new(ptr::null_mut()) };
}

// ---------------------------------------------------------------- registry helpers (native, no reg.exe)

fn wc(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

unsafe fn open_key(root: HKEY, sub: &str) -> Option<HKEY> {
    let k = wc(sub);
    let mut h: HKEY = ptr::null_mut();
    (RegOpenKeyExW(root, k.as_ptr(), 0, KEY_READ, &mut h) == ERROR_SUCCESS).then_some(h)
}

unsafe fn create_key(root: HKEY, sub: &str) -> Option<HKEY> {
    let k = wc(sub);
    let mut h: HKEY = ptr::null_mut();
    let r = RegCreateKeyExW(root, k.as_ptr(), 0, ptr::null(), REG_OPTION_NON_VOLATILE, KEY_READ | KEY_WRITE, ptr::null(), &mut h, ptr::null_mut());
    (r == ERROR_SUCCESS).then_some(h)
}

/// The UTF-16 text of a REG_SZ / REG_EXPAND_SZ value (`name` None = the key's default value).
unsafe fn string_value(h: HKEY, name: Option<&str>) -> Option<String> {
    let n = name.map(wc);
    let p = n.as_ref().map_or(ptr::null(), |n| n.as_ptr());
    let (mut ty, mut len) = (0u32, 0u32);
    if RegQueryValueExW(h, p, ptr::null(), &mut ty, ptr::null_mut(), &mut len) != ERROR_SUCCESS || len == 0 {
        return None;
    }
    if ty != REG_SZ && ty != REG_EXPAND_SZ {
        return None;
    }
    let mut b = vec![0u8; len as usize + 2]; // room for the NUL that may not be counted
    if RegQueryValueExW(h, p, ptr::null(), &mut ty, b.as_mut_ptr(), &mut len) != ERROR_SUCCESS {
        return None;
    }
    // the data is little-endian UTF-16: decoded by hand so no alignment is assumed on the byte buffer
    let units: Vec<u16> = b.chunks_exact(2).take(len as usize / 2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s = String::from_utf16_lossy(&units);
    let s = s.trim_end_matches('\0').to_string();
    Some(if ty == REG_EXPAND_SZ { expand(&s) } else { s })
}

/// `%SystemRoot%\x` -> the value of the variable (REG_EXPAND_SZ commands are written that way).
fn expand(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('%') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let var = &after[..j];
        match std::env::var(var) {
            Ok(v) => out.push_str(&v),
            Err(_) => {
                out.push('%');
                out.push_str(var);
                out.push('%');
            }
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

unsafe fn subkeys(h: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0u32;
    loop {
        let mut buf = vec![0u16; 256];
        let mut n = buf.len() as u32;
        let r = RegEnumKeyExW(h, i, buf.as_mut_ptr(), &mut n, ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut());
        if r != ERROR_SUCCESS {
            break;
        }
        if n > 0 {
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
        i += 1;
    }
    out
}

/// The value names of `h` (an `OpenWithProgids` key stores the ProgIDs this way, `SupportedTypes` the extensions).
unsafe fn value_names(h: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0u32;
    loop {
        let mut buf = vec![0u16; 256];
        let mut n = buf.len() as u32;
        // a scratch buffer for the data: only the name is wanted, and a value with more data answers
        // ERROR_MORE_DATA - which still carries the name
        let mut data = [0u8; 16];
        let mut dlen = data.len() as u32;
        let mut ty = 0u32;
        let r = RegEnumValueW(h, i, buf.as_mut_ptr(), &mut n, ptr::null(), &mut ty, data.as_mut_ptr(), &mut dlen);
        if r != ERROR_SUCCESS && r != ERROR_MORE_DATA {
            break;
        }
        if n > 0 {
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
        i += 1;
    }
    out
}

unsafe fn set_string(h: HKEY, name: &str, value: &str) {
    let n = wc(name);
    let v = wc(value);
    RegSetValueExW(h, n.as_ptr(), 0, REG_SZ, v.as_ptr() as *const u8, (v.len() * 2) as u32);
}

unsafe fn set_none(h: HKEY, name: &str) {
    let n = wc(name);
    RegSetValueExW(h, n.as_ptr(), 0, REG_NONE, ptr::null(), 0);
}

// ---------------------------------------------------------------- the association and the candidates

/// The executable Windows would use for `ext` (".txt"), if the type has a real handler.
///
/// `AssocQueryStringW` answers S_FALSE with the size it needs when the buffer does not fit, so the size is asked for
/// first. It does *not* fail for a type with no association: it answers S_OK with Windows' own picker
/// (`%SystemRoot%\System32\OpenWith.exe`), which is precisely the window that cannot render here - that answer is
/// treated as "no association".
unsafe fn assoc_exe(ext: &str) -> Option<PathBuf> {
    let e = wc(ext);
    let mut n = 0u32;
    let hr = AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, e.as_ptr(), ptr::null(), ptr::null_mut(), &mut n);
    if hr < 0 || n == 0 {
        return None;
    }
    let mut buf = vec![0u16; n as usize];
    let hr = AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_EXECUTABLE, e.as_ptr(), ptr::null(), buf.as_mut_ptr(), &mut n);
    if hr != S_OK {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..n.saturating_sub(1) as usize]);
    let p = PathBuf::from(s.trim());
    if p.as_os_str().is_empty() || is_open_with(&p) {
        return None;
    }
    Some(p)
}

/// `...\System32\OpenWith.exe`: what Windows hands back for a type that has no association at all.
fn is_open_with(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("OpenWith.exe"))
}

/// The ProgID the type itself is registered to (`HKCR\.<ext>`'s default value, "txtfile" and the like).
unsafe fn class_progid(key: &str) -> Option<String> {
    for (root, pre) in CLASS_ROOTS {
        if let Some(k) = open_key(root, &format!("{pre}{key}")) {
            let v = string_value(k, None);
            RegCloseKey(k);
            if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
                return Some(v);
            }
        }
    }
    None
}

/// The ProgID the user picked in "Open with" (Windows keeps it in FileExts\<ext>\UserChoice). Read only: the value is
/// hash-protected, and writing it by hand leaves a type no program will open.
unsafe fn user_choice(key: &str) -> Option<String> {
    let path = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\{key}\UserChoice");
    let k = open_key(HKEY_CURRENT_USER, &path)?;
    let v = string_value(k, Some("ProgId"));
    RegCloseKey(k);
    v.filter(|v| !v.trim().is_empty())
}

/// True when the file's type has an association: the double click then keeps going straight through ShellExecute.
///
/// Three sources, because the shell API alone lies without explorer: `AssocQueryString` answers with OpenWith.exe for
/// a type nobody registered, the merged classes view knows the ProgID, and UserChoice holds the user's own pick.
pub fn associated(file: &Path) -> bool {
    let Some(ext) = file.extension() else { return false };
    let key = format!(".{}", ext.to_string_lossy().to_lowercase());
    unsafe { assoc_exe(&key).is_some() || class_progid(&key).is_some() || user_choice(&key).is_some() }
}

/// A candidate in the list.
struct App {
    exe: PathBuf,
    name: String,
    /// The exe path, shown dimmed under the name.
    hint: String,
    /// The ProgID it came from, when it came from one: what "always use" writes.
    progid: Option<String>,
}

fn app_of(exe: PathBuf, name: Option<String>, progid: Option<String>) -> App {
    let name = name
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
    let hint = exe.display().to_string();
    App { exe, name, hint, progid }
}

/// The first token of a `shell\open\command` line: the exe. Quoted paths are the normal case; an unquoted one ends at
/// ".exe" (a path with spaces is always quoted by the shell).
fn parse_exe(cmd: &str) -> Option<PathBuf> {
    let s = cmd.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(rest) = s.strip_prefix('"') {
        let end = rest.find('"')?;
        return (!rest[..end].is_empty()).then(|| PathBuf::from(&rest[..end]));
    }
    let lower = s.to_ascii_lowercase();
    if let Some(i) = lower.find(".exe") {
        return Some(PathBuf::from(&s[..i + 4]));
    }
    s.split_whitespace().next().map(PathBuf::from)
}

unsafe fn command_of(progid: &str) -> Option<String> {
    let sub = format!(r"{progid}\shell\open\command");
    for (root, pre) in CLASS_ROOTS {
        if let Some(k) = open_key(root, &format!("{pre}{sub}")) {
            let v = string_value(k, None);
            RegCloseKey(k);
            if let Some(v) = v.filter(|v| !v.trim().is_empty()) {
                return Some(v);
            }
        }
    }
    None
}

unsafe fn name_of_progid(progid: &str) -> Option<String> {
    for (root, pre) in CLASS_ROOTS {
        if let Some(k) = open_key(root, &format!(r"{pre}{progid}\Application")) {
            let v = string_value(k, Some("ApplicationName"));
            RegCloseKey(k);
            if v.is_some() {
                return v;
            }
        }
    }
    None
}

unsafe fn name_of_app(exe_name: &str) -> Option<String> {
    for (root, pre) in CLASS_ROOTS {
        if let Some(k) = open_key(root, &format!(r"{pre}Applications\{exe_name}")) {
            let v = string_value(k, Some("FriendlyAppName"));
            RegCloseKey(k);
            if v.is_some() {
                return v;
            }
        }
    }
    None
}

unsafe fn app_of_progid(progid: &str) -> Option<App> {
    let exe = parse_exe(&command_of(progid)?)?;
    Some(app_of(exe, name_of_progid(progid), Some(progid.to_string())))
}

/// Adds `app` unless its exe is already in the list (Windows often lists one app under two ProgIDs).
fn add(out: &mut Vec<App>, seen: &mut Vec<String>, app: App) {
    if out.len() >= MAX_APPS {
        return;
    }
    let key = app.exe.to_string_lossy().to_lowercase();
    if seen.contains(&key) {
        return;
    }
    seen.push(key);
    out.push(app);
}

fn system_exe(name: &str) -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from)?;
    let p = root.join("System32").join(name);
    p.is_file().then_some(p)
}

/// The full path of an application named in `Applications\<exe>`: App Paths first (that is where Windows looks when the
/// key has no command of its own, which is the common case), then the system folder.
unsafe fn app_path(name: &str) -> Option<PathBuf> {
    for (root, pre) in [
        (HKEY_CURRENT_USER, r"Software\Microsoft\Windows\CurrentVersion\App Paths\"),
        (HKEY_LOCAL_MACHINE, r"Software\Microsoft\Windows\CurrentVersion\App Paths\"),
    ] {
        if let Some(k) = open_key(root, &format!("{pre}{name}")) {
            let v = string_value(k, None);
            RegCloseKey(k);
            if let Some(v) = v {
                let p = PathBuf::from(v.trim().trim_matches('"'));
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    system_exe(name)
}

/// Every app that can open `file`, in the order Windows lists them, deduplicated by exe.
fn candidates(file: &Path) -> Vec<App> {
    let ext = file.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out: Vec<App> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    if !ext.is_empty() {
        let key = format!(".{ext}");
        unsafe {
            // 1. the ProgIDs registered for the type
            for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
                let path = format!(r"Software\Classes\{key}\OpenWithProgids");
                let Some(k) = open_key(root, &path) else { continue };
                for progid in value_names(k) {
                    if let Some(app) = app_of_progid(&progid) {
                        add(&mut out, &mut seen, app);
                    }
                }
                RegCloseKey(k);
            }
            // 2. applications that declare the type in SupportedTypes
            for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
                let Some(apps) = open_key(root, r"Software\Classes\Applications") else { continue };
                for name in subkeys(apps) {
                    if !name.to_ascii_lowercase().ends_with(".exe") {
                        continue;
                    }
                    let Some(st) = open_key(apps, &format!(r"{name}\SupportedTypes")) else { continue };
                    let supported = value_names(st).iter().any(|v| v.eq_ignore_ascii_case(&key));
                    RegCloseKey(st);
                    if !supported {
                        continue;
                    }
                    // the key rarely carries a command: the exe named by it is resolved through App Paths
                    let exe = command_of(&format!(r"Applications\{name}")).and_then(|c| parse_exe(&c)).or_else(|| app_path(&name));
                    if let Some(exe) = exe {
                        let app = app_of(exe, name_of_app(&name), Some(format!(r"Applications\{name}")));
                        add(&mut out, &mut seen, app);
                    }
                }
                RegCloseKey(apps);
            }
        }
    }
    // 3. something rather than an empty list
    let mut extra = Vec::new();
    if ["txt", "log", "ini", "md", "json", "xml", "csv"].contains(&ext.as_str()) {
        if let Some(exe) = system_exe("notepad.exe") {
            extra.push(app_of(exe, None, None));
        }
    }
    if ["html", "htm", "url"].contains(&ext.as_str()) {
        if let Some(exe) = unsafe { assoc_exe(".html") } {
            extra.push(app_of(exe, None, None));
        }
    }
    if crate::is_pic(file) {
        if let Some(exe) = system_exe("mspaint.exe") {
            extra.push(app_of(exe, None, None));
        }
    }
    for app in extra {
        add(&mut out, &mut seen, app);
    }
    out
}

/// "Always use this app": makes `exe` (or the ProgID it was found under) the handler for the file's type.
///
/// Only HKCU\Software\Classes is written. UserChoice (%APPDATA%\...\FileExts\.<ext>\UserChoice) is hash-protected by
/// Windows: writing it by hand leaves a type every program refuses to open, so it is never touched here.
unsafe fn remember(file: &Path, exe: &Path, progid: Option<String>) {
    let Some(ext) = file.extension() else { return };
    let key = format!(".{}", ext.to_string_lossy().to_lowercase());
    let Some(progid) = progid.or_else(|| exe.file_name().map(|n| format!(r"Applications\{}", n.to_string_lossy()))) else {
        return;
    };
    let base = format!(r"Software\Classes\{key}");
    if let Some(k) = create_key(HKEY_CURRENT_USER, &base) {
        set_string(k, "", &progid); // the default value of .<ext> = the ProgID
        RegCloseKey(k);
    }
    if let Some(k) = create_key(HKEY_CURRENT_USER, &format!(r"{base}\OpenWithProgids")) {
        set_none(k, &progid); // and it is listed as one of the type's handlers
        RegCloseKey(k);
    }
}

/// Starts `exe` with `file` as its argument, in the file's own folder (an app that looks for files next to the
/// document finds them). The path is quoted: "C:\Program Files\..." is one argv entry, not two.
unsafe fn launch(exe: &Path, file: &Path) -> bool {
    let app = wc(&exe.to_string_lossy());
    let cmd = format!("\"{}\" \"{}\"", exe.display(), file.display());
    let mut line: Vec<u16> = cmd.encode_utf16().chain([0]).collect();
    let dir = file.parent().map(|d| wc(&d.to_string_lossy()));
    let mut si: STARTUPINFOW = zeroed();
    si.cb = size_of::<STARTUPINFOW>() as u32;
    si.dwFlags = STARTF_USESHOWWINDOW;
    si.wShowWindow = SW_SHOWNORMAL as u16;
    let mut pi: PROCESS_INFORMATION = zeroed();
    let ok = CreateProcessW(
        app.as_ptr(),
        line.as_mut_ptr(),
        ptr::null(),
        ptr::null(),
        0,
        0,
        ptr::null(),
        dir.as_ref().map_or(ptr::null(), |d| d.as_ptr()),
        &si,
        &mut pi,
    ) != 0;
    if ok {
        CloseHandle(pi.hProcess);
        CloseHandle(pi.hThread);
    }
    ok
}

// ---------------------------------------------------------------- the dialog

/// What the picker came back with.
enum Pick {
    /// Start this exe; the bool is "always use this app".
    App(PathBuf, Option<String>, bool),
    /// "Search another app...": the caller falls back to the classic picker.
    Search,
    /// Dismissed.
    None,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hit {
    None,
    Row(usize),
    Check,
    Open,
    Cancel,
}

struct Dlg {
    apps: Vec<App>,
    icons: Vec<HICON>, // from winrust_common::icon::best: cached and shared, never destroyed
    file_name: String,
    title: String,
    search: String,
    always_text: String,
    open_text: String,
    cancel_text: String,
    n: usize, // apps.len() + the "search another app" row
    w: i32,
    head: i32,
    sel: usize,
    hover: Hit,
    tracking: bool,
    /// A double click in progress: the click that opens the row is its button-up, so the dialog is gone only after the
    /// whole gesture has been consumed.
    dbl: bool,
    always: bool,
    chosen: Option<usize>,
    done: bool,
    px: Vec<u32>, // the blurred screen behind the window, taken before it is shown
    fonts: [HFONT; 4], // title, body, small, glyph
    check_row: RECT,
    open_btn: RECT,
    cancel_btn: RECT,
}

impl Drop for Dlg {
    fn drop(&mut self) {
        // The icons are shared copies out of common::icon's cache: destroying one would break every later paint of the
        // same exe. The fonts are ours.
        for f in self.fonts {
            if !f.is_null() {
                unsafe { DeleteObject(f) };
            }
        }
    }
}

fn row_rect(d: &Dlg, i: usize) -> RECT {
    let top = d.head + sc(ROW_H) * i as i32;
    RECT { left: sc(8), top: top + sc(2), right: d.w - sc(8), bottom: top + sc(ROW_H) - sc(2) }
}

fn inside(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn hit(d: &Dlg, x: i32, y: i32) -> Hit {
    if inside(d.open_btn, x, y) {
        return Hit::Open;
    }
    if inside(d.cancel_btn, x, y) {
        return Hit::Cancel;
    }
    if inside(d.check_row, x, y) {
        return Hit::Check;
    }
    for i in 0..d.n {
        if inside(row_rect(d, i), x, y) {
            return Hit::Row(i);
        }
    }
    Hit::None
}

unsafe fn glyph(dc: HDC, f: HFONT, g: &str, r: RECT, c: u32) {
    let old = SelectObject(dc, f);
    SetTextColor(dc, c);
    let mut rr = r;
    let s = crate::w(g);
    DrawTextW(dc, s.as_ptr(), -1, &mut rr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, old);
}

unsafe fn paint(h: HWND, d: &Dlg) {
    let mut ps: PAINTSTRUCT = zeroed();
    let dc = BeginPaint(h, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(h, &mut cr);
    let p = ui::pal();

    // the same recipe as the context menu: what was behind the window, blurred, with the theme over it
    let alpha = (255 * winrust_common::look::menu_alpha() as u32 / 100) as u8;
    let mut painted = false;
    if !d.px.is_empty() {
        painted = winrust_common::frost::paint_capture(dc, &d.px, cr, p.bg, alpha);
    }
    if !painted {
        let b = CreateSolidBrush(p.bg);
        FillRect(dc, &cr, b);
        DeleteObject(b);
    }
    SetBkMode(dc, TRANSPARENT as i32);

    // the heading: what the dialog is for and which file it is about
    let old = SelectObject(dc, d.fonts[0]);
    ui::text(dc, &d.title, RECT { left: sc(20), top: sc(14), right: cr.right - sc(20), bottom: sc(38) }, p.text, DT_LEFT);
    SelectObject(dc, d.fonts[2]);
    ui::text(dc, &d.file_name, RECT { left: sc(20), top: sc(38), right: cr.right - sc(20), bottom: sc(58) }, p.dim, DT_LEFT);
    SelectObject(dc, old);
    let line = CreateSolidBrush(p.line);
    FillRect(dc, &RECT { left: 0, top: d.head - sc(8), right: cr.right, bottom: d.head - sc(7) }, line);
    DeleteObject(line);

    // the candidates
    let text_left = sc(8) + sc(10) + sc(ICON) + sc(12);
    for (i, app) in d.apps.iter().enumerate() {
        let r = row_rect(d, i);
        if i == d.sel {
            ui::fill(dc, r, ui::mix(ui::listbg(), p.accent, 0.22), sc(7));
        } else if d.hover == Hit::Row(i) {
            ui::fill(dc, r, p.hover, sc(7));
        }
        if let Some(&ic) = d.icons.get(i) {
            let (ix, iy) = (r.left + sc(10), (r.top + r.bottom - sc(ICON)) / 2);
            winrust_common::icon::draw(dc, ic, ix, iy, sc(ICON));
        }
        SelectObject(dc, d.fonts[1]);
        ui::text(dc, &app.name, RECT { left: text_left, top: r.top + sc(4), right: r.right - sc(10), bottom: r.top + sc(25) }, p.text, DT_LEFT);
        SelectObject(dc, d.fonts[2]);
        ui::text(dc, &app.hint, RECT { left: text_left, top: r.top + sc(24), right: r.right - sc(10), bottom: r.bottom - sc(2) }, p.dim, DT_LEFT);
        SelectObject(dc, old);
    }
    // the last row is always "Search another app...": the classic picker, the one place it still makes sense
    {
        let i = d.apps.len();
        let r = row_rect(d, i);
        if i == d.sel {
            ui::fill(dc, r, ui::mix(ui::listbg(), p.accent, 0.22), sc(7));
        } else if d.hover == Hit::Row(i) {
            ui::fill(dc, r, p.hover, sc(7));
        }
        glyph(dc, d.fonts[3], "\u{E721}", RECT { left: r.left + sc(10), top: r.top, right: r.left + sc(10) + sc(ICON), bottom: r.bottom }, p.dim);
        SelectObject(dc, d.fonts[1]);
        ui::text(dc, &d.search, RECT { left: text_left, top: r.top, right: r.right - sc(10), bottom: r.bottom }, p.text, DT_LEFT);
    }

    // "always use this app": a rounded box, the accent when it is on
    let cy = (d.check_row.top + d.check_row.bottom) / 2;
    let cb = sc(18);
    let check = RECT { left: sc(22), top: cy - cb / 2, right: sc(22) + cb, bottom: cy - cb / 2 + cb };
    ui::fill(dc, check, if d.always { p.accent } else { ui::mix(p.bg, p.text, 0.10) }, sc(5));
    if d.always {
        glyph(dc, d.fonts[3], "\u{E73E}", check, winrust_common::style::on_accent());
    }
    SelectObject(dc, d.fonts[1]);
    ui::text(dc, &d.always_text, RECT { left: check.right + sc(10), top: d.check_row.top, right: d.check_row.right, bottom: d.check_row.bottom }, p.text, DT_LEFT);

    // the buttons: Open in the accent, Cancel a plain surface
    let open_bg = if d.hover == Hit::Open { ui::mix(p.accent, p.text, 0.12) } else { p.accent };
    ui::fill(dc, d.open_btn, open_bg, sc(6));
    SelectObject(dc, d.fonts[1]);
    ui::text(dc, &d.open_text, d.open_btn, winrust_common::style::on_accent(), DT_CENTER);
    let cancel_bg = if d.hover == Hit::Cancel { ui::mix(p.bg, p.text, 0.16) } else { ui::mix(p.bg, p.text, 0.08) };
    ui::fill(dc, d.cancel_btn, cancel_bg, sc(6));
    ui::text(dc, &d.cancel_text, d.cancel_btn, p.text, DT_CENTER);
    SelectObject(dc, old);
    EndPaint(h, &ps);
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Dlg;
    if p.is_null() {
        return DefWindowProcW(h, msg, wp, lp);
    }
    let d = &mut *p;
    let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(h, d);
            0
        }
        WM_MOUSEMOVE => {
            if !d.tracking {
                d.tracking = true;
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                TrackMouseEvent(&mut t);
            }
            let hov = hit(d, x, y);
            if hov != d.hover {
                d.hover = hov;
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_MOUSELEAVE => {
            d.tracking = false;
            if d.hover != Hit::None {
                d.hover = Hit::None;
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            d.dbl = false; // a new gesture: any earlier double click is over
            0
        }
        WM_LBUTTONUP => {
            let dbl = d.dbl;
            d.dbl = false;
            match hit(d, x, y) {
                Hit::Open => {
                    d.chosen = Some(d.sel);
                    d.done = true;
                }
                Hit::Cancel => d.done = true,
                Hit::Check => {
                    d.always = !d.always;
                    InvalidateRect(h, ptr::null(), 0);
                }
                Hit::Row(i) => {
                    if dbl {
                        d.sel = i;
                        d.chosen = Some(i);
                        d.done = true;
                    } else if d.sel != i {
                        d.sel = i;
                        InvalidateRect(h, ptr::null(), 0);
                    }
                }
                Hit::None => {}
            }
            0
        }
        WM_LBUTTONDBLCLK => {
            if let Hit::Row(i) = hit(d, x, y) {
                d.sel = i;
                d.dbl = true; // opened by the button-up that ends this gesture
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            let k = wp as u16;
            match k {
                VK_ESCAPE => d.done = true,
                VK_RETURN => {
                    d.chosen = Some(d.sel);
                    d.done = true;
                }
                VK_SPACE => {
                    d.always = !d.always;
                    InvalidateRect(h, ptr::null(), 0);
                }
                VK_UP => {
                    d.sel = d.sel.saturating_sub(1);
                    InvalidateRect(h, ptr::null(), 0);
                }
                VK_DOWN => {
                    d.sel = (d.sel + 1).min(d.n - 1);
                    InvalidateRect(h, ptr::null(), 0);
                }
                _ => {}
            }
            0
        }
        WM_ACTIVATE => {
            // a click anywhere else dismisses it, like a menu (WA_INACTIVE = 0)
            if (wp as u32) & 0xffff == 0 {
                d.done = true;
            }
            0
        }
        WM_CLOSE => {
            d.done = true;
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn ensure_class() {
    let name = wc(CLASS);
    let mut wc: WNDCLASSW = zeroed();
    if GetClassInfoW(GetModuleHandleW(ptr::null()), name.as_ptr(), &mut wc) != 0 {
        return;
    }
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(proc),
        hInstance: GetModuleHandleW(ptr::null()),
        hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    RegisterClassW(&wc);
}

/// Shows the picker for `file`, centred on `owner`'s monitor, and returns what was picked.
unsafe fn dialog(owner: HWND, file: &Path, mut apps: Vec<App>) -> Pick {
    ensure_class();
    // centred on the monitor of the window that asked, inside its work area (never the primary's, on a second screen)
    let mon = winrust_common::monitors::of(owner);
    let area = if mon.work.right > mon.work.left && mon.work.bottom > mon.work.top { mon.work } else { mon.rc };
    // a short screen (or a big DPI scale) shows fewer rows: the buttons must stay reachable
    let fixed = sc(HEAD) + sc(48) + sc(56);
    let fit = (((area.bottom - area.top) - fixed) / sc(ROW_H)).max(1) as usize;
    apps.truncate(fit.saturating_sub(1)); // one of the rows is "search another app"
    let n = apps.len() + 1;
    let w = sc(WIN_W);
    let head = sc(HEAD);
    let rows = sc(ROW_H) * n as i32;
    let h = head + rows + sc(48) + sc(56);
    let check_row = RECT { left: sc(8), top: head + rows + sc(4), right: w - sc(8), bottom: head + rows + sc(44) };
    let (bh, bw) = (sc(32), sc(92));
    let by = h - sc(12) - bh;
    let open_btn = RECT { left: w - sc(14) - bw, top: by, right: w - sc(14), bottom: by + bh };
    let cancel_btn = RECT { left: w - sc(14) - 2 * bw - sc(10), top: by, right: w - sc(14) - bw - sc(10), bottom: by + bh };
    let icons: Vec<HICON> = apps.iter().map(|a| unsafe { winrust_common::icon::best(&a.exe, sc(ICON)) }).collect();
    let d = Box::new(Dlg {
        apps,
        icons,
        file_name: file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        title: t!("Выберите приложение", "Choose an application", ja: "アプリを選択", pt: "Escolher um aplicativo", zh: "选择应用").to_string(),
        search: t!("Поиск другого приложения…", "Search another app…", ja: "別のアプリを探す…", pt: "Procurar outro aplicativo…", zh: "查找其他应用…").to_string(),
        always_text: t!("Всегда использовать это приложение", "Always use this app", ja: "常にこのアプリを使用する", pt: "Usar sempre este aplicativo", zh: "始终使用此应用").to_string(),
        open_text: t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开").to_string(),
        cancel_text: t!("Отмена", "Cancel", ja: "キャンセル", pt: "Cancelar", zh: "取消").to_string(),
        n,
        w,
        head,
        sel: 0,
        hover: Hit::None,
        tracking: false,
        dbl: false,
        always: false,
        chosen: None,
        done: false,
        px: Vec::new(),
        fonts: [
            ui::font(14, 600, ui::ui_face()) as HFONT,
            ui::font(13, 400, ui::ui_face()) as HFONT,
            ui::font(11, 400, ui::ui_face()) as HFONT,
            ui::font(15, 400, winrust_common::style::icon_face()) as HFONT,
        ],
        check_row,
        open_btn,
        cancel_btn,
    });

    // the two formulas from the layout above: centred on the work area of the owner's monitor
    let x = area.left + ((area.right - area.left) - w) / 2;
    let y = area.top + ((area.bottom - area.top) - h) / 2;

    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        wc(CLASS).as_ptr(),
        wc(&d.title).as_ptr(),
        WS_POPUP,
        x,
        y,
        w,
        h,
        owner,
        ptr::null_mut(),
        GetModuleHandleW(ptr::null()),
        ptr::null(),
    );
    if hwnd.is_null() {
        return Pick::None; // the Box drops here: its fonts go with it
    }
    let raw = Box::into_raw(d);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
    // invisible until ShowWindow: what is on screen there is what is behind the dialog
    if winrust_common::look::menu_blur() {
        (*raw).px = winrust_common::frost::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + h }, winrust_common::look::menu_radius());
    }
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, sc(16), sc(16)), 1);

    OPEN.set(hwnd);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(hwnd); // the keyboard has to land here: the arrows, Enter and Escape are the point of the dialog
    InvalidateRect(hwnd, ptr::null(), 0);

    let mut msg: MSG = zeroed();
    loop {
        let r = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
        if r <= 0 {
            break;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if (*raw).done {
            break;
        }
    }
    OPEN.set(ptr::null_mut());
    // an explicit borrow (not through `(*raw).field.method()`, which the implicit-autoref lint rejects): the box is
    // still ours until Box::from_raw below
    let dlg: &Dlg = &*raw;
    let pick = match dlg.chosen {
        Some(i) if i < dlg.apps.len() => Pick::App(dlg.apps[i].exe.clone(), dlg.apps[i].progid.clone(), dlg.always),
        Some(_) => Pick::Search,
        None => Pick::None,
    };
    DestroyWindow(hwnd);
    drop(Box::from_raw(raw)); // the Drop impl deletes the fonts; the icons stay in the shared cache
    // the owner gets the keyboard back (the dialog had it)
    if !owner.is_null() && IsWindow(owner) != 0 {
        SetForegroundWindow(owner);
    }
    pick
}

/// Opens the picker for `file`: the chosen app, or the classic picker through "Search another app...".
pub unsafe fn open(owner: HWND, file: &Path) {
    if file.is_dir() {
        return;
    }
    let cur = OPEN.with(|c| c.get());
    if !cur.is_null() {
        // already up: bring it to the front rather than stacking a second one
        SetForegroundWindow(cur);
        return;
    }
    match dialog(owner, file, candidates(file)) {
        Pick::App(exe, progid, always) => {
            if always {
                remember(file, &exe, progid);
            }
            if !launch(&exe, file) {
                let msg = t!("Не удалось запустить приложение", "Could not start the application", ja: "アプリを起動できませんでした", pt: "Não foi possível iniciar o aplicativo", zh: "无法启动该应用");
                MessageBoxW(owner, wc(msg).as_ptr(), wc("WinRust Explorer").as_ptr(), MB_ICONWARNING);
            }
        }
        // the documented last resort: Windows' classic picker is a plain Win32 window and works without explorer
        Pick::Search => {
            let _ = std::process::Command::new("rundll32.exe").args(["shell32.dll,OpenAs_RunDLL", &file.to_string_lossy()]).spawn();
        }
        Pick::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::parse_exe;
    use std::path::PathBuf;

    #[test]
    fn reads_the_exe_out_of_a_command() {
        assert_eq!(parse_exe(r#""C:\Program Files\App\app.exe" "%1""#), Some(PathBuf::from(r"C:\Program Files\App\app.exe")));
        assert_eq!(parse_exe(r"C:\Windows\notepad.exe %1"), Some(PathBuf::from(r"C:\Windows\notepad.exe")));
        assert_eq!(parse_exe("notepad.exe"), Some(PathBuf::from("notepad.exe")));
        assert_eq!(parse_exe("   "), None);
    }
}
