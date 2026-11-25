//! Where a program opens.
//!
//! Windows puts a new window on the monitor of the window that had the focus, or on the primary one, so an app started
//! from the bar on the second screen still shows up on the first. The launcher places the new window on the monitor
//! under the cursor: that is where the user is looking when they click.
//!
//! Fullscreen works the same way. A game's swap chain follows the monitor of its window, so moving the window before it
//! goes fullscreen moves the game; a late fullscreen resize (a game that was windowed for a second) is caught by a short
//! watch. Only the window that was placed is touched, never the dialogs the app opens on its own.
#![allow(unsafe_op_in_unsafe_fn)] // every function here is Win32 calls; the unsafe ones are marked as such

use std::{
    ffi::c_void,
    mem::zeroed,
    thread,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    UI::WindowsAndMessaging::{
        EnumWindows, GetCursorPos, GetWindow, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsWindow,
        IsWindowVisible, IsZoomed, SetWindowPos, ShowWindow, GWL_EXSTYLE, GW_OWNER, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
        SWP_NOZORDER, SW_MAXIMIZE, SW_RESTORE, WS_EX_TOOLWINDOW,
    },
};

use crate::monitors::{self, Mon};

/// The monitor under the cursor.
pub fn cursor() -> Mon {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut p) };
    monitors::at(p.x, p.y)
}

/// A monitor as plain numbers: the placement runs on a worker thread, and an `HMONITOR` (a raw pointer) cannot travel.
#[derive(Clone, Copy)]
struct Target {
    rc: RECT,
    work: RECT,
    id: usize,
}

impl Target {
    fn of(m: Mon) -> Self {
        Target { rc: m.rc, work: m.work, id: m.handle as usize }
    }
    fn width(self) -> i32 {
        self.rc.right - self.rc.left
    }
    fn height(self) -> i32 {
        self.rc.bottom - self.rc.top
    }
}

/// The position and size a window of `w`x`h` gets when it opens: centered on the monitor under the cursor, inside its
/// work area. For our own windows, which otherwise ask Windows and land on the primary one.
pub fn centered(w: i32, h: i32) -> (i32, i32, i32, i32) {
    let m = cursor();
    let (ww, wh) = (m.work.right - m.work.left, m.work.bottom - m.work.top);
    let (w, h) = (w.min(ww).max(1), h.min(wh).max(1));
    (m.work.left + (ww - w) / 2, m.work.top + (wh - h) / 2, w, h)
}

/// Watches the process `pid` for a moment and moves its window to the monitor under the cursor. Runs on a thread: the
/// caller returns at once.
pub fn after_start(pid: u32) {
    if pid == 0 {
        return;
    }
    let mon = Target::of(cursor());
    thread::spawn(move || watch(pid, mon));
}

/// The same for a launcher that does not tell us the pid (`runas` starts the program itself): the first new process with
/// this exe name that was not in `before`. `before` is taken before the launch, with `crate::proc::pids`.
pub fn after_start_named(exe: String, before: Vec<u32>) {
    let mon = Target::of(cursor());
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            if let Some(pid) = crate::proc::pids(&exe).into_iter().find(|p| !before.contains(p)) {
                return watch(pid, mon);
            }
            thread::sleep(Duration::from_millis(120));
        }
    });
}

struct Found {
    hwnd: HWND,
    area: i64,
}

struct Scan {
    pid: u32,
    out: Vec<Found>,
}

/// A window of the app: visible, not a tool window and not one of its dialogs (an owned popup belongs to another window).
unsafe extern "system" fn scan(hwnd: HWND, data: isize) -> i32 {
    let s = &mut *(data as *mut Scan);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid != s.pid || IsWindowVisible(hwnd) == 0 {
        return 1;
    }
    if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
        return 1;
    }
    if !GetWindow(hwnd, GW_OWNER).is_null() {
        return 1;
    }
    let mut r: RECT = zeroed();
    if GetWindowRect(hwnd, &mut r) == 0 {
        return 1;
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w < 160 || h < 120 {
        return 1; // a splash tip, a progress strip: not the window the user asked for
    }
    s.out.push(Found { hwnd, area: w as i64 * h as i64 });
    1
}

fn windows_of(pid: u32) -> Vec<Found> {
    let mut s = Scan { pid, out: Vec::new() };
    unsafe { EnumWindows(Some(scan), &mut s as *mut _ as isize) };
    s.out
}

fn watch(pid: u32, mon: Target) {
    let began = Instant::now();
    let mut placed: Option<HWND> = None;
    let mut moves = 0;
    while began.elapsed() < Duration::from_secs(14) && moves < 4 {
        let found = windows_of(pid);
        match placed {
            None => {
                // the largest window is the one the user asked for; a splash is small and comes first
                if let Some(f) = found.iter().max_by_key(|f| f.area) {
                    placed = Some(f.hwnd);
                    if unsafe { put_on(f.hwnd, mon) } {
                        moves += 1;
                    }
                }
            }
            Some(h) => {
                if unsafe { IsWindow(h) } == 0 {
                    return;
                }
                // it went fullscreen after opening: a game that fills the wrong monitor is moved again
                let mut r: RECT = unsafe { zeroed() };
                if unsafe { GetWindowRect(h, &mut r) } != 0 {
                    let cur = monitors::of(h);
                    let full = (r.right - r.left) >= cur.width() - 16 && (r.bottom - r.top) >= cur.height() - 16;
                    if full && cur.handle as usize != mon.id && unsafe { put_on(h, mon) } {
                        moves += 1;
                    }
                }
                if moves > 0 && began.elapsed() > Duration::from_secs(8) {
                    return; // placed and settled
                }
            }
        }
        let nap = if began.elapsed() < Duration::from_secs(3) { 100 } else { 400 };
        thread::sleep(Duration::from_millis(nap));
    }
}

/// Moves a window to `mon`: a windowed one keeps its size (centered in the work area), a fullscreen one takes the whole
/// monitor. A maximized window is restored, placed and maximized again, which is what actually changes its monitor.
unsafe fn put_on(hwnd: HWND, mon: Target) -> bool {
    let mut r: RECT = zeroed();
    if GetWindowRect(hwnd, &mut r) == 0 {
        return false;
    }
    let cur = monitors::of(hwnd);
    if cur.handle as usize == mon.id {
        return false; // already where the user is
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let fullscreen = w >= cur.width() - 16 && h >= cur.height() - 16;
    let maxed = IsZoomed(hwnd) != 0;
    if maxed {
        ShowWindow(hwnd, SW_RESTORE);
    }
    let (tw, th, x, y) = if fullscreen {
        (mon.width(), mon.height(), mon.rc.left, mon.rc.top)
    } else {
        let (ww, wh) = (mon.work.right - mon.work.left, mon.work.bottom - mon.work.top);
        let (tw, th) = (w.min(ww), h.min(wh));
        (tw, th, mon.work.left + (ww - tw) / 2, mon.work.top + (wh - th) / 2)
    };
    SetWindowPos(hwnd, std::ptr::null_mut(), x, y, tw, th, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
    if maxed {
        ShowWindow(hwnd, SW_MAXIMIZE);
    }
    true
}

/// Keeps `windows_sys` honest about the unused import in builds where nothing else needs it.
#[allow(dead_code)]
fn _types(_: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a unit test: it opens a real window for a couple of seconds to check the placement on this desktop.
    /// `cargo test -p winrust-common a_program_opens -- --ignored --nocapture`
    #[test]
    #[ignore = "opens a window on the desktop"]
    fn a_program_opens_on_the_cursor_monitor() {
        let want = cursor();
        // winver: a plain Win32 top-level window, not one of the packaged stubs (`notepad.exe`) that start another process
        let mut child = std::process::Command::new(r"C:\Windows\System32\winver.exe").spawn().expect("winver");
        after_start(child.id());
        thread::sleep(Duration::from_millis(2500));
        let hwnd = windows_of(child.id()).into_iter().max_by_key(|f| f.area).map(|f| f.hwnd);
        let got = hwnd.map(|h| monitors::of(h));
        let _ = child.kill();
        let got = got.expect("winver's window");
        assert_eq!(got.handle, want.handle, "window opened on the wrong monitor");
    }
}
