//! One app identity for every WinRust process: Task Manager groups them under one entry (AppUserModelID).
//!
//! Windows groups the windows, the child processes and the tray icons of a program by its AppUserModelID, so the same
//! call Chrome and Electron make on start makes Task Manager show one "WinRust" entry with all of them.
use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;

/// Sets this process's AppUserModelID. Call it first thing in `main()`, before any window exists.
///
/// The id is the name Task Manager shows for the group: every WinRust process passes the same one. A failure (an old
/// Windows that lacks the call) is ignored: the program works as before, it is only left ungrouped.
pub fn set(id: &str) {
    let mut wide: Vec<u16> = id.encode_utf16().collect();
    wide.push(0); // the API takes a NUL-terminated UTF-16 string
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(wide.as_ptr());
    }
}
