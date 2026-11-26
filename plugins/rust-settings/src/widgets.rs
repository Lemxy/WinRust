//! The Widgets page's link to rs-metrics: its `%ProgramData%\WinRust\metrics.txt` and its process.
//!
//! The widget re-reads that file every second, so writing a key here applies live; no IPC is needed. Only the keys the
//! page owns are rewritten — the comments and any key this version does not know stay as they are.
use std::{env, fs, path::PathBuf, process::Command};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{FindWindowExW, FindWindowW, PostMessageW, WM_CLOSE},
};

/// Module keys of metrics.txt, in the widget's order (`config::MODS`).
pub const MODS: [&str; 8] = ["cpu", "gpu", "ram", "disk", "io", "net", "temp", "uptime"];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn file() -> PathBuf {
    PathBuf::from(env::var_os("ProgramData").unwrap_or_default()).join("WinRust").join("metrics.txt")
}

fn lines() -> Vec<String> {
    fs::read_to_string(file()).map(|s| s.lines().map(str::to_string).collect()).unwrap_or_default()
}

fn value(key: &str) -> Option<String> {
    lines().iter().find_map(|l| {
        let (k, v) = l.trim().split_once('=')?;
        (k.trim() == key).then(|| v.trim().to_string())
    })
}

/// `key=1/0` as the widget writes it; a missing file or key means the widget's default.
pub fn flag(key: &str, default: bool) -> bool {
    value(key).map_or(default, |v| v != "0")
}

/// Replaces the key in place (or appends it), keeping every other line.
pub fn set(key: &str, val: &str) {
    let mut ls = lines();
    let mut found = false;
    for l in ls.iter_mut() {
        if let Some((k, _)) = l.split_once('=') {
            if k.trim() == key {
                *l = format!("{key}={val}");
                found = true;
                break;
            }
        }
    }
    if !found {
        ls.push(format!("{key}={val}"));
    }
    if let Some(d) = file().parent() {
        let _ = fs::create_dir_all(d);
    }
    let _ = fs::write(file(), ls.join("\n") + "\n");
}

fn hwnd() -> HWND {
    unsafe {
        // topmost=0 makes the widget a child of the desktop window, which FindWindow does not see: look under Progman
        // first, then fall back to the top-level case (topmost=1).
        let pm = FindWindowW(wide("Progman").as_ptr(), wide("Program Manager").as_ptr());
        if !pm.is_null() {
            let w = FindWindowExW(pm, std::ptr::null_mut(), wide("WinRustMetrics").as_ptr(), std::ptr::null());
            if !w.is_null() {
                return w;
            }
        }
        FindWindowW(wide("WinRustMetrics").as_ptr(), std::ptr::null())
    }
}

pub fn running() -> bool {
    !hwnd().is_null()
}

/// The Widgets page's on/off (`enabled` in metrics.txt). Hidden but alive when off: turning it back on is instant, and
/// the state survives a sign-in because rs-shell only starts the process.
pub fn enabled() -> bool {
    flag("enabled", false)
}

/// Writes the switch; turning it on also starts the process when nothing is running (first launch after an update).
pub fn set_enabled(on: bool) {
    set("enabled", if on { "1" } else { "0" });
    if on && !running() {
        launch();
    }
}

fn exe() -> PathBuf {
    PathBuf::from(env::var_os("ProgramFiles").unwrap_or_default()).join("rs-metrics").join("rs-metrics.exe")
}

pub fn launch() {
    if !running() {
        let _ = Command::new(exe()).spawn();
    }
}

pub fn stop() {
    let h = hwnd();
    if !h.is_null() {
        unsafe { PostMessageW(h, WM_CLOSE, 0, 0) };
    }
}

pub fn toggle() {
    if running() {
        stop();
    } else {
        launch();
    }
}

pub fn restart() {
    stop();
    std::thread::sleep(std::time::Duration::from_millis(400));
    launch();
}

pub fn open_config() {
    let _ = Command::new("notepad.exe").arg(file()).spawn();
}
