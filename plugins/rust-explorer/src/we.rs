//! Wallpaper Engine behind the WinRust desktop, through WE's official command line.
//!
//! WE is not injected and does not need Explorer: it finds the shell window hierarchy and paints into it. This desktop
//! exposes that hierarchy (`Progman` titled "Program Manager", a `SHELLDLL_DefView`, the `WorkerW` that `0x052C` spawns)
//! and a wallpaper host window per monitor (`desk::hosts_sync`). What this module adds is the *bridge* of
//! `tools/wallpaper-engine.md` phase 1: ask WE to open its wallpaper in a window of ours
//!
//! ```text
//! wallpaper64.exe -control openWallpaper -file <project.json|scene.pkg|.mp4|index.html>
//!                 -playInWindow "WinRust Wallpaper <n>" -width <w> -height <h> -x <x> -y <y> -borderless
//! ```
//!
//! and reparent that window into the host of its monitor with `SetParent`, under the icon list. The CLI takes one window
//! per monitor (unique `-playInWindow` names), so `we-span=0` opens one per monitor; `we-span=1` opens a single window
//! over the whole virtual screen for scenes that should not be cut at the screen edge.
//!
//! Everything is behind `we=1` in `%ProgramData%\WinRust\look.txt`, off by default: without it the desktop paints the
//! picture it always painted and nothing here runs. When WE is missing, fails to start or its window never appears, the
//! hosts keep painting the picture and the reason goes to `%LOCALAPPDATA%\rs-shell\desktop.log`.
//!
//! Limits, from the analysis: pop-outs do not support playlists (WE's own words), and a pop-out that renders black when
//! reparented is a DirectComposition / `WS_EX_NOREDIRECTIONBITMAP` window, which GDI cannot composite. In that case do not
//! fight it: turn `we` off again; `tools/we-checklist.md` has the diagnosis and the recovery.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    cell::RefCell,
    env, fs,
    io::Write,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{Foundation::HWND, System::SystemInformation::GetTickCount64, UI::WindowsAndMessaging::*};

use super::w;

/// The window name WE gives each pop-out (`-playInWindow`); index n = monitor n of `monitors::all()`.
pub const TITLE: &str = "WinRust Wallpaper ";
/// Class of the per-monitor wallpaper host (`desk::host_proc`).
pub const HOST_CLASS: &str = "WinRustWallpaper";
const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW: `-control` must not flash a console
const WS_EX_NOREDIRECTIONBITMAP: u32 = 0x0020_0000;
const RETRY_MS: u64 = 6000; // a pop-out window should exist by then
const TRIES: u8 = 3;
const START_GRACE_MS: u64 = 15_000;
const REG_A: &str = r"Software\Valve\Steam";
const REG_B: &str = r"SOFTWARE\WOW6432Node\Valve\Steam";

/// A monitor host as `desk` keeps it: window and screen rectangle (x, y, w, h).
#[derive(Clone, Copy)]
pub struct Host {
    pub hwnd: HWND,
    pub screen: (i32, i32, i32, i32),
}

/// One requested WE window: `-playInWindow` name and the screen rectangle it is opened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Popout {
    pub title: String,
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// One pop-out per monitor (`span` = one window over the whole virtual screen). Pure: the monitor list is the same
/// `common::monitors::all()` order the bars use, so pop-out n always lands on the monitor of bar n.
pub fn plan(screens: &[(i32, i32, i32, i32)], span: bool) -> Vec<Popout> {
    if screens.is_empty() {
        return Vec::new();
    }
    if span {
        let x = screens.iter().map(|s| s.0).min().unwrap_or(0);
        let y = screens.iter().map(|s| s.1).min().unwrap_or(0);
        let r = screens.iter().map(|s| s.0 + s.2).max().unwrap_or(0);
        let b = screens.iter().map(|s| s.1 + s.3).max().unwrap_or(0);
        return vec![Popout { title: format!("{TITLE}0"), rect: Rect { x, y, w: r - x, h: b - y } }];
    }
    screens
        .iter()
        .enumerate()
        .map(|(i, s)| Popout { title: format!("{TITLE}{i}"), rect: Rect { x: s.0, y: s.1, w: s.2, h: s.3 } })
        .collect()
}

/// The documented command line for one pop-out. Screen coordinates: WE is told where to open the window before we
/// reparent it; `-borderless` keeps WE from drawing a caption that `SetParent` would not remove on its own.
pub fn args(file: &Path, p: &Popout) -> Vec<String> {
    let mut v: Vec<String> = ["-control", "openWallpaper", "-file", &file.to_string_lossy(), "-playInWindow", &p.title]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for (k, n) in [("-width", p.rect.w), ("-height", p.rect.h), ("-x", p.rect.x), ("-y", p.rect.y)] {
        v.push(k.to_string());
        v.push(n.to_string());
    }
    v.push("-borderless".to_string());
    v
}

/// `-control closeWallpaper -location "<window name>"` (WE's own way to close one pop-out).
fn close_args(title: &str) -> Vec<String> {
    ["-control", "closeWallpaper", "-location", title].iter().map(|s| s.to_string()).collect()
}

/// The Steam library folders in `steamapps/libraryfolders.vdf`, e.g. `"path"		"D:\\SteamLibrary"`. Escaped
/// backslashes (`D:\\SteamLibrary`) are unescaped. Pure, so the VDF shape is tested without Steam.
pub fn library_paths(vdf: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in vdf.lines() {
        let Some(rest) = line.trim().strip_prefix("\"path\"") else { continue };
        let Some(rest) = rest.trim().strip_prefix('"') else { continue };
        let Some(end) = rest.rfind('"') else { continue };
        let p = rest[..end].replace("\\\\", "\\");
        if !p.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&p)) {
            out.push(p);
        }
    }
    out
}

/// `-control getWallpaper` prints the active wallpaper path; take the last non-empty line (WE may prepend log lines),
/// unquoted. The caller checks that it exists.
pub fn parse_get_wallpaper(out: &str) -> Option<PathBuf> {
    out.lines()
        .map(str::trim)
        .map(|l| l.trim_matches('"').trim())
        .filter(|l| !l.is_empty())
        .next_back()
        .map(PathBuf::from)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A value of REG_SZ / REG_EXPAND_SZ under HKCU or HKLM (`root` = "HKCU"/"HKLM").
fn reg_value(root: &str, key: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
    let hkey = match root {
        "HKCU" => HKEY_CURRENT_USER,
        "HKLM" => HKEY_LOCAL_MACHINE,
        _ => return None,
    };
    let mut buf = vec![0u16; 2048];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(hkey, wide(key).as_ptr(), wide(name).as_ptr(), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
    };
    if r != 0 {
        return None;
    }
    buf.truncate((len as usize / 2).saturating_sub(1));
    let s = String::from_utf16_lossy(&buf);
    (!s.trim().is_empty()).then_some(s)
}

/// Steam's own folder: HKCU is what the installed client writes, the HKLM key is the fallback.
fn steam_roots() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = Vec::new();
    for (root, key, name) in [("HKCU", REG_A, "SteamPath"), ("HKLM", REG_B, "InstallPath")] {
        if let Some(p) = reg_value(root, key, name) {
            let p = PathBuf::from(p);
            if p.is_dir() && !v.iter().any(|q| q.to_string_lossy().eq_ignore_ascii_case(&p.to_string_lossy())) {
                v.push(p);
            }
        }
    }
    v
}

/// Where WE is installed: `WINRUST_WE_EXE` (handy for a test install), else `wallpaper_engine` in Steam and every Steam
/// library listed in `libraryfolders.vdf`. `None` = not installed; the desktop keeps the picture.
pub fn find_exe() -> Option<PathBuf> {
    if let Some(p) = env::var_os("WINRUST_WE_EXE").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    for steam in steam_roots() {
        let mut libs = vec![steam.clone()];
        if let Ok(vdf) = fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")) {
            libs.extend(library_paths(&vdf).into_iter().map(PathBuf::from));
        }
        for lib in libs {
            for name in ["wallpaper64.exe", "wallpaper32.exe"] {
                let p = lib.join(r"steamapps\common\wallpaper_engine").join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// True when a WE main process is running (its window must already be there for `-control` to answer).
pub fn running() -> bool {
    winrust_common::proc::list().iter().any(|(_, n)| n.eq_ignore_ascii_case("wallpaper64.exe") || n.eq_ignore_ascii_case("wallpaper32.exe"))
}

/// Appends a line to %LOCALAPPDATA%\rs-shell\desktop.log: a shell has no console, and a WE problem must be diagnosable
/// after the fact.
pub fn log(msg: &str) {
    let Ok(base) = env::var("LOCALAPPDATA") else { return };
    let dir = PathBuf::from(base).join("rs-shell");
    let _ = fs::create_dir_all(&dir);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("desktop.log")) {
        let _ = writeln!(f, "[{t}] we: {msg}");
    }
}

fn base_cmd(exe: &Path) -> Command {
    let mut c = Command::new(exe);
    c.stdin(Stdio::null()).creation_flags(NO_WINDOW);
    if let Some(d) = exe.parent() {
        c.current_dir(d);
    }
    c
}

fn spawn_open(exe: &Path, file: &Path, p: &Popout) -> std::io::Result<()> {
    base_cmd(exe).args(args(file, p)).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

fn spawn_close(exe: &Path, title: &str) {
    let _ = base_cmd(exe).args(close_args(title)).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// Asks WE for the wallpaper it already shows, off the UI thread (a broken install must not freeze the desktop).
fn ask_wallpaper(exe: &Path) -> Receiver<Option<PathBuf>> {
    let (tx, rx) = mpsc::channel();
    let exe = exe.to_path_buf();
    std::thread::spawn(move || {
        let out = base_cmd(&exe).args(["-control", "getWallpaper"]).stdout(Stdio::piped()).stderr(Stdio::null()).output();
        let p = out.ok().and_then(|o| parse_get_wallpaper(&String::from_utf8_lossy(&o.stdout)));
        let _ = tx.send(p.filter(|p| p.is_file() || p.is_dir()));
    });
    rx
}

struct Slot {
    pop: Popout,
    hwnd: HWND,
    asked_at: u64,
    tries: u8,
    attached: bool,
    gave_up: bool,
    warned: bool,
}

impl Slot {
    fn new(pop: Popout) -> Self {
        Slot { pop, hwnd: std::ptr::null_mut(), asked_at: 0, tries: 0, attached: false, gave_up: false, warned: false }
    }
}

#[derive(Default)]
struct State {
    on: bool,
    exe: Option<PathBuf>,
    exe_checked: u64,
    launched_at: u64,
    file: Option<PathBuf>,
    asking: Option<Receiver<Option<PathBuf>>>,
    ask_failed: bool,
    slots: Vec<Slot>,
    note: String,
}

thread_local! {
    static ST: RefCell<State> = RefCell::new(State::default());
}

fn now() -> u64 {
    unsafe { GetTickCount64() }
}

/// One log line per distinct situation, not one every 2 s.
fn note(st: &mut State, msg: &str) {
    if st.note != msg {
        st.note = msg.to_string();
        log(msg);
    }
}

/// Called from the desktop's 2 s timer. Takes the state out of the thread-local so the logic runs without a borrow
/// held across Win32 calls and puts it back even when it bails out early.
pub unsafe fn tick(pm: HWND, list: HWND, hosts: &[Host], native: bool) {
    let on = winrust_common::look::we();
    let mut st = ST.with_borrow_mut(std::mem::take);
    if !on {
        if st.on || !st.slots.is_empty() {
            close_all(&mut st, "off: pop-outs closed, picture back");
        }
    } else if native {
        // WE found the Progman/WorkerW hierarchy and attached its own desktop window: that is the better route (playlists,
        // profiles, pause rules), so do not put pop-outs on top of it.
        if st.on || !st.slots.is_empty() {
            close_all(&mut st, "wallpaper engine attached itself to WorkerW: pop-outs closed, using its desktop wallpaper");
        }
    } else {
        session(&mut st, pm, list, hosts);
    }
    ST.with_borrow_mut(|s| *s = st);
}

/// `we` on: make sure WE runs and has a pop-out window in every host (or one over the virtual screen). `we` off,
/// WE gone or the wallpaper failing: close what we opened and leave the picture in the hosts.
unsafe fn session(st: &mut State, pm: HWND, list: HWND, hosts: &[Host]) {
    let t = now();
    if !st.on {
        st.on = true;
        st.exe = None;
        st.file = None;
        st.asking = None;
        st.ask_failed = false;
        log(&format!("on: {} host(s), span={}", hosts.len(), winrust_common::look::we_span()));
    }

    // 1. where is WE
    if st.exe.is_none() && t.saturating_sub(st.exe_checked) > 30_000 {
        st.exe = find_exe();
        st.exe_checked = t;
        match &st.exe {
            Some(p) => log(&format!("found {}", p.display())),
            None => log("not found: no wallpaper_engine under the Steam libraries (and no WINRUST_WE_EXE)"),
        }
    }
    let Some(exe) = st.exe.clone() else {
        note(st, "wallpaper engine is not installed: the desktop keeps the picture");
        return;
    };

    if !running() {
        if st.launched_at == 0 {
            st.launched_at = t;
            match Command::new(&exe).current_dir(exe.parent().unwrap_or(Path::new("."))).creation_flags(NO_WINDOW).spawn() {
                Ok(_) => log("started the app; waiting for it to come up"),
                Err(e) => log(&format!("could not start {}: {e}", exe.display())),
            }
            note(st, "waiting for wallpaper engine to start");
        } else if t.saturating_sub(st.launched_at) < START_GRACE_MS {
            note(st, "waiting for wallpaper engine to start");
        } else {
            note(st, "wallpaper engine is not running (start failed?): keeping the picture");
        }
        return; // never send -control to a process that is not there: it could start a second app
    }
    st.launched_at = 0;

    // 2. which wallpaper
    if st.file.is_none() {
        let key = winrust_common::look::we_file();
        if !key.trim().is_empty() {
            st.file = Some(PathBuf::from(key));
        }
    }
    if st.file.is_none() && st.asking.is_none() && !st.ask_failed {
        log("no we-file: asking WE for the wallpaper it already shows (-control getWallpaper)");
        st.asking = Some(ask_wallpaper(&exe));
    }
    if let Some(rx) = st.asking.take() {
        match rx.try_recv() {
            Ok(Some(p)) => {
                log(&format!("getWallpaper: {}", p.display()));
                st.file = Some(p);
            }
            Ok(None) | Err(TryRecvError::Disconnected) => {
                st.ask_failed = true;
                log("getWallpaper returned nothing; set we-file=<project.json|scene.pkg|.mp4|index.html> in look.txt");
            }
            Err(TryRecvError::Empty) => st.asking = Some(rx),
        }
    }
    let Some(file) = st.file.clone() else {
        note(st, "no wallpaper to open: keeping the picture");
        return;
    };
    if !file.exists() {
        note(st, "we-file does not exist: keeping the picture");
        return;
    }
    st.note.clear(); // we are live: a later failure must be logged again

    // 3. one window per monitor (or one over all): the plan follows the current monitor list
    let screens: Vec<(i32, i32, i32, i32)> = hosts.iter().map(|h| h.screen).collect();
    let span = winrust_common::look::we_span();
    let wanted = plan(&screens, span);
    if st.slots.len() != wanted.len() || st.slots.iter().zip(&wanted).any(|(s, w)| s.pop.title != w.title) {
        let mut old = std::mem::take(&mut st.slots);
        let mut kept: Vec<Slot> = Vec::with_capacity(wanted.len());
        for w in &wanted {
            match old.iter().position(|s| s.pop.title == w.title) {
                Some(i) => {
                    let mut s = old.remove(i);
                    s.pop.rect = w.rect;
                    kept.push(s);
                }
                None => kept.push(Slot::new(w.clone())),
            }
        }
        for s in old {
            spawn_close(&exe, &s.pop.title);
            if !s.hwnd.is_null() && IsWindow(s.hwnd) != 0 {
                PostMessageW(s.hwnd, WM_CLOSE, 0, 0);
            }
        }
        st.slots = kept;
    } else {
        for (s, w) in st.slots.iter_mut().zip(&wanted) {
            s.pop.rect = w.rect;
        }
    }

    let (vx, vy, ..) = super::virt();
    for i in 0..st.slots.len() {
        // the borrow of st is re-taken per slot: `spawn_open`/`attach` must not run while a &mut Slot is held
        {
            let s = &mut st.slots[i];
            if !s.hwnd.is_null() && IsWindow(s.hwnd) == 0 {
                log(&format!("pop-out {} disappeared", s.pop.title));
                s.hwnd = std::ptr::null_mut();
                s.attached = false;
                s.asked_at = 0;
                s.tries += 1;
                if s.tries >= TRIES {
                    s.gave_up = true;
                    log(&format!("{} keeps closing: that monitor keeps the picture", s.pop.title));
                }
            }
        }
        let (want_open, pop) = {
            let s = &st.slots[i];
            (s.hwnd.is_null() && !s.gave_up, s.pop.clone())
        };
        if want_open {
            let asked_at = st.slots[i].asked_at;
            if asked_at == 0 {
                match spawn_open(&exe, &file, &pop) {
                    Ok(()) => {
                        log(&format!("opening {} on {}x{} at {},{}{}", file.display(), pop.rect.w, pop.rect.h, pop.rect.x, pop.rect.y, if span { " (span)" } else { "" }));
                        st.slots[i].asked_at = t;
                    }
                    Err(e) => {
                        log(&format!("could not run {} -control openWallpaper: {e}", exe.display()));
                        st.slots[i].gave_up = true;
                    }
                }
            } else {
                let title = wide(&pop.title);
                let h = FindWindowW(std::ptr::null(), title.as_ptr());
                if !h.is_null() && IsWindow(h) != 0 {
                    log(&format!("found {}", pop.title));
                    st.slots[i].hwnd = h;
                    st.slots[i].asked_at = 0;
                } else if t.saturating_sub(asked_at) > RETRY_MS {
                    st.slots[i].asked_at = 0;
                    st.slots[i].tries += 1;
                    if st.slots[i].tries >= TRIES {
                        st.slots[i].gave_up = true;
                        log(&format!("{} never appeared after {} tries: that monitor keeps the picture", pop.title, TRIES));
                    }
                }
            }
        }
        let (hwnd, attached) = (st.slots[i].hwnd, st.slots[i].attached);
        if hwnd.is_null() {
            continue;
        }
        let host = if span { None } else { hosts.get(i).map(|h| h.hwnd) };
        let expected = host.unwrap_or(pm);
        if !attached || GetParent(hwnd) != expected {
            let s = &mut st.slots[i];
            attach(s, pm, list, host, (vx, vy));
            s.attached = true;
        } else if host.is_some() {
            let s = &st.slots[i];
            SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, s.pop.rect.w, s.pop.rect.h, SWP_NOACTIVATE | SWP_NOZORDER);
        }
    }
}

/// `SetParent` of a WE pop-out into a host (or into Progman for a spanning window, directly under the icon list).
/// `virt` is the virtual screen's top-left: Progman's client origin (the desktop window is created there).
unsafe fn attach(s: &mut Slot, pm: HWND, list: HWND, host: Option<HWND>, virt: (i32, i32)) {
    let hwnd = s.hwnd;
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let style = (style & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX)) | WS_CHILD;
    SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ((ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW) as isize);
    if ex & WS_EX_NOREDIRECTIONBITMAP != 0 {
        log(&format!("{} has WS_EX_NOREDIRECTIONBITMAP: if it shows black, see tools/we-checklist.md (DComp pop-out)", s.pop.title));
    }
    let parent = host.unwrap_or(pm);
    SetParent(hwnd, parent);
    if GetParent(hwnd) != parent {
        if !s.warned {
            s.warned = true;
            log(&format!("SetParent({}) was refused: WE and the shell must run at the same integrity level (not elevated)", s.pop.title));
        }
    } else {
        s.warned = false;
    }
    if host.is_some() {
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, s.pop.rect.w, s.pop.rect.h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED | SWP_SHOWWINDOW);
    } else {
        // spanning window: in Progman client coordinates, inserted directly below the icon list
        SetWindowPos(hwnd, list, s.pop.rect.x - virt.0, s.pop.rect.y - virt.1, s.pop.rect.w, s.pop.rect.h, SWP_NOACTIVATE | SWP_FRAMECHANGED | SWP_SHOWWINDOW);
    }
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
}

/// Close every pop-out we opened: WE's own `closeWallpaper` (it stops the renderer and removes the window) plus a
/// `WM_CLOSE` for a window that does not answer the CLI. The WE process itself is left alone: the user may use it.
fn close_all(st: &mut State, why: &str) {
    let exe = st.exe.clone().or_else(find_exe);
    for s in &st.slots {
        if let Some(exe) = &exe {
            spawn_close(exe, &s.pop.title);
        }
        unsafe {
            if !s.hwnd.is_null() && IsWindow(s.hwnd) != 0 {
                PostMessageW(s.hwnd, WM_CLOSE, 0, 0);
            }
        }
    }
    *st = State::default();
    log(why);
}

/// Logoff / desktop shutdown: do not leave WE windows orphaned in a host that is going away.
pub fn shutdown() {
    let mut st = ST.with_borrow_mut(std::mem::take);
    if st.on || !st.slots.is_empty() {
        close_all(&mut st, "desktop closing: pop-outs closed");
    }
    ST.with_borrow_mut(|s| *s = st);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screens() -> Vec<(i32, i32, i32, i32)> {
        // the left monitor at a negative coordinate, then a taller primary: the order monitors::all() returns
        vec![(-1920, 0, 1920, 1080), (0, 0, 2560, 1440)]
    }

    #[test]
    fn one_popout_per_monitor_in_monitor_order() {
        let p = plan(&screens(), false);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].title, "WinRust Wallpaper 0");
        assert_eq!(p[0].rect, Rect { x: -1920, y: 0, w: 1920, h: 1080 });
        assert_eq!(p[1].title, "WinRust Wallpaper 1");
        assert_eq!(p[1].rect, Rect { x: 0, y: 0, w: 2560, h: 1440 });
    }

    #[test]
    fn a_spanning_popout_covers_the_virtual_screen() {
        let p = plan(&screens(), true);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].title, "WinRust Wallpaper 0");
        assert_eq!(p[0].rect, Rect { x: -1920, y: 0, w: 4480, h: 1440 });
        assert!(plan(&[], false).is_empty() && plan(&[], true).is_empty());
    }

    #[test]
    fn the_command_line_is_the_documented_one() {
        let p = plan(&screens(), false).remove(0);
        let a = args(Path::new(r"C:\WE\project.json"), &p);
        assert_eq!(
            a,
            [
                "-control", "openWallpaper", "-file", r"C:\WE\project.json", "-playInWindow", "WinRust Wallpaper 0",
                "-width", "1920", "-height", "1080", "-x", "-1920", "-y", "0", "-borderless",
            ]
        );
        assert_eq!(close_args("WinRust Wallpaper 1"), ["-control", "closeWallpaper", "-location", "WinRust Wallpaper 1"]);
    }

    #[test]
    fn steam_libraries_come_out_of_the_vdf() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(library_paths(vdf), [r"C:\Program Files (x86)\Steam", r"D:\SteamLibrary"]);
        assert!(library_paths("").is_empty());
        assert!(library_paths("{\"path\" \"\"}").is_empty());
    }

    #[test]
    fn get_wallpaper_takes_the_last_non_empty_line() {
        assert_eq!(parse_get_wallpaper("\r\nC:\\w\\scene.pkg\r\n\r\n"), Some(PathBuf::from(r"C:\w\scene.pkg")));
        assert_eq!(parse_get_wallpaper("\"D:\\wall papers\\a.mp4\""), Some(PathBuf::from(r"D:\wall papers\a.mp4")));
        assert_eq!(parse_get_wallpaper("  \r\n"), None);
    }
}
