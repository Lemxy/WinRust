// rs-metrics: a small desktop metrics widget (CPU, GPU, RAM, disks, disk I/O, network, temperatures), in the WinRust
// palette. Borderless, always on top, per-pixel alpha through UpdateLayeredWindow, never activated.
//
//   - one sampler thread every second does the cheap kernel calls (CPU, RAM, disks, network), PDH for disk I/O and GPU;
//   - a second, lowest-priority thread reads the ACPI thermal zone every 3 s and only while the module is on;
//   - this thread paints what the sampler left behind, once a second, and handles the mouse and the menu.
//
// Config: %ProgramData%\WinRust\metrics.txt (key=value, re-read once a second). Log: %LOCALAPPDATA%\rs-shell\metrics.log.
#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
#[macro_use]
extern crate winrust_common;

mod canvas;
mod config;
mod menu;
mod metrics;
mod render;

use std::{
    cell::RefCell,
    io::Write,
    path::PathBuf,
    process::Command,
    ptr::null_mut,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering::Relaxed},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::{BeginPaint, EndPaint, GetMonitorInfoW, MapWindowPoints, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT},
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{CreateMutexW, GetCurrentThreadId},
    },
    UI::{
        HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::{ReleaseCapture, SetCapture},
        WindowsAndMessaging::*,
    },
};

use canvas::{Canvas, Fonts};
use config::Config;
use metrics::Sample;
use render::Series;
use winrust_common::style;

const CLASS: &str = "WinRustMetrics";
const TITLE: &str = "rs-metrics";

static CFG: OnceLock<Arc<Mutex<Config>>> = OnceLock::new();
static SAMPLE: OnceLock<Arc<Mutex<Option<Sample>>>> = OnceLock::new();
static STOP: AtomicBool = AtomicBool::new(false);
static QUITTING: AtomicBool = AtomicBool::new(false);
/// The live widget window (0 when it was destroyed with the desktop: the watchdog rebuilds it).
static WIDGET: AtomicIsize = AtomicIsize::new(0);
static UI_TID: AtomicU32 = AtomicU32::new(0);
const WM_APP_MENU: u32 = WM_APP + 2;
const WM_APP_REBUILD: u32 = WM_APP + 3;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Dark scrollbars and menus for the whole process, like the other WinRust windows (uxtheme's unnamed exports).
fn dark_menus() {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    unsafe {
        let m = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if m.is_null() {
            return;
        }
        if let Some(f) = GetProcAddress(m, 135usize as *const u8) {
            std::mem::transmute::<_, extern "system" fn(i32) -> i32>(f)(2); // SetPreferredAppMode(ForceDark)
        }
        if let Some(f) = GetProcAddress(m, 136usize as *const u8) {
            std::mem::transmute::<_, extern "system" fn()>(f)(); // FlushMenuThemes
        }
    }
}

fn log(msg: &str) {
    static LOCK: Mutex<()> = Mutex::new(());
    let Ok(base) = std::env::var("LOCALAPPDATA") else { return };
    let dir = PathBuf::from(base).join("rs-shell");
    let _ = std::fs::create_dir_all(&dir);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(_guard) = LOCK.lock() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("metrics.log")) {
            let _ = writeln!(f, "[{t}] {msg}");
        }
    }
}

fn cfg() -> Arc<Mutex<Config>> {
    CFG.get().expect("config").clone()
}

fn sample_slot() -> Arc<Mutex<Option<Sample>>> {
    SAMPLE.get().expect("sample").clone()
}

// ---------- UI state ----------

struct Ui {
    cv: Option<Canvas>,
    fonts: Option<Fonts>,
    series: Series,
    scale: f32,
    face: String,
    bg: Option<Vec<u32>>,
    bg_key: Option<(i32, i32, u8, u32, u32, i32, i32)>,
    last_seq: u64,
    /// Screen cursor and window origin when the drag started.
    drag: Option<(i32, i32, i32, i32)>,
    /// The card's last size, for WM_NCHITTEST.
    size: (i32, i32),
    shadow: i32,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui {
        cv: None, fonts: None, series: Series::default(), scale: 1.0, face: String::new(),
        bg: None, bg_key: None, last_seq: 0, drag: None, size: (0, 0), shadow: 0,
    });
}

fn scale_of(hwnd: HWND) -> f32 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 { 1.0 } else { (d as f32 / 96.0).max(0.5) }
}

fn snapshot_cfg() -> Config {
    cfg().lock().map(|c| c.clone()).unwrap_or_else(|e| e.into_inner().clone())
}

fn snapshot_sample() -> Sample {
    sample_slot().lock().map(|s| s.clone().unwrap_or_default()).unwrap_or_else(|e| e.into_inner().clone().unwrap_or_default())
}

// ---------- the desktop ----------

/// The rust-explorer desktop ("Progman" / "Program Manager", marked with WinRustDesktop), or Explorer's own Progman.
fn desktop_hwnd() -> HWND {
    let d = winrust_common::desktop::window();
    if !d.is_null() {
        return d;
    }
    unsafe { FindWindowW(wide("Progman").as_ptr(), wide("Program Manager").as_ptr()) }
}

/// Screen point -> the parent's client point (a cross-process child is placed in its parent's coordinates).
fn screen_to_parent(parent: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut p = POINT { x, y };
    unsafe { MapWindowPoints(null_mut(), parent, &mut p, 1) };
    (p.x, p.y)
}

/// Where UpdateLayeredWindow has to paint: the window's origin in the parent's client area when it is a child.
fn paint_origin(hwnd: HWND) -> (i32, i32) {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        let (mut x, mut y) = (r.left, r.top);
        let parent = GetParent(hwnd);
        if !parent.is_null() {
            let mut p = POINT { x, y };
            MapWindowPoints(null_mut(), parent, &mut p, 1);
            x = p.x;
            y = p.y;
        }
        (x, y)
    }
}

/// Moves the window to a *screen* position (the config and the mouse always speak screen coordinates).
unsafe fn move_screen(hwnd: HWND, x: i32, y: i32) {
    let parent = GetParent(hwnd);
    let (px, py) = if parent.is_null() { (x, y) } else { screen_to_parent(parent, x, y) };
    SetWindowPos(hwnd, null_mut(), px, py, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
}

/// Makes the window a child of the desktop (`topmost=0`: below every application window, over the wallpaper) or an
/// always-on-top popup (`topmost=1`), then puts it back at its saved screen position.
unsafe fn place(hwnd: HWND, cfg: &Config) {
    let parent = GetParent(hwnd);
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    let desk = if cfg.topmost { null_mut() } else { desktop_hwnd() };
    if !desk.is_null() {
        if parent != desk {
            SetWindowLongPtrW(hwnd, GWL_STYLE, ((style & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME)) | WS_CHILD) as isize);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_TOPMOST) as isize);
            SetParent(hwnd, desk);
            log(&format!("desktop parent {desk:p}"));
        }
        // just below the desktop's icon list: the icons (and their labels) always paint above the card, while the
        // widget stays above the wallpaper/host windows behind it
        let (px, py) = screen_to_parent(desk, cfg.x, cfg.y);
        let list = FindWindowExW(desk, null_mut(), wide("SysListView32").as_ptr(), null_mut());
        if !list.is_null() {
            SetWindowPos(hwnd, list, px, py, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        } else {
            SetWindowPos(hwnd, HWND_TOP, px, py, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        return;
    }
    if !parent.is_null() {
        SetParent(hwnd, null_mut());
    }
    let style = (style & !WS_CHILD) | WS_POPUP;
    if cfg.topmost {
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex | WS_EX_TOPMOST) as isize);
        SetWindowPos(hwnd, HWND_TOPMOST, cfg.x, cfg.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
    } else {
        // no desktop window to sit on yet: an ordinary window at the bottom of the z-order, re-parented once it appears
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_TOPMOST) as isize);
        SetWindowPos(hwnd, HWND_BOTTOM, cfg.x, cfg.y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

/// Creates the widget window (also after the desktop restarted and took the child with it).
unsafe fn build_widget(c: &Config) -> HWND {
    let inst = GetModuleHandleW(null_mut());
    let class = wide(CLASS);
    let plan = render::build(c, &metrics::quick(c), 1.0);
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | if c.topmost { WS_EX_TOPMOST } else { 0 };
    let hwnd = CreateWindowExW(
        ex,
        class.as_ptr(),
        wide(TITLE).as_ptr(),
        WS_POPUP,
        c.x,
        c.y,
        plan.w,
        plan.h,
        null_mut(),
        null_mut(),
        inst,
        null_mut(),
    );
    if hwnd.is_null() {
        return hwnd;
    }
    WIDGET.store(hwnd as isize, Relaxed);
    place(hwnd, c);
    // enabled=0 in metrics.txt (the Widgets page's on/off) keeps the window built but hidden
    ShowWindow(hwnd, if c.enabled { SW_SHOWNOACTIVATE } else { SW_HIDE });
    SetTimer(hwnd, 1, 1000, None);
    paint(hwnd);
    hwnd
}

// ---------- painting ----------

fn paint(hwnd: HWND) {
    let cfg = snapshot_cfg();
    if !cfg.enabled {
        return; // off: nothing to draw (the window is hidden; the sampler keeps the last values warm)
    }
    let smp = snapshot_sample();
    let scale = scale_of(hwnd);
    let plan = render::build(&cfg, &smp, scale);
    // the window is exactly the card: grow/shrink without moving or activating it
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        if r.right - r.left != plan.w || r.bottom - r.top != plan.h {
            SetWindowPos(hwnd, null_mut(), 0, 0, plan.w, plan.h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
    let (px, py) = paint_origin(hwnd);
    UI.with(|u| {
        let mut u = u.borrow_mut();
        u.size = (plan.w, plan.h);
        u.shadow = plan.shadow;
        let face = style::ui_face();
        if u.fonts.is_none() || (u.face != face) || (u.scale - scale).abs() > 0.01 {
            u.fonts = Some(Fonts::new(face, scale));
            u.face = face.to_string();
            u.scale = scale;
            u.bg = None; // the rounded corners are scaled too
        }
        let sized = matches!(&u.cv, Some(c) if c.w == plan.w && c.h == plan.h);
        if !sized {
            u.cv = Canvas::new(plan.w, plan.h);
            u.bg = None;
        }
        let mut cv = match u.cv.take() {
            Some(c) => c,
            None => return,
        };
        let p = style::pal();
        let key = (plan.w, plan.h, cfg.opacity, p.bg, p.line, plan.radius as i32, plan.shadow);
        if u.bg_key != Some(key) {
            render::draw_bg(&mut cv, &plan, &cfg);
            u.bg = Some(cv.snapshot());
            u.bg_key = Some(key);
        } else if let Some(bg) = u.bg.as_ref() {
            cv.restore(bg);
        }
        // the rolling history advances once per finished sample, not once per repaint
        let seq = smp.seq;
        if seq != u.last_seq {
            u.series.push(&smp);
            u.last_seq = seq;
        }
        if let Some(fonts) = u.fonts.as_ref() {
            render::draw_content(&mut cv, &plan, &cfg, &smp, &u.series, fonts);
        }
        cv.present(hwnd, (px, py));
        u.cv = Some(cv);
    });
}

// ---------- the sampler threads ----------

fn spawn_sampler() {
    let cfg = cfg();
    let out = sample_slot();
    std::thread::spawn(move || {
        let mut s = metrics::Sampler::default();
        while !STOP.load(Relaxed) {
            let started = Instant::now();
            let c = cfg.lock().map(|c| c.clone()).unwrap_or_else(|e| e.into_inner().clone());
            let smp = s.collect(&c);
            if let Ok(mut slot) = out.lock() {
                *slot = Some(smp);
            }
            let spent = started.elapsed();
            std::thread::sleep(Duration::from_millis(1000).saturating_sub(spent));
        }
    });
}

// ---------- config reload ----------

fn stamp() -> Option<(u64, u64)> {
    let f = config::file()?;
    let m = std::fs::metadata(f).ok()?;
    let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
    Some((t, m.len()))
}

/// Applies a changed file. Returns true when something was re-read.
fn reload_config(hwnd: HWND, last: &mut Option<(u64, u64)>) -> bool {
    let now = stamp();
    if now.is_none() || now == *last {
        return false;
    }
    *last = now;
    let fresh = config::read();
    let (x, y) = (fresh.x, fresh.y);
    let (was_on, on) = (snapshot_cfg().enabled, fresh.enabled);
    let want_child = !fresh.topmost;
    let is_child = unsafe { !GetParent(hwnd).is_null() };
    let drag = UI.with(|u| u.borrow().drag.is_some());
    {
        let c = cfg();
        let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
        *c = fresh;
    }
    unsafe {
        if on != was_on {
            ShowWindow(hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
            if on {
                paint(hwnd); // show the current values at once
            }
        } else if want_child != is_child {
            place(hwnd, &snapshot_cfg());
        } else if !drag {
            move_screen(hwnd, x, y);
        }
    }
    log("config reloaded");
    true
}

// ---------- mouse ----------

fn snap_and_save(hwnd: HWND) {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let (mut x, mut y) = (r.left, r.top);
        if mon != null_mut() && GetMonitorInfoW(mon, &mut mi) != 0 {
            let (l, t, rr, b) = (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right - (r.right - r.left), mi.rcWork.bottom - (r.bottom - r.top));
            let s = (12.0 * scale_of(hwnd)) as i32;
            if (x - l).abs() < s {
                x = l;
            }
            if (y - t).abs() < s {
                y = t;
            }
            if (x - rr).abs() < s {
                x = rr;
            }
            if (y - b).abs() < s {
                y = b;
            }
            x = x.clamp(l, rr.max(l));
            y = y.clamp(t, b.max(t));
        }
        if x != r.left || y != r.top {
            move_screen(hwnd, x, y);
        }
        let c = cfg();
        let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
        if c.x != x || c.y != y {
            c.x = x;
            c.y = y;
            c.save();
        }
    }
}

// ---------- the menu ----------

fn find_exe(name: &str, folder: &str) -> Option<PathBuf> {
    let own = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join(name)));
    if let Some(p) = own.filter(|p| p.is_file()) {
        return Some(p);
    }
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    let p = PathBuf::from(pf).join(folder).join(name);
    p.is_file().then_some(p)
}

fn open_config() {
    let Some(path) = config::file() else { return };
    if !path.exists() {
        config::read(); // writes the defaults
    }
    log(&format!("open config {}", path.display()));
    let r = match find_exe("rust-notepad.exe", "rust-notepad") {
        Some(rn) => Command::new(rn).arg(&path).spawn(),
        None => Command::new("notepad.exe").arg(&path).spawn(),
    };
    if let Err(e) = r {
        log(&format!("could not open the config: {e}"));
    }
}

fn open_settings() {
    match find_exe("rust-settings.exe", "rust-settings") {
        Some(p) => {
            if let Err(e) = Command::new(p).spawn() {
                log(&format!("rust-settings: {e}"));
            }
        }
        None => log("rust-settings.exe not found"),
    }
}

fn show_menu(hwnd: HWND) {
    let (mut x, mut y): (i32, i32) = (0, 0);
    unsafe {
        let mut p: POINT = std::mem::zeroed();
        if GetCursorPos(&mut p) != 0 {
            x = p.x;
            y = p.y;
        }
    }
    let id = unsafe { menu::show(x, y, &snapshot_cfg(), scale_of(hwnd)) };
    if id == 0 {
        return;
    }
    log(&format!("menu -> {id}"));
    let mut quit = false;
    {
        let a = cfg();
        let mut c = a.lock().unwrap_or_else(|e| e.into_inner());
        if (menu::ID_TOGGLE..menu::ID_TOGGLE + 8).contains(&id) {
            let i = (id - menu::ID_TOGGLE) as usize;
            let next = if c.on[i] { "0" } else { "1" };
            c.set(config::MODS[i], next);
            log(&format!("module {} -> {}", config::MODS[i], c.on[i]));
        } else if (menu::ID_STYLE..menu::ID_STYLE + 64).contains(&id) {
            let n = id - menu::ID_STYLE;
            let (i, k) = ((n / 8) as usize, (n % 8) as usize);
            if let Some(st) = config::Style::all().get(k) {
                c.set(&format!("style-{}", config::MODS[i]), st.key());
                log(&format!("style {} -> {}", config::MODS[i], st.key()));
            }
        } else {
            match id {
                menu::ID_RELOAD => {
                    let fresh = config::read();
                    *c = fresh;
                    log("menu: reload");
                }
                menu::ID_OPEN => open_config(),
                menu::ID_SETTINGS => open_settings(),
                menu::ID_COLUMNS => {
                    let next = if c.cols == 2 { "1" } else { "2" };
                    c.set("cols", next);
                }
                menu::ID_RESET => {
                    c.set("x", "60");
                    c.set("y", "60");
                    unsafe { SetWindowPos(hwnd, null_mut(), 60, 60, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE) };
                }
                menu::ID_CLOSE => quit = true,
                _ => {}
            }
        }
    }
    if quit {
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        return;
    }
    paint(hwnd);
}

// ---------- window ----------

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            0
        }
        WM_NCHITTEST => {
            // the shadow margin is decoration: clicks there reach whatever is behind the widget
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut r);
            let (cx, cy) = (x - r.left, y - r.top);
            let (w, h, sh) = UI.with(|u| {
                let u = u.borrow();
                (u.size.0, u.size.1, u.shadow)
            });
            if w > 0 && (cx < sh - 1 || cy < sh - 1 || cx >= w - sh + 1 || cy >= h - sh + 1) {
                return HTTRANSPARENT as LRESULT;
            }
            HTCLIENT as LRESULT
        }
        WM_LBUTTONDOWN => {
            let mut p: POINT = std::mem::zeroed();
            let mut r: RECT = std::mem::zeroed();
            GetCursorPos(&mut p);
            GetWindowRect(hwnd, &mut r);
            UI.with(|u| u.borrow_mut().drag = Some((p.x, p.y, r.left, r.top)));
            SetCapture(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let drag = UI.with(|u| u.borrow().drag);
            if let Some((ax, ay, wx, wy)) = drag {
                let mut p: POINT = std::mem::zeroed();
                GetCursorPos(&mut p);
                move_screen(hwnd, wx + p.x - ax, wy + p.y - ay);
            }
            0
        }
        WM_LBUTTONUP => {
            let dragging = UI.with(|u| u.borrow_mut().drag.take().is_some());
            if dragging {
                ReleaseCapture();
                snap_and_save(hwnd);
            }
            0
        }
        WM_CAPTURECHANGED => {
            UI.with(|u| u.borrow_mut().drag = None);
            0
        }
        WM_RBUTTONUP | WM_CONTEXTMENU => {
            // the menu opens from the message queue, after the button messages of this click are done: opened right
            // inside WM_RBUTTONUP, TrackPopupMenu closes on the same gesture
            PostMessageW(hwnd, WM_APP_MENU, 0, 0);
            0
        }
        WM_APP_MENU => {
            show_menu(hwnd);
            0
        }
        WM_TIMER => {
            // the message loop in main handles the widget's own timer; menu/flyout timers land here
            0
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            paint(hwnd);
            0
        }
        WM_DEVICECHANGE => {
            // a drive was plugged in or unplugged: the sampler re-reads its letters on the next tick
            metrics::REFRESH_DISKS.store(true, Relaxed);
            0
        }
        WM_CLOSE => {
            QUITTING.store(true, Relaxed);
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            // the desktop went away and took its child with it: the watchdog rebuilds us (an exit here would leave
            // nothing on screen until the next sign-in)
            WIDGET.store(0, Relaxed);
            if QUITTING.load(Relaxed) {
                STOP.store(true, Relaxed);
                log("exit");
                PostQuitMessage(0);
            } else {
                log("the desktop destroyed the widget; waiting for it to come back");
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn main() {
    winrust_common::appid::set("WinRust");
    let _ = lang!();
    unsafe { SetProcessDpiAwarenessContext(-4isize as _) }; // per-monitor v2
    dark_menus();
    log("start");

    // one widget per session
    let name = wide(r"Local\WinRustRsMetrics");
    unsafe {
        CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            log("another rs-metrics is already running");
            return;
        }
    }

    let first = config::read();
    let _ = CFG.set(Arc::new(Mutex::new(first.clone())));
    let _ = SAMPLE.set(Arc::new(Mutex::new(Some(metrics::quick(&first)))));
    spawn_sampler();
    std::thread::spawn(move || metrics::temp_loop(cfg()));

    unsafe {
        let inst = GetModuleHandleW(null_mut());
        let class = wide(CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(proc),
            hInstance: inst,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            log("RegisterClassW failed");
            return;
        }
        UI_TID.store(GetCurrentThreadId(), Relaxed);
        let hwnd = build_widget(&first);
        if hwnd.is_null() {
            log("the widget window could not be created");
            return;
        }
        // the desktop can be restarted (rust-explorer is respawned): the child window dies with it, so a watcher asks
        // this thread to build a new one instead of leaving the process alive with no window
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_secs(3));
            let h = WIDGET.load(Relaxed) as HWND;
            if h.is_null() || IsWindow(h) == 0 {
                PostThreadMessageW(UI_TID.load(Relaxed), WM_APP_REBUILD, 0, 0);
            }
        });

        // the one place the file is polled: cheap stat once a second, and a repaint with the new numbers
        let mut last = stamp();
        let mut msg: MSG = std::mem::zeroed();
        let mut tick = 0u32;
        loop {
            let r = GetMessageW(&mut msg, null_mut(), 0, 0);
            if r <= 0 {
                break;
            }
            if msg.hwnd.is_null() && msg.message == WM_APP_REBUILD {
                let h = WIDGET.load(Relaxed) as HWND;
                if h.is_null() || IsWindow(h) == 0 {
                    log("rebuilding the widget window");
                    let c = snapshot_cfg();
                    if build_widget(&c).is_null() {
                        log("the widget could not be rebuilt yet");
                    }
                }
                continue;
            }
            if msg.message == WM_TIMER && msg.hwnd == WIDGET.load(Relaxed) as HWND && !msg.hwnd.is_null() {
                let h = msg.hwnd;
                reload_config(h, &mut last);
                // still unparented with topmost=0 (the desktop was not up when we started): keep trying
                tick = tick.wrapping_add(1);
                if tick % 5 == 0 {
                    let c = snapshot_cfg();
                    if !c.topmost && GetParent(h).is_null() && !desktop_hwnd().is_null() {
                        place(h, &c);
                    }
                }
                paint(h);
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        STOP.store(true, Relaxed);
        let h = WIDGET.load(Relaxed) as HWND;
        if !h.is_null() {
            KillTimer(h, 1);
        }
    }
}
