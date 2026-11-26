//! The selection marquee ("lazo") of the desktop and of the file list: a light whitish wash inside a crisp frame,
//! instead of the control's opaque blue rectangle.
//!
//! It is a *child of the window that owns the list* (the desktop window for the icons, the file window for the list), so
//! it can never paint over another application, and it is presented with `UpdateLayeredWindow`: per-pixel alpha is the
//! only way to get a translucent fill *and* a sharp frame (a colour key can only cut pixels out, never fade them).
//! The drag itself is ours too: while it lasts the mouse is captured by the caller (`catcher_proc` on the desktop,
//! `list_proc` in the file window), which forwards the screen position here; on release `select` marks every list item
//! whose rectangle meets the band, like the shell does.
#![allow(unsafe_op_in_unsafe_fn)]

use std::cell::Cell;
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION, BeginPaint,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EndPaint, GetDC, HDC,
        PAINTSTRUCT, ReleaseDC, SelectObject,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, Input::KeyboardAndMouse::GetKeyState, WindowsAndMessaging::*},
};

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

thread_local! {
    static BAND: Cell<isize> = const { Cell::new(0) };
    /// The list the band belongs to (it is also the window the band sits right above).
    static LIST: Cell<isize> = const { Cell::new(0) };
    /// Clamp the band to the list rectangle: the file list must not paint its marquee over the toolbar, while the
    /// desktop list only covers the primary monitor and its band has to reach the others.
    static CLIP: Cell<bool> = const { Cell::new(false) };
    static ANCHOR: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
    /// The band's surface: a 32-bit DIB that only grows while a drag lasts, freed when it ends.
    static BAND_DC: Cell<isize> = const { Cell::new(0) };
    static BAND_BMP: Cell<isize> = const { Cell::new(0) };
    static BAND_OLD: Cell<isize> = const { Cell::new(0) };
    static BAND_BITS: Cell<isize> = const { Cell::new(0) };
    static BAND_ALLOC: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
}

pub fn dragging() -> bool {
    DRAGGING.with(|d| d.get())
}

/// Starts a drag at a screen position. `clip` = keep the band inside the list rectangle (the file window).
pub unsafe fn begin(list: HWND, x: i32, y: i32, clip: bool) {
    LIST.set(list as isize);
    CLIP.set(clip);
    ANCHOR.set((x, y));
    DRAGGING.set(true);
}

/// The band window, a child of the list's parent.
unsafe fn window(pm: HWND) -> HWND {
    let b = BAND.get() as HWND;
    if !b.is_null() && IsWindow(b) != 0 && GetParent(b) == pm {
        return b;
    }
    if !b.is_null() && IsWindow(b) != 0 {
        DestroyWindow(b); // the owner was rebuilt: the child would be orphaned
        BAND.set(0);
    }
    let b = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
        w("WinRustBand").as_ptr(),
        std::ptr::null(),
        WS_CHILD,
        0,
        0,
        1,
        1,
        pm,
        0 as _,
        GetModuleHandleW(std::ptr::null()),
        std::ptr::null(),
    );
    if b.is_null() {
        return b;
    }
    BAND.set(b as isize);
    b
}

/// Frees the band's surface (end of a drag, or before growing it).
unsafe fn free() {
    let dc = BAND_DC.get() as HDC;
    if !dc.is_null() {
        if BAND_OLD.get() != 0 {
            SelectObject(dc, BAND_OLD.get() as _);
        }
        if BAND_BMP.get() != 0 {
            DeleteObject(BAND_BMP.get() as _);
        }
        DeleteDC(dc);
    }
    BAND_DC.set(0);
    BAND_BMP.set(0);
    BAND_OLD.set(0);
    BAND_BITS.set(0);
    BAND_ALLOC.set((0, 0));
}

/// The surface for a `w`x`h` band, reused (grow only) for the whole drag. `(null, null)` on failure.
unsafe fn buf(w: i32, h: i32) -> (HDC, *mut u32) {
    let (aw, ah) = BAND_ALLOC.get();
    if !(BAND_DC.get() as HDC).is_null() && aw >= w && ah >= h {
        return (BAND_DC.get() as HDC, BAND_BITS.get() as *mut u32);
    }
    free();
    let (nw, nh) = (w.max(64), h.max(64));
    let screen = GetDC(std::ptr::null_mut());
    let dc = CreateCompatibleDC(screen);
    ReleaseDC(std::ptr::null_mut(), screen);
    if dc.is_null() {
        return (std::ptr::null_mut(), std::ptr::null_mut());
    }
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: nw,
            biHeight: -nh, // top-down: row 0 is the top of the band
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..std::mem::zeroed()
        },
        ..std::mem::zeroed()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if bmp.is_null() || bits.is_null() {
        DeleteDC(dc);
        return (std::ptr::null_mut(), std::ptr::null_mut());
    }
    let old = SelectObject(dc, bmp as _);
    BAND_DC.set(dc as isize);
    BAND_BMP.set(bmp as isize);
    BAND_OLD.set(old as isize);
    BAND_BITS.set(bits as isize);
    BAND_ALLOC.set((nw, nh));
    (dc, bits as *mut u32)
}

/// Abandons the drag without selecting anything (capture lost: Alt+Tab, a menu, the window closing).
pub unsafe fn cancel() {
    DRAGGING.set(false);
    hide();
}

pub unsafe fn hide() {
    let b = BAND.get() as HWND;
    if !b.is_null() && IsWindow(b) != 0 {
        ShowWindow(b, SW_HIDE);
    }
    free();
}

/// Paints and shows the band for a rectangle in screen coordinates.
unsafe fn show(l: i32, t: i32, r: i32, b: i32) {
    let list = LIST.get() as HWND;
    let pm = GetParent(list);
    if pm.is_null() {
        return;
    }
    let (mut l, mut t, mut r, mut b) = (l, t, r, b);
    if CLIP.get() {
        // inside the list only: the file window's toolbar and status line are siblings above it, and in details view
        // the column header (a child of the list) must stay clear too
        let mut o: RECT = std::mem::zeroed();
        GetWindowRect(list, &mut o);
        let hdr = SendMessageW(list, LVM_GETHEADER, 0, 0) as HWND;
        let mut top = o.top;
        if !hdr.is_null() && IsWindow(hdr) != 0 {
            let mut hr: RECT = std::mem::zeroed();
            GetWindowRect(hdr, &mut hr);
            top = hr.bottom;
        }
        l = l.max(o.left);
        t = t.max(top);
        r = r.min(o.right);
        b = b.min(o.bottom);
    }
    let (w, h) = (r - l, b - t);
    if w < 3 || h < 3 {
        let bw = BAND.get() as HWND;
        if !bw.is_null() && IsWindow(bw) != 0 {
            ShowWindow(bw, SW_HIDE);
        }
        return;
    }
    let bw = window(pm);
    if bw.is_null() {
        return;
    }
    // Right above the list, so the wash covers the items too; being a child of the owning window, every application
    // window still sits above it.
    SetWindowPos(bw, list, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
    let (dc, bits) = buf(w, h);
    if dc.is_null() {
        return;
    }
    let stride = BAND_ALLOC.get().0 as usize;
    let (uw, uh) = (w as usize, h as usize);
    // premultiplied white: (a, a, a, a) in BGRA order
    let tint = |a: u32| (a << 24) | (a << 16) | (a << 8) | a;
    let fill = tint(0x24); // ~14 % white: a visible wash, still see-through
    let inner = tint(0xD8); // the bright frame line
    let outer = 0x60u32 << 24; // black, ~38 %: keeps the frame visible on white backgrounds
    for y in 0..uh {
        let row = bits.add(y * stride);
        for x in 0..uw {
            *row.add(x) = fill;
        }
    }
    for x in 0..uw {
        *bits.add(x) = outer;
        *bits.add((uh - 1) * stride + x) = outer;
    }
    for y in 0..uh {
        *bits.add(y * stride) = outer;
        *bits.add(y * stride + uw - 1) = outer;
    }
    if uw > 3 && uh > 3 {
        for x in 1..uw - 1 {
            *bits.add(stride + x) = inner;
            *bits.add((uh - 2) * stride + x) = inner;
        }
        for y in 1..uh - 1 {
            *bits.add(y * stride + 1) = inner;
            *bits.add(y * stride + uw - 2) = inner;
        }
    }
    // UpdateLayeredWindow positions a *child* relative to its parent's client area, and the owner can start at a
    // negative screen coordinate when a monitor sits left of / above the primary one.
    let mut pr: RECT = std::mem::zeroed();
    GetWindowRect(pm, &mut pr);
    let screen = GetDC(std::ptr::null_mut());
    let dst = POINT { x: l - pr.left, y: t - pr.top };
    let size = SIZE { cx: w, cy: h };
    let src = POINT { x: 0, y: 0 };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    UpdateLayeredWindow(bw, screen, &dst, &size, dc, &src, 0, &blend, ULW_ALPHA);
    ReleaseDC(std::ptr::null_mut(), screen);
}

/// Follows the cursor while a drag lasts (screen coordinates).
pub unsafe fn track(x: i32, y: i32) {
    if !DRAGGING.get() {
        return;
    }
    let (ax, ay) = ANCHOR.get();
    show(ax.min(x), ay.min(y), ax.max(x), ay.max(y));
}

/// Ends the drag: hides the band and selects the items it covers. Returns true if a drag was running.
pub unsafe fn finish(x: i32, y: i32) -> bool {
    if !DRAGGING.get() {
        return false;
    }
    DRAGGING.set(false);
    hide();
    let list = LIST.get() as HWND;
    if list.is_null() || IsWindow(list) == 0 {
        return true;
    }
    let (ax, ay) = ANCHOR.get();
    let band = RECT { left: ax.min(x), top: ay.min(y), right: ax.max(x), bottom: ay.max(y) };
    let additive = GetKeyState(0x11) < 0; // VK_CONTROL
    let empty = band.right - band.left <= 3 && band.bottom - band.top <= 3;
    if !empty || !additive {
        select(list, band, additive);
    }
    true
}

/// Marks every item whose rectangle meets `band` (screen coordinates). `additive` keeps what was selected (Ctrl).
unsafe fn select(list: HWND, band: RECT, additive: bool) {
    if !additive {
        let mut it: LVITEMW = std::mem::zeroed();
        it.stateMask = LVIS_SELECTED;
        it.state = 0;
        SendMessageW(list, LVM_SETITEMSTATE, usize::MAX, &mut it as *mut _ as isize);
    }
    let empty = band.right - band.left <= 3 && band.bottom - band.top <= 3;
    if empty {
        return;
    }
    let n = SendMessageW(list, LVM_GETITEMCOUNT, 0, 0) as usize;
    let mut o: RECT = std::mem::zeroed();
    GetWindowRect(list, &mut o);
    let mut first: Option<usize> = None;
    for i in 0..n {
        let mut r: RECT = std::mem::zeroed();
        if SendMessageW(list, LVM_GETITEMRECT, i, &mut r as *mut _ as isize) == 0 {
            continue;
        }
        let (l, t, rr, b) = (r.left + o.left, r.top + o.top, r.right + o.left, r.bottom + o.top);
        if l < band.right && rr > band.left && t < band.bottom && b > band.top {
            let mut it: LVITEMW = std::mem::zeroed();
            it.stateMask = LVIS_SELECTED;
            it.state = LVIS_SELECTED;
            SendMessageW(list, LVM_SETITEMSTATE, i, &mut it as *mut _ as isize);
            first.get_or_insert(i);
        }
    }
    if let Some(i) = first {
        let mut it: LVITEMW = std::mem::zeroed();
        it.stateMask = LVIS_FOCUSED;
        it.state = LVIS_FOCUSED;
        SendMessageW(list, LVM_SETITEMSTATE, i, &mut it as *mut _ as isize);
    }
}

pub unsafe extern "system" fn proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        // the pixels come from UpdateLayeredWindow: nothing to paint, but the update region must be validated
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(h, &mut ps);
            EndPaint(h, &ps);
            0
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}
