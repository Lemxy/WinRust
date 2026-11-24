//! A title bar the window draws itself: Windows' own caption goes away, the client area covers the whole window, and the
//! window paints a strip with flat minimize / maximize / close buttons in the WinRust look (the red close of Windows 11).
//! Everything people expect from a caption still works, because the hit test answers with the real HT* codes: dragging,
//! double-click to maximize, Aero Snap and Shake, the window menu (right click or Alt+Space), resizing from every edge,
//! and on Windows 11 the snap layouts that open when the pointer rests on maximize.
//!
//! Use: `install(hwnd)` once after creating the window, `handle(...)` first thing in the window procedure, and
//! `paint_buttons(...)` at the end of WM_PAINT. The window keeps WS_CAPTION / WS_THICKFRAME: they carry the snapping and
//! the animations, only their drawing is gone.
#![allow(unsafe_op_in_unsafe_fn)]
use crate::style::mix;
use std::{cell::RefCell, collections::HashMap};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{Dwm::DwmExtendFrameIntoClientArea, Gdi::*},
    UI::{
        Controls::MARGINS,
        HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi},
        Input::KeyboardAndMouse::{TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent},
        WindowsAndMessaging::*,
    },
};

pub const MIN: u8 = 1;
pub const MAX: u8 = 2;
pub const CLOSE: u8 = 4;

/// What a window's caption has, in 96-DPI pixels: the buttons' height, how far down from the top the window can be
/// dragged (a tall header drags as a whole; at least `height`), and which buttons.
#[derive(Clone, Copy, Debug)]
pub struct Caption {
    pub height: i32,
    pub drag: i32,
    pub buttons: u8,
}

const BTN_W: i32 = 46; // Windows' own caption buttons are this wide
const EDGE: i32 = 6; // the band along the border that resizes

#[derive(Default, Clone, Copy)]
struct State {
    hot: u32,     // HT* code under the pointer, 0 = none
    pressed: u32, // HT* code the button went down on
}

thread_local! { static STATE: RefCell<HashMap<isize, State>> = RefCell::new(HashMap::new()); }

fn state(h: HWND) -> State {
    STATE.with_borrow(|m| m.get(&(h as isize)).copied().unwrap_or_default())
}

fn set_state(h: HWND, s: State) {
    STATE.with_borrow_mut(|m| m.insert(h as isize, s));
}

/// Pixels per 96-DPI pixel for this window.
pub fn scale(h: HWND) -> f32 {
    let dpi = unsafe { GetDpiForWindow(h) };
    if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 }
}

fn px(h: HWND, v: i32) -> i32 {
    (v as f32 * scale(h)).round() as i32
}

/// The caption strip's height in physical pixels.
pub fn height(h: HWND, c: Caption) -> i32 {
    px(h, c.height)
}

/// How much of the caption's right end the buttons take, in physical pixels: keep other things clear of it.
pub fn buttons_width(h: HWND, c: Caption) -> i32 {
    px(h, BTN_W) * c.buttons.count_ones() as i32
}

/// The buttons left to right with their HT* code and box, in client coordinates.
fn buttons(h: HWND, c: Caption) -> Vec<(u32, RECT)> {
    let mut cr: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut cr) };
    let (w, hh) = (px(h, BTN_W), height(h, c));
    let codes: Vec<u32> = [(MIN, HTMINBUTTON), (MAX, HTMAXBUTTON), (CLOSE, HTCLOSE)].into_iter().filter(|&(b, _)| c.buttons & b != 0).map(|(_, ht)| ht).collect();
    let n = codes.len() as i32;
    codes.into_iter().enumerate().map(|(k, ht)| {
        let left = cr.right - (n - k as i32) * w;
        (ht, RECT { left, top: 0, right: left + w, bottom: hh })
    }).collect()
}

/// Once, right after the window exists: the standard caption goes (a frame change runs WM_NCCALCSIZE again) and DWM keeps
/// its shadow, which it draws only while some frame reaches into the client area: one pixel at the top is enough.
pub unsafe fn install(h: HWND) {
    let m = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 };
    DwmExtendFrameIntoClientArea(h, &m);
    SetWindowPos(h, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
}

/// The window procedure's first stop. `interactive(x, y)` says whether a point of the caption strip (client coordinates)
/// belongs to one of the window's own controls there (a tab, a search box), which must not drag the window.
/// Some = handled, return it; None = carry on as usual.
pub unsafe fn handle(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, c: Caption, interactive: impl Fn(i32, i32) -> bool) -> Option<LRESULT> {
    match msg {
        WM_NCCALCSIZE if wp != 0 => {
            // the whole window is client area; a maximized one hangs its frame off the screen, so that part is left out
            if IsZoomed(h) != 0 {
                let p = &mut *(lp as *mut NCCALCSIZE_PARAMS);
                let dpi = GetDpiForWindow(h);
                let fx = GetSystemMetricsForDpi(SM_CXFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                let fy = GetSystemMetricsForDpi(SM_CYFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                let r = &mut p.rgrc[0];
                r.left += fx;
                r.right -= fx;
                r.top += fy;
                r.bottom -= fy;
            }
            Some(0)
        }
        WM_NCHITTEST => Some(hit(h, lp, c, &interactive) as LRESULT),
        WM_NCMOUSEMOVE => {
            let code = wp as u32;
            let on_button = matches!(code, HTMINBUTTON | HTMAXBUTTON | HTCLOSE);
            let mut s = state(h);
            if s.hot != if on_button { code } else { 0 } {
                s.hot = if on_button { code } else { 0 };
                set_state(h, s);
                redraw(h, c);
            }
            if on_button {
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE | TME_NONCLIENT, hwndTrack: h, dwHoverTime: 0 };
                TrackMouseEvent(&mut t);
            }
            None // Windows 11 opens the snap layouts from here
        }
        WM_NCMOUSELEAVE | WM_MOUSEMOVE => {
            let s = state(h);
            if s.hot != 0 || (msg == WM_NCMOUSELEAVE && s.pressed != 0) {
                set_state(h, State { hot: 0, pressed: if msg == WM_NCMOUSELEAVE { 0 } else { s.pressed } });
                redraw(h, c);
            }
            None
        }
        WM_LBUTTONUP => {
            // pressed on a button, let go somewhere in the window: nothing happens, the button lets go
            if state(h).pressed != 0 {
                set_state(h, State::default());
                redraw(h, c);
            }
            None
        }
        WM_NCLBUTTONDOWN | WM_NCLBUTTONDBLCLK if matches!(wp as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) => {
            // not to DefWindowProc: it would draw the classic buttons on top of ours while it tracks the click
            set_state(h, State { hot: wp as u32, pressed: wp as u32 });
            redraw(h, c);
            Some(0)
        }
        WM_NCLBUTTONUP if matches!(wp as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) => {
            let s = state(h);
            set_state(h, State { hot: s.hot, pressed: 0 });
            redraw(h, c);
            if s.pressed == wp as u32 {
                let cmd = match wp as u32 {
                    HTMINBUTTON => SC_MINIMIZE,
                    HTMAXBUTTON if IsZoomed(h) != 0 => SC_RESTORE,
                    HTMAXBUTTON => SC_MAXIMIZE,
                    _ => SC_CLOSE,
                };
                PostMessageW(h, WM_SYSCOMMAND, cmd as usize, 0);
            }
            Some(0)
        }
        WM_NCRBUTTONDOWN | WM_NCRBUTTONUP if matches!(wp as u32, HTMINBUTTON | HTMAXBUTTON | HTCLOSE) => Some(0),
        WM_ACTIVATE | WM_SIZE => {
            redraw(h, c); // dimmed when inactive; maximize turns into restore
            None
        }
        WM_NCDESTROY => {
            STATE.with_borrow_mut(|m| m.remove(&(h as isize)));
            None
        }
        _ => None,
    }
}

fn hit(h: HWND, lp: LPARAM, c: Caption, interactive: &dyn Fn(i32, i32) -> bool) -> u32 {
    let mut p = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
    unsafe { ScreenToClient(h, &mut p) };
    let mut cr: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(h, &mut cr) };
    let zoomed = unsafe { IsZoomed(h) != 0 };
    let sizable = unsafe { GetWindowLongW(h, GWL_STYLE) } as u32 & WS_THICKFRAME != 0;
    if sizable && !zoomed {
        let e = px(h, EDGE);
        let (l, r, t, b) = (p.x < e, p.x >= cr.right - e, p.y < e, p.y >= cr.bottom - e);
        let code = match (l, r, t, b) {
            (true, _, true, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, ..) => HTLEFT,
            (_, true, ..) => HTRIGHT,
            (_, _, true, _) => HTTOP,
            (.., true) => HTBOTTOM,
            _ => 0,
        };
        // the buttons reach the top edge: there they win over the resize band, as in Windows
        let on_button = buttons(h, c).iter().any(|(_, b)| p.x >= b.left && p.x < b.right && p.y < b.bottom);
        if code != 0 && !(on_button && code == HTTOP) {
            return code;
        }
    }
    if let Some((ht, _)) = buttons(h, c).into_iter().find(|(_, b)| p.x >= b.left && p.x < b.right && p.y >= b.top && p.y < b.bottom) {
        return ht;
    }
    if p.y >= 0 && p.y < px(h, c.drag.max(c.height)) && !interactive(p.x, p.y) {
        return HTCAPTION;
    }
    HTCLIENT
}

fn redraw(h: HWND, c: Caption) {
    let bs = buttons(h, c);
    if let (Some(first), Some(last)) = (bs.first(), bs.last()) {
        let r = RECT { left: first.1.left, top: 0, right: last.1.right, bottom: last.1.bottom };
        // a child window may lie over the caption (it then paints the buttons itself, see `paint_buttons`)
        unsafe { RedrawWindow(h, &r, std::ptr::null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN) };
    }
}

/// The buttons, drawn into `dc` (the window's back buffer) over `bg`, the colour the caption strip has under them;
/// `fg` is the glyph colour. Hover and press use Windows 11's shades; close turns red. A child window lying over the
/// caption at the client's top left corner calls this with its own DC and the top-level `h`: the boxes are the same.
/// It also answers WM_NCHITTEST with HTTRANSPARENT over the caption's free parts, so they reach the window.
pub unsafe fn paint_buttons(dc: HDC, h: HWND, c: Caption, bg: u32, fg: u32) {
    let s = state(h);
    let active = GetForegroundWindow() == h || GetAncestor(GetForegroundWindow(), GA_ROOTOWNER) == h;
    let face: Vec<u16> = crate::style::icon_face().encode_utf16().chain([0]).collect();
    let font = CreateFontW(-px(h, 10), 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, face.as_ptr());
    let old = SelectObject(dc, font);
    SetBkMode(dc, TRANSPARENT as i32);
    for (ht, r) in buttons(h, c) {
        let (hot, down) = (s.hot == ht, s.pressed == ht && s.hot == ht);
        let (fill, glyph_c) = match (ht, hot, down) {
            (HTCLOSE, true, false) => (Some(rgb(196, 43, 28)), rgb(255, 255, 255)),
            (HTCLOSE, true, true) => (Some(rgb(200, 64, 49)), rgb(255, 255, 255)),
            (_, true, false) => (Some(mix(bg, fg, 0.10)), fg),
            (_, true, true) => (Some(mix(bg, fg, 0.06)), mix(fg, bg, 0.2)),
            _ => (None, if active { fg } else { mix(fg, bg, 0.45) }),
        };
        if let Some(f) = fill {
            let br = CreateSolidBrush(f);
            FillRect(dc, &r, br);
            DeleteObject(br);
        }
        let glyph = match ht {
            HTMINBUTTON => "\u{E921}",
            HTMAXBUTTON if IsZoomed(h) != 0 => "\u{E923}",
            HTMAXBUTTON => "\u{E922}",
            _ => "\u{E8BB}",
        };
        let mut t: Vec<u16> = glyph.encode_utf16().collect();
        let mut rr = r;
        SetTextColor(dc, glyph_c);
        DrawTextW(dc, t.as_mut_ptr(), t.len() as i32, &mut rr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
    SelectObject(dc, old);
    DeleteObject(font);
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
