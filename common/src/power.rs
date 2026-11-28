//! Shut down, restart, sleep, sign out. Used by the topbar (sleep) and the desktop's Alt+F4 box (rust-explorer).
use std::{
    io,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    Security::{AdjustTokenPrivileges, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES},
    System::{Power::SetSuspendState, Threading::{GetCurrentProcess, OpenProcessToken}},
};

const NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    ShutDown,
    Restart,
    Sleep,
    SignOut,
}

impl Action {
    /// What `shutdown.exe` is given; None: not its job (sleep).
    fn shutdown_args(self) -> Option<&'static [&'static str]> {
        match self {
            Action::ShutDown => Some(&["/s", "/t", "0"]),
            Action::Restart => Some(&["/r", "/t", "0"]),
            Action::SignOut => Some(&["/l"]),
            Action::Sleep => None,
        }
    }

    /// Does it. Sleep returns only after wake-up: call from a thread that may block.
    pub fn run(self) -> io::Result<()> {
        match self.shutdown_args() {
            Some(args) => Command::new("shutdown.exe").args(args).creation_flags(NO_WINDOW).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(drop),
            None => {
                sleep_now();
                Ok(())
            }
        }
    }
}

/// Sleep (S3 / modern standby), never hibernate. SetSuspendState needs the shutdown privilege switched on in our token.
/// Not `rundll32 powrprof.dll,SetSuspendState`: rundll32 passes garbage arguments and the PC hibernates instead.
pub fn sleep_now() {
    unsafe {
        let mut tok = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &mut tok) != 0 {
            let mut tp: TOKEN_PRIVILEGES = std::mem::zeroed();
            tp.PrivilegeCount = 1;
            tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
            let name: Vec<u16> = "SeShutdownPrivilege".encode_utf16().chain([0]).collect();
            if LookupPrivilegeValueW(null(), name.as_ptr(), &mut tp.Privileges[0].Luid) != 0 {
                AdjustTokenPrivileges(tok, 0, &tp, 0, null_mut(), null_mut());
            }
            CloseHandle(tok);
        }
        SetSuspendState(false, false, false);
    }
}

#[cfg(test)]
mod tests {
    use super::Action;

    #[test]
    fn each_action_asks_for_the_right_thing() {
        assert_eq!(Action::ShutDown.shutdown_args(), Some(&["/s", "/t", "0"][..]));
        assert_eq!(Action::Restart.shutdown_args(), Some(&["/r", "/t", "0"][..]));
        assert_eq!(Action::SignOut.shutdown_args(), Some(&["/l"][..]));
        assert_eq!(Action::Sleep.shutdown_args(), None);
    }
}
