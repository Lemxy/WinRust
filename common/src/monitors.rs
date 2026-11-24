//! Monitor geometry.
//!
//! `SM_CXSCREEN`/`SM_CYSCREEN` are always the **primary** monitor; anything that has to work on a second screen (a bar
//! per monitor, a popup anchored to the bar, a toast on the active screen, per-monitor DPI) must ask here instead of
//! hard-coding the screen metrics. One place so the bar, the desktop and the notifications agree on what a monitor is.

use std::ffi::c_void;

use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, MONITORINFO, MONITORENUMPROC,
        MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
    },
};

/// A monitor and the two rectangles that matter: the whole screen (`rc`) and the part left free for applications
/// (`work`, with the taskbar/appbar strips already removed by the shell).
#[derive(Clone, Copy)]
pub struct Mon {
    pub handle: *mut c_void,
    pub rc: RECT,
    pub work: RECT,
    pub primary: bool,
}

fn info(handle: *mut c_void) -> Option<Mon> {
    if handle.is_null() {
        return None;
    }
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(handle, &mut mi) == 0 {
            return None;
        }
        Some(Mon { handle, rc: mi.rcMonitor, work: mi.rcWork, primary: mi.dwFlags & 1 != 0 })
    }
}

unsafe extern "system" fn each(h: *mut c_void, _dc: *mut c_void, _rc: *mut RECT, data: isize) -> i32 {
    let v = &mut *(data as *mut Vec<Mon>);
    if let Some(m) = info(h) {
        v.push(m);
    }
    1
}

/// Every monitor, ordered left to right / top to bottom so an index is stable across calls for a given layout.
pub fn all() -> Vec<Mon> {
    let mut v: Vec<Mon> = Vec::new();
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut v as *mut _ as isize);
    }
    v.sort_by_key(|m| (m.rc.left, m.rc.top));
    if v.is_empty() {
        if let Some(m) = info(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }) {
            v.push(m);
        }
    }
    v
}

/// The monitor Windows calls primary (the one at the origin of the screen coordinate space).
pub fn primary() -> Mon {
    info(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }).unwrap_or(Mon {
        handle: std::ptr::null_mut(),
        rc: RECT { left: 0, top: 0, right: 1, bottom: 1 },
        work: RECT { left: 0, top: 0, right: 1, bottom: 1 },
        primary: true,
    })
}

/// The monitor under a screen point (the cursor, a popup's anchor).
pub fn at(x: i32, y: i32) -> Mon {
    info(unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) }).unwrap_or_else(primary)
}

/// The monitor a window is on (mostly: its largest intersection).
pub fn of(hwnd: HWND) -> Mon {
    info(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }).unwrap_or_else(primary)
}

/// The n-th monitor of `all()` (0 = leftmost), falling back to the primary.
pub fn nth(i: usize) -> Mon {
    all().get(i).copied().unwrap_or_else(primary)
}

impl Mon {
    pub fn width(self) -> i32 {
        self.rc.right - self.rc.left
    }

    pub fn height(self) -> i32 {
        self.rc.bottom - self.rc.top
    }
}
