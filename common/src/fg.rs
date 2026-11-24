//! Bringing windows to the front: `force()` for our own popups (Start, the search card) that need keyboard focus
//! even without recent input (#72), and `activate()` for switching to another application's window (restoring it if
//! minimized without disrupting exclusive-fullscreen games).
use windows_sys::Win32::{
    Foundation::HWND,
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{INPUT, INPUT_KEYBOARD, IsWindowEnabled, KEYEVENTF_KEYUP, SendInput, SetFocus, VK_MENU},
        WindowsAndMessaging::{
            AllowSetForegroundWindow, BringWindowToTop, GetForegroundWindow, GetLastActivePopup, GetWindowThreadProcessId,
            IsIconic, IsWindow, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindow, SwitchToThisWindow,
        },
    },
};

pub fn force(h: HWND) {
    unsafe {
        // a click on our own window already gives us the right; attaching while its button is down would hand it back
        if SetForegroundWindow(h) != 0 && GetForegroundWindow() == h {
            SetFocus(h);
            return;
        }
        // 1) share the input queue of the foreground thread for a moment
        let me = GetCurrentThreadId();
        let ft = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
        let attach = ft != 0 && ft != me && AttachThreadInput(me, ft, 1) != 0;
        SetForegroundWindow(h);
        if attach {
            AttachThreadInput(me, ft, 0);
        }
        // 2) nothing to attach to (looking at the desktop: no foreground window) or an elevated window in front (UIPI refuses
        // the attach): Windows lets the foreground change while Alt is down. 0xE8 (unassigned) in between, so the release is
        // not a lone Alt that opens a menu bar.
        if GetForegroundWindow() != h {
            keys(&[(VK_MENU, false), (0xE8, false), (0xE8, true)]);
            SetForegroundWindow(h);
            keys(&[(VK_MENU, true)]);
        }
        SetFocus(h); // detaching drops this thread's keyboard focus; without it the window gets no typing
    }
}

/// Switches to another application's window `w` (restoring it first if minimized).
/// Acquires foreground rights before calling `ShowWindow(w, SW_RESTORE)` so a minimized exclusive-fullscreen game is
/// not restored behind the current window (causing DXGI to immediately re-minimize it), and avoids injecting synthetic
/// Alt keys once `SetForegroundWindow` succeeds.
pub fn activate(w: HWND) {
    unsafe {
        if w.is_null() || IsWindow(w) == 0 {
            return;
        }
        let target = last_active_popup(w);
        let mut pid = 0u32;
        GetWindowThreadProcessId(target, &mut pid);
        let iconic = IsIconic(w) != 0;

        // Already un-minimized and we hold foreground rights (e.g. a click on the bar): switch directly without
        // attaching thread queues or injecting keys.
        if !iconic {
            if pid != 0 {
                AllowSetForegroundWindow(pid);
            }
            if SetForegroundWindow(target) != 0 {
                return;
            }
        }

        // 1) Share the current foreground thread's input queue *before* restoring/activating so we inherit foreground
        // permission during ShowWindow(SW_RESTORE) and AllowSetForegroundWindow(pid).
        let me = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let ft = if !fg.is_null() { GetWindowThreadProcessId(fg, std::ptr::null_mut()) } else { 0 };
        let attach = ft != 0 && ft != me && AttachThreadInput(me, ft, 1) != 0;
        if pid != 0 {
            AllowSetForegroundWindow(pid);
        }
        if iconic {
            ShowWindow(w, SW_RESTORE);
        }
        BringWindowToTop(target);
        let ok = SetForegroundWindow(target) != 0;
        if attach {
            AttachThreadInput(me, ft, 0);
        }

        // 2) Fallback only when SetForegroundWindow actually failed (looking at the desktop with no attachable
        // foreground thread, or UIPI refused AttachThreadInput). Never run this after a successful cross-process
        // SetForegroundWindow: GetForegroundWindow() can lag behind an exclusive-fullscreen mode switch, and a
        // synthetic Alt tap then knocks the game right back out of fullscreen.
        if !ok && !is_fg(w, target) {
            keys(&[(VK_MENU, false), (0xE8, false), (0xE8, true)]);
            if pid != 0 {
                AllowSetForegroundWindow(pid);
            }
            if IsIconic(w) != 0 {
                ShowWindow(w, SW_RESTORE);
            }
            BringWindowToTop(target);
            let ok2 = SetForegroundWindow(target) != 0;
            keys(&[(VK_MENU, true)]);
            if !ok2 && !is_fg(w, target) {
                SwitchToThisWindow(target, 1);
            }
        }
    }
}

fn last_active_popup(w: HWND) -> HWND {
    unsafe {
        let p = GetLastActivePopup(w);
        if !p.is_null() && p != w && IsWindowVisible(p) != 0 && IsWindowEnabled(p) != 0 { p } else { w }
    }
}

fn is_fg(w: HWND, target: HWND) -> bool {
    let fg = unsafe { GetForegroundWindow() };
    !fg.is_null() && (fg == target || fg == w)
}

fn keys(k: &[(u16, bool)]) {
    for &(vk, up) in k {
        let mut i: INPUT = unsafe { std::mem::zeroed() };
        i.r#type = INPUT_KEYBOARD;
        i.Anonymous.ki.wVk = vk;
        i.Anonymous.ki.dwFlags = if up { KEYEVENTF_KEYUP } else { 0 };
        unsafe { SendInput(1, &i, size_of::<INPUT>() as i32) };
    }
}
