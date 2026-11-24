//! Screen, monitor and window capture with plain GDI (BitBlt + CAPTUREBLT over the virtual desktop, PrintWindow for a
//! single window). Coordinates are physical pixels: the process is per-monitor DPI aware v2, so a second monitor at
//! another scale is neither cut short nor blurred.
use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::{
        Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
        Gdi::{
            BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetWindowDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
            SelectObject,
        },
    },
    Storage::Xps::PrintWindow,
    System::Threading::GetCurrentProcessId,
    UI::WindowsAndMessaging::{
        CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, EnumWindows, GA_ROOT, GetAncestor, GetClassNameW, GetCursorInfo, GetForegroundWindow,
        GetCursorPos, GetSystemMetrics, GetWindowLongW, GetWindowRect, GWL_EXSTYLE, IsIconic, IsWindowVisible, PW_RENDERFULLCONTENT, SM_CXVIRTUALSCREEN,
        SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
    },
};

use winrust_common::monitors;

use crate::img::Image;

pub fn cursor_pos() -> POINT {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    p
}

pub unsafe fn full_rect() -> RECT {
    RECT {
        left: GetSystemMetrics(SM_XVIRTUALSCREEN),
        top: GetSystemMetrics(SM_YVIRTUALSCREEN),
        right: GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN),
        bottom: GetSystemMetrics(SM_YVIRTUALSCREEN) + GetSystemMetrics(SM_CYVIRTUALSCREEN),
    }
}

/// Monitor `n` of the left-to-right list, or the one under the cursor when `n` is None.
pub fn monitor_rect(n: Option<usize>) -> RECT {
    match n {
        Some(i) => monitors::nth(i).rc,
        None => {
            let p = cursor_pos();
            monitors::at(p.x, p.y).rc
        }
    }
}

pub unsafe fn foreground() -> HWND {
    let h = GetForegroundWindow();
    if h.is_null() { null_mut() } else { GetAncestor(h, GA_ROOT) }
}

/// The visible bounds of a window (DWM drops the invisible resize border), falling back to GetWindowRect.
pub unsafe fn window_rect(hwnd: HWND) -> RECT {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let hr = DwmGetWindowAttribute(
        hwnd,
        DWMWA_EXTENDED_FRAME_BOUNDS as u32,
        &mut r as *mut _ as *mut core::ffi::c_void,
        std::mem::size_of::<RECT>() as u32,
    );
    if hr != 0 || r.right - r.left < 2 || r.bottom - r.top < 2 {
        GetWindowRect(hwnd, &mut r);
    }
    r
}

unsafe fn draw_cursor(mem: HDC, origin: POINT) {
    let mut ci: CURSORINFO = std::mem::zeroed();
    ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
    if GetCursorInfo(&mut ci) != 0 && ci.flags & CURSOR_SHOWING != 0 && !ci.hCursor.is_null() {
        DrawIconEx(mem, ci.ptScreenPos.x - origin.x, ci.ptScreenPos.y - origin.y, ci.hCursor, 0, 0, 0, null_mut(), DI_NORMAL);
    }
}

/// A bitmap of a screen rectangle (virtual-screen coordinates).
pub unsafe fn grab_rect(r: RECT, with_cursor: bool) -> Option<HBITMAP> {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    let scr = GetDC(null_mut());
    if scr.is_null() {
        return None;
    }
    let mem = CreateCompatibleDC(scr);
    let bmp = CreateCompatibleBitmap(scr, w, h);
    let old = SelectObject(mem, bmp as HGDIOBJ);
    let ok = BitBlt(mem, 0, 0, w, h, scr, r.left, r.top, SRCCOPY | CAPTUREBLT) != 0;
    if ok && with_cursor {
        draw_cursor(mem, POINT { x: r.left, y: r.top });
    }
    SelectObject(mem, old);
    DeleteDC(mem);
    ReleaseDC(null_mut(), scr);
    if ok { Some(bmp) } else { DeleteObject(bmp as HGDIOBJ); None }
}

/// A bitmap of a window. PrintWindow renders it even when another window covers it; a screen copy is the fallback.
pub unsafe fn grab_window(hwnd: HWND, with_cursor: bool) -> Option<HBITMAP> {
    if hwnd.is_null() {
        return None;
    }
    let r = window_rect(hwnd);
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    let wdc = GetWindowDC(hwnd);
    let mem = CreateCompatibleDC(wdc);
    let bmp = CreateCompatibleBitmap(wdc, w, h);
    let old = SelectObject(mem, bmp as HGDIOBJ);
    let mut ok = PrintWindow(hwnd, mem, PW_RENDERFULLCONTENT) != 0;
    if !ok {
        let scr = GetDC(null_mut());
        ok = BitBlt(mem, 0, 0, w, h, scr, r.left, r.top, SRCCOPY | CAPTUREBLT) != 0;
        ReleaseDC(null_mut(), scr);
    }
    if ok && with_cursor {
        draw_cursor(mem, POINT { x: r.left, y: r.top });
    }
    SelectObject(mem, old);
    DeleteDC(mem);
    ReleaseDC(hwnd, wdc);
    if ok { Some(bmp) } else { DeleteObject(bmp as HGDIOBJ); None }
}

pub unsafe fn capture_rect(r: RECT, with_cursor: bool) -> Option<Image> {
    let bmp = grab_rect(r, with_cursor)?;
    let img = Image::from_hbitmap(bmp);
    DeleteObject(bmp as HGDIOBJ);
    img
}

pub unsafe fn capture_window(hwnd: HWND, with_cursor: bool) -> Option<Image> {
    let bmp = grab_window(hwnd, with_cursor)?;
    let img = Image::from_hbitmap(bmp);
    DeleteObject(bmp as HGDIOBJ);
    img
}

struct Find {
    x: i32,
    y: i32,
    found: HWND,
}

unsafe extern "system" fn each_window(h: HWND, p: isize) -> i32 {
    let f = &mut *(p as *mut Find);
    if f.found.is_null()
        && IsWindowVisible(h) != 0
        && IsIconic(h) == 0
        && GetWindowLongW(h, GWL_EXSTYLE) as u32 & (WS_EX_TRANSPARENT | WS_EX_NOACTIVATE) == 0
    {
        let mut pid = 0u32;
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(h, &mut pid);
        let mut cloaked = 0u32;
        DwmGetWindowAttribute(h, DWMWA_CLOAKED as u32, &mut cloaked as *mut _ as *mut core::ffi::c_void, 4);
        let mut cls = [0u16; 64];
        let n = GetClassNameW(h, cls.as_mut_ptr(), cls.len() as i32);
        let class = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        let skip = matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Windows.UI.Core.CoreWindow");
        if pid != GetCurrentProcessId() && cloaked == 0 && !skip {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            GetWindowRect(h, &mut r);
            if f.x >= r.left && f.x < r.right && f.y >= r.top && f.y < r.bottom {
                f.found = h;
            }
        }
    }
    1
}

/// The topmost ordinary window under a screen point (EnumWindows is front to back). The overlay itself is skipped.
pub unsafe fn window_at(x: i32, y: i32) -> Option<HWND> {
    let mut f = Find { x, y, found: null_mut() };
    EnumWindows(Some(each_window), &mut f as *mut _ as isize);
    (!f.found.is_null()).then_some(f.found)
}

