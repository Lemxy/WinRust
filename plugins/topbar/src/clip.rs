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
    Graphics::{
        Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute},
        Gdi::*,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

/// The file name of the last saved region, for the bar's notice (taken by `clip_notice`).
pub static DONE: Mutex<Option<String>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

struct St {
    orig: HBITMAP,
    dim: HBITMAP,
    buf: HBITMAP,
    w: i32,
    h: i32,
    wins: Vec<RECT>,
    hovered: Option<RECT>,
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

fn win_at(wins: &[RECT], pt: (i32, i32)) -> Option<RECT> {
    wins.iter().find(|r| pt.0 >= r.left && pt.0 < r.right && pt.1 >= r.top && pt.1 < r.bottom).copied()
}

fn sel_of(from: Option<(i32, i32)>, to: (i32, i32), hovered: Option<RECT>) -> Option<RECT> {
    match from {
        Some(f) if (f.0 - to.0).abs() >= 3 || (f.1 - to.1).abs() >= 3 => Some(rect(f, to)),
        _ => hovered,
    }
}

fn sel() -> Option<RECT> {
    ST.with_borrow(|s| s.as_ref().and_then(|s| sel_of(s.from, s.to, s.hovered)))
}

unsafe fn collect_wins(vx: i32, vy: i32, vw: i32, vh: i32) -> Vec<RECT> {
    struct Ctx {
        vx: i32,
        vy: i32,
        vw: i32,
        vh: i32,
        out: Vec<RECT>,
    }
    unsafe extern "system" fn cb(w: HWND, lp: LPARAM) -> i32 {
        let ctx = unsafe { &mut *(lp as *mut Ctx) };
        if unsafe { IsWindowVisible(w) == 0 || IsIconic(w) != 0 } {
            return 1;
        }
        let ex = unsafe { GetWindowLongW(w, GWL_EXSTYLE) } as u32;
        if ex & WS_EX_TOOLWINDOW != 0 {
            return 1;
        }
        let mut cloaked = 0u32;
        if unsafe { DwmGetWindowAttribute(w, DWMWA_CLOAKED as u32, &mut cloaked as *mut u32 as _, size_of::<u32>() as u32) } == 0 && cloaked != 0 {
            return 1;
        }
        let mut r: RECT = unsafe { std::mem::zeroed() };
        if unsafe { DwmGetWindowAttribute(w, DWMWA_EXTENDED_FRAME_BOUNDS as u32, &mut r as *mut RECT as _, size_of::<RECT>() as u32) } != 0
            && unsafe { GetWindowRect(w, &mut r) } == 0
        {
            return 1;
        }
        let r = RECT {
            left: (r.left - ctx.vx).clamp(0, ctx.vw),
            top: (r.top - ctx.vy).clamp(0, ctx.vh),
            right: (r.right - ctx.vx).clamp(0, ctx.vw),
            bottom: (r.bottom - ctx.vy).clamp(0, ctx.vh),
        };
        if r.right - r.left >= 3 && r.bottom - r.top >= 3 {
            ctx.out.push(r);
        }
        1
    }
    let mut ctx = Ctx { vx, vy, vw, vh, out: Vec::new() };
    EnumWindows(Some(cb), &mut ctx as *mut Ctx as LPARAM);
    ctx.out
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
    let (orig, dim, buf, black) = (CreateCompatibleBitmap(scr, w, h), CreateCompatibleBitmap(scr, w, h), CreateCompatibleBitmap(scr, w, h), CreateCompatibleBitmap(scr, 1, 1));
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
    let wins = collect_wins(vx, vy, w, h);
    let mut cur_pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut cur_pt);
    let hovered = win_at(&wins, (cur_pt.x - vx, cur_pt.y - vy));
    ST.set(Some(St { orig, dim, buf, w, h, wins, hovered, from: None, to: (0, 0) }));

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
    DeleteObject(buf);
    ST.set(None);
}

unsafe fn paint(hw: HWND) {
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let hdc = BeginPaint(hw, &mut ps);
    let p = ps.rcPaint;
    let (pw, ph) = (p.right - p.left, p.bottom - p.top);
    let (dim, orig, buf) = ST.with_borrow(|s| s.as_ref().map(|s| (s.dim, s.orig, s.buf))).unwrap_or((null_mut(), null_mut(), null_mut()));
    let (src_dc, buf_dc) = (CreateCompatibleDC(hdc), CreateCompatibleDC(hdc));
    let (old_src, old_buf) = (SelectObject(src_dc, dim), SelectObject(buf_dc, buf));
    BitBlt(buf_dc, p.left, p.top, pw, ph, src_dc, p.left, p.top, SRCCOPY);
    if let Some(r) = sel() {
        // the bright part: the selection within the dirty area
        let (l, t, rr, b) = (r.left.max(p.left), r.top.max(p.top), r.right.min(p.right), r.bottom.min(p.bottom));
        SelectObject(src_dc, orig);
        if rr > l && b > t {
            BitBlt(buf_dc, l, t, rr - l, b - t, src_dc, l, t, SRCCOPY);
        }
        FrameRect(buf_dc, &RECT { left: r.left - 1, top: r.top - 1, right: r.right + 1, bottom: r.bottom + 1 }, GetStockObject(WHITE_BRUSH) as HBRUSH);
    }
    BitBlt(hdc, p.left, p.top, pw, ph, buf_dc, p.left, p.top, SRCCOPY);
    SelectObject(src_dc, old_src);
    SelectObject(buf_dc, old_buf);
    DeleteDC(src_dc);
    DeleteDC(buf_dc);
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
            let before = sel();
            let dragging = wp & 1 != 0 && ST.with_borrow(|s| s.as_ref().is_some_and(|s| s.from.is_some()));
            ST.with_borrow_mut(|s| {
                if let Some(s) = s {
                    let clamped = (pt.0.clamp(0, s.w), pt.1.clamp(0, s.h));
                    if dragging {
                        s.to = clamped;
                    } else {
                        s.hovered = win_at(&s.wins, clamped);
                    }
                }
            });
            let key = |r: Option<RECT>| r.map(|r| (r.left, r.top, r.right, r.bottom));
            if key(sel()) != key(before) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_at_picks_topmost_window_and_sel_prefers_drag() {
        let wins = [
            RECT { left: 100, top: 100, right: 300, bottom: 300 },
            RECT { left: 0, top: 0, right: 500, bottom: 500 },
        ];
        assert_eq!(win_at(&wins, (150, 150)).map(|r| r.right), Some(300));
        assert_eq!(win_at(&wins, (50, 50)).map(|r| r.right), Some(500));
        assert!(win_at(&wins, (600, 600)).is_none());

        let hov = Some(wins[0]);
        assert_eq!(sel_of(Some((150, 150)), (151, 151), hov).map(|r| r.right), Some(300)); // click picks hovered window
        assert_eq!(sel_of(Some((10, 10)), (80, 90), hov).map(|r| (r.left, r.right)), Some((10, 80))); // drag overrides
    }
}
