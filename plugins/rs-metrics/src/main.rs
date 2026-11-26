// rs-metrics: a small desktop metrics widget (CPU, GPU, RAM, disks, disk I/O, network, temperatures), in the WinRust
// palette. One transparent layered window holds a frosted pill per module; a detached module gets its own window.
//
//   - one sampler thread every second does the cheap kernel calls (CPU, RAM, disks, network), PDH for disk I/O and GPU;
//   - one lowest-priority thread reads NVML / SMART / LibreHardwareMonitor / the ACPI zone every few seconds;
//   - this thread paints what they leave behind, handles the pills, the reorder drag and the menu.
//
// Config: %ProgramData%\WinRust\metrics.txt (key=value, re-read once a second). Log: %LOCALAPPDATA%\rs-shell\metrics.log.
#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
#[macro_use]
extern crate winrust_common;

mod canvas;
mod config;
mod frost;
mod menu;
mod metrics;
mod render;
mod temps;

use std::{
    cell::RefCell,
    collections::HashMap,
    io::Write,
    path::PathBuf,
    process::Command,
    ptr::null_mut,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering::Relaxed},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::{
        BeginPaint, EndPaint, GetMonitorInfoW, MapWindowPoints, MonitorFromWindow, ScreenToClient, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{CreateMutexW, GetCurrentThreadId},
    },
    UI::{
        HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext},
        Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, VK_SHIFT},
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
const WM_APP_MENU: u32 = WM_APP + 2;
const WM_APP_REBUILD: u32 = WM_APP + 3;

static CFG: OnceLock<Arc<Mutex<Config>>> = OnceLock::new();
static SAMPLE: OnceLock<Arc<Mutex<Option<Sample>>>> = OnceLock::new();
static STOP: AtomicBool = AtomicBool::new(false);
static QUITTING: AtomicBool = AtomicBool::new(false);
static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static UI_TID: AtomicU32 = AtomicU32::new(0);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
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

/// Dark scrollbars and menus for the whole process, like the other WinRust windows.
fn dark_menus() {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    unsafe {
        let m = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if m.is_null() {
            return;
        }
        if let Some(f) = GetProcAddress(m, 135usize as *const u8) {
            std::mem::transmute::<_, extern "system" fn(i32) -> i32>(f)(2);
        }
        if let Some(f) = GetProcAddress(m, 136usize as *const u8) {
            std::mem::transmute::<_, extern "system" fn()>(f)();
        }
    }
}

fn cfg() -> Arc<Mutex<Config>> {
    CFG.get().expect("config").clone()
}

fn sample_slot() -> Arc<Mutex<Option<Sample>>> {
    SAMPLE.get().expect("sample").clone()
}

fn snapshot_cfg() -> Config {
    cfg().lock().map(|c| c.clone()).unwrap_or_else(|e| e.into_inner().clone())
}

fn snapshot_sample() -> Sample {
    sample_slot().lock().map(|s| s.clone().unwrap_or_default()).unwrap_or_else(|e| e.into_inner().clone().unwrap_or_default())
}

fn windows() -> Vec<isize> {
    WINDOWS.lock().map(|w| w.clone()).unwrap_or_default()
}

fn main_hwnd() -> HWND {
    windows().first().copied().unwrap_or(0) as HWND
}

fn register(hwnd: HWND) {
    if hwnd.is_null() {
        return;
    }
    if let Ok(mut w) = WINDOWS.lock() {
        if !w.contains(&(hwnd as isize)) {
            w.push(hwnd as isize);
        }
    }
}

fn unregister(hwnd: HWND) {
    if let Ok(mut w) = WINDOWS.lock() {
        w.retain(|&h| h != hwnd as isize);
    }
}

/// 0 = the card, module+1 = a detached window.
fn window_module(hwnd: HWND) -> Option<usize> {
    let v = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as usize;
    (v > 0).then_some(v - 1)
}

// ---------- caches ----------

struct Cache {
    cv: Option<Canvas>,
    bg: Option<Vec<u32>>,
    bg_key: u64,
    size: (i32, i32),
    /// The pills of the last frame, in client coordinates (hit-testing for the reorder/libre menu).
    pills: Vec<(usize, i32, i32, i32, i32)>,
}

impl Default for Cache {
    fn default() -> Self {
        Cache { cv: None, bg: None, bg_key: u64::MAX, size: (0, 0), pills: Vec::new() }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DragKind {
    Move,
    Reorder(usize),
}

struct Drag {
    kind: DragKind,
    /// cursor and window origin at the start, both in screen coordinates
    sx: i32,
    sy: i32,
    wx: i32,
    wy: i32,
    moved: bool,
    order_changed: bool,
}

struct Ui {
    fonts: Option<Fonts>,
    face: String,
    scale: f32,
    series: Series,
    last_seq: u64,
    drag: Option<Drag>,
}

thread_local! {
    static UI: RefCell<Ui> = RefCell::new(Ui { fonts: None, face: String::new(), scale: 1.0, series: Series::default(), last_seq: 0, drag: None });
    static CACHE: RefCell<HashMap<isize, Cache>> = RefCell::new(HashMap::new());
}

fn scale_of(hwnd: HWND) -> f32 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 { 1.0 } else { (d as f32 / 96.0).max(0.5) }
}

// ---------- the desktop ----------

fn desktop_hwnd() -> HWND {
    let d = winrust_common::desktop::window();
    if !d.is_null() {
        return d;
    }
    unsafe { FindWindowW(wide("Progman").as_ptr(), wide("Program Manager").as_ptr()) }
}

fn screen_to_parent(parent: HWND, x: i32, y: i32) -> (i32, i32) {
    let mut p = POINT { x, y };
    unsafe { MapWindowPoints(null_mut(), parent, &mut p, 1) };
    (p.x, p.y)
}

/// Where UpdateLayeredWindow paints: the window origin in its parent's client area when it is a child.
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

/// The window origin in *screen* coordinates: what the frost samples the wallpaper with.
fn screen_origin(hwnd: HWND) -> (i32, i32) {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        (r.left, r.top)
    }
}

unsafe fn move_screen(hwnd: HWND, x: i32, y: i32) {
    let parent = GetParent(hwnd);
    let (px, py) = if parent.is_null() { (x, y) } else { screen_to_parent(parent, x, y) };
    SetWindowPos(hwnd, null_mut(), px, py, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
}

/// Child of the desktop (topmost=0) or an always-on-top popup (topmost=1). `screen` is the desired origin.
unsafe fn place(hwnd: HWND, cfg: &Config, screen: (i32, i32)) {
    let parent = GetParent(hwnd);
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
    let desk = if cfg.topmost { null_mut() } else { desktop_hwnd() };
    if !desk.is_null() {
        if parent != desk {
            SetWindowLongPtrW(hwnd, GWL_STYLE, ((style & !(WS_POPUP | WS_CAPTION | WS_THICKFRAME)) | WS_CHILD) as isize);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_TOPMOST) as isize);
            SetParent(hwnd, desk);
        }
        let (px, py) = screen_to_parent(desk, screen.0, screen.1);
        // just below the icon list: the icons paint over the pills, the pills over the wallpaper hosts
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
        SetWindowPos(hwnd, HWND_TOPMOST, screen.0, screen.1, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    } else {
        SetWindowLongPtrW(hwnd, GWL_STYLE, style as isize);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (ex & !WS_EX_TOPMOST) as isize);
        SetWindowPos(hwnd, HWND_BOTTOM, screen.0, screen.1, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// Keeps the pill band right under the icon list (the desktop's marquee catcher otherwise climbs above us).
unsafe fn reassert_z(hwnd: HWND) {
    let parent = GetParent(hwnd);
    if parent.is_null() {
        return;
    }
    let list = FindWindowExW(parent, null_mut(), wide("SysListView32").as_ptr(), null_mut());
    if list.is_null() {
        return;
    }
    let mut r: RECT = std::mem::zeroed();
    GetWindowRect(hwnd, &mut r);
    SetWindowPos(hwnd, list, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    let _ = r;
}

// ---------- painting ----------

fn paint(hwnd: HWND) {
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return;
    }
    let cfg = snapshot_cfg();
    let smp = snapshot_sample();
    let t = temps::snapshot();
    let scale = scale_of(hwnd);
    let module = window_module(hwnd);
    let plan = render::build(&cfg, &smp, &t, scale, module);
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        if r.right - r.left != plan.w || r.bottom - r.top != plan.h {
            SetWindowPos(hwnd, null_mut(), 0, 0, plan.w, plan.h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
    let origin = paint_origin(hwnd);
    let screen = screen_origin(hwnd);
    // fonts follow the style/DPI once for the whole process
    let face = style::ui_face();
    UI.with(|u| {
        let mut u = u.borrow_mut();
        if u.fonts.is_none() || u.face != face || (u.scale - scale).abs() > 0.01 {
            u.fonts = Some(Fonts::new(face, scale));
            u.face = face.to_string();
            u.scale = scale;
            CACHE.with(|c| c.borrow_mut().clear());
        }
    });
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        let e = c.entry(hwnd as isize).or_default();
        if e.size != (plan.w, plan.h) {
            e.cv = Canvas::new(plan.w, plan.h);
            e.bg = None;
            e.bg_key = u64::MAX;
            e.size = (plan.w, plan.h);
        }
        e.pills = plan.pills.iter().map(|p| (p.module, p.x, p.y, p.w, p.h)).collect();
        let Some(cv) = e.cv.as_mut() else { return };
        let key = render::bg_key(&plan, &cfg, screen);
        if e.bg_key != key || e.bg.is_none() {
            render::draw_bg(cv, &plan, &cfg, screen);
            e.bg = Some(cv.snapshot());
            e.bg_key = key;
        } else if let Some(bg) = e.bg.as_ref() {
            cv.restore(bg);
        }
        UI.with(|u| {
            let u = u.borrow();
            if let Some(fonts) = u.fonts.as_ref() {
                render::draw_content(cv, &plan, &cfg, &smp, &u.series, fonts);
            }
        });
        cv.present(hwnd, origin);
    });
}

fn paint_all() {
    for h in windows() {
        let hwnd = h as HWND;
        if !hwnd.is_null() && unsafe { IsWindow(hwnd) } != 0 {
            paint(hwnd);
        }
    }
}

// ---------- window creation ----------

unsafe fn build_window(c: &Config, module: Option<usize>) -> HWND {
    let inst = GetModuleHandleW(null_mut());
    let class = wide(CLASS);
    let scale = 1.0;
    let plan = render::build(c, &metrics::quick(c), &temps::snapshot(), scale, module);
    let (sx, sy) = match module {
        Some(m) => (c.dx[m], c.dy[m]),
        None => (c.x, c.y),
    };
    let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | if c.topmost { WS_EX_TOPMOST } else { 0 };
    let hwnd = CreateWindowExW(
        ex,
        class.as_ptr(),
        wide(TITLE).as_ptr(),
        WS_POPUP,
        sx,
        sy,
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
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, module.map_or(0, |m| m as isize + 1));
    place(hwnd, c, (sx, sy));
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    if module.is_none() {
        SetTimer(hwnd, 1, 1000, None);
    }
    register(hwnd);
    paint(hwnd);
    log(&format!("window {} at {},{}", module.map_or("card".into(), |m| config::MODS[m].to_string()), sx, sy));
    hwnd
}

unsafe fn destroy_all() {
    for h in std::mem::take(&mut *WINDOWS.lock().unwrap_or_else(|e| e.into_inner())) {
        let hwnd = h as HWND;
        if IsWindow(hwnd) != 0 {
            DestroyWindow(hwnd);
        }
    }
    CACHE.with(|c| c.borrow_mut().clear());
}

/// Recreates the card and every detached pill (also after the desktop restarted and destroyed its children).
unsafe fn rebuild_windows() {
    let c = snapshot_cfg();
    destroy_all();
    let main = build_window(&c, None);
    if main.is_null() {
        log("the card could not be created");
        return;
    }
    for m in 0..8 {
        if c.detached[m] && c.on[m] {
            build_window(&c, Some(m));
        }
    }
    if !c.enabled {
        for h in windows() {
            ShowWindow(h as HWND, SW_HIDE);
        }
    }
}

fn spawn_detached(c: &Config, m: usize) {
    unsafe {
        let h = build_window(c, Some(m));
        if !h.is_null() && !c.enabled {
            ShowWindow(h, SW_HIDE);
        }
    }
}

// ---------- sampler thread ----------

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

fn detached_of(c: &Config) -> [bool; 8] {
    c.detached
}

/// Applies a changed file. Returns true when the window set has to be rebuilt.
fn reload_config(main: HWND, last: &mut Option<(u64, u64)>) -> bool {
    let now = stamp();
    if now.is_none() || now == *last {
        return false;
    }
    *last = now;
    let fresh = config::read();
    let old = snapshot_cfg();
    let drag = UI.with(|u| u.borrow().drag.is_some());
    let structural = detached_of(&fresh) != detached_of(&old) || fresh.topmost != old.topmost || fresh.order != old.order;
    let enabled = fresh.enabled;
    let was_enabled = old.enabled;
    let (x, y) = (fresh.x, fresh.y);
    {
        let c = cfg();
        let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
        *c = fresh;
    }
    if enabled != was_enabled {
        for h in windows() {
            unsafe { ShowWindow(h as HWND, if enabled { SW_SHOWNOACTIVATE } else { SW_HIDE }) };
        }
        log(if enabled { "shown" } else { "hidden" });
    }
    if structural {
        log("config: rebuilding the windows");
        return true;
    }
    if !drag && !main.is_null() {
        unsafe { move_screen(main, x, y) };
    }
    false
}

// ---------- mouse ----------

fn snap_position(hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    unsafe {
        let mut r: RECT = std::mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let (mut x, mut y) = (x, y);
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
        (x, y)
    }
}

fn finish_drag(hwnd: HWND) {
    let drag = UI.with(|u| u.borrow_mut().drag.take());
    let Some(d) = drag else { return };
    if d.kind == DragKind::Move {
        unsafe {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(hwnd, &mut r);
            let (x, y) = snap_position(hwnd, r.left, r.top);
            if x != r.left || y != r.top {
                move_screen(hwnd, x, y);
            }
            let c = cfg();
            let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
            let m = window_module(hwnd);
            match m {
                Some(m) => {
                    c.dx[m] = x;
                    c.dy[m] = y;
                }
                None => {
                    c.x = x;
                    c.y = y;
                }
            }
            c.save();
        }
    } else if d.order_changed {
        let c = cfg();
        let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
        c.save();
    }
    paint_all();
}

/// Moves the dragged module to the slot of the pill under the cursor.
fn reorder_under(hwnd: HWND, dragged: usize, x: i32, y: i32) {
    let target = CACHE.with(|c| {
        let c = c.borrow();
        c.get(&(hwnd as isize)).and_then(|e| {
            e.pills.iter().find(|p| x >= p.1 && y >= p.2 && x < p.1 + p.3 && y < p.2 + p.4).map(|p| p.0)
        })
    });
    let Some(target) = target else { return };
    if target == dragged {
        return;
    }
    let c = cfg();
    let mut c = c.lock().unwrap_or_else(|e| e.into_inner());
    let Some(from) = c.order.iter().position(|m| *m == dragged) else { return };
    let Some(to) = c.order.iter().position(|m| *m == target) else { return };
    let m = c.order.remove(from);
    c.order.insert(to, m);
    drop(c);
    UI.with(|u| {
        if let Some(d) = u.borrow_mut().drag.as_mut() {
            d.order_changed = true;
        }
    });
    paint_all();
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
        config::read();
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
    // the pill under the cursor on the card, or the window's own module when detached
    let module = window_module(hwnd);
    let context = match module {
        Some(m) => Some(m),
        None => {
            let mut p = POINT { x, y };
            unsafe { ScreenToClient(hwnd, &mut p) };
            CACHE.with(|c| {
                let c = c.borrow();
                c.get(&(hwnd as isize)).and_then(|e| e.pills.iter().find(|q| p.x >= q.1 && p.y >= q.2 && p.x < q.1 + q.3 && p.y < q.2 + q.4).map(|q| q.0))
            })
        }
    };
    let c = snapshot_cfg();
    let id = unsafe { menu::show(x, y, &c, scale_of(hwnd), context) };
    if id == 0 {
        return;
    }
    log(&format!("menu -> {id}"));
    let mut quit = false;
    let mut detach = None;
    let mut attach = None;
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
            }
        } else {
            match id {
                menu::ID_RELOAD => {
                    *c = config::read();
                }
                menu::ID_OPEN => open_config(),
                menu::ID_SETTINGS => open_settings(),
                menu::ID_COLUMNS => {
                    let next = if c.cols == 2 { "1" } else { "2" };
                    c.set("cols", next);
                }
                menu::ID_RESET => {
                    let (x, y) = match module {
                        Some(m) => {
                            c.dx[m] = 60 + m as i32 * 30;
                            c.dy[m] = 60 + m as i32 * 40;
                            (c.dx[m], c.dy[m])
                        }
                        None => {
                            c.x = 60;
                            c.y = 60;
                            (60, 60)
                        }
                    };
                    unsafe { move_screen(hwnd, x, y) };
                    c.save();
                }
                menu::ID_CLOSE => quit = true,
                menu::ID_DETACH => detach = context,
                menu::ID_ATTACH => attach = module,
                _ => {}
            }
        }
    }
    if quit {
        unsafe {
            QUITTING.store(true, Relaxed);
            STOP.store(true, Relaxed);
            destroy_all();
            PostQuitMessage(0);
        }
        return;
    }
    if let Some(m) = detach {
        {
            let a = cfg();
            let mut c = a.lock().unwrap_or_else(|e| e.into_inner());
            c.detached[m] = true;
            c.save();
            let clone = c.clone();
            drop(c);
            spawn_detached(&clone, m);
        }
        paint_all();
        return;
    }
    if let Some(m) = attach {
        {
            let a = cfg();
            let mut c = a.lock().unwrap_or_else(|e| e.into_inner());
            c.detached[m] = false;
            c.save();
        }
        // close that module's own window; the card picks the pill up again
        for h in windows() {
            let w = h as HWND;
            if window_module(w) == Some(m) {
                unsafe { DestroyWindow(w) };
            }
        }
        paint_all();
        return;
    }
    paint_all();
}

// ---------- window procedure ----------

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
            // the pill windows are small: everything in them is content (the 6 px shadow margin included)
            HTCLIENT as LRESULT
        }
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let mut p: POINT = std::mem::zeroed();
            let mut r: RECT = std::mem::zeroed();
            GetCursorPos(&mut p);
            GetWindowRect(hwnd, &mut r);
            let shift = GetKeyState(VK_SHIFT as i32) < 0;
            let kind = if shift {
                DragKind::Move
            } else if let Some(m) = window_module(hwnd) {
                let _ = m;
                DragKind::Move
            } else {
                let pill = CACHE.with(|c| {
                    let c = c.borrow();
                    c.get(&(hwnd as isize)).and_then(|e| e.pills.iter().find(|q| x >= q.1 && y >= q.2 && x < q.1 + q.3 && y < q.2 + q.4).map(|q| q.0))
                });
                pill.map_or(DragKind::Move, DragKind::Reorder)
            };
            UI.with(|u| u.borrow_mut().drag = Some(Drag { kind, sx: p.x, sy: p.y, wx: r.left, wy: r.top, moved: false, order_changed: false }));
            SetCapture(hwnd);
            0
        }
        WM_MOUSEMOVE => {
            let drag = UI.with(|u| u.borrow().drag.as_ref().map(|d| (d.kind, d.sx, d.sy, d.wx, d.wy)));
            if let Some((kind, ax, ay, wx, wy)) = drag {
                let mut p: POINT = std::mem::zeroed();
                unsafe { GetCursorPos(&mut p) };
                let moved = (p.x - ax).abs() > 3 || (p.y - ay).abs() > 3;
                if moved {
                    UI.with(|u| {
                        if let Some(d) = u.borrow_mut().drag.as_mut() {
                            d.moved = true;
                        }
                    });
                }
                match kind {
                    DragKind::Move => {
                        if moved {
                            move_screen(hwnd, wx + p.x - ax, wy + p.y - ay);
                        }
                    }
                    DragKind::Reorder(module) => {
                        // the cursor position in the card's client coordinates
                        let mut c: POINT = POINT { x: p.x, y: p.y };
                        ScreenToClient(hwnd, &mut c);
                        reorder_under(hwnd, module, c.x, c.y);
                    }
                }
            }
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            finish_drag(hwnd);
            0
        }
        WM_CAPTURECHANGED => {
            UI.with(|u| u.borrow_mut().drag = None);
            0
        }
        WM_RBUTTONUP | WM_CONTEXTMENU => {
            PostMessageW(hwnd, WM_APP_MENU, 0, 0);
            0
        }
        WM_APP_MENU => {
            show_menu(hwnd);
            0
        }
        WM_TIMER => 0,
        WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            paint(hwnd);
            0
        }
        WM_DEVICECHANGE => {
            temps::REFRESH_DISKS.store(true, Relaxed);
            0
        }
        WM_CLOSE => {
            QUITTING.store(true, Relaxed);
            STOP.store(true, Relaxed);
            destroy_all();
            PostQuitMessage(0);
            0
        }
        WM_DESTROY => {
            unregister(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------- the tick ----------

fn tick(main: HWND, last: &mut Option<(u64, u64)>, count: &mut u32) {
    if reload_config(main, last) {
        unsafe { rebuild_windows() };
        return;
    }
    frost::ensure();
    let smp = snapshot_sample();
    UI.with(|u| {
        let mut u = u.borrow_mut();
        if smp.seq != u.last_seq {
            u.series.push(&smp);
            u.last_seq = smp.seq;
        }
    });
    *count = count.wrapping_add(1);
    if *count % 5 == 0 {
        for h in windows() {
            unsafe { reassert_z(h as HWND) };
        }
    }
    paint_all();
}

fn main() {
    winrust_common::appid::set("WinRust");
    let _ = lang!();
    unsafe { SetProcessDpiAwarenessContext(-4isize as _) };
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
    std::thread::spawn(move || temps::temp_loop(cfg()));
    frost::ensure();

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

        let main = build_window(&first, None);
        if main.is_null() {
            log("the widget window could not be created");
            return;
        }
        for m in 0..8 {
            if first.detached[m] && first.on[m] {
                build_window(&first, Some(m));
            }
        }
        if !first.enabled {
            for h in windows() {
                ShowWindow(h as HWND, SW_HIDE);
            }
        }
        // the desktop can be restarted (rust-explorer is respawned): its children die with it, so a watcher asks this
        // thread to build the whole set again instead of leaving the process alive with no window
        std::thread::spawn(|| loop {
            std::thread::sleep(Duration::from_secs(3));
            if STOP.load(Relaxed) {
                continue;
            }
            let ws = windows();
            let dead = ws.is_empty() || ws.iter().any(|&h| IsWindow(h as HWND) == 0);
            if dead {
                PostThreadMessageW(UI_TID.load(Relaxed), WM_APP_REBUILD, 0, 0);
            }
        });

        let mut last = stamp();
        let mut count = 0u32;
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, null_mut(), 0, 0);
            if r <= 0 {
                break;
            }
            if msg.hwnd.is_null() && msg.message == WM_APP_REBUILD {
                if !STOP.load(Relaxed) {
                    let ws = windows();
                    let dead = ws.is_empty() || ws.iter().any(|&h| IsWindow(h as HWND) == 0);
                    if dead {
                        log("rebuilding the widget windows");
                        rebuild_windows();
                    }
                }
                continue;
            }
            let live_main = main_hwnd();
            if msg.message == WM_TIMER && msg.hwnd == live_main && !live_main.is_null() {
                tick(live_main, &mut last, &mut count);
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        STOP.store(true, Relaxed);
        let h = main_hwnd();
        if !h.is_null() {
            KillTimer(h, 1);
        }
    }
}
