//! rs-notify: raises one notification pop-up on the WinRust bar.
//!
//! It opens `\\.\pipe\winrust-notify` and writes one `winrust_common::notify::NotifyWire`; nothing else. The bar must be
//! running (topbar owns the pipe). Exit codes: 0 sent, 1 bad arguments, 2 the bar is not listening, 3 the write failed.
#![windows_subsystem = "console"]
use std::{
    ptr::{null, null_mut},
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{CreateFileW, WriteFile, OPEN_EXISTING},
};

use winrust_common::notify::{NotifyWire, FLAG_PROGRESS, PIPE};

const USAGE: &str = "\
rs-notify: a notification pop-up on the WinRust bar

Usage:
  rs-notify --app \"Name\" --title \"...\" --body \"...\" [options]

Options:
  --icon PATH        exe, .lnk or image whose shell icon the toast shows
  --timeout MS       how long it stays (default: notify-timeout in look.txt)
  --progress 0..1    draws a progress bar and keeps the toast open
  --id NAME          update the visible toast with this id instead of adding one
  --action LABEL     text of the action (kept for the history entry)
  --cmd \"program args\"   clicking the toast runs this command
  -h, --help         this help

Exit codes: 0 sent, 1 bad arguments, 2 the bar is not listening, 3 write failed.";

fn main() {
    winrust_common::appid::set("WinRust"); // group every WinRust process under one Task Manager entry
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut app = String::new();
    let mut title = String::new();
    let mut body = String::new();
    let mut icon = String::new();
    let mut timeout = 0u32;
    let mut progress: Option<f32> = None;
    let mut id = String::new();
    let mut action = String::new();
    let mut cmd = String::new();

    let mut i = 0;
    while i < args.len() {
        let take = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_default()
        };
        match args[i].as_str() {
            "--app" => app = take(&mut i),
            "--title" => title = take(&mut i),
            "--body" => body = take(&mut i),
            "--icon" => icon = take(&mut i),
            "--timeout" => {
                let v = take(&mut i);
                timeout = match v.parse::<u32>() {
                    Ok(v) => v,
                    Err(_) => {
                        eprintln!("rs-notify: --timeout needs milliseconds, got {v:?}");
                        return 1;
                    }
                };
            }
            "--progress" => {
                let v = take(&mut i);
                progress = match v.parse::<f32>() {
                    Ok(v) if v.is_finite() => Some(v.clamp(0.0, 1.0)),
                    _ => {
                        eprintln!("rs-notify: --progress needs a number from 0 to 1, got {v:?}");
                        return 1;
                    }
                };
            }
            "--id" => id = take(&mut i),
            "--action" => action = take(&mut i),
            "--cmd" => cmd = take(&mut i),
            "-h" | "--help" => {
                println!("{USAGE}");
                return 0;
            }
            other => {
                eprintln!("rs-notify: unknown option {other:?}\n\n{USAGE}");
                return 1;
            }
        }
        i += 1;
    }

    let mut w = NotifyWire::new();
    w.timeout_ms = timeout;
    if let Some(p) = progress {
        w.progress = p;
        w.flags |= FLAG_PROGRESS;
    }
    w.set_app(&app);
    w.set_title(&title);
    w.set_body(&body);
    w.set_icon(&icon);
    w.set_action_label(&action);
    w.set_action_cmd(&cmd);
    w.set_id(&id);

    let h = match connect() {
        Some(h) => h,
        None => {
            eprintln!("rs-notify: the WinRust bar is not listening on {PIPE} (is topbar running?)");
            return 2;
        }
    };
    let bytes = w.as_bytes();
    let mut written = 0u32;
    let ok = unsafe { WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut written, null_mut()) };
    unsafe {
        CloseHandle(h);
    }
    if ok == 0 || written as usize != bytes.len() {
        eprintln!("rs-notify: the bar closed the pipe before the notification was written");
        return 3;
    }
    0
}

/// Opens the pipe; retries while every instance is busy (the bar is reading another notification) or while the bar is
/// between two instances (the listener creates the next one right after closing the previous connection).
fn connect() -> Option<HANDLE> {
    let name: Vec<u16> = PIPE.encode_utf16().chain([0]).collect();
    for _ in 0..10 {
        let h = unsafe { CreateFileW(name.as_ptr(), GENERIC_WRITE, 0, null(), OPEN_EXISTING, 0, null_mut()) };
        if h != INVALID_HANDLE_VALUE {
            return Some(h);
        }
        let err = unsafe { GetLastError() };
        if err != ERROR_PIPE_BUSY && err != ERROR_FILE_NOT_FOUND {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}
