//! Win+Shift+S: the region clipper. The desktop is copied once, shown dimmed on a full-screen window, and the rectangle the
//! user drags stays bright. On release that region goes to the clipboard and to Pictures\Screenshots (like PrtSc does);
//! Esc or the right button cancels. It has its own thread and message loop, so a busy bar cannot stall it.
use std::{
    cell::RefCell,
    ptr::{null, null_mut},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

/// The file name of the last saved region, for the bar's notice (taken by `clip_notice`).
pub static DONE: Mutex<Option<String>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

struct St {
    orig: HBITMAP,
    dim: HBITMAP,
    w: i32,
    h: i32,
    from: Option<(i32, i32)>,
    to: (i32, i32),
}

thread_local! {
    static ST: RefCell<Option<St>> = const { RefCell::new(None) };
}

/// The rectangle between two corners, whichever way it was dragged.
fn rect(a: (i32, i32), b: (i32, i32)) -> RECT {
    RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0), bottom: a.1.max(b.1) }
}

fn sel() -> Option<RECT> {
    ST.with_borrow(|s| s.as_ref().and_then(|s| s.from.map(|f| rect(f, s.to))))
}

pub fn start() {
    if BUSY.swap(true, Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        unsafe { run() };
        BUSY.store(false, Relaxed);
    });
}

unsafe fn run() {
    // physical pixels on every monitor: with system awareness a secondary monitor at another scale is cut short (#77)
    windows_sys::Win32::UI::HiDpi::SetThreadDpiAwarenessContext(-4isize as _);
    let (vx, vy, w, h) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
    if w <= 0 || h <= 0 {
        return;
    }
    let scr = GetDC(null_mut());
    let (mem, dd, one) = (CreateCompatibleDC(scr), CreateCompatibleDC(scr), CreateCompatibleDC(scr));
    let (orig, dim, black) = (CreateCompatibleBitmap(scr, w, h), CreateCompatibleBitmap(scr, w, h), CreateCompatibleBitmap(scr, 1, 1));
    let (o1, o2, o3) = (SelectObject(mem, orig), SelectObject(dd, dim), SelectObject(one, black));
    BitBlt(mem, 0, 0, w, h, scr, vx, vy, SRCCOPY | 0x4000_0000); // CAPTUREBLT: layered windows too
    BitBlt(dd, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    PatBlt(one, 0, 0, 1, 1, BLACKNESS);
    AlphaBlend(dd, 0, 0, w, h, one, 0, 0, 1, 1, BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 110, AlphaFormat: 0 });
    SelectObject(mem, o1);
    SelectObject(dd, o2);
    SelectObject(one, o3);
    for d in [mem, dd, one] {
        DeleteDC(d);
    }
    DeleteObject(black);
    ReleaseDC(null_mut(), scr);
    ST.set(Some(St { orig, dim, w, h, from: None, to: (0, 0) }));

    let (cls, hi) = (super::wide("WinRustClip"), GetModuleHandleW(null()));
    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.lpfnWndProc = Some(proc);
    wc.hInstance = hi;
    wc.hCursor = LoadCursorW(null_mut(), IDC_CROSS);
    wc.lpszClassName = cls.as_ptr();
    RegisterClassW(&wc); // 0 when it exists already (a second clip): fine
    let hw = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, cls.as_ptr(), null(), WS_POPUP | WS_VISIBLE, vx, vy, w, h, null_mut(), null_mut(), hi, null());
    if !hw.is_null() {
        super::force_fg(hw);
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    DeleteObject(orig);
    DeleteObject(dim);
    ST.set(None);
}

unsafe fn paint(hw: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hw, &mut ps);
    let p = ps.rcPaint;
    let mem = CreateCompatibleDC(hdc);
    let (dim, orig) = ST.with_borrow(|s| s.as_ref().map(|s| (s.dim, s.orig))).unwrap_or((null_mut(), null_mut()));
    let old = SelectObject(mem, dim);
    BitBlt(hdc, p.left, p.top, p.right - p.left, p.bottom - p.top, mem, p.left, p.top, SRCCOPY);
    if let Some(r) = sel() {
        // the bright part: the selection within the dirty area
        let (l, t, rr, b) = (r.left.max(p.left), r.top.max(p.top), r.right.min(p.right), r.bottom.min(p.bottom));
        SelectObject(mem, orig);
        if rr > l && b > t {
            BitBlt(hdc, l, t, rr - l, b - t, mem, l, t, SRCCOPY);
        }
        FrameRect(hdc, &RECT { left: r.left - 1, top: r.top - 1, right: r.right + 1, bottom: r.bottom + 1 }, GetStockObject(WHITE_BRUSH) as HBRUSH);
    }
    SelectObject(mem, old);
    DeleteDC(mem);
    EndPaint(hw, &ps);
}

/// Repaint what the selection covered before and covers now.
unsafe fn dirty(hw: HWND, before: Option<RECT>) {
    let now = sel();
    let r = match (before, now) {
        (Some(a), Some(b)) => RECT { left: a.left.min(b.left), top: a.top.min(b.top), right: a.right.max(b.right), bottom: a.bottom.max(b.bottom) },
        (Some(a), None) | (None, Some(a)) => a,
        _ => return,
    };
    InvalidateRect(hw, &RECT { left: r.left - 3, top: r.top - 3, right: r.right + 3, bottom: r.bottom + 3 }, 0);
}

/// Cuts the selected region out of the desktop copy and hands it on.
unsafe fn finish(hw: HWND) {
    let (r, orig) = match (sel(), ST.with_borrow(|s| s.as_ref().map(|s| s.orig))) {
        (Some(r), Some(o)) => (r, o),
        _ => return DestroyWindow_(hw),
    };
    let (rw, rh) = (r.right - r.left, r.bottom - r.top);
    if rw < 3 || rh < 3 {
        return DestroyWindow_(hw); // a click, not a drag
    }
    let scr = GetDC(null_mut());
    let (src, dst) = (CreateCompatibleDC(scr), CreateCompatibleDC(scr));
    let piece = CreateCompatibleBitmap(scr, rw, rh);
    let (o1, o2) = (SelectObject(src, orig), SelectObject(dst, piece));
    BitBlt(dst, 0, 0, rw, rh, src, r.left, r.top, SRCCOPY);
    SelectObject(src, o1);
    SelectObject(dst, o2);
    DeleteDC(src);
    DeleteDC(dst);
    ReleaseDC(null_mut(), scr);
    DestroyWindow_(hw); // the dimmed screen goes first; saving takes a moment
    if let Some(name) = super::save_bitmap(piece) {
        if let Ok(mut d) = DONE.lock() {
            *d = Some(name);
        }
        PostMessageW(super::BAR.load(Relaxed) as HWND, super::SNIP, 2, 0);
    }
}

#[allow(non_snake_case)]
unsafe fn DestroyWindow_(hw: HWND) {
    DestroyWindow(hw);
}

unsafe extern "system" fn proc(hw: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let pt = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hw);
            0
        }
        WM_LBUTTONDOWN => {
            ST.with_borrow_mut(|s| {
                if let Some(s) = s {
                    s.from = Some(pt);
                    s.to = pt;
                }
            });
            SetCapture(hw);
            0
        }
        WM_MOUSEMOVE => {
            if wp & 1 != 0 && sel().is_some() { // MK_LBUTTON
                let before = sel();
                ST.with_borrow_mut(|s| {
                    if let Some(s) = s {
                        s.to = (pt.0.clamp(0, s.w), pt.1.clamp(0, s.h));
                    }
                });
                dirty(hw, before);
            }
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            finish(hw);
            0
        }
        WM_RBUTTONDOWN => {
            DestroyWindow(hw);
            0
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            DestroyWindow(hw);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hw, msg, wp, lp),
    }
}
