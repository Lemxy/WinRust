//! Whole-window translucency for the apps' top-level windows (Explorer, Settings, Control Panel).
//!
//! Windows' acrylic (`SetWindowCompositionAttribute` with `ACCENT_ENABLE_ACRYLICBLURBEHIND`) is the nicer effect, but
//! it only shows through pixels the app leaves unpainted: these windows fill their whole client area with GDI each
//! paint (`WM_ERASEBKGND`, then hand-painted chrome, solid fills), so the backdrop would never be visible and the window
//! would stay opaque. A layered window with a constant alpha fades every pixel the app draws, children included, which
//! is what "the window at 87 %" means here. Text keeps its contrast because the tint behind it fades with it.
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{
        GetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
        SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, WS_EX_LAYERED,
    },
};

/// Makes `hwnd` and everything inside it `alpha`/255 opaque (222 = 87 %). `255` leaves it solid. Reading the value from
/// `look.txt` is the caller's job (`look::window_opacity`), so this stays a one-line call.
pub unsafe fn translucent(hwnd: HWND, alpha: u8) {
    if hwnd.is_null() {
        return;
    }
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if ex & WS_EX_LAYERED as isize == 0 {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
            SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
        // 255 is a no-op value for a layered window: the window still paints exactly as before, so the setting can go
        // back to solid without taking the style off again.
        SetLayeredWindowAttributes(hwnd, 0, alpha, LWA_ALPHA);
    }
}
