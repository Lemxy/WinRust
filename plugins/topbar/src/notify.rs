//! The notification pop-ups (toasts) of the bar, their stack, their history and the notification centre.
//!
//! A toast is its own `topbar_toast` window: the same frost as the menus (a screen capture + blur + the `menu-alpha`
//! tint), rounded, topmost and `WS_EX_NOACTIVATE`, so it never takes the focus from the program the user is typing in.
//! At most three are visible; a new one enters at the bottom right of the primary monitor's work area and pushes the
//! older ones up. The fourth sends the oldest to the history, which the bell next to the clock lists.
//!
//! Other programs raise a notification by connecting to `\\.\pipe\winrust-notify` and writing one
//! `winrust_common::notify::NotifyWire`; the pipe thread here only turns it into a `Notification` and hands it to the
//! bar thread through `post`.
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    ffi::c_void,
    mem::{size_of, zeroed},
    path::Path,
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Mutex,
    },
};

use windows_sys::Win32::{
    Foundation::SIZE,
    Storage::FileSystem::{ReadFile, PIPE_ACCESS_INBOUND},
    System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT},
};

use winrust_common::{
    look,
    notify::NotifyWire,
    style::{icon_face, ui_face},
};

use super::*;

/// Posted to the bar window by `post`: new records are waiting in `INBOX`.
pub(crate) const POST_MSG: u32 = WM_APP + 20;

const TOAST_W: i32 = 380;
const TOAST_H: i32 = 96;
const TOAST_GAP: i32 = 8;
const MAX_TOASTS: usize = 3;
const TOAST_ANIM_MS: f32 = 180.0;
const TOAST_SLIDE: i32 = 24;

const PANEL_W: i32 = 360;
const PANEL_H: i32 = 420;
const PANEL_HEAD: i32 = 56;
const PANEL_PAD: i32 = 16;
const PANEL_ROW: i32 = 64;
const HISTORY_MAX: usize = 50;
/// How many rows fit in the panel (the wheel scrolls the rest).
const PANEL_ROWS: usize = 5;

/// One notification while it is in the bar: the toast and the history entry share it.
#[derive(Clone)]
pub struct Notification {
    pub id: String,
    pub app: String,
    pub title: String,
    pub body: String,
    /// Path of an exe / shortcut / image the shell can give an icon for; empty = the app glyph.
    pub icon: String,
    /// Milliseconds; 0 = the `notify-timeout` setting.
    pub timeout_ms: u32,
    /// 0..1 draws the progress bar and keeps the toast open; negative = no bar.
    pub progress: f32,
    pub action_label: String,
    pub action_cmd: String,
}

#[derive(Clone)]
struct HistoryItem {
    n: Notification,
    at: u64, // GetTickCount64 when it first arrived
}

/// The inbox the pipe thread fills and the bar thread drains.
static INBOX: Mutex<Vec<Notification>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The visible toasts, newest first: index 0 is the bottom one.
thread_local! {
    static STACK: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
    static HISTORY: RefCell<VecDeque<HistoryItem>> = RefCell::new(VecDeque::new());
    static UNREAD: Cell<bool> = const { Cell::new(false) };
    static PANEL: Cell<HWND> = const { Cell::new(null_mut()) };
    static PANEL_PX: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static P_OPENED: Cell<u64> = const { Cell::new(0) };
    static P_SCROLL: Cell<usize> = const { Cell::new(0) };
    static P_HOT: Cell<i32> = const { Cell::new(-1) };
    static P_FONTS: Cell<[usize; 5]> = const { Cell::new([0; 5]) };
}

/// Registers both window classes. Call once, from the bar thread.
pub(crate) unsafe fn init(hi: HINSTANCE) {
    let toast = wide("topbar_toast");
    if GetClassInfoW(hi, toast.as_ptr(), &mut zeroed()) == 0 {
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(toast_proc),
            hInstance: hi,
            hCursor: LoadCursorW(null_mut(), IDC_HAND),
            lpszClassName: toast.as_ptr(),
            ..zeroed()
        });
    }
    let panel = wide("topbar_notify");
    if GetClassInfoW(hi, panel.as_ptr(), &mut zeroed()) == 0 {
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(panel_proc),
            hInstance: hi,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: panel.as_ptr(),
            ..zeroed()
        });
    }
}

/// Starts the named-pipe server (one thread, one connection at a time). Call after the bar window exists.
pub(crate) fn listen() {
    std::thread::spawn(server);
}

/// Queues a notification from any thread. The bar thread shows it on its next `POST_MSG`.
pub fn post(n: Notification) {
    if !look::notify() {
        return;
    }
    INBOX.lock().unwrap_or_else(|e| e.into_inner()).push(n);
    let bar = crate::BAR.load(Relaxed) as HWND;
    if !bar.is_null() {
        unsafe {
            PostMessageW(bar, POST_MSG, 0, 0);
        }
    }
}

/// Bar thread: drains the inbox (called on `POST_MSG`).
pub(crate) unsafe fn pump() {
    let items: Vec<Notification> = INBOX.lock().unwrap_or_else(|e| e.into_inner()).drain(..).collect();
    for n in items {
        show(n);
    }
}

fn server() {
    let name = wide(winrust_common::notify::PIPE);
    loop {
        unsafe {
            let h = CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND,
                PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                WIRE_BYTES as u32,
                WIRE_BYTES as u32,
                0,
                null(),
            );
            if h == INVALID_HANDLE_VALUE {
                std::thread::sleep(std::time::Duration::from_millis(500));
                continue;
            }
            let connected = ConnectNamedPipe(h, null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED;
            if connected {
                let mut buf = [0u8; WIRE_BYTES];
                let mut got = 0u32;
                let ok = ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut got, null_mut());
                if ok != 0 && got > 0 {
                    let w = winrust_common::notify::from_bytes(&buf[..got as usize]);
                    post(Notification {
                        id: w.id(),
                        app: w.app(),
                        title: w.title(),
                        body: w.body(),
                        icon: w.icon(),
                        timeout_ms: w.timeout_ms,
                        progress: w.progress().map_or(-1.0, |p| p),
                        action_label: w.action_label(),
                        action_cmd: w.action_cmd(),
                    });
                }
            }
            DisconnectNamedPipe(h);
            CloseHandle(h);
        }
    }
}

const WIRE_BYTES: usize = winrust_common::notify::WIRE_SIZE;

/// Shows a record: updates the visible toast with the same id, or adds a new one at the bottom of the stack.
unsafe fn show(mut n: Notification) {
    if !look::notify() {
        return;
    }
    if n.id.is_empty() {
        n.id = format!("auto-{}", NEXT_ID.fetch_add(1, Relaxed));
    }
    n.progress = if n.progress.is_finite() && n.progress >= 0.0 { n.progress.clamp(0.0, 1.0) } else { -1.0 };

    // With the centre open a banner would only cover it: the record goes straight to the list (as Windows does).
    if panel_visible() {
        record(&n, true);
        invalidate_panel();
        return;
    }

    if let Some(h) = visible(&n.id) {
        if let Some(t) = tst(h) {
            t.n = n.clone();
            if has_progress(&n) {
                KillTimer(h, 2);
            } else if t.phase == 1 && !t.hover {
                let ms = timeout(&n);
                t.due = now_ms() + ms as u64;
                t.remaining = ms as u64;
                SetTimer(h, 2, ms, None);
            }
        }
        InvalidateRect(h, null(), 0);
        record(&n, false);
        invalidate_panel();
        return;
    }

    while STACK.with(|s| s.borrow().len()) >= MAX_TOASTS {
        let oldest = STACK.with(|s| s.borrow().last().copied());
        match oldest {
            Some(h) => start_out(h),
            None => break,
        }
    }
    // Move the ones already there up before the capture: the new toast gets a clean desktop behind it.
    let list = STACK.with(|s| s.borrow().clone());
    for (i, &h) in list.iter().enumerate() {
        place_toast(h, i + 1);
    }

    let (w, h) = (sc(TOAST_W), sc(TOAST_H));
    let (x, y) = slot(0, w, h);
    let class = wide("topbar_toast");
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        class.as_ptr(),
        class.as_ptr(),
        WS_POPUP,
        x + sc(TOAST_SLIDE),
        y,
        w,
        h,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    if hwnd.is_null() {
        return;
    }
    let icon = if n.icon.is_empty() { null_mut() } else { winrust_common::icon::best(Path::new(&n.icon), sc(32)) };
    let face = wide(ui_face());
    let gface = wide(icon_face());
    let font = |f: &[u16], px: i32, wt: i32| unsafe { CreateFontW(-sc(px), 0, 0, 0, wt, 0, 0, 0, 1, 0, 0, 5, 0, f.as_ptr()) };
    let st = Box::new(Toast {
        n: n.clone(),
        base_x: x,
        base_y: y,
        t0: now_ms(),
        phase: 0,
        hover: false,
        xhot: false,
        due: 0,
        remaining: timeout(&n) as u64,
        px: Vec::new(),
        icon,
        fonts: [font(&face, 11, 400), font(&face, 12, 400), font(&face, 13, 600), font(&gface, 16, 400)],
    });
    let ptr = Box::into_raw(st);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
    // The window is still hidden, so this is what the screen shows behind the toast.
    let px = if look::menu_blur() {
        let radius = sc(look::menu_radius()).max(h / 22).max(sc(8));
        crate::blur::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + h }, radius)
    } else {
        Vec::new()
    };
    if let Some(t) = tst(hwnd) {
        t.px = px;
    }
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, rr(12), rr(12)), 1);
    SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
    STACK.with(|s| s.borrow_mut().insert(0, hwnd));
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    InvalidateRect(hwnd, null(), 0);
    record(&n, true);
    if PANEL.with(|p| p.get()).is_null() || IsWindowVisible(PANEL.with(|p| p.get())) == 0 {
        UNREAD.set(true);
    }
    invalidate_bar();
    invalidate_panel();
    SetTimer(hwnd, 1, 16, None);
}

fn visible(id: &str) -> Option<HWND> {
    let list = STACK.with(|s| s.borrow().clone());
    list.into_iter().find(|&h| unsafe { tst(h).is_some_and(|t| t.n.id == id) })
}

fn panel_visible() -> bool {
    let h = PANEL.with(|p| p.get());
    !h.is_null() && unsafe { IsWindowVisible(h) != 0 }
}

fn has_progress(n: &Notification) -> bool {
    n.progress.is_finite() && n.progress >= 0.0
}

fn timeout(n: &Notification) -> u32 {
    let ms = if n.timeout_ms > 0 { n.timeout_ms } else { (look::notify_timeout().max(1) as u32) * 1000 };
    ms.clamp(500, 600_000)
}

fn record(n: &Notification, add: bool) {
    let at = unsafe { now_ms() };
    HISTORY.with(|h| {
        let mut m = h.borrow_mut();
        if let Some(e) = m.iter_mut().find(|e| e.n.id == n.id) {
            let first = e.at;
            e.n = n.clone();
            e.at = first;
        } else if add {
            m.push_front(HistoryItem { n: n.clone(), at });
            while m.len() > HISTORY_MAX {
                m.pop_back();
            }
        }
    });
}

/// Closes every toast and the panel when the setting is switched off, and repaints the bell either way.
pub(crate) unsafe fn refresh() {
    if !look::notify() {
        let list = STACK.with(|s| s.borrow().clone());
        for h in list {
            start_out(h);
        }
        close_center();
        UNREAD.set(false);
    }
    invalidate_bar();
}

/// The accent dot on the bell.
pub(crate) fn unread_active() -> bool {
    look::notify() && UNREAD.with(|u| u.get())
}

fn invalidate_bar() {
    let bar = crate::BAR.load(Relaxed) as HWND;
    if !bar.is_null() {
        unsafe {
            InvalidateRect(bar, null(), 0);
        }
    }
}

fn invalidate_panel() {
    let h = PANEL.with(|p| p.get());
    if !h.is_null() {
        unsafe {
            InvalidateRect(h, null(), 0);
        }
    }
}

// ---------- the toast window ----------

struct Toast {
    n: Notification,
    base_x: i32,
    base_y: i32,
    t0: u64,
    /// 0 entering, 1 shown, 2 leaving.
    phase: u8,
    hover: bool,
    xhot: bool,
    /// When the auto-close timer fires (GetTickCount64).
    due: u64,
    /// What is left of the timeout while the pointer rests on the toast.
    remaining: u64,
    px: Vec<u32>,
    icon: HICON,
    fonts: [HFONT; 4],
}

fn tptr(hwnd: HWND) -> *mut Toast {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Toast }
}

unsafe fn tst<'a>(hwnd: HWND) -> Option<&'a mut Toast> {
    tptr(hwnd).as_mut()
}

/// Screen position of stack slot `i` (0 = the bottom one), inside the primary monitor's work area.
unsafe fn slot(i: usize, w: i32, h: i32) -> (i32, i32) {
    let wa = work_area();
    // the bar is only on the primary: on a secondary monitor the toast may use the whole work area
    let primary = wa.right == GetSystemMetrics(SM_CXSCREEN) && wa.bottom <= GetSystemMetrics(SM_CYSCREEN);
    let bottom = if vert() || at_top() || !primary { wa.bottom } else { wa.bottom.min(bar_top()) };
    let x = wa.right - w - sc(8);
    let y = (bottom - h - sc(8) - i as i32 * (h + sc(TOAST_GAP))).max(wa.top + sc(8));
    (x, y)
}

unsafe fn work_area() -> RECT {
    // the toast follows the pointer: the program that raised it is usually the one being used
    let mut pt: POINT = zeroed();
    GetCursorPos(&mut pt);
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    if !mon.is_null() && GetMonitorInfoW(mon, &mut mi) != 0 && mi.rcWork.right > mi.rcWork.left {
        return mi.rcWork;
    }
    let mut r: RECT = zeroed();
    if SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut RECT as *mut c_void, 0) == 0 || r.right <= r.left || r.bottom <= r.top {
        r = RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) };
    }
    r
}

unsafe fn place_toast(hwnd: HWND, i: usize) {
    let (w, h) = (sc(TOAST_W), sc(TOAST_H));
    let (x, y) = slot(i, w, h);
    if let Some(t) = tst(hwnd) {
        t.base_x = x;
        t.base_y = y;
        if t.phase != 2 {
            SetWindowPos(hwnd, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

unsafe fn reposition() {
    let list = STACK.with(|s| s.borrow().clone());
    for (i, &h) in list.iter().enumerate() {
        place_toast(h, i);
    }
}

unsafe fn start_out(hwnd: HWND) {
    if let Some(t) = tst(hwnd) {
        if t.phase == 2 {
            return;
        }
        t.phase = 2;
        t.t0 = now_ms();
    }
    KillTimer(hwnd, 1);
    KillTimer(hwnd, 2);
    SetTimer(hwnd, 1, 16, None);
    STACK.with(|s| s.borrow_mut().retain(|&h| h != hwnd));
    reposition();
}

/// One 16 ms animation frame: the toast slides in from the right while fading in, and out the same way.
unsafe fn step(hwnd: HWND) {
    let now = now_ms();
    let Some(t) = tst(hwnd) else { return };
    let phase = t.phase;
    let (bx, by) = (t.base_x, t.base_y);
    let q = ((now.saturating_sub(t.t0)) as f32 / TOAST_ANIM_MS).min(1.0);
    let e = ease(q);
    match phase {
        0 => {
            SetLayeredWindowAttributes(hwnd, 0, (e * 255.0) as u8, LWA_ALPHA);
            let x = bx + ((1.0 - e) * sc(TOAST_SLIDE) as f32) as i32;
            SetWindowPos(hwnd, HWND_TOPMOST, x, by, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
            if q >= 1.0 {
                t.phase = 1;
                KillTimer(hwnd, 1);
                if !has_progress(&t.n) && !t.hover {
                    let ms = timeout(&t.n);
                    t.due = now + ms as u64;
                    t.remaining = ms as u64;
                    SetTimer(hwnd, 2, ms, None);
                }
            }
        }
        2 => {
            SetLayeredWindowAttributes(hwnd, 0, ((1.0 - e) * 255.0) as u8, LWA_ALPHA);
            let x = bx + (e * sc(TOAST_SLIDE) as f32) as i32;
            SetWindowPos(hwnd, HWND_TOPMOST, x, by, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
            if q >= 1.0 {
                DestroyWindow(hwnd);
            }
        }
        _ => {
            KillTimer(hwnd, 1);
        }
    }
}

fn close_rect(w: i32) -> RECT {
    RECT { left: w - sc(30), top: sc(6), right: w - sc(6), bottom: sc(30) }
}

fn in_rect(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

unsafe fn paint_toast(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let wdc = BeginPaint(hwnd, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(hwnd, &mut cr);
    if cr.right <= 0 || cr.bottom <= 0 {
        EndPaint(hwnd, &ps);
        return;
    }
    let dc = CreateCompatibleDC(wdc);
    let bmp = CreateCompatibleBitmap(wdc, cr.right, cr.bottom);
    let old = SelectObject(dc, bmp);
    let p = pal();
    let Some(t) = tst(hwnd) else {
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
        return;
    };
    // the same frost as the menus: the capture, clipped at the rounded corners, then the tint over it
    let mut drew = false;
    if !t.px.is_empty() {
        let keep = SaveDC(dc);
        BeginPath(dc);
        RoundRect(dc, 0, 0, cr.right + 1, cr.bottom + 1, rr(12), rr(12));
        EndPath(dc);
        SelectClipPath(dc, RGN_AND);
        let a = (255 * look::menu_alpha().max(62) as u32 / 100) as u8;
        crate::blur::paint_capture(dc, &t.px, cr, p.bg, a);
        RestoreDC(dc, keep);
        drew = true;
    }
    if !drew {
        fill(dc, cr, p.bg, rr(12));
    }
    SetBkMode(dc, TRANSPARENT as i32);
    let [f_app, f_body, f_title, f_glyph] = t.fonts;

    // the icon (or the app glyph on an accent-tinted tile)
    let (is, ix, iy) = (sc(32), sc(14), (cr.bottom - sc(32)) / 2);
    let ir = RECT { left: ix, top: iy, right: ix + is, bottom: iy + is };
    if !t.icon.is_null() {
        winrust_common::icon::draw(dc, t.icon, ix, iy, is);
    } else {
        fill(dc, ir, mix(p.bg, p.accent, 0.18), sc(8));
        SelectObject(dc, f_glyph);
        text(dc, "\u{E7E7}", ir, p.accent, DT_CENTER);
    }

    let left = sc(56);
    let right = cr.right - sc(40);
    let app: &str = if t.n.app.is_empty() { t!("Уведомление", "Notification", ja: "通知", pt: "Notificação", zh: "通知") } else { t.n.app.as_str() };
    SelectObject(dc, f_app);
    text(dc, app, RECT { left, top: sc(10), right, bottom: sc(24) }, p.dim, DT_LEFT);
    SelectObject(dc, f_title);
    text(dc, &t.n.title, RECT { left, top: sc(23), right, bottom: sc(42) }, p.text, DT_LEFT);

    let body_col = mix(p.text, p.dim, 0.25);
    SelectObject(dc, f_body);
    let (l1, l2) = wrap2(dc, &t.n.body, right - left);
    if !l1.is_empty() {
        text(dc, &l1, RECT { left, top: sc(43), right, bottom: sc(58) }, body_col, DT_LEFT);
    }
    if !l2.is_empty() {
        text(dc, &l2, RECT { left, top: sc(57), right, bottom: sc(72) }, body_col, DT_LEFT);
    }

    if has_progress(&t.n) {
        let by = cr.bottom - sc(13);
        let br = RECT { left, top: by, right: cr.right - sc(14), bottom: by + sc(3) };
        fill(dc, br, mix(p.bg, p.text, 0.16), sc(2));
        let fw = ((br.right - br.left) as f32 * t.n.progress.clamp(0.0, 1.0)) as i32;
        if fw > 0 {
            fill(dc, RECT { right: br.left + fw, ..br }, p.accent, sc(2));
        }
    }

    let xr = close_rect(cr.right);
    if t.xhot {
        fill(dc, xr, mix(p.bg, p.accent, 0.22), sc(6));
    }
    SelectObject(dc, f_glyph);
    text(dc, "\u{E711}", xr, if t.xhot { p.accent } else { mix(p.dim, p.text, 0.3) }, DT_CENTER);

    BitBlt(wdc, 0, 0, cr.right, cr.bottom, dc, 0, 0, SRCCOPY);
    SelectObject(dc, old);
    DeleteObject(bmp);
    DeleteDC(dc);
    EndPaint(hwnd, &ps);
}

/// Fits the body into two lines, adding an ellipsis when there is more text than that.
unsafe fn wrap2(dc: HDC, body: &str, width: i32) -> (String, String) {
    let mut out = [String::new(), String::new()];
    let words: Vec<&str> = body.split_whitespace().collect();
    let mut i = 0;
    let mut more = false;
    for line in 0..2 {
        while i < words.len() {
            let cand = if out[line].is_empty() { words[i].to_string() } else { format!("{} {}", out[line], words[i]) };
            if fits(dc, &cand, width) {
                out[line] = cand;
                i += 1;
            } else if out[line].is_empty() {
                // a single word wider than the whole line: cut it by characters
                let mut part = String::new();
                for c in words[i].chars() {
                    let c2 = format!("{part}{c}");
                    if fits(dc, &c2, width) {
                        part = c2;
                    } else {
                        more = true;
                        break;
                    }
                }
                out[line] = part;
                i += 1;
                break;
            } else {
                break;
            }
        }
        if i < words.len() {
            more = true;
        }
    }
    if i < words.len() || more {
        while !out[1].is_empty() && !fits(dc, &format!("{}…", out[1]), width) {
            out[1].pop();
        }
        out[1].push('…');
    }
    (out[0].clone(), out[1].clone())
}

unsafe fn fits(dc: HDC, s: &str, width: i32) -> bool {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut size: SIZE = zeroed();
    GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
    size.cx <= width
}

unsafe extern "system" fn toast_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp & 0xffff) as i16 as i32;
    let y = ((lp >> 16) & 0xffff) as i16 as i32;
    match msg {
        WM_PAINT => {
            paint_toast(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_TIMER if wp == 1 => {
            step(hwnd);
            0
        }
        WM_TIMER if wp == 2 => {
            KillTimer(hwnd, 2);
            if let Some(t) = tst(hwnd) {
                if t.phase == 1 {
                    if t.hover {
                        let left = t.due.saturating_sub(now_ms()).max(200) as u32;
                        SetTimer(hwnd, 2, left, None);
                    } else {
                        start_out(hwnd);
                    }
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            let mut cr: RECT = zeroed();
            GetClientRect(hwnd, &mut cr);
            let hot = in_rect(close_rect(cr.right), x, y);
            if let Some(t) = tst(hwnd) {
                if t.xhot != hot {
                    t.xhot = hot;
                    InvalidateRect(hwnd, null(), 0);
                }
                if !t.hover {
                    t.hover = true;
                    if t.phase == 1 {
                        t.remaining = t.due.saturating_sub(now_ms());
                        KillTimer(hwnd, 2);
                    }
                }
            }
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut tme);
            0
        }
        WM_MOUSELEAVE => {
            if let Some(t) = tst(hwnd) {
                t.hover = false;
                t.xhot = false;
                InvalidateRect(hwnd, null(), 0);
                if t.phase == 1 && !has_progress(&t.n) {
                    let ms = t.remaining.max(200) as u32;
                    t.due = now_ms() + ms as u64;
                    SetTimer(hwnd, 2, ms, None);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            let mut cr: RECT = zeroed();
            GetClientRect(hwnd, &mut cr);
            let hot = in_rect(close_rect(cr.right), x, y);
            if !hot {
                if let Some(t) = tst(hwnd) {
                    if !t.n.action_cmd.is_empty() {
                        run_cmd(&t.n.action_cmd);
                    }
                }
            }
            start_out(hwnd);
            0
        }
        WM_NCDESTROY => {
            let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Toast;
            if !ptr.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let t = Box::from_raw(ptr);
                for f in t.fonts {
                    if !f.is_null() {
                        DeleteObject(f);
                    }
                }
            }
            STACK.with(|s| s.borrow_mut().retain(|&h| h != hwnd));
            reposition();
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------- the notification centre ----------

/// Opens the panel, or closes it when it is already open (the bell click).
pub(crate) unsafe fn toggle_center() {
    let h = PANEL.with(|p| p.get());
    if !h.is_null() && IsWindow(h) != 0 && IsWindowVisible(h) != 0 {
        close_center();
    } else {
        open_center();
    }
}

unsafe fn open_center() {
    if !look::notify() {
        return;
    }
    let cur = PANEL.with(|p| p.get());
    if !cur.is_null() && IsWindow(cur) != 0 {
        crate::force_fg(cur);
        return;
    }
    // The toasts are already in the history: absorb them into the panel so it captures a clean desktop behind it.
    let toasts = STACK.with(|s| s.borrow().clone());
    for h in toasts {
        DestroyWindow(h);
    }
    let (w, h) = (sc(PANEL_W), sc(PANEL_H));
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let x = if vert() { crate::side_x(w) } else { sw - w - sc(8) };
    let y = crate::pop_y(h);
    let class = wide("topbar_notify");
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_LAYERED,
        class.as_ptr(),
        class.as_ptr(),
        WS_POPUP,
        x,
        y,
        w,
        h,
        null_mut(),
        null_mut(),
        GetModuleHandleW(null()),
        null(),
    );
    if hwnd.is_null() {
        return;
    }
    // a layered window without attributes is never shown: the panel is painted with GDI, so it is opaque
    SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, rr(16), rr(16)), 1);
    let px = if look::menu_blur() {
        let radius = sc(look::menu_radius()).max(h / 22).max(sc(10));
        crate::blur::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + h }, radius)
    } else {
        Vec::new()
    };
    PANEL_PX.with(|p| *p.borrow_mut() = px);
    let fonts = [
        make_font(ui_face(), 13, 400),
        make_font(ui_face(), 11, 400),
        make_font(ui_face(), 18, 700),
        make_font(ui_face(), 13, 600),
        make_font(icon_face(), 16, 400),
    ];
    P_FONTS.with(|f| f.set(fonts.map(|h| h as usize)));
    P_SCROLL.set(0);
    P_HOT.set(-1);
    P_OPENED.set(now_ms());
    PANEL.set(hwnd);
    UNREAD.set(false);
    ShowWindow(hwnd, SW_SHOW);
    crate::force_fg(hwnd);
    InvalidateRect(hwnd, null(), 0);
    invalidate_bar();
}

unsafe fn close_center() {
    let h = PANEL.replace(null_mut());
    if !h.is_null() {
        DestroyWindow(h);
    }
    invalidate_bar();
}

unsafe fn make_font(face: &str, px: i32, wt: i32) -> HFONT {
    let f = wide(face);
    CreateFontW(-sc(px), 0, 0, 0, wt, 0, 0, 0, 1, 0, 0, 5, 0, f.as_ptr())
}

fn clear_rect(w: i32) -> RECT {
    RECT { left: w - sc(108), top: sc(12), right: w - sc(PANEL_PAD), bottom: sc(42) }
}

unsafe fn entry_at(x: i32, y: i32, w: i32) -> Option<usize> {
    let top = sc(PANEL_HEAD);
    if x < sc(8) || x > w - sc(8) || y < top || y >= top + sc(PANEL_ROW) * PANEL_ROWS as i32 {
        return None;
    }
    let row = P_SCROLL.get() + ((y - top) / sc(PANEL_ROW)) as usize;
    let n = HISTORY.with(|h| h.borrow().len());
    (row < n).then_some(row)
}

fn relative(at: u64) -> String {
    let s = unsafe { now_ms().saturating_sub(at) } / 1000;
    if s < 45 {
        t!("Только что", "Just now", ja: "たった今", pt: "Agora mesmo", zh: "刚刚").to_string()
    } else if s < 3600 {
        tf!("{} мин назад", "{} min ago", ja: "{} 分前", pt: "Há {} min", zh: "{} 分钟前", s / 60)
    } else if s < 86_400 {
        tf!("{} ч назад", "{} h ago", ja: "{} 時間前", pt: "Há {} h", zh: "{} 小时前", s / 3600)
    } else {
        tf!("{} дн назад", "{} d ago", ja: "{} 日前", pt: "Há {} d", zh: "{} 天前", s / 86_400)
    }
}

unsafe fn paint_panel(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let wdc = BeginPaint(hwnd, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(hwnd, &mut cr);
    if cr.right <= 0 || cr.bottom <= 0 {
        EndPaint(hwnd, &ps);
        return;
    }
    let dc = CreateCompatibleDC(wdc);
    let bmp = CreateCompatibleBitmap(wdc, cr.right, cr.bottom);
    let old = SelectObject(dc, bmp);
    let p = pal();
    let mut drew = false;
    PANEL_PX.with(|px| {
        let px = px.borrow();
        if px.is_empty() {
            return;
        }
        unsafe {
            let keep = SaveDC(dc);
            BeginPath(dc);
            RoundRect(dc, 0, 0, cr.right + 1, cr.bottom + 1, rr(16), rr(16));
            EndPath(dc);
            SelectClipPath(dc, RGN_AND);
            let a = (255 * look::menu_alpha().max(62) as u32 / 100) as u8;
            crate::blur::paint_capture(dc, px.as_slice(), cr, p.bg, a);
            RestoreDC(dc, keep);
            drew = true;
        }
    });
    if !drew {
        fill(dc, cr, p.bg, rr(16));
    }
    SetBkMode(dc, TRANSPARENT as i32);
    let [f_body, f_small, f_title, f_row, _] = P_FONTS.with(|f| f.get()).map(|f| f as HFONT);
    SelectObject(dc, f_title);
    text(dc, t!("Уведомления", "Notifications", ja: "通知", pt: "Notificações", zh: "通知"), RECT { left: sc(PANEL_PAD), top: sc(12), right: clear_rect(cr.right).left - sc(8), bottom: sc(42) }, p.text, DT_LEFT);

    // "Clear all" on the right of the header
    SelectObject(dc, f_small);
    text(dc, t!("Очистить всё", "Clear all", ja: "すべて消去", pt: "Limpar tudo", zh: "全部清除"), clear_rect(cr.right), p.dim, DT_RIGHT);
    fill(dc, RECT { left: sc(PANEL_PAD), top: sc(PANEL_HEAD - 5), right: cr.right - sc(PANEL_PAD), bottom: sc(PANEL_HEAD - 4) }, p.line, 0);

    let items: Vec<HistoryItem> = HISTORY.with(|h| h.borrow().iter().cloned().collect());
    if items.is_empty() {
        SelectObject(dc, f_body);
        text(dc, t!("Уведомлений нет", "No notifications", ja: "通知はありません", pt: "Nenhuma notificação", zh: "没有通知"), RECT { left: sc(PANEL_PAD), top: sc(PANEL_HEAD) + sc(40), right: cr.right - sc(PANEL_PAD), bottom: sc(PANEL_HEAD) + sc(80) }, p.dim, DT_CENTER);
    } else {
        let scroll = P_SCROLL.get().min(items.len().saturating_sub(1));
        let hot = P_HOT.with(|h| h.get());
        for (k, item) in items.iter().skip(scroll).take(PANEL_ROWS).enumerate() {
            let i = scroll + k;
            let top = sc(PANEL_HEAD) + k as i32 * sc(PANEL_ROW);
            let r = RECT { left: sc(8), top, right: cr.right - sc(8), bottom: top + sc(PANEL_ROW) - sc(2) };
            if hot == i as i32 {
                fill(dc, r, p.hover, sc(8));
            }
            let left = r.left + sc(10);
            let right = r.right - sc(10);
            let app: &str = if item.n.app.is_empty() { t!("Уведомление", "Notification", ja: "通知", pt: "Notificação", zh: "通知") } else { item.n.app.as_str() };
            SelectObject(dc, f_small);
            text(dc, app, RECT { left, top: top + sc(6), right: right - sc(58), bottom: top + sc(21) }, p.dim, DT_LEFT);
            text(dc, &relative(item.at), RECT { left: right - sc(58), top: top + sc(6), right, bottom: top + sc(21) }, p.dim, DT_RIGHT);
            SelectObject(dc, f_row);
            text(dc, &item.n.title, RECT { left, top: top + sc(21), right, bottom: top + sc(39) }, p.text, DT_LEFT);
            if !item.n.body.is_empty() {
                SelectObject(dc, f_small);
                text(dc, &item.n.body, RECT { left, top: top + sc(39), right, bottom: top + sc(55) }, mix(p.text, p.dim, 0.45), DT_LEFT);
            }
            if k + 1 < items.len().saturating_sub(scroll).min(PANEL_ROWS) {
                fill(dc, RECT { left, top: r.bottom, right, bottom: r.bottom + 1 }, p.line, 0);
            }
        }
    }

    BitBlt(wdc, 0, 0, cr.right, cr.bottom, dc, 0, 0, SRCCOPY);
    SelectObject(dc, old);
    DeleteObject(bmp);
    DeleteDC(dc);
    EndPaint(hwnd, &ps);
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp & 0xffff) as i16 as i32;
    let y = ((lp >> 16) & 0xffff) as i16 as i32;
    match msg {
        WM_PAINT => {
            paint_panel(hwnd);
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            let mut cr: RECT = zeroed();
            GetClientRect(hwnd, &mut cr);
            let hit = entry_at(x, y, cr.right).map_or(-1, |i| i as i32);
            if P_HOT.with(|h| h.replace(hit)) != hit {
                InvalidateRect(hwnd, null(), 0);
            }
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut tme);
            0
        }
        WM_MOUSELEAVE => {
            if P_HOT.with(|h| h.replace(-1)) != -1 {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let mut cr: RECT = zeroed();
            GetClientRect(hwnd, &mut cr);
            if in_rect(clear_rect(cr.right), x, y) {
                HISTORY.with(|h| h.borrow_mut().clear());
                P_SCROLL.set(0);
                P_HOT.set(-1);
                UNREAD.set(false);
                InvalidateRect(hwnd, null(), 0);
                invalidate_bar();
                return 0;
            }
            if let Some(i) = entry_at(x, y, cr.right) {
                let item = HISTORY.with(|h| h.borrow().get(i).map(|e| e.n.clone()));
                if let Some(n) = item {
                    if n.action_cmd.is_empty() {
                        show(n);
                    } else {
                        run_cmd(&n.action_cmd);
                    }
                    close_center();
                }
                return 0;
            }
            0
        }
        WM_MOUSEWHEEL => {
            let up = (wp >> 16) as i16 > 0;
            let n = HISTORY.with(|h| h.borrow().len());
            let max = n.saturating_sub(PANEL_ROWS);
            let cur = P_SCROLL.get() as i32;
            P_SCROLL.set((cur + if up { -3 } else { 3 }).clamp(0, max as i32) as usize);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_KEYDOWN => {
            if wp as u16 == VK_ESCAPE {
                close_center();
            }
            0
        }
        WM_CHAR => {
            if wp == 27 {
                close_center();
            }
            0
        }
        WM_ACTIVATE if wp & 0xffff == 0 => {
            if now_ms().saturating_sub(P_OPENED.with(|t| t.get())) > 180 {
                close_center();
            }
            0
        }
        WM_DESTROY => {
            PANEL.set(null_mut());
            for f in P_FONTS.with(|f| f.get()) {
                if f != 0 {
                    DeleteObject(f as HFONT);
                }
            }
            P_FONTS.set([0; 5]);
            PANEL_PX.with(|p| p.borrow_mut().clear());
            invalidate_bar();
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

// ---------- actions ----------

/// `"explorer.exe C:\\ruta"` -> the program and its arguments. A quoted program may itself contain spaces.
fn split_cmd(cmd: &str) -> (String, String) {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
    }
    let mut it = cmd.splitn(2, char::is_whitespace);
    let file = it.next().unwrap_or_default().to_string();
    let args = it.next().unwrap_or_default().trim().to_string();
    (file, args)
}

unsafe fn run_cmd(cmd: &str) {
    let (file, args) = split_cmd(cmd);
    if file.is_empty() {
        return;
    }
    let f = wide(&file);
    let a = wide(&args);
    let ap: *const u16 = if args.is_empty() { null() } else { a.as_ptr() };
    ShellExecuteW(null_mut(), wide("open").as_ptr(), f.as_ptr(), ap, null(), SW_SHOWNORMAL);
}
