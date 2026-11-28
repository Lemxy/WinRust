//! Process helpers on plain Win32, so nobody has to start PowerShell (30-60 MB and 0.3 s) just to list or end a process.
use std::{ffi::c_void, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
    System::{
        Diagnostics::{
            Debug::ReadProcessMemory,
            ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
        },
        Threading::{OpenProcess, TerminateProcess},
    },
};

const _: () = assert!(size_of::<usize>() == 8, "the PEB offsets below are x64"); // shortcut: x64 only, add the x86 offsets if a 32-bit build appears

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(h: *mut c_void, class: u32, out: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

/// Calls `f` with every running process (one snapshot).
fn each(mut f: impl FnMut(&PROCESSENTRY32W)) {
    unsafe {
        let s = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if s == INVALID_HANDLE_VALUE {
            return;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(s, &mut e) != 0;
        while ok {
            f(&e);
            ok = Process32NextW(s, &mut e) != 0;
        }
        CloseHandle(s);
    }
}

/// `(pid, exe name)` of every running process.
pub fn list() -> Vec<(u32, String)> {
    let mut v = vec![];
    each(|e| {
        let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
        v.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
    });
    v
}

/// Pids of the running processes that one of `parents` started.
pub fn children(parents: &[u32]) -> Vec<u32> {
    let mut v = vec![];
    if !parents.is_empty() {
        each(|e| if parents.contains(&e.th32ParentProcessID) { v.push(e.th32ProcessID) });
    }
    v
}

/// Full executable path of one process, without taking a process snapshot.
pub fn exe_path(pid: u32) -> Option<std::path::PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::{
        Foundation::{ERROR_INSUFFICIENT_BUFFER, GetLastError},
        System::Threading::{PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW},
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let read = |buf: &mut [u16]| {
            let mut len = buf.len() as u32;
            if QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0 {
                Ok(std::path::PathBuf::from(OsString::from_wide(&buf[..len as usize])))
            } else {
                Err(GetLastError())
            }
        };
        let mut short = [0u16; 520];
        let path = match read(&mut short) {
            Err(ERROR_INSUFFICIENT_BUFFER) => read(&mut vec![0u16; 32768]),
            result => result,
        };
        CloseHandle(h);
        path.ok()
    }
}

/// Exe file name of one process (for example, SearchHost.exe).
pub fn exe_name(pid: u32) -> Option<String> {
    Some(exe_path(pid)?.file_name()?.to_string_lossy().into_owned())
}

/// Compare full image paths. Canonicalize only if Windows returned a different spelling.
pub fn image_paths_match(actual: &std::path::Path, expected: &std::path::Path) -> bool {
    actual == expected || std::fs::canonicalize(actual).ok().is_some_and(|actual| {
        std::fs::canonicalize(expected).is_ok_and(|expected| actual == expected)
    })
}

/// Match the first argument, using Windows quoting rules for the executable and argument.
pub fn first_arg_is(command: &str, expected: &str) -> bool {
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
    let command: Vec<u16> = command.encode_utf16().chain([0]).collect();
    let mut count = 0;
    unsafe {
        let args = CommandLineToArgvW(command.as_ptr(), &mut count);
        if args.is_null() {
            return false;
        }
        let matches = count > 1 && expected.encode_utf16().chain([0]).enumerate()
            .all(|(i, ch)| *(*args.add(1)).add(i) == ch);
        LocalFree(args.cast());
        matches
    }
}

/// Pids of the processes with this exe name (case-insensitive, e.g. `explorer.exe`).
pub fn pids(name: &str) -> Vec<u32> {
    list().into_iter().filter(|(_, n)| n.eq_ignore_ascii_case(name)).map(|p| p.0).collect()
}

/// The command line of a process of the same user, read from its PEB. None when it cannot be opened.
pub fn cmdline(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(0x0410, 0, pid); // QUERY_INFORMATION | VM_READ
        if h.is_null() {
            return None;
        }
        let read = |addr: usize, buf: *mut c_void, n: usize| ReadProcessMemory(h, addr as *const c_void, buf, n, null_mut()) != 0;
        let r = (|| {
            let mut pbi = [0usize; 6]; // PROCESS_BASIC_INFORMATION: [1] = PebBaseAddress
            if NtQueryInformationProcess(h, 0, pbi.as_mut_ptr().cast(), 48, null_mut()) != 0 {
                return None;
            }
            let mut params = 0usize; // PEB.ProcessParameters
            if !read(pbi[1] + 0x20, (&mut params as *mut usize).cast(), 8) {
                return None;
            }
            let mut us = [0u8; 16]; // RTL_USER_PROCESS_PARAMETERS.CommandLine, a UNICODE_STRING: len u16, max u16, pad, buffer ptr
            if !read(params + 0x70, us.as_mut_ptr().cast(), 16) {
                return None;
            }
            let len = u16::from_le_bytes([us[0], us[1]]) as usize;
            let buf = usize::from_le_bytes(us[8..16].try_into().ok()?);
            let mut w = vec![0u16; len / 2];
            if !read(buf, w.as_mut_ptr().cast(), len) {
                return None;
            }
            Some(String::from_utf16_lossy(&w))
        })();
        CloseHandle(h);
        r
    }
}

pub fn kill(pid: u32) {
    unsafe {
        let h = OpenProcess(1, 0, pid); // PROCESS_TERMINATE
        if !h.is_null() {
            TerminateProcess(h, 1);
            CloseHandle(h);
        }
    }
}

/// An explorer.exe started with no path: the one that runs as the shell. Folder windows (started with a path) and the COM host
/// (`/factory ... -Embedding`) are other explorer.exe processes and must stay.
fn is_shell_cmdline(c: &str) -> bool {
    let mut s = c.trim_end().to_ascii_lowercase();
    if let Some(r) = s.strip_suffix("/nouaccheck") {
        if !r.ends_with(char::is_whitespace) {
            return false;
        }
        s = r.trim_end().to_string();
    }
    s.strip_suffix('"').unwrap_or(&s).ends_with("explorer.exe")
}

/// Pids of the explorer.exe processes that are a shell.
pub fn shell_explorers() -> Vec<u32> {
    pids("explorer.exe").into_iter().filter(|&p| cmdline(p).is_some_and(|c| is_shell_cmdline(&c))).collect()
}

pub fn kill_shell_explorers() {
    shell_explorers().into_iter().for_each(kill);
}

/// Whether this process runs with an elevated (administrator) token.
pub fn elevated() -> bool {
    use windows_sys::Win32::{Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation}, System::Threading::{GetCurrentProcess, OpenProcessToken}};
    let (mut t, mut e, mut n) = (null_mut(), TOKEN_ELEVATION { TokenIsElevated: 0 }, 0u32);
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t) == 0 {
            return false;
        }
        let ok = GetTokenInformation(t, TokenElevation, &mut e as *mut _ as _, size_of::<TOKEN_ELEVATION>() as u32, &mut n);
        CloseHandle(t);
        ok != 0 && e.TokenIsElevated != 0
    }
}

/// Starts Explorer as the shell with the user's normal token. Started straight from an unelevated process: under the
/// `runas /trustlevel` token it is a shell that never shows a Store app (Settings stays a blue splash, its window cloaked).
/// That token is only for an elevated caller, which must not hand Explorer its admin rights.
pub fn start_shell_explorer() -> std::io::Result<()> {
    use std::{os::windows::process::CommandExt, process::Command};
    if elevated() {
        Command::new("runas.exe").args(["/trustlevel:0x20000", r"C:\Windows\explorer.exe"]).creation_flags(0x0800_0000).status().map(|_| ())
    } else {
        Command::new(r"C:\Windows\explorer.exe").spawn().map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::is_shell_cmdline;

    #[test]
    fn checks_the_first_argument_with_windows_quoting() {
        assert!(super::first_arg_is(r#""C:\Program Files\WinRust\rust-explorer.exe" "--desktop" extra"#, "--desktop"));
        assert!(!super::first_arg_is(r#""C:\Program Files\WinRust\rust-explorer.exe" folder --desktop"#, "--desktop"));
        assert!(!super::first_arg_is("rust-explorer.exe --desktop-extra", "--desktop"));
        assert!(!super::first_arg_is("rust-explorer.exe", "--desktop"));
    }

    #[test]
    fn only_a_bare_explorer_is_the_shell() {
        assert!(is_shell_cmdline(r"C:\Windows\explorer.exe"));
        assert!(is_shell_cmdline(r#""C:\Windows\explorer.exe""#));
        assert!(is_shell_cmdline(r#""C:\Windows\EXPLORER.EXE" /NoUACCheck "#));
        assert!(!is_shell_cmdline(r"C:\Windows\explorer.exe C:\Users"));
        assert!(!is_shell_cmdline(r"C:\Windows\explorer.exe /factory,{5BD95610-9434-43C2-886C-57852CC8A120} -Embedding"));
        assert!(!is_shell_cmdline(r"C:\Windows\explorer.exe ms-settings:"));
    }

    #[test]
    fn lists_own_process() {
        let me = std::process::id();
        assert!(super::list().iter().any(|(p, _)| *p == me));
        assert!(super::cmdline(me).is_some_and(|c| !c.is_empty()));
        assert!(super::exe_path(me).is_some_and(|p| p.is_absolute()));
        let name = super::exe_name(me).unwrap();
        assert!(name.to_lowercase().ends_with(".exe") && !name.contains('\\'), "{name}");
        assert!(super::list().iter().any(|(p, n)| *p == me && n.eq_ignore_ascii_case(&name)));
    }

    #[test]
    fn lists_the_children_of_a_process() {
        let mut child = std::process::Command::new("cmd").args(["/c", "ping -n 30 127.0.0.1 >nul"]).spawn().unwrap();
        let kids = super::children(&[std::process::id()]);
        let _ = child.kill();
        assert!(kids.contains(&child.id()), "{kids:?}");
        assert!(super::children(&[]).is_empty());
    }
}
