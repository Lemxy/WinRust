//! The whole text of a label that did not fit: a window that paints its own text notes, while painting, every text it had
//! to cut short (`cut`), and the pointer resting on one for a moment shows it in full in a small popup. The text is
//! whatever the label says, so it is in the language the label is in.
//!
//! Use: `clear()` at the start of WM_PAINT, `cut(...)` for each shortened text, `track(hwnd, x, y)` on WM_MOUSEMOVE,
//! `timer(hwnd, wp)` on WM_TIMER, `hide()` on WM_MOUSELEAVE and clicks.
#![allow(unsafe_op_in_unsafe_fn)]
use std::cell::{Cell, RefCell};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::*},
};

const TIMER: usize = 0x7171; // the owner's timer id for the delay before showing
const DELAY_MS: u32 = 450;
const MAX_W: i32 = 420; // the popup wraps at this width (96-DPI px)

thread_local! {
    static CUT: RefCell<Vec<(RECT, String)>> = const { RefCell::new(Vec::new()) }; // this paint's shortened texts
    static AT: Cell<Option<usize>> = const { Cell::new(None) }; // the one under the pointer
    static POP: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static TEXT: RefCell<String> = const { RefCell::new(String::new()) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A new paint starts: the old list goes.
pub fn clear() {
    CUT.with_borrow_mut(|c| c.clear());
}

/// `text` was drawn into `r` (client pixels) with `flags` in `dc`'s current font: when it did not fit, it is noted.
/// Call it right after drawing; it returns whether the text was cut.
pub unsafe fn cut(dc: HDC, text: &str, r: RECT, flags: u32) -> bool {
    let mut c = r;
    let w = wide(text);
    // the same width, no ellipsis, no vertical centring: how big the whole text would be
    DrawTextW(dc, w.as_ptr(), -1, &mut c, (flags & !(DT_END_ELLIPSIS | DT_PATH_ELLIPSIS | DT_WORD_ELLIPSIS | DT_VCENTER | DT_BOTTOM)) | DT_CALCRECT | DT_NOPREFIX);
    let cut = c.right > r.right + 1 || c.bottom > r.bottom + 1;
    if cut && !text.trim().is_empty() {
        CUT.with_borrow_mut(|v| v.push((r, text.to_string())));
    }
    cut
}

/// The pointer moved in the owner's client area: a noted text under it shows after a short rest.
pub unsafe fn track(owner: HWND, x: i32, y: i32) {
    let hit = CUT.with_borrow(|v| v.iter().position(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom));
    if hit == AT.get() {
        return;
    }
    hide();
    AT.set(hit);
    if hit.is_some() {
        SetTimer(owner, TIMER, DELAY_MS, None);
    }
}

/// WM_TIMER of the owner: true when it was ours (then nothing else is to be done with it).
pub unsafe fn timer(owner: HWND, wp: WPARAM) -> bool {
    if wp != TIMER {
        return false;
    }
    KillTimer(owner, TIMER);
    let Some(text) = AT.get().and_then(|i| CUT.with_borrow(|v| v.get(i).map(|t| t.1.clone()))) else { return true };
    show(owner, &text);
    true
}

/// The popup goes (pointer left, a click, the page changed).
pub fn hide() {
    AT.set(None);
    let p = POP.get();
    if !p.is_null() {
        unsafe { ShowWindow(p, SW_HIDE) };
    }
}

fn scale(h: HWND) -> f32 {
    let d = unsafe { GetDpiForWindow(h) };
    if d == 0 { 1.0 } else { d as f32 / 96.0 }
}

unsafe fn font(s: f32) -> HFONT {
    CreateFontW(-((12.5 * s) as i32), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide(crate::style::ui_face()).as_ptr())
}

unsafe fn show(owner: HWND, text: &str) {
    let mut pop = POP.get();
    if pop.is_null() {
        let class = wide("WinRustTip");
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: GetModuleHandleW(std::ptr::null()), lpszClassName: class.as_ptr(), hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW), ..std::mem::zeroed() };
        RegisterClassW(&wc);
        pop = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE, class.as_ptr(), std::ptr::null(), WS_POPUP, 0, 0, 1, 1, owner, std::ptr::null_mut(), wc.hInstance, std::ptr::null());
        POP.set(pop);
    }
    TEXT.with_borrow_mut(|t| *t = text.to_string());
    // size: the text wrapped at MAX_W, plus padding
    let s = scale(owner);
    let pad = (8.0 * s) as i32;
    let dc = GetDC(pop);
    let f = font(s);
    let of = SelectObject(dc, f);
    let w = wide(text);
    let mut r = RECT { left: 0, top: 0, right: (MAX_W as f32 * s) as i32, bottom: 0 };
    DrawTextW(dc, w.as_ptr(), -1, &mut r, DT_CALCRECT | DT_WORDBREAK | DT_NOPREFIX);
    SelectObject(dc, of);
    DeleteObject(f);
    ReleaseDC(pop, dc);
    let (pw, ph) = (r.right + 2 * pad, r.bottom + 2 * pad);
    // under the pointer, kept on the screen it is on
    let mut cur = POINT { x: 0, y: 0 };
    GetCursorPos(&mut cur);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromPoint(cur, MONITOR_DEFAULTTONEAREST), &mut mi);
    let wa = mi.rcWork;
    let x = (cur.x + (12.0 * s) as i32).min(wa.right - pw).max(wa.left);
    let mut y = cur.y + (20.0 * s) as i32;
    if y + ph > wa.bottom {
        y = cur.y - ph - (8.0 * s) as i32;
    }
    SetWindowPos(pop, HWND_TOPMOST, x, y, pw, ph, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    InvalidateRect(pop, std::ptr::null(), 0);
}

unsafe extern "system" fn proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(h, &mut r);
            let p = crate::style::pal();
            let (line, fill) = (CreateSolidBrush(p.line), CreateSolidBrush(crate::style::mix(p.bg, p.text, 0.06)));
            FillRect(dc, &r, line);
            let inner = RECT { left: 1, top: 1, right: r.right - 1, bottom: r.bottom - 1 };
            FillRect(dc, &inner, fill);
            DeleteObject(line);
            DeleteObject(fill);
            let s = scale(h);
            let pad = (8.0 * s) as i32;
            let f = font(s);
            let of = SelectObject(dc, f);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, p.text);
            let mut tr = RECT { left: pad, top: pad, right: r.right - pad, bottom: r.bottom - pad };
            TEXT.with_borrow(|t| {
                let w = wide(t);
                DrawTextW(dc, w.as_ptr(), -1, &mut tr, DT_WORDBREAK | DT_NOPREFIX);
            });
            SelectObject(dc, of);
            DeleteObject(f);
            EndPaint(h, &ps);
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCHITTEST => HTTRANSPARENT as LRESULT, // the pointer goes through it to the window below
        _ => DefWindowProcW(h, m, wp, lp),
    }
}
