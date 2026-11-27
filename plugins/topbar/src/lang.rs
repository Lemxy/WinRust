//! topbar: lang (split out of main.rs; shares its items through `super::*`).
use super::*;

/// Next keyboard layout for the app window. Posted to its focused control (that is where Chrome, Office... look for it);
/// timer 7 checks it took and otherwise presses Win+Space, the system switcher (games, terminals ignore the message).
/// The installed keyboard layouts and the one the app in front uses.
pub(crate) unsafe fn layouts() -> (Vec<HKL>, HKL) {
    let mut l: [HKL; 32] = [null_mut(); 32];
    let n = GetKeyboardLayoutList(32, l.as_mut_ptr()).clamp(0, 32) as usize;
    (l[..n].to_vec(), GetKeyboardLayout(GetWindowThreadProcessId(app_window(), null_mut())))
}

/// The window in front is one of the bar's own (Start, a flyout).
pub(crate) unsafe fn own_foreground() -> bool {
    let mut pid = 0u32;
    GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
    pid == std::process::id()
}

/// The next layout (or the previous one with `back`, Win+Shift+Space) for the app in front.
pub(crate) unsafe fn next_lang(bar: HWND, back: bool) {
    let own = own_foreground(); // our Start in front: switch this thread (app_window() is the app behind it)
    let (l, cur) = if own { (layouts().0, GetKeyboardLayout(0)) } else { layouts() };
    if l.len() < 2 {
        return; // one layout: nothing to switch to
    }
    let i = l.iter().position(|&k| k == cur);
    let n = l.len();
    let next = l[match (i, back) { (Some(i), false) => (i + 1) % n, (Some(i), true) => (i + n - 1) % n, (None, _) => 0 }];
    if own {
        ActivateKeyboardLayout(next, 0);
        InvalidateRect(bar, null(), 0);
    } else {
        set_lang(bar, next);
    }
}

/// Asks the app in front to switch to layout `next` (the bar's own thread cannot change another app's language).
pub(crate) unsafe fn set_lang(bar: HWND, next: HKL) {
    let w = app_window();
    let tid = GetWindowThreadProcessId(w, null_mut());
    let cur = GetKeyboardLayout(tid);
    let mut gi: GUITHREADINFO = zeroed();
    gi.cbSize = size_of::<GUITHREADINFO>() as u32;
    let target = if GetGUIThreadInfo(tid, &mut gi) != 0 && !gi.hwndFocus.is_null() { gi.hwndFocus } else { w };
    PostMessageW(target, WM_INPUTLANGCHANGEREQUEST, 0, next as isize);
    LANG_TRY.set((tid, cur as usize, next as usize));
    SetTimer(bar, 7, 250, None);
}

thread_local! {
    pub(crate) static SIG: Cell<(u64, u32)> = const { Cell::new((0, 0)) }; // (hash of the last painted state, ticks since the last repaint)
}
