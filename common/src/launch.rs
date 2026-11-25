//! Starting things the way the shell does, without relying on Explorer being the shell.
//!
//! `explorer.exe <target>` is how WinRust hands most targets over: it starts paths, shortcuts, shell addresses and protocol
//! URIs (`ms-settings:`) with the desktop user's normal, unelevated token. A program is the exception — Explorer treats it
//! as a path to show, and when Explorer is not the shell (WinRust replaces it: see `rust-explorer`) the command only opens
//! a folder window and Windows never starts the program. That is why "Terminal" in the Win+X menu opened the file manager,
//! and why the installer's own "start it now" steps started nothing at all.
//!
//! [`program`] starts those the way Explorer would, with the same restricted token (`runas /trustlevel:0x20000`, the trick
//! the rest of WinRust uses for a normal-rights child) and ShellExecuteW as the last resort.

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
///
/// The program's own folder is the working directory: `runas` inherits the caller's, and a game or a portable app that
/// looks for its data next to the exe then exits at once (Blur.exe from `H:\Blur` did, started from `C:\Windows\System32`).
///
/// The shell goes first unless WinRust itself is elevated. `runas /trustlevel` drops the token, which is what an
/// elevated shell needs, but a restricted token cannot start a program whose manifest asks for elevation: runas exits
/// with an error, nothing runs and no UAC prompt appears - the installer that "does nothing" when clicked. The shell
/// handles both cases. When `runas` does fail, the shell gets its turn from a thread, so the click returns at once.
pub fn program(target: &str) -> bool {
    use std::os::windows::process::CommandExt;
    let target = target.trim().trim_matches('"');
    if target.is_empty() {
        return false;
    }
    let dir = parent_of(target);
    if matches!(ext_of(target).as_str(), "exe" | "com" | "scr") {
        if !crate::proc::elevated() && shell_execute_in(target, "", dir.as_deref()) {
            return true;
        }
        let mut cmd = Command::new("runas");
        cmd.args(["/trustlevel:0x20000", target]).creation_flags(NO_WINDOW);
        if let Some(d) = &dir {
            cmd.current_dir(d);
        }
        if let Ok(mut child) = cmd.spawn() {
            let (t, d) = (target.to_string(), dir.clone());
            std::thread::spawn(move || {
                if child.wait().map(|s| !s.success()).unwrap_or(true) {
                    shell_execute_in(&t, "", d.as_deref());
                }
            });
            return true;
        }
    }
    shell_execute_in(target, "", dir.as_deref())
}

/// The folder the program lives in, when the target has one (a bare `notepad.exe` has none).
fn parent_of(target: &str) -> Option<std::path::PathBuf> {
    let p = std::path::Path::new(target);
    p.parent().filter(|d| !d.as_os_str().is_empty()).map(|d| d.to_path_buf())
}

/// The shell's own "open" association: the last resort, and the normal way for `.msc` / `.cpl` / `.bat`.
fn shell_execute(target: &str) -> bool {
    shell_execute_in(target, "", parent_of(target).as_deref())
}

/// The same with arguments: `line(r"C:\Windows\notepad.exe", r"C:\notes.txt")`. ShellExecuteW passes `params` to the
/// program untouched, with the user's normal token: the Run box never elevates by itself. `dir` is the working
/// directory the handler starts in.
fn shell_execute_in(target: &str, params: &str, dir: Option<&std::path::Path>) -> bool {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::{UI::Shell::ShellExecuteW, UI::WindowsAndMessaging::SW_SHOWNORMAL};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (verb, file, args) = (wide("open"), wide(target), wide(params));
    let dir = dir.map(|d| wide(&d.to_string_lossy()));
    let dir_ptr = dir.as_ref().map_or(null(), |d| d.as_ptr());
    unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), file.as_ptr(), args.as_ptr(), dir_ptr, SW_SHOWNORMAL) as isize > 32 }
}

/// Runs a command line whose program and arguments are already split (`split_command` in the Run box does that without
/// guessing quoting rules). A program without arguments keeps [`program`]'s unelevated `runas` path.
pub fn line(exe: &str, args: &str) -> bool {
    let exe = exe.trim().trim_matches('"');
    if exe.is_empty() {
        return false;
    }
    if args.trim().is_empty() {
        return program(exe);
    }
    shell_execute_in(exe, args.trim(), parent_of(exe).as_deref())
}

#[cfg(test)]
mod tests {
    use super::is_program;

    #[test]
    fn the_program_folder_is_its_working_directory() {
        assert_eq!(super::parent_of(r"H:\Blur\Blur.exe"), Some(std::path::PathBuf::from(r"H:\Blur")));
        // a bare name has no folder of its own: it keeps the caller's
        assert_eq!(super::parent_of("notepad.exe"), None);
    }

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
