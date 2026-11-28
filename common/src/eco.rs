//! While the task manager's "Game" profile is on, background programs that keep a CPU busy get efficiency mode (EcoQoS +
//! idle priority), as Windows' own efficiency mode does; the game in front never does. The profile off, or the program
//! brought to the front, gets its old priority back. topbar runs `auto_loop` on a thread.
use std::{collections::HashMap, path::PathBuf, thread, time::Duration};
use windows_sys::Win32::{
    Foundation::{CloseHandle, FILETIME, HANDLE},
    System::{
        Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
        Threading::*,
    },
    UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
};

const TICK: Duration = Duration::from_secs(5);

/// Never touched: what keeps sound, the screen, voice chat and streaming smooth, and the shell itself.
const KEEP: &[&str] = &[
    "system", "registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe", "dwm.exe",
    "audiodg.exe", "fontdrvhost.exe", "svchost.exe", "msmpeng.exe", "topbar.exe", "reshell.exe", "rs-shell.exe", "task manager.exe",
    "discord.exe", "obs64.exe", "obs32.exe", "streamlabs obs.exe", "teamspeak.exe", "ts3client_win64.exe", "mumble.exe",
];

fn profile_file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join(r"WinRust\profile.txt"))
}

/// The "Game" profile is on (rtaskmgr writes `game` as the first line of profile.txt).
pub fn game_on() -> bool {
    profile_file().and_then(|f| std::fs::read_to_string(f).ok()).is_some_and(|t| t.lines().next().map(str::trim) == Some("game"))
}

/// Used over half a core on average during the last tick (CPU time in 100 ns units).
fn hog(cpu_delta: u64) -> bool {
    cpu_delta > TICK.as_nanos() as u64 / 100 / 2
}

fn exempt(name: &str, front: &str) -> bool {
    name == front || KEEP.contains(&name)
}

fn ft(t: FILETIME) -> u64 {
    (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64
}

/// (creation time, user + kernel time) of an open process.
unsafe fn times(h: HANDLE) -> Option<(u64, u64)> {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    (unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) } != 0).then(|| (ft(c), ft(k) + ft(u)))
}

fn throttle(h: HANDLE, on: bool) {
    let s = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 }, StateMask: if on { PROCESS_POWER_THROTTLING_EXECUTION_SPEED } else { 0 } };
    unsafe { SetProcessInformation(h, ProcessPowerThrottling, (&s as *const PROCESS_POWER_THROTTLING_STATE).cast(), size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) };
}

fn processes() -> Vec<(u32, String)> {
    let mut v = vec![];
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return v;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            v.push((e.th32ProcessID, String::from_utf16_lossy(&e.szExeFile[..n]).to_lowercase()));
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    v
}

fn open(pid: u32) -> Option<HANDLE> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SET_INFORMATION, 0, pid) };
    (!h.is_null()).then_some(h)
}

/// What we changed: pid -> (creation time, so a reused pid is left alone; the priority class it had).
type Changed = HashMap<u32, (u64, u32)>;

fn restore(pid: u32, (born, prio): (u64, u32)) {
    if let Some(h) = open(pid) {
        if unsafe { times(h) }.is_some_and(|t| t.0 == born) {
            throttle(h, false);
            unsafe { SetPriorityClass(h, prio) };
        }
        unsafe { CloseHandle(h) };
    }
}

/// Endless loop for topbar. Does nothing (one file check per tick) while the profile is off.
/// shortcut: what was changed lives in memory only, so a topbar restart mid-game leaves those programs slow until they restart; persist it if that bites.
pub fn auto_loop() {
    let mut cpu: HashMap<u32, u64> = HashMap::new();
    let mut changed = Changed::new();
    loop {
        thread::sleep(TICK);
        if !game_on() {
            changed.drain().for_each(|(p, c)| restore(p, c));
            cpu.clear();
            continue;
        }
        let mut fg = 0u32;
        unsafe { GetWindowThreadProcessId(GetForegroundWindow(), &mut fg) };
        if let Some(c) = changed.remove(&fg) {
            restore(fg, c); // brought to the front: full speed again
        }
        let me = std::process::id();
        let procs = processes();
        let front = procs.iter().find(|p| p.0 == fg).map(|p| p.1.clone()).unwrap_or_default();
        let mut seen = HashMap::new();
        for (pid, name) in procs {
            if pid <= 4 || pid == me || pid == fg || exempt(&name, &front) {
                continue;
            }
            let Some(h) = open(pid) else { continue }; // protected, or another user's
            if let Some((born, total)) = unsafe { times(h) } {
                seen.insert(pid, total);
                let was = cpu.get(&pid).copied();
                if !changed.contains_key(&pid) && was.is_some_and(|w| hog(total.saturating_sub(w))) {
                    let prio = unsafe { GetPriorityClass(h) };
                    if prio != 0 && prio != REALTIME_PRIORITY_CLASS {
                        throttle(h, true);
                        unsafe { SetPriorityClass(h, IDLE_PRIORITY_CLASS) };
                        changed.insert(pid, (born, prio));
                    }
                }
            }
            unsafe { CloseHandle(h) };
        }
        changed.retain(|p, _| seen.contains_key(p)); // exited
        cpu = seen;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule() {
        assert!(hog(30_000_000), "3 s of CPU in a 5 s tick");
        assert!(!hog(20_000_000), "2 s of 5: under half a core");
        assert!(exempt("game.exe", "game.exe") && exempt("audiodg.exe", "game.exe") && exempt("discord.exe", ""));
        assert!(!exempt("chrome.exe", "game.exe"));
        assert!(KEEP.iter().all(|k| k.to_lowercase() == *k), "names are compared in lower case");
    }

    #[test]
    fn sees_this_process() {
        let me = std::process::id();
        assert!(processes().iter().any(|p| p.0 == me));
        let h = open(me).unwrap();
        let (born, _) = unsafe { times(h) }.unwrap();
        assert!(born > 0);
        unsafe { CloseHandle(h) };
    }
}
