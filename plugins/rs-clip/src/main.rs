//! rs-clip: WinRust's clipboard history (Win+V) and emoji picker (Win+.).
//!
//! A resident, windowless process. A message-only window owns `AddClipboardFormatListener`, so every copy lands in
//! `%APPDATA%\WinRust\clipboard\` (see `store`), and two hotkeys open the frost popups:
//!   Win+V      history: type to search, Up/Down + Enter or a click pastes, Del removes one, the footer clears all.
//!   Win+.      emoji: five Segoe UI Emoji groups, search by English name, click/Enter inserts into the active window.
//! Both paste by putting the entry on the clipboard and sending Ctrl+V to the window that was in front when the hotkey
//! fired (the entry is also left on the clipboard, so a refused send is harmless).
//!
//! Clipboard formats: CF_UNICODETEXT, CF_DIB/DIBV5 (CF_BITMAP as a fallback) and CF_HDROP for file lists. Content that
//! Windows or a password manager marks private (ExcludeClipboardContentFromMonitorProcessing, Clipboard Viewer Ignore,
//! CanIncludeInClipboardHistory=0, CanUploadToCloudClipboard=0) is skipped; that is best effort, not a promise.
//! No cloud sync, no per-app knowledge, no encryption.
#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
#[macro_use]
extern crate winrust_common;

mod emoji;
mod history;
mod img;
mod store;
mod ui;

use std::{
    io::Write,
    path::PathBuf,
    ptr::null_mut,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicIsize, Ordering::Relaxed},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM},
    System::{
        DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener},
        LibraryLoader::GetModuleHandleW,
        Threading::CreateMutexW,
    },
    UI::{
        HiDpi::SetProcessDpiAwarenessContext,
        Input::KeyboardAndMouse::{MOD_NOREPEAT, MOD_WIN, RegisterHotKey, VK_OEM_PERIOD},
        WindowsAndMessaging::*,
    },
};

use store::Store;

const CLASS: &str = "WinRustClipWatcher";
const HK_HISTORY: i32 = 1;
const HK_EMOJI: i32 = 2;

static STORE: OnceLock<Arc<Mutex<Store>>> = OnceLock::new();
/// 0 = no popup, -1 = one is being created, >0 = its HWND (so a second hotkey closes it).
static POPUP: AtomicIsize = AtomicIsize::new(0);

fn log(msg: &str) {
    static LOCK: Mutex<()> = Mutex::new(());
    let Ok(base) = std::env::var("LOCALAPPDATA") else { return };
    let dir = PathBuf::from(base).join("rs-shell");
    let _ = std::fs::create_dir_all(&dir);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(_guard) = LOCK.lock() {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("rs-clip.log")) {
            let _ = writeln!(f, "[{t}] {msg}");
        }
    }
}

fn main() {
    winrust_common::appid::set("WinRust");
    let _ = lang!();
    unsafe { SetProcessDpiAwarenessContext(-4isize as _) }; // per-monitor v2: popups land on the right monitor
    log("start");

    // One watcher per session. A second copy (an update, a stray Run entry) exits before touching the clipboard.
    let name = ui::wide(r"Local\WinRustRsClip");
    unsafe {
        CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            log("another rs-clip is already running");
            return;
        }
    }

    let store: Arc<Mutex<Store>> = Arc::new(Mutex::new(Store::load()));
    let _ = STORE.set(store);

    unsafe {
        let inst = GetModuleHandleW(null_mut());
        let class = ui::wide(CLASS);
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: inst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        if RegisterClassW(&wc) == 0 {
            log("RegisterClassW failed");
            return;
        }
        let hwnd = CreateWindowExW(0, class.as_ptr(), ui::wide(ui::TITLE).as_ptr(), 0, 0, 0, 0, 0, HWND_MESSAGE, null_mut(), inst, null_mut());
        if hwnd.is_null() {
            log("the hidden window could not be created");
            return;
        }
        if AddClipboardFormatListener(hwnd) == 0 {
            log(&format!("AddClipboardFormatListener failed (error {})", GetLastError()));
        }

        let v = RegisterHotKey(hwnd, HK_HISTORY, MOD_WIN | MOD_NOREPEAT, 'V' as u32);
        if v == 0 {
            log(&format!("Win+V is taken (error {}): the system clipboard panel keeps it", GetLastError()));
        }
        let dot = RegisterHotKey(hwnd, HK_EMOJI, MOD_WIN | MOD_NOREPEAT, VK_OEM_PERIOD as u32);
        if dot == 0 {
            log(&format!("Win+. is taken (error {}): the system emoji panel keeps it", GetLastError()));
        }
        log(&format!("hotkeys: Win+V={} Win+.={}", v != 0, dot != 0));

        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

/// Opens history (1) or emoji (2). A hotkey while a popup is open closes it: the second press is a toggle, like the
/// system panels.
fn open(kind: u8) {
    if POPUP.compare_exchange(0, -1, Relaxed, Relaxed).is_err() {
        let h = POPUP.load(Relaxed);
        if h > 0 {
            unsafe { PostMessageW(h as HWND, WM_CLOSE, 0, 0) };
        }
        return;
    }
    let target = unsafe { GetForegroundWindow() } as isize;
    let Some(store) = STORE.get().cloned() else {
        POPUP.store(0, Relaxed);
        return;
    };
    std::thread::spawn(move || {
        if kind == 1 {
            history::open(store, target);
        } else {
            emoji::open(target);
        }
        POPUP.store(0, Relaxed);
    });
}

pub(crate) fn popup_created(hwnd: HWND) {
    POPUP.store(hwnd as isize, Relaxed);
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            if let Some(store) = STORE.get() {
                let mut s = store.lock().unwrap_or_else(|e| e.into_inner());
                // The listener also fires for our own paste. `capture` compares against LAST_WRITE and skips that copy.
                if s.capture(h) {
                    log("captured");
                }
            }
            0
        }
        WM_HOTKEY => {
            match wp as i32 {
                HK_HISTORY => open(1),
                HK_EMOJI => open(2),
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            RemoveClipboardFormatListener(h);
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
