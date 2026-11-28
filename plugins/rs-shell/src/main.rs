// rs-shell              run as the user's shell: wallpaper, Run keys + Startup folders, topbar (restarted when it dies),
//                       explorer.exe as the fallback shell if topbar keeps dying
// rs-shell --enable     make it THIS user's shell (HKCU Winlogon\Shell; other users and the system are untouched)
// rs-shell --disable    back to the normal shell (deletes that one value)
// rs-shell --dry-run    print what it would start, change nothing
// rs-shell --overlay    start topbar next to Explorer; replace Explorer only when the installer's switch is enabled.
// rs-shell --overlay --desktop    opt into the additional desktop even while Explorer remains the shell.
// Put it next to topbar.exe. Recovery from a black screen: Ctrl+Alt+Del -> Task Manager -> run explorer.exe,
// then `rs-shell --disable`; or sign in as another admin and run `reg delete` on the key printed by --enable.
// Also: starts `rust-explorer.exe --desktop` (desktop icons), Win+D = show desktop / bring the windows back (winrust-common),
// Win+E = file manager (rust-explorer.exe if next to us). Put rust-explorer.exe next to rs-shell.exe.
// shortcuts: Win+D only minimizes ordinary top-level windows (no per-monitor / virtual-desktop logic),
// Run/Startup entries are started through `cmd /c start` (no RunOnce, no startup-delay rules, no disabled-list in
// StartupApproved), topbar's "open" still shells out to explorer.exe for /select; upgrade when those matter.
use std::{os::windows::{io::{AsRawHandle, FromRawHandle, OwnedHandle}, process::CommandExt}, path::{Path, PathBuf}, process::{Child, Command}, ptr, thread, time::{Duration, Instant}};
use windows_sys::{core::BOOL, Win32::{Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, RECT, WAIT_FAILED, WAIT_OBJECT_0}, System::Threading::{CreateEventW, CreateMutexW, OpenProcess, PROCESS_TERMINATE, SetEvent, TerminateProcess, WaitForSingleObject}, UI::{Input::KeyboardAndMouse::{MOD_NOREPEAT, MOD_WIN, RegisterHotKey}, WindowsAndMessaging::*}}};
use winrust_common::reg;

const WINLOGON: &str = r"HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon";
const NO_WINDOW: u32 = 0x0800_0000;

/// A Command with null std handles. After FreeConsole() the shell has no valid ones, and spawn() inheriting
/// them fails with "The handle is invalid" (this broke every start the first time).
fn quiet(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(program);
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    c
}

fn reg(args: &[&str]) -> Option<String> {
    let o = Command::new("reg").args(args).creation_flags(NO_WINDOW).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

type Values = [(String, u32, Vec<u8>)];

/// (value name, command) pairs of a Run key's values (`reg::values`); empty commands are left out.
fn parse_run_named(vals: &Values) -> Vec<(String, String)> {
    vals.iter().filter_map(|(n, ty, b)| Some((n.clone(), reg::text(*ty, b)?.trim().to_string())).filter(|(_, v)| !v.is_empty())).collect()
}

/// Names (lowercase) the user turned off in Task Manager's Startup tab: under StartupApproved each entry is a
/// REG_BINARY whose first byte is even when enabled (02, 06) and odd when disabled (03, 07). Explorer skips those.
fn parse_disabled(vals: &Values) -> std::collections::HashSet<String> {
    vals.iter().filter(|(_, ty, b)| *ty == 3 && b.first().is_some_and(|f| f & 1 == 1)).map(|(n, _, _)| n.to_lowercase()).collect()
}

const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

/// The exe a Run command line points at, %VARS% expanded; None when it is not an .exe we can tell apart (scripts, URIs).
fn exe_path(cmd: &str) -> Option<String> {
    let mut c = cmd.trim().to_string();
    while let Some(a) = c.find('%') {
        let b = c[a + 1..].find('%')?;
        let val = std::env::var(&c[a + 1..a + 1 + b]).ok()?;
        c = format!("{}{}{}", &c[..a], val, &c[a + b + 2..]);
    }
    let path = match c.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?.to_string(),
        None => c[..c.to_lowercase().find(".exe")? + 4].to_string(),
    };
    path.to_lowercase().ends_with(".exe").then_some(path)
}

/// Everything the shell should start at logon (commands for `cmd /c start ""`).
fn autostart() -> Vec<String> {
    let mut v = vec![];
    let off = |key: String| parse_disabled(&reg::values(&key));
    // each Run key with the StartupApproved list Task Manager keeps for it; 32-bit programs register under WOW6432Node
    let runs = [
        (r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run", format!(r"HKLM\{APPROVED}\Run")),
        (r"HKLM\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run", format!(r"HKLM\{APPROVED}\Run32")),
        (r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", format!(r"HKCU\{APPROVED}\Run")),
    ];
    for (key, approved) in runs {
        let disabled = off(approved);
        v.extend(parse_run_named(&reg::values(key)).into_iter().filter(|(n, _)| !disabled.contains(&n.to_lowercase())).map(|(_, c)| c));
    }
    let dirs = [
        (std::env::var("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs\Startup")), format!(r"HKCU\{APPROVED}\StartupFolder")),
        (std::env::var("ProgramData").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs\StartUp")), format!(r"HKLM\{APPROVED}\StartupFolder")),
    ];
    for (d, approved) in dirs {
        let Ok(d) = d else { continue };
        let disabled = off(approved); // Startup folder items are listed by file name
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name == "desktop.ini" || disabled.contains(&name) { continue; }
            v.push(format!("\"{}\"", e.path().display()));
        }
    }
    // the same program in the 64- and 32-bit keys starts once
    let mut seen = std::collections::HashSet::new();
    v.retain(|c| seen.insert(c.to_lowercase()));
    v.retain(|c| !c.to_lowercase().contains("rs-shell")); // never start ourselves twice
    // topbar's own helper (reshell) is started from our folder below, not from the Run entry that points into Program Files
    v.retain(|c| !c.to_lowercase().contains(r"\topbar\"));
    // a Run entry whose exe is gone is skipped silently by explorer; `cmd /c start` would pop an error dialog (Rave did)
    v.retain(|c| exe_path(c).is_none_or(|p| std::path::Path::new(&p).exists()));
    v
}

fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain([0]).collect() }

fn dir() -> PathBuf { std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())).unwrap_or_default() }

/// An exe next to us, else the one the topbar plugin installed (the rust-explorer plugin keeps only its own files in its folder).
fn find_exe(name: &str) -> PathBuf {
    let own = dir().join(name);
    if own.exists() { return own; }
    PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into())).join("topbar").join(name)
}

fn bar_window() -> HWND {
    unsafe { FindWindowW(wide("topbar").as_ptr(), ptr::null()) }
}

fn bar_up() -> bool { !bar_window().is_null() }

fn bar_responds() -> bool {
    let window = bar_window();
    !window.is_null() && responds(window) && bar_window() == window
}

/// Explorer is not the shell, so the dialogs it normally places (Windows' "How do you want to open this file?" chooser,
/// Properties, ...) can be created at (0,0) or off the monitor and get clipped (#191). Pull a shown top-level window back
/// inside its monitor when it sticks out. Small windows only, so a maximized or full-screen one is never touched.
unsafe fn keep_inside(h: HWND) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL};
    unsafe {
        if h.is_null() || IsWindowVisible(h) == 0 || IsIconic(h) != 0 || IsZoomed(h) != 0 { return; }
        if GetAncestor(h, GA_ROOT) != h { return; } // top-level only: a child's rectangle is relative to its parent
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(h, &mut r) == 0 || r.right <= r.left || r.bottom <= r.top { return; }
        // not on any monitor at all = parked there on purpose; a tool or no-activate window (bars, popups sliding in) is no dialog
        if MonitorFromWindow(h, MONITOR_DEFAULTTONULL).is_null() { return; }
        if GetWindowLongW(h, GWL_EXSTYLE) as u32 & (WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE) != 0 { return; }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 { return; }
        let a = mi.rcWork;
        if r.right - r.left > a.right - a.left || r.bottom - r.top > a.bottom - a.top { return; } // bigger than the screen: not a stray dialog
        let x = r.left.min(a.right - (r.right - r.left)).max(a.left);
        let y = r.top.min(a.bottom - (r.bottom - r.top)).max(a.top);
        if (x, y) != (r.left, r.top) {
            SetWindowPos(h, ptr::null_mut(), x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

/// Keeps stray dialogs on screen for sessions without Explorer. A WinEvent hook, not a poll: nothing runs until a window
/// shows, and only that window is looked at. Installed on its own thread, which pumps the messages the hook needs.
fn watch_stray_windows() {
    use windows_sys::Win32::UI::Accessibility::SetWinEventHook;
    unsafe extern "system" fn on_show(_hook: *mut std::ffi::c_void, _event: u32, h: HWND, id_object: i32, id_child: i32, _thread: u32, _time: u32) {
        if id_object == OBJID_WINDOW && id_child == CHILDID_SELF as i32 { unsafe { keep_inside(h) }; }
    }
    thread::spawn(|| unsafe {
        SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, ptr::null_mut(), Some(on_show), 0, 0, WINEVENT_OUTOFCONTEXT);
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, ptr::null_mut(), 0, 0) > 0 { TranslateMessage(&m); DispatchMessageW(&m); }
    });
}

/// A minimized window left on screen (or minimized before the hide setting was applied) shows as a caption stub on
/// the desktop: park it where Windows itself parks hidden minimized windows.
unsafe extern "system" fn park(h: HWND, _: LPARAM) -> BOOL {
    unsafe {
        if IsWindowVisible(h) != 0 && IsIconic(h) != 0 {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            if r.left > -30000 { SetWindowPos(h, ptr::null_mut(), -32000, -32000, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE); }
        }
        1
    }
}

/// HKCU\Control Panel\Desktop\WallPaper through the registry API: `reg query` prints in the OEM code page, which garbled
/// non-ASCII paths (a Cyrillic user name), and REG_EXPAND_SZ came back unexpanded.
/// Shell mode only: without explorer's transcoder the system desktop paints only BMP. Next to Explorer (`--overlay`) the
/// system wallpaper is never touched: Explorer shows the original itself.
fn set_wallpaper() {
    let Some(path) = winrust_common::wallpaper::bmp(false) else { return };
    unsafe { SystemParametersInfoW(0x14, 0, wide(&path.to_string_lossy()).as_ptr() as _, 0) }; // SPI_SETDESKWALLPAPER
}

/// Appends a line to %LOCALAPPDATA%\rs-shell\log.txt: a shell has no console, so this is the only way to see what it did.
fn log(msg: &str) {
    use std::io::Write;
    let Ok(base) = std::env::var("LOCALAPPDATA") else { return };
    let dir = PathBuf::from(base).join("rs-shell");
    let _ = std::fs::create_dir_all(&dir);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("log.txt")) { let _ = writeln!(f, "[{t}] {msg}"); }
}

fn run_shell() {
    unsafe { windows_sys::Win32::System::Console::FreeConsole() };
    log("start");
    let _desktop_owner = claim_desktop();
    watch_stray_windows(); // Explorer is gone here: keep its dialogs (the "open this file" chooser) on screen (#191)
    // without explorer a minimized window is drawn as a little caption stub on the desktop (apps started with
    // --minimized left grey rectangles over the icons): hide them, the bar lists the windows anyway
    let mut mm = MINIMIZEDMETRICS { cbSize: size_of::<MINIMIZEDMETRICS>() as u32, iWidth: 0, iHorzGap: 0, iVertGap: 0, iArrange: ARW_HIDE as i32 };
    unsafe { SystemParametersInfoW(SPI_SETMINIMIZEDMETRICS, mm.cbSize, &mut mm as *mut _ as _, 0) };
    // the desktop is blank without explorer: repaint the user's wallpaper (again below, once the session has settled)
    thread::spawn(set_wallpaper); // the first conversion takes a second or two; do not hold up autostart
    let list = autostart();
    log(&format!("autostart: {} entries", list.len()));
    for c in list {
        let r = quiet("cmd").raw_arg(format!("/c start \"\" {c}")).creation_flags(NO_WINDOW).spawn();
        log(&format!("start {c} -> {:?}", r.map(|_| ())));
    }
    thread::sleep(Duration::from_secs(4)); // topbar may already have been started by its own Run entry
    set_wallpaper();
    // no bar yet (its logon task is gone in user mode): start it and its hotkey helper from our folder, with our normal rights
    if !bar_up() { log(&format!("topbar spawn -> {:?}", quiet(find_exe("topbar.exe")).spawn().map(|_| ()))); }
    let hotkeys = find_exe("reshell.exe"); // lives in topbar's folder, not ours
    if hotkeys.exists() { log(&format!("reshell spawn -> {:?}", quiet(&hotkeys).spawn().map(|_| ()))); }
    let desktop = dir().join("rust-explorer.exe");
    // the rust-explorer desktop: class "Progman" (so Wallpaper Engine finds it), title "rs-desktop" (explorer's is "Program Manager")
    let desktop_up = || unsafe { !FindWindowW(wide("Progman").as_ptr(), wide("rs-desktop").as_ptr()).is_null() };
    log(&format!("desktop exe {} exists={} up={}", desktop.display(), desktop.exists(), desktop_up()));
    if desktop.exists() && !desktop_up() { log(&format!("desktop spawn -> {:?}", quiet(&desktop).arg("--desktop").spawn().map(|_| ()))); }
    // the supervisor runs on its own thread; this one owns the hotkeys (RegisterHotKey needs the thread's message loop)
    thread::spawn(supervise);
    hotkey_loop();
}

/// Win+D (show desktop) and Win+E (file manager) for sessions where Explorer is not the shell. Never returns.
fn hotkey_loop() {
    unsafe {
        RegisterHotKey(ptr::null_mut(), 1, MOD_WIN | MOD_NOREPEAT, 'D' as u32);
        RegisterHotKey(ptr::null_mut(), 2, MOD_WIN | MOD_NOREPEAT, 'E' as u32);
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, ptr::null_mut(), 0, 0) > 0 {
            match (m.message == WM_HOTKEY).then_some(m.wParam) {
                Some(1) => winrust_common::desktop::toggle(),
                Some(2) => { let fm = dir().join("rust-explorer.exe"); let _ = quiet(if fm.exists() { fm } else { "explorer.exe".into() }).spawn(); }
                _ => {}
            }
        }
    }
}

/// Keeps topbar alive; if it will not stay up, hands the session to explorer.exe and ends the shell.
fn supervise() {
    let (mut starts, mut since, mut misses) = (0, Instant::now(), 0);
    loop {
        unsafe { EnumWindows(Some(park), 0) };
        if bar_up() { misses = 0; } else { misses += 1; }
        if misses >= 6 { // ~6 s: the loop ticks every second now
            if since.elapsed() > Duration::from_secs(120) { (starts, since) = (0, Instant::now()); }
            starts += 1;
            if starts > 3 || quiet(find_exe("topbar.exe")).spawn().is_err() {
                // topbar will not stay up: hand the session to explorer so the user is never left with nothing
                let _ = quiet(find_exe("topbar.exe")).arg("--restore").creation_flags(NO_WINDOW).status();
                let _ = quiet("explorer.exe").spawn();
                std::process::exit(0);
            }
            misses = 0;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// Request termination on the process handle behind this window. Keep the
/// handle while waiting so a different process cannot be mistaken for it.
fn terminate_window_process(window: HWND, pid: u32) -> Option<OwnedHandle> {
    const SYNCHRONIZE: u32 = 0x0010_0000;
    let process = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        return None;
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(process) };
    if window_pid(window) != Some(pid) || unsafe { TerminateProcess(process, 1) } == 0 {
        return None;
    }
    Some(handle)
}

fn stop_window_process(window: HWND, pid: u32) -> bool {
    terminate_window_process(window, pid).is_some_and(|process| {
        unsafe { WaitForSingleObject(process.as_raw_handle(), 1000) == WAIT_OBJECT_0 }
    })
}

fn window_pid(window: HWND) -> Option<u32> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    (pid != 0).then_some(pid)
}

fn belongs_to(window: HWND, name: &str) -> Option<u32> {
    let pid = window_pid(window)?;
    winrust_common::proc::exe_name(pid).is_some_and(|exe| exe.eq_ignore_ascii_case(name)).then_some(pid)
}

fn shell_window() -> Option<(HWND, u32)> {
    let window = unsafe { GetShellWindow() };
    if window.is_null() {
        return None;
    }
    Some((window, window_pid(window)?))
}

/// Request a stop only for the Explorer that passed the autostart checks.
fn request_explorer_stop(observed: Option<(HWND, u32)>) -> Result<Option<OwnedHandle>, ()> {
    if shell_window() != observed {
        return Err(());
    }
    let Some((window, pid)) = observed else { return Ok(None) };
    if belongs_to(window, "explorer.exe") != Some(pid) {
        return Err(());
    }
    terminate_window_process(window, pid).map(Some).ok_or(())
}

fn claim_desktop() -> Option<OwnedHandle> {
    let marker = unsafe {
        CreateMutexW(ptr::null(), 0, wide(winrust_common::settings::DESKTOP_OWNER_MUTEX).as_ptr())
    };
    (!marker.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(marker) })
}

/// How long after the overlay starts Explorer may be ended even without the markers below: newer Windows builds (26300)
/// no longer write them, and the overlay then waited forever. Explorer gets through the autostart well within this.
const AUTOSTART_GRACE: Duration = Duration::from_secs(90);

/// How long Explorer must have started no new program before it may go. It starts the autostart one program at a time,
/// about a second apart (up to 4 s between the Run keys and the Startup folder on a Windows 10 PC).
const AUTOSTART_QUIET: Duration = Duration::from_secs(8);

/// True once Explorer has begun this session's Run keys and Startup folder. The overlay starts at logon (a scheduled task),
/// before Explorer gets to them: ending Explorer earlier would lose the user's autostart apps. Explorer writes each marker
/// just before its pass, not after it (StartupHasBeenRun 30 ms before the first Startup folder app), so a marker alone
/// does not mean the programs are running: `may_end_explorer` also waits until Explorer stops starting new ones.
fn startup_run() -> bool {
    let mut sid = 0;
    unsafe { windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId(std::process::id(), &mut sid) };
    let k = format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\SessionInfo\{sid}");
    // read in place: this runs every 3 s during logon (it started two reg.exe processes each time)
    ["RunStuffHasBeenRun", "StartupHasBeenRun"].iter().all(|n| winrust_common::reg::exists(&format!(r"{k}\{n}"), None))
}

/// What Explorer's autostart should leave running: the exe names (lowercase) of the entries we can recognise, and how many
/// we cannot (Startup folder shortcuts, scripts, URIs).
fn expected_autostart(list: &[String]) -> (Vec<String>, usize) {
    let exes: Vec<String> = list
        .iter()
        .filter_map(|c| {
            // Squirrel apps (Discord, Slack...) autostart as `Update.exe --processStart "App.exe"`: Update.exe quits at once,
            // the app is what keeps running
            let lc = c.to_lowercase();
            if let Some(i) = lc.find("--processstart") {
                let app = c[i + "--processstart".len()..].trim_start().trim_start_matches('"');
                return app.split(['"', ' ']).next().filter(|a| a.to_lowercase().ends_with(".exe")).map(str::to_lowercase);
            }
            exe_path(c).and_then(|p| std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().to_lowercase()))
        })
        .collect();
    let untracked = list.len() - exes.len();
    (exes, untracked)
}

/// The expected exes that are not running yet.
fn missing_autostart<'a>(expected: &'a [String], running: &[(u32, String)]) -> Vec<&'a str> {
    expected.iter().filter(|e| !running.iter().any(|(_, n)| n.eq_ignore_ascii_case(e))).map(String::as_str).collect()
}

/// Explorer may go once it has started the autostart: its markers say so (older builds), or every program we expect is
/// running and Explorer had its startup delay (~10 s; 30 s when there are entries we cannot watch, such as Startup
/// folder shortcuts, which it runs in the same pass), or the grace time ran out (an app that starts and quits at once).
/// In every case Explorer must have started nothing new for `quiet`: ending it in the middle of a pass loses the rest (#159).
fn may_end_explorer(markers: bool, since_start: Duration, all_running: bool, untracked: usize, quiet: Duration) -> bool {
    let settle = Duration::from_secs(if untracked > 0 { 30 } else { 15 });
    quiet >= AUTOSTART_QUIET && (markers || since_start >= AUTOSTART_GRACE || (all_running && since_start >= settle))
}

/// When Explorer last started a program: `kids` are its children now, `seen` the ones of the last look. Returns the
/// new `last` (now if a child is new) and keeps `kids` as the next `seen`.
fn last_start(seen: &mut Vec<u32>, kids: Vec<u32>, last: Instant, now: Instant) -> Instant {
    let new = kids.iter().any(|k| !seen.contains(k));
    *seen = kids;
    if new { now } else { last }
}

#[derive(Debug, PartialEq, Eq)]
enum ExplorerShell {
    Missing,
    Unresponsive,
    Ready,
}

fn explorer_window() -> Option<(HWND, u32)> {
    let observed = shell_window()?;
    (belongs_to(observed.0, "explorer.exe") == Some(observed.1)).then_some(observed)
}

fn responds(window: HWND) -> bool {
    unsafe { SendMessageTimeoutW(window, WM_NULL, 0, 0, SMTO_ABORTIFHUNG, 100, ptr::null_mut()) != 0 }
}

fn explorer_shell_state() -> ExplorerShell {
    let Some(observed) = explorer_window() else { return ExplorerShell::Missing };
    if responds(observed.0) && shell_window() == Some(observed) {
        ExplorerShell::Ready
    } else {
        ExplorerShell::Unresponsive
    }
}

fn recover_explorer(
    mut state: impl FnMut() -> ExplorerShell,
    launch: impl FnOnce() -> bool,
    mut pause: impl FnMut(),
) -> bool {
    let mut launch = Some(launch);
    for _ in 0..20 {
        match state() {
            ExplorerShell::Ready => return true,
            ExplorerShell::Missing => {
                if let Some(start) = launch.take() {
                    if !start() {
                        return false;
                    }
                }
            }
            ExplorerShell::Unresponsive => {}
        }
        // Spawn can return before Explorer registers its shell window.
        pause();
    }
    state() == ExplorerShell::Ready
}

fn desktop_window() -> HWND {
    unsafe { FindWindowW(wide("Progman").as_ptr(), wide("rs-desktop").as_ptr()) }
}

fn desktop_ready(window: HWND) -> bool {
    // A named window can exist before it is visible or while its UI thread is hung.
    unsafe { !window.is_null() && IsWindowVisible(window) != 0 && IsHungAppWindow(window) == 0 }
}

fn desktop_window_owner(window: HWND, executable: &Path) -> Option<u32> {
    let pid = window_pid(window)?;
    let path = winrust_common::proc::exe_path(pid)?;
    (winrust_common::proc::image_paths_match(&path, executable)
        && winrust_common::proc::cmdline(pid).is_some_and(|args| winrust_common::proc::first_arg_is(&args, "--desktop")))
        .then_some(pid)
}

fn desktop_responds(executable: &Path) -> bool {
    let window = desktop_window();
    if !desktop_ready(window) {
        return false;
    }
    let Some(owner) = desktop_window_owner(window, executable) else { return false };
    // Probe the message loop only at the handoff and make sure the same window answered.
    responds(window) && desktop_window() == window && window_pid(window) == Some(owner)
}

const DESKTOP_START_GRACE: Duration = Duration::from_secs(12);

struct DesktopChild {
    process: Child,
    launched: Instant,
}

fn start_desktop(path: &Path, child: &mut Option<DesktopChild>) {
    if !path.is_file() || !desktop_window().is_null() {
        return;
    }
    if let Some(DesktopChild { process, .. }) = child {
        if !matches!(process.try_wait(), Ok(Some(_))) {
            return; // a previous launch may still be starting or loading the desktop
        }
    }
    match quiet(path).arg("--desktop").spawn() {
        Ok(process) => {
            *child = Some(DesktopChild { process, launched: Instant::now() });
            log("desktop started");
        }
        Err(error) => log(&format!("desktop could not start: {error}")),
    }
}

fn stop_desktop(executable: &Path, child: &mut Option<DesktopChild>) -> bool {
    if let Some(DesktopChild { process, .. }) = child.as_mut() {
        if !matches!(process.try_wait(), Ok(Some(_))) {
            let _ = process.kill();
            if unsafe { WaitForSingleObject(process.as_raw_handle(), 1000) } != WAIT_OBJECT_0 {
                return false;
            }
        }
        *child = None;
    }

    // Settings can have relaunched the desktop. Only stop the file manager
    // instance that's explicitly running as our desktop.
    let window = desktop_window();
    if window.is_null() {
        return true;
    }
    desktop_window_owner(window, executable).is_some_and(|pid| stop_window_process(window, pid))
}

fn desktop_start_expired(ready: bool, age: Option<Duration>) -> bool {
    !ready && age.is_some_and(|age| age >= DESKTOP_START_GRACE)
}

fn restart_desktop(path: &Path, child: &mut Option<DesktopChild>) {
    let window = desktop_window();
    let hung = !window.is_null() && unsafe { IsHungAppWindow(window) } != 0;
    let owned_age = child.as_mut().and_then(|child| {
        (matches!(child.process.try_wait(), Ok(None))
            && (window.is_null() || window_pid(window) == Some(child.process.id())))
            .then(|| child.launched.elapsed())
    });
    let expired = desktop_start_expired(desktop_ready(window), owned_age);
    if (hung || expired) && !stop_desktop(path, child) {
        log("overlay: could not stop the unresponsive desktop");
        return;
    }
    // start_desktop still rejects a responsive window or a launch in progress.
    start_desktop(path, child);
}

#[derive(Debug, PartialEq, Eq)]
enum DesktopAction {
    Wait,
    Restart,
    RestoreExplorer,
}

fn wants_desktop(replace: bool, desktop_overlay: bool) -> bool {
    replace || desktop_overlay
}

#[derive(Default)]
struct DesktopWatch {
    misses: u8,
    retried: bool,
}

impl DesktopWatch {
    fn observe(&mut self, ready: bool, lent: bool, replaced: bool, starting: bool) -> DesktopAction {
        if ready || lent {
            *self = Self::default();
            return DesktopAction::Wait;
        }
        self.misses = self.misses.saturating_add(1);
        if starting || self.misses < 2 {
            return DesktopAction::Wait;
        }
        if !replaced {
            self.misses = 0;
            return DesktopAction::Restart;
        }
        if !self.retried {
            self.retried = true;
            self.misses = 2;
            return DesktopAction::Restart;
        }
        if self.misses >= 4 {
            DesktopAction::RestoreExplorer
        } else {
            DesktopAction::Wait
        }
    }
}

fn overlay(desktop_overlay: bool) {
    unsafe { windows_sys::Win32::System::Console::FreeConsole() };
    // Signal the existing watcher even if this copy is only forwarding an explicit opt-in.
    let desktop_request = unsafe { CreateEventW(ptr::null(), 0, 0, wide("Local\\WinRust-rs-overlay-desktop").as_ptr()) };
    if desktop_overlay && !desktop_request.is_null() {
        unsafe { SetEvent(desktop_request) };
    }
    // The logon task, the Run entry and a manual launch must share one watchdog.
    let instance = unsafe {
        CreateMutexW(ptr::null(), 0, wide(winrust_common::settings::OVERLAY_MUTEX).as_ptr())
    };
    if instance.is_null() || unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return; // Windows releases our handle when the process exits
    }

    let replace = winrust_common::settings::replace_explorer_requested();
    let requested = desktop_overlay || (!desktop_request.is_null()
        && unsafe { WaitForSingleObject(desktop_request, 0) } == WAIT_OBJECT_0);
    let desktop = wants_desktop(replace, requested).then(|| dir().join("rust-explorer.exe"));
    log(&format!("overlay start (replace explorer: {replace}, custom desktop: {})", desktop.is_some()));
    let wp = desktop.as_ref().map(|_| thread::spawn(|| winrust_common::wallpaper::bmp(false)));
    if !bar_up() { log(&format!("topbar spawn -> {:?}", quiet(find_exe("topbar.exe")).spawn().map(|_| ()))); }
    // Win+I / Win+E / Win+D live in reshell: its Run entry may be missing (another account's HKCU).
    let hk = find_exe("reshell.exe");
    if hk.exists() { log(&format!("reshell spawn -> {:?}", quiet(&hk).spawn().map(|_| ()))); }
    if let Some(wp) = wp {
        let _ = wp.join();
    }
    overlay_watch(replace, desktop, desktop_request);
}

/// Gives the system taskbar back and verifies that Explorer is responding as the shell.
fn restore_overlay_shell(replace: bool, desktop: Option<&Path>, child: &mut Option<DesktopChild>) {
    if desktop.is_some_and(|path| !stop_desktop(path, child)) {
        log("overlay: could not confirm the WinRust desktop stopped");
    }
    let _ = quiet(find_exe("topbar.exe")).arg("--restore").creation_flags(NO_WINDOW).status();
    if replace && !winrust_common::settings::explorer_lent() {
        let recovered = recover_explorer(
            explorer_shell_state,
            || match winrust_common::proc::start_shell_explorer() {
                Ok(()) => true,
                Err(error) => {
                    log(&format!("overlay: could not launch Explorer: {error}"));
                    false
                }
            },
            || thread::sleep(Duration::from_millis(250)),
        );
        log(if recovered { "overlay: Explorer shell is responding" } else { "overlay: could not confirm Explorer shell recovery" });
    }
}

/// Watches the selected surfaces and only gives up Explorer once both replacements are ready.
fn overlay_watch(replace: bool, mut desktop: Option<PathBuf>, desktop_request: HANDLE) {
    let (mut starts, mut misses, mut stable, mut replaced) = (0, 0, 0, false);
    let mut desktop_watch = DesktopWatch::default();
    let mut child = None;
    if let Some(path) = &desktop {
        if !winrust_common::settings::explorer_lent() {
            start_desktop(path, &mut child);
        }
    }

    let mut began = Instant::now();
    let (expected, untracked) = if replace { expected_autostart(&autostart()) } else { (vec![], 0) };
    if replace { log(&format!("autostart to wait for: {} exe(s) {:?}, {} other entr(ies)", expected.len(), expected, untracked)); }
    let (mut kids, mut last) = (vec![], began);
    let mut observed_shell = if replace { shell_window() } else { None };
    let mut use_startup_markers = true;
    let mut desktop_owner = None;
    let mut stopping_explorer: Option<OwnedHandle> = None;
    loop {
        if desktop.is_none() && !desktop_request.is_null() {
            match unsafe { WaitForSingleObject(desktop_request, 3000) } {
                WAIT_OBJECT_0 => {
                    let path = dir().join("rust-explorer.exe");
                    let _ = winrust_common::wallpaper::bmp(false);
                    if !winrust_common::settings::explorer_lent() {
                        start_desktop(&path, &mut child);
                    }
                    desktop = Some(path);
                    continue;
                }
                WAIT_FAILED => thread::sleep(Duration::from_secs(3)),
                _ => {}
            }
        } else {
            thread::sleep(Duration::from_secs(3));
        }
        if !replaced && desktop_owner.is_some()
            && stopping_explorer.as_ref().is_none_or(|process| {
                unsafe { WaitForSingleObject(process.as_raw_handle(), 0) == WAIT_OBJECT_0 }
            })
        {
            stopping_explorer = None;
            replaced = true;
            watch_stray_windows();
            // Returning from the watchdog also ends the hotkey thread.
            thread::spawn(hotkey_loop);
        }
        let bar = if replaced { bar_responds() } else { bar_up() };
        if let Some(path) = &desktop {
            let window = desktop_window();
            let ready = desktop_ready(window);
            // Settings borrows Explorer temporarily. An Explorer started outside
            // the watcher also takes priority over a desktop restart.
            let lent = !ready && (winrust_common::settings::explorer_lent()
                || (replaced && explorer_shell_state() == ExplorerShell::Ready));
            let starting = !ready && !lent && child.as_mut().is_some_and(|child: &mut DesktopChild| {
                child.launched.elapsed() < DESKTOP_START_GRACE
                    && (window.is_null() || unsafe { IsHungAppWindow(window) } == 0)
                    && matches!(child.process.try_wait(), Ok(None))
            });
            match desktop_watch.observe(ready, lent, replaced, starting) {
                DesktopAction::Wait => {}
                DesktopAction::Restart => restart_desktop(path, &mut child),
                DesktopAction::RestoreExplorer => {
                    log("overlay: desktop did not recover, returning to the system shell");
                    drop(desktop_owner.take());
                    restore_overlay_shell(replace, Some(path), &mut child);
                    return;
                }
            }

            if replace && !replaced && stopping_explorer.is_none() {
                let now = Instant::now();
                let shell = shell_window();
                if observed_shell != shell {
                    observed_shell = shell;
                    use_startup_markers = false; // the previous Explorer may have written these
                    kids.clear();
                    began = now;
                    last = now;
                    stable = 0;
                }
                stable = if bar && ready { stable + 1 } else { 0 };
                let missing = missing_autostart(&expected, &winrust_common::proc::list());
                let children = observed_shell.map_or_else(Vec::new, |(_, pid)| winrust_common::proc::children(&[pid]));
                last = last_start(&mut kids, children, last, now);
                let markers = stable >= 3 && use_startup_markers && startup_run();
                if stable >= 3 && may_end_explorer(markers, began.elapsed(), missing.is_empty(), untracked, last.elapsed())
                    && !winrust_common::settings::explorer_lent() && winrust_common::settings::replace_explorer_requested()
                    && bar_responds() && desktop_responds(path)
                {
                    log(&format!("autostart done (markers: {}, after {} s, last start {} s ago, not running: {:?})", markers, began.elapsed().as_secs(), last.elapsed().as_secs(), missing));
                    // Publish ownership before Windows can lose its old shell.
                    // Settings will see either Explorer or a WinRust desktop owner.
                    if let Some(owner) = claim_desktop() {
                        if !winrust_common::settings::explorer_lent() {
                            match request_explorer_stop(observed_shell) {
                                Ok(pending) => {
                                    desktop_owner = Some(owner);
                                    stopping_explorer = pending;
                                }
                                Err(()) => {
                                    stable = 0;
                                    log("overlay: Windows shell stop was not accepted");
                                }
                            }
                        }
                    }
                }
            }
        }

        if bar { misses = 0; continue; }
        if stopping_explorer.is_some() { continue; }
        misses += 1;
        if misses < 2 || (replace && winrust_common::settings::explorer_lent()) { continue; }
        starts += 1;
        if starts > 3 || quiet(find_exe("topbar.exe")).spawn().is_err() {
            log("overlay: topbar keeps dying, returning to the system shell");
            drop(desktop_owner.take());
            restore_overlay_shell(replace, desktop.as_deref(), &mut child);
            return;
        }
        misses = 0;
    }
}

/// Windows will not start a shell from a folder the user can write to (a boot from %LOCALAPPDATA% fell back to explorer),
/// so the copy in Program Files is only a loader: it runs the real rs-shell from %LOCALAPPDATA%\plugin-launcher\bin, which
/// can be updated without admin. true = the user-folder shell took over (the loader must not run the shell logic as well).
/// A missing copy or one that dies within 3 s (it started nothing yet) leaves false: the loader then does the job itself.
fn bootstrap() -> bool {
    let Ok(base) = std::env::var("LOCALAPPDATA") else { return false };
    let real = PathBuf::from(base).join(r"plugin-launcher\bin\rs-shell.exe");
    let me = std::env::current_exe().and_then(|p| p.canonicalize()).ok();
    let age = |p: &PathBuf| p.metadata().and_then(|m| m.modified()).ok();
    // an older user copy is a leftover (nothing writes there any more): the installed shell runs itself
    if !real.exists() || real.canonicalize().ok() == me || age(&real) < std::env::current_exe().ok().as_ref().and_then(age) {
        return false;
    }
    let t = Instant::now();
    match quiet(&real).spawn() {
        Ok(mut child) => {
            log(&format!("loader: started {}", real.display()));
            let _ = child.wait();
            t.elapsed() > Duration::from_secs(3)
        }
        Err(e) => {
            log(&format!("loader: cannot start {}: {e}", real.display()));
            false
        }
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let code = match a.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => { if !bootstrap() { run_shell(); } 0 }
        ["--overlay"] => { overlay(false); 0 }
        ["--overlay", "--desktop"] => { overlay(true); 0 }
        ["--dry-run"] => {
            println!("wallpaper: {:?}", reg::get_text(r"HKCU\Control Panel\Desktop", "WallPaper"));
            autostart().iter().for_each(|c| println!("autostart: {c}"));
            println!("topbar: {} ({})", find_exe("topbar.exe").display(), if find_exe("topbar.exe").exists() { "found" } else { "MISSING" });
            println!("explorer ran this session's autostart: {}", startup_run());
            0
        }
        ["--enable"] => {
            let exe = std::env::current_exe().unwrap_or_default();
            if !find_exe("topbar.exe").exists() {
                eprintln!("refusing: topbar.exe is not next to {} (no bar = no way to launch anything)", exe.display());
                1
            } else if reg(&["add", WINLOGON, "/v", "Shell", "/t", "REG_SZ", "/d", &exe.to_string_lossy(), "/f"]).is_some() {
                println!("done: you get rs-shell instead of explorer at the next sign-in.\nundo: rs-shell --disable   (or: reg delete \"{WINLOGON}\" /v Shell /f)");
                0
            } else { eprintln!("reg add failed"); 1 }
        }
        ["--disable"] => {
            // deleting a value that is not there is fine too
            let _ = reg(&["delete", WINLOGON, "/v", "Shell", "/f"]);
            println!("done: normal shell at the next sign-in");
            0
        }
        _ => { eprintln!("usage: rs-shell [--enable | --disable | --dry-run | --overlay [--desktop]]"); 2 }
    };
    std::process::exit(code);
}

#[cfg(test)]
mod tests {
    use super::{exe_path, may_end_explorer};
    use std::time::Duration;
    #[test]
    fn waits_for_a_slow_child_then_attempts_one_restart_before_fallback() {
        use super::{DesktopAction::*, DesktopWatch};
        let mut watch = DesktopWatch::default();
        for _ in 0..4 {
            assert_eq!(watch.observe(false, false, true, true), Wait);
        }
        assert_eq!(watch.observe(false, false, true, false), Restart);
        assert_eq!(watch.observe(false, false, true, true), Wait);
        assert_eq!(watch.observe(false, false, true, false), RestoreExplorer);
    }

    #[test]
    fn recovery_launches_for_a_missing_explorer_and_requires_a_responsive_shell() {
        use super::{ExplorerShell::*, recover_explorer};
        let mut states = [Missing, Missing, Unresponsive, Ready].into_iter();
        let mut launches = 0;
        assert!(recover_explorer(|| states.next().unwrap(), || { launches += 1; true }, || {}));
        assert_eq!(launches, 1);
        let mut states = [Unresponsive, Missing, Ready].into_iter();
        assert!(recover_explorer(|| states.next().unwrap(), || { launches += 1; true }, || {}));
        assert_eq!(launches, 2);
        assert!(!recover_explorer(|| Missing, || true, || {}));
        assert!(!recover_explorer(|| Unresponsive, || panic!("Explorer is already open"), || {}));
        assert!(recover_explorer(|| Ready, || panic!("Explorer is already ready"), || panic!("no wait needed")));
    }

    #[test]
    fn a_matching_basename_is_not_enough_to_identify_the_desktop_binary() {
        let executable = std::env::current_exe().unwrap();
        let other = executable.with_file_name("other-dir").join(executable.file_name().unwrap());
        assert!(winrust_common::proc::image_paths_match(&executable, &executable));
        assert!(!winrust_common::proc::image_paths_match(&other, &executable));
    }

    #[test]
    fn custom_desktop_requires_replacement_or_explicit_overlay() {
        assert!(!super::wants_desktop(false, false));
        assert!(super::wants_desktop(true, false));
        assert!(super::wants_desktop(false, true));
        assert!(super::wants_desktop(true, true));
    }

    #[test]
    fn desktop_failure_restarts_then_returns_to_explorer_after_handoff() {
        use super::{DesktopAction::*, DesktopWatch};
        let mut watch = DesktopWatch::default();
        let expected = [Wait, Restart, Wait, RestoreExplorer];
        for action in expected {
            assert_eq!(watch.observe(false, false, true, false), action);
        }
        assert_eq!(watch.observe(false, true, true, false), Wait);
        assert_eq!(watch.misses, 0, "Settings owns Explorer temporarily");
        assert_eq!(watch.observe(false, false, true, false), Wait);
        assert_eq!(watch.observe(true, false, true, false), Wait);
        assert_eq!(watch.misses, 0, "a recovered desktop clears the counter");
    }

    #[test]
    fn a_recovery_launch_gets_its_full_startup_grace() {
        use super::{DesktopAction::*, DesktopWatch};
        let mut watch = DesktopWatch { misses: 3, retried: true };
        assert_eq!(watch.observe(false, false, true, true), Wait);
        assert_eq!(watch.observe(false, false, true, true), Wait);
        assert_eq!(watch.observe(false, false, true, false), RestoreExplorer);
    }

    #[test]
    fn a_desktop_child_has_time_to_start_but_cannot_block_recovery_forever() {
        use super::{DESKTOP_START_GRACE, desktop_start_expired};
        assert!(!desktop_start_expired(false, None));
        assert!(!desktop_start_expired(false, Some(Duration::from_secs(1))));
        assert!(!desktop_start_expired(true, Some(DESKTOP_START_GRACE)));
        assert!(desktop_start_expired(false, Some(DESKTOP_START_GRACE)));
    }

    #[test]
    fn a_desktop_failure_before_handoff_keeps_explorer() {
        use super::{DesktopAction::*, DesktopWatch};
        let mut watch = DesktopWatch::default();
        let expected = [Wait, Restart, Wait, Restart];
        for action in expected {
            assert_eq!(watch.observe(false, false, false, false), action);
        }
    }

    #[test]
    fn ends_explorer_on_markers_or_after_grace() {
        let (s, q) = (Duration::from_secs, Duration::from_secs(8)); // q: Explorer has started nothing new for 8 s
        assert!(may_end_explorer(true, s(9), false, 0, q));
        assert!(!may_end_explorer(false, s(30), false, 0, q)); // something still missing
        assert!(may_end_explorer(false, s(90), false, 3, q)); // grace over, whatever is missing
        // everything we watch is running: after Explorer's startup delay, longer with shortcuts we cannot watch
        assert!(!may_end_explorer(false, s(9), true, 0, q));
        assert!(may_end_explorer(false, s(15), true, 0, q));
        assert!(!may_end_explorer(false, s(20), true, 1, q));
        assert!(may_end_explorer(false, s(30), true, 1, q));
    }
    #[test]
    fn waits_while_explorer_still_starts_programs() {
        // #159: the markers are written before Explorer starts the programs; ending it then lost the rest of the pass
        let recent = Duration::from_secs(2);
        assert!(!may_end_explorer(true, Duration::from_secs(20), true, 0, recent));
        assert!(!may_end_explorer(false, Duration::from_secs(90), false, 0, recent));
        assert!(!may_end_explorer(false, Duration::from_secs(30), true, 1, recent));
    }
    #[test]
    fn notices_each_program_explorer_starts() {
        let t0 = std::time::Instant::now();
        let (t1, t2, t3) = (t0 + Duration::from_secs(3), t0 + Duration::from_secs(6), t0 + Duration::from_secs(9));
        let mut seen = vec![];
        assert_eq!(super::last_start(&mut seen, vec![10, 11], t0, t1), t1); // the first look: everything is new
        assert_eq!(super::last_start(&mut seen, vec![10, 11], t1, t2), t1); // nothing new
        assert_eq!(super::last_start(&mut seen, vec![11], t1, t2), t1); // one quit: still nothing new
        assert_eq!(super::last_start(&mut seen, vec![11, 12], t1, t3), t3); // a new one
    }
    #[test]
    fn watches_the_autostart_exes() {
        let list = [r#""C:\a b\Steam.exe" -silent"#.to_string(), r"C:\d\y.exe /background".to_string(), r#""C:\Users\u\Startup\x.lnk""#.to_string()];
        let (exes, untracked) = super::expected_autostart(&list);
        assert_eq!((exes.clone(), untracked), (vec!["steam.exe".to_string(), "y.exe".to_string()], 1));
        let running = [(1, "STEAM.EXE".to_string()), (2, "explorer.exe".to_string())];
        assert_eq!(super::missing_autostart(&exes, &running), ["y.exe"]);
        assert!(super::missing_autostart(&exes, &[(1, "steam.exe".into()), (3, "y.exe".into())]).is_empty());
        // Squirrel: the app it starts, not Update.exe
        let sq = [r#"C:\Users\u\AppData\Local\Discord\Update.exe --processStart Discord.exe"#.to_string(), r#""C:\S\Update.exe" --processStart "slack.exe" --process-start-args "--startup""#.to_string()];
        assert_eq!(super::expected_autostart(&sq).0, ["discord.exe", "slack.exe"]);
    }
    #[test]
    fn finds_the_exe_in_run_commands() {
        assert_eq!(exe_path(r#""C:\a b\x.exe" --hidden"#).as_deref(), Some(r"C:\a b\x.exe"));
        assert_eq!(exe_path(r"C:\d\y.exe /background").as_deref(), Some(r"C:\d\y.exe"));
        assert_eq!(exe_path("script.bat"), None);
        assert!(exe_path(r"%windir%\system32\z.exe").unwrap().to_lowercase().ends_with(r"system32\z.exe"));
    }
    #[test]
    fn reads_run_values() {
        let sz = |n: &str, ty: u32, s: &str| (n.to_string(), ty, s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect::<Vec<u8>>());
        let vals = [sz("Foo", 1, "foo.exe --x"), sz("Bar", 2, "%APPDATA%\\bar.exe"), sz("Empty", 1, " "), ("Bin".into(), 3, vec![1, 2])];
        let named = super::parse_run_named(&vals);
        assert_eq!(named.iter().map(|x| x.1.as_str()).collect::<Vec<_>>(), ["foo.exe --x", "%APPDATA%\\bar.exe"]);
        assert_eq!(super::parse_run_named(&[sz("Steam Client", 1, "\"C:\\s\\steam.exe\" -silent")]), [("Steam Client".to_string(), "\"C:\\s\\steam.exe\" -silent".to_string())]);
    }
    #[test]
    fn reads_task_managers_disabled_entries() {
        let b = |n: &str, d: &[u8]| (n.to_string(), 3u32, d.to_vec());
        let out = [b("Discord", &[3, 0, 0, 0, 0x6A]), b("OneDrive", &[2, 0, 0, 0]), b("Spotify Web", &[7, 0, 0, 0, 0xFF]), b("Odd", &[6]), b("Junk", &[]), ("Sz".into(), 1, vec![3, 0])];
        let off = super::parse_disabled(&out);
        assert!(off.contains("discord") && off.contains("spotify web"));
        assert!(!off.contains("onedrive") && !off.contains("odd") && !off.contains("junk"));
    }
}
