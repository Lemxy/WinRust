//! Starting things the way the shell does, without relying on Explorer being the shell.
//!
//! `explorer.exe <target>` is how WinRust hands most targets over: it starts paths, shortcuts, shell addresses and protocol
//! URIs (`ms-settings:`) with the desktop user's normal, unelevated token. A program is the exception — Explorer treats it
//! as a path to show, and when Explorer is not the shell (WinRust replaces it: see `rust-explorer`) the command only opens
//! a folder window and Windows never starts the program. That is why "Terminal" in the Win+X menu opened the file manager,
//! and why the installer's own "start it now" steps started nothing at all.
//!
//! [`program`] starts those the way Explorer would. From an elevated caller (the bar runs as admin, so does the installer)
//! the program gets the session's own token, borrowed from sihost.exe (or Explorer), so it can still ask UAC for more.
//! `runas /trustlevel:0x20000` made a "basic user" token out of the caller's admin one instead, and UAC cannot raise that:
//! an app opened from the bar that asked for admin rights got a copy without them, quietly (WinRust Disk relaunched
//! itself that way forever). Other callers keep `runas /trustlevel`; ShellExecuteW is the last resort.

use std::process::Command;

/// CreateProcessW flag: no console window for the helper.
const NO_WINDOW: u32 = 0x0800_0000;

/// The extensions Explorer, not being the shell, opens as a folder instead of starting the program.
const PROGRAM_EXTS: [&str; 7] = ["exe", "com", "scr", "bat", "cmd", "msc", "cpl"];

fn ext_of(target: &str) -> String {
    match target.trim().trim_matches('"').rsplit_once('.') {
        Some((_, ext)) => ext.trim().to_ascii_lowercase(),
        None => String::new(),
    }
}

/// True for the targets [`program`] has to start itself: `wt.exe`, `taskmgr.exe`, `devmgmt.msc`, `powercfg.cpl`, a `.bat`.
pub fn is_program(target: &str) -> bool {
    let t = target.trim().trim_matches('"');
    if t.is_empty() || is_uri(t) {
        return false;
    }
    PROGRAM_EXTS.contains(&ext_of(t).as_str())
}

/// A protocol URI or a shell address: the shell's, even when its host name ends in `.com` like an exe would.
fn is_uri(t: &str) -> bool {
    if t.contains("://") || t.starts_with("::") {
        return true;
    }
    match t.find(':') {
        Some(i) if (1..8).contains(&i) && !(i == 1 && (t[i + 1..].starts_with('\\') || t[i + 1..].starts_with('/'))) => {
            t[..i].chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
        }
        _ => false,
    }
}

/// Starts a program with the desktop user's normal token, without a UAC prompt: true when something was started.
/// `runas` starts executables only (it cannot pass them arguments), so the console and control-panel files go through the
/// shell's own association, which is also where their elevation comes from.
pub fn program(target: &str) -> bool {
    use std::os::windows::process::CommandExt;
    let target = target.trim().trim_matches('"');
    if target.is_empty() {
        return false;
    }
    if matches!(ext_of(target).as_str(), "exe" | "com" | "scr") {
        if elevated() && as_session_user(&format!("\"{target}\"")) {
            return true;
        }
        if Command::new("runas").args(["/trustlevel:0x20000", target]).creation_flags(NO_WINDOW).spawn().is_ok() {
            return true;
        }
    }
    shell_execute(target)
}

/// This process really has admin rights: the Administrators group is on. The "basic user" token says it is elevated too
/// (High integrity), but there the group is deny-only.
fn elevated() -> bool {
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}

/// Starts `cmdline` with the token of the session's own shell host (sihost.exe, always there and never elevated; Explorer
/// when it is missing): the token a program started from the desktop gets. Needs an elevated caller (CreateProcessWithTokenW
/// wants SeImpersonatePrivilege). False when nothing was started.
fn as_session_user(cmdline: &str) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        Security::{DuplicateTokenEx, SecurityImpersonation, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY, TokenPrimary},
        System::{
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
            RemoteDesktop::ProcessIdToSessionId,
            Threading::{CREATE_UNICODE_ENVIRONMENT, CreateProcessWithTokenW, OpenProcess, OpenProcessToken, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW},
        },
    };
    const MAXIMUM_ALLOWED: u32 = 0x0200_0000;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    unsafe {
        let mut mine = 0u32;
        ProcessIdToSessionId(std::process::id(), &mut mine);
        // the donors in this session, sihost first
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap as isize == -1 {
            return false;
        }
        let mut pe: PROCESSENTRY32W = std::mem::zeroed();
        pe.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut donors: Vec<(u8, u32)> = vec![];
        let mut more = Process32FirstW(snap, &mut pe) != 0;
        while more {
            let name = String::from_utf16_lossy(&pe.szExeFile[..pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)]).to_lowercase();
            let rank = match name.as_str() {
                "sihost.exe" => Some(0),
                "explorer.exe" => Some(1),
                _ => None,
            };
            let mut s = u32::MAX;
            if let Some(r) = rank
                && ProcessIdToSessionId(pe.th32ProcessID, &mut s) != 0
                && s == mine
            {
                donors.push((r, pe.th32ProcessID));
            }
            more = Process32NextW(snap, &mut pe) != 0;
        }
        CloseHandle(snap);
        donors.sort();
        let dir = std::env::var("USERPROFILE").map(|d| wide(&d)).unwrap_or_else(|_| vec![0]);
        for (_, pid) in donors {
            let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if p.is_null() {
                continue;
            }
            let (mut tok, mut prim): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
            let got = OpenProcessToken(p, TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY, &mut tok) != 0;
            CloseHandle(p);
            if !got {
                continue;
            }
            let dup = DuplicateTokenEx(tok, MAXIMUM_ALLOWED, std::ptr::null(), SecurityImpersonation, TokenPrimary, &mut prim) != 0;
            CloseHandle(tok);
            if !dup {
                continue;
            }
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = size_of::<STARTUPINFOW>() as u32;
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let mut cmd = wide(cmdline);
            let ok = CreateProcessWithTokenW(prim, 0, std::ptr::null(), cmd.as_mut_ptr(), CREATE_UNICODE_ENVIRONMENT, std::ptr::null(), dir.as_ptr(), &si, &mut pi) != 0;
            CloseHandle(prim);
            if ok {
                CloseHandle(pi.hProcess);
                CloseHandle(pi.hThread);
                return true;
            }
        }
        false
    }
}

/// The shell's own "open" association: the last resort, and the normal way for `.msc` / `.cpl` / `.bat`.
fn shell_execute(target: &str) -> bool {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{UI::Shell::ShellExecuteW, UI::WindowsAndMessaging::SW_SHOWNORMAL};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(target));
    unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL) as isize > 32 }
}

#[cfg(test)]
mod tests {
    use super::is_program;

    #[test]
    fn programs_are_told_from_shell_targets() {
        for p in ["wt.exe", "taskmgr.exe", r"C:\Windows\System32\taskmgr.exe", "devmgmt.msc", "powercfg.cpl"] {
            assert!(is_program(p), "{p} is a program");
        }
        // these go through explorer.exe like before: it hands them to the shell without needing to be the shell
        for p in ["shell:MyComputerFolder", "ms-settings:", "https://example.com", r"C:\Users", "app.lnk", "regedit"] {
            assert!(!is_program(p), "{p} is not a program");
        }
    }
}
