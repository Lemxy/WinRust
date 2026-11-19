//! rust-settings: Windows 11 Settings rebuilt in Rust (Direct2D + DirectWrite), ~100 pages that read and
//! change the real system values (display, sound, Bluetooth, Wi-Fi, apps, privacy, update, ...).
//! `rust-settings.exe [page]` - a page name ("Bluetooth & devices") or an ms-settings: address.
#![windows_subsystem = "windows"]

mod dialog;
mod flyout;
mod menu;
mod perf;
mod qsx;
mod quick;
mod settings;
mod shims;
pub(crate) use shims::{files, launcher, notify, photos, session, taskbar, wallpaper};

/// Kept for code shared with the file manager window of the project this came from.
pub(crate) const FILES_ID: i32 = 94;

use windows::Win32::System::Com::*;

/// Windows 11's look for a pop-up: dark, rounded corners, acrylic behind it.
pub(crate) unsafe fn fluent_window(h: windows::Win32::Foundation::HWND) -> windows::core::Result<()> {
    use windows::Win32::Graphics::Dwm::*;
    let set = |attr: DWMWINDOWATTRIBUTE, v: u32| {
        let _ = DwmSetWindowAttribute(h, attr, &v as *const _ as *const std::ffi::c_void, 4);
    };
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    set(DWMWA_WINDOW_CORNER_PREFERENCE, 2);
    set(DWMWA_SYSTEMBACKDROP_TYPE, 3);
    DwmExtendFrameIntoClientArea(h, &windows::Win32::UI::Controls::MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 })
}
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        flyout::dark_menus();
        if let Some(a) = std::env::args().nth(1) {
            settings::request(settings::page_for_uri(&a).unwrap_or(&a));
        }
        let inst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        settings::show(inst.into());
        // closing the window only hides it: leave once it is gone from the screen
        unsafe extern "system" fn check(_: windows::Win32::Foundation::HWND, _: u32, _: usize, _: u32) {
            if !IsWindowVisible(windows::Win32::Foundation::HWND(settings::WIN.load(std::sync::atomic::Ordering::Relaxed) as *mut _)).as_bool() {
                PostQuitMessage(0);
            }
        }
        SetTimer(None, 0, 500, Some(check));
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}
