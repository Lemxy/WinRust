//! rs-notify-bridge: forwards the notifications Windows itself collects to the WinRust bar.
//!
//! Discord, Steam, Serein and every other program that posts a toast goes through the Windows Notification Platform.
//! WinRust has no `explorer.exe` and `ShellExperienceHost` is blocked, so those toasts never draw, but the platform still
//! stores them: `Windows.UI.Notifications.Management.UserNotificationListener` returns them and this process copies each
//! one to `\\.\pipe\winrust-notify` as a `winrust_common::notify::NotifyWire`. The bar shows them like any other toast.
//!
//! Design notes:
//! - `GetNotificationsAsync` is polled every ~2 s. The `NotificationChanged` event needs a message loop and a packaged
//!   process to be reliable, and a desktop console has neither.
//! - The listener does not need package identity on Windows 11 26300 (measured, see tools\notify-bridge.md); the sparse
//!   package in tools\notify-bridge is still the supported route and keeps it working if a later build enforces the
//!   capability.
//! - Notifications that already existed when the bridge started are marked as seen without being sent, so a restart does
//!   not replay the whole Windows notification centre. `--all` overrides that for testing.
//! - Nothing about the pipe protocol changes: one `NotifyWire` per connection, exactly like `rs-notify`.
//!
//! Exit codes: 0 ok, 1 bad arguments, 2 the listener API is unavailable, 3 `--once` and the bar was not listening,
//! 4 access to notifications was denied, 5 the `--db` fallback could not run.

#![windows_subsystem = "console"]

use std::{
    collections::VecDeque,
    env, fs,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use windows::{
    UI::Notifications::{NotificationKinds, UserNotification},
    UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_WRITE, HANDLE,
        INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{CreateFileW, WriteFile, OPEN_EXISTING},
    System::{
        Com::{CoInitializeEx, COINIT_MULTITHREADED},
        Console::{GetConsoleProcessList, GetConsoleWindow},
        Threading::CreateMutexW,
    },
    UI::WindowsAndMessaging::{ShowWindow, SW_HIDE},
};

use winrust_common::notify::{NotifyWire, PIPE};

const EXIT_OK: i32 = 0;
const EXIT_USAGE: i32 = 1;
const EXIT_NO_LISTENER: i32 = 2;
const EXIT_NO_BAR: i32 = 3;
const EXIT_NO_ACCESS: i32 = 4;
const EXIT_DB: i32 = 5;

/// Notifications created within this window before the first poll are still sent: the bridge may start a little after
/// the app that raised them (logon, a bar restart).
const START_GRACE: Duration = Duration::from_secs(120);
/// How many notification keys are remembered for deduplication.
const SEEN_MAX: usize = 200;
/// How many wires wait for the bar to come back.
const PENDING_MAX: usize = 64;
/// `FILETIME` ticks (100 ns) between 1601-01-01 and 1970-01-01, the base of `Foundation::DateTime`.
const FILETIME_UNIX_EPOCH: i64 = 116_444_736_000_000_000;
const TICKS_PER_SEC: i64 = 10_000_000;

const USAGE: &str = "\
rs-notify-bridge: forwards Windows notifications to the WinRust bar

Usage:
  rs-notify-bridge [options]

Options:
  --once             poll once, send what is fresh, exit (testing)
  --all              send every notification Windows has now, ignoring age and the seen list (implies --once)
  --interval MS      poll period, default 2000
  --verbose, -v      log every notification and every write
  --db [PATH]        diagnose the SQLite fallback (never used automatically)
  -h, --help         this help

The bar must be running (topbar owns the pipe). If it is not, unsent notifications are kept and retried.
Exit codes: 0 ok, 1 bad arguments, 2 no notification listener, 3 no bar with --once, 4 access denied, 5 no --db tool.";

fn main() {
    std::process::exit(run());
}

// ---------------------------------------------------------------- arguments

struct Args {
    once: bool,
    all: bool,
    verbose: bool,
    interval: Duration,
    db: Option<Option<String>>,
}

fn parse_args() -> Result<Args, i32> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut out = Args { once: false, all: false, verbose: false, interval: Duration::from_secs(2), db: None };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--once" => out.once = true,
            "--all" => out.all = true,
            "--verbose" | "-v" => out.verbose = true,
            "--interval" => {
                i += 1;
                let v = args.get(i).ok_or(EXIT_USAGE)?;
                let ms = v.parse::<u64>().map_err(|_| EXIT_USAGE)?;
                out.interval = Duration::from_millis(ms.max(200));
            }
            "--db" => {
                // An optional path: the next argument, unless it is another option.
                let next = args.get(i + 1).filter(|a| !a.starts_with('-')).cloned();
                if next.is_some() {
                    i += 1;
                }
                out.db = Some(next);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Err(EXIT_OK);
            }
            other => {
                eprintln!("rs-notify-bridge: unknown option {other:?}\n\n{USAGE}");
                return Err(EXIT_USAGE);
            }
        }
        i += 1;
    }
    if out.all {
        out.once = true;
    }
    Ok(out)
}

// ---------------------------------------------------------------- main flow

fn run() -> i32 {
    let args = match parse_args() {
        Ok(a) => a,
        Err(code) => return code,
    };

    if let Some(path) = &args.db {
        return db_diagnostic(path.as_deref());
    }

    // WinRT calls are made from this thread; an MTA apartment is what a console process wants. S_FALSE (already
    // initialised) is not an error.
    unsafe {
        CoInitializeEx(null(), COINIT_MULTITHREADED as u32);
    }

    if !args.once && !args.verbose {
        hide_own_console();
    }

    // A logon task and a manual start must not both forward the same notifications.
    let _single = if args.once || args.all {
        None
    } else {
        match single_instance() {
            Some(h) => Some(h),
            None => {
                if args.verbose {
                    println!("rs-notify-bridge: another instance is already running");
                }
                return EXIT_OK;
            }
        }
    };

    let listener = match UserNotificationListener::Current() {
        Ok(l) => l,
        Err(e) => {
            eprintln!(
                "rs-notify-bridge: Windows.UI.Notifications.Management is unavailable (0x{:08X} {}).\n\
                 This Windows build does not expose the notification listener; see tools\\notify-bridge.md for the \
                 database fallback.",
                e.code().0 as u32, e.message()
            );
            return EXIT_NO_LISTENER;
        }
    };

    if let Err(code) = ensure_access(&listener, args.verbose) {
        return code;
    }

    let mut state = State::load(args.verbose);
    if args.all {
        state.seen.clear();
    }

    let mut bar_up = true;
    let start = Instant::now();

    loop {
        match poll(&listener) {
            Ok(mut items) => {
                items.sort_by_key(|n| n.created);
                if args.verbose {
                    log(start, &format!("poll: {} notification(s)", items.len()));
                }
                process(&args, &mut state, items, &mut bar_up, start);
            }
            Err(e) => {
                eprintln!(
                    "rs-notify-bridge: GetNotificationsAsync failed (0x{:08X} {})",
                    e.code().0 as u32,
                    e.message()
                );
                // A revoked permission makes the API return nothing rather than fail; a real failure is worth one
                // status check so the message says what happened.
                if let Ok(s) = listener.GetAccessStatus() {
                    if s != UserNotificationListenerAccessStatus::Allowed {
                        eprintln!("rs-notify-bridge: access to notifications is now {s:?}");
                    }
                }
            }
        }

        // Retry whatever the bar missed, whether or not this poll brought anything new.
        drain_pending(&args, &mut state, &mut bar_up, start);

        if args.once {
            state.save();
            if state.pending.is_empty() {
                return EXIT_OK;
            }
            eprintln!("rs-notify-bridge: {} notification(s) could not be delivered: is topbar running?", state.pending.len());
            return EXIT_NO_BAR;
        }

        std::thread::sleep(args.interval);
    }
}

/// Reads the current access status and, only when Windows says the user has not decided yet, asks once.
fn ensure_access(listener: &UserNotificationListener, verbose: bool) -> Result<(), i32> {
    let status = match listener.GetAccessStatus() {
        Ok(s) => s,
        Err(e) => {
            eprintln!(
                "rs-notify-bridge: GetAccessStatus failed (0x{:08X} {})",
                e.code().0 as u32,
                e.message()
            );
            return Err(EXIT_NO_LISTENER);
        }
    };
    if verbose {
        println!("rs-notify-bridge: access status {status:?}");
    }
    match status {
        UserNotificationListenerAccessStatus::Allowed => Ok(()),
        UserNotificationListenerAccessStatus::Denied => {
            eprintln!(
                "rs-notify-bridge: access to notifications was denied. Allow this app under\n\
                 Settings > Privacy & security > Notifications, then start the bridge again."
            );
            Err(EXIT_NO_ACCESS)
        }
        _ => match listener.RequestAccessAsync().and_then(|op| op.join()) {
            Ok(UserNotificationListenerAccessStatus::Allowed) => Ok(()),
            Ok(s) => {
                eprintln!(
                    "rs-notify-bridge: access to notifications is {s:?}. Windows did not grant it; allow the app under\n\
                     Settings > Privacy & security > Notifications (a console process cannot show the consent prompt)."
                );
                Err(EXIT_NO_ACCESS)
            }
            Err(e) => {
                eprintln!(
                    "rs-notify-bridge: RequestAccessAsync failed (0x{:08X} {}). Allow the app under\n\
                     Settings > Privacy & security > Notifications.",
                    e.code().0 as u32,
                    e.message()
                );
                Err(EXIT_NO_ACCESS)
            }
        },
    }
}

/// One `GetNotificationsAsync(Toast)` call. The listener returns an empty list when permission was revoked, so a failure
/// here is an API failure, not "no notifications".
fn poll(listener: &UserNotificationListener) -> windows::core::Result<Vec<Native>> {
    let list = listener.GetNotificationsAsync(NotificationKinds::Toast)?.join()?;
    let count = list.Size()?;
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        // One odd notification must not lose the others: every property is optional.
        if let Ok(u) = list.GetAt(i) {
            out.push(native(&u));
        }
    }
    Ok(out)
}

/// A notification reduced to the fields the bar needs, read defensively.
struct Native {
    app: String,
    title: String,
    body: String,
    aumid: String,
    id: u32,
    /// `Foundation::DateTime::UniversalTime`, 0 when Windows did not say.
    created: i64,
}

fn native(u: &UserNotification) -> Native {
    let (aumid, display) = match u.AppInfo() {
        Ok(info) => {
            let aumid = info.AppUserModelId().map(|s| s.to_string_lossy()).unwrap_or_default();
            let display = info.DisplayInfo().and_then(|d| d.DisplayName()).map(|s| s.to_string_lossy()).unwrap_or_default();
            (aumid, display)
        }
        Err(_) => (String::new(), String::new()),
    };
    let (title, body) = text_of(u);
    let app = if !display.trim().is_empty() {
        display
    } else if !aumid.is_empty() {
        // Unpackaged senders often have a bare AUMID; it is still better than an empty title.
        aumid.clone()
    } else {
        "Windows".to_string()
    };
    Native {
        app,
        title,
        body,
        aumid,
        id: u.Id().unwrap_or(0),
        created: u.CreationTime().map(|d| d.UniversalTime).unwrap_or(0),
    }
}

/// The first text element is the title, the rest is the body (as the Windows docs recommend). Prefers the ToastGeneric
/// binding and falls back to the first binding that has any text, because third-party toasts are not always well formed.
fn text_of(u: &UserNotification) -> (String, String) {
    let mut texts: Vec<String> = Vec::new();
    if let Ok(visual) = u.Notification().and_then(|n| n.Visual()) {
        if let Ok(bindings) = visual.Bindings() {
            let count = bindings.Size().unwrap_or(0);
            let mut best: Option<(bool, Vec<String>)> = None;
            for i in 0..count {
                let Ok(b) = bindings.GetAt(i) else { continue };
                let generic = b.Template().map(|t| t.to_string_lossy().contains("ToastGeneric")).unwrap_or(false);
                let Ok(elems) = b.GetTextElements() else { continue };
                let n = elems.Size().unwrap_or(0);
                let mut v = Vec::new();
                for j in 0..n {
                    if let Ok(t) = elems.GetAt(j).and_then(|e| e.Text()) {
                        v.push(t.to_string_lossy());
                    }
                }
                if !v.iter().any(|s| !s.trim().is_empty()) {
                    continue;
                }
                let better = match &best {
                    None => true,
                    Some((had_generic, _)) => generic && !*had_generic,
                };
                if better {
                    best = Some((generic, v));
                }
                if generic {
                    break;
                }
            }
            if let Some((_, v)) = best {
                texts = v;
            }
        }
    }
    let mut it = texts.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let title = it.next().unwrap_or_default();
    let body = it.collect::<Vec<_>>().join("\n");
    (title, body)
}

// ---------------------------------------------------------------- delivery

struct State {
    /// `aumid|id|created` of the last `SEEN_MAX` notifications, persisted so a restart does not replay them.
    seen: VecDeque<String>,
    pending: VecDeque<(String, NotifyWire)>,
    seen_dirty: bool,
    seen_path: Option<PathBuf>,
}

impl State {
    fn load(verbose: bool) -> State {
        let mut seen = VecDeque::new();
        let seen_path = data_dir().map(|d| d.join("notify-bridge.seen"));
        if let Some(p) = &seen_path {
            if let Ok(text) = fs::read_to_string(p) {
                for line in text.lines().take(SEEN_MAX) {
                    if !line.trim().is_empty() {
                        seen.push_back(line.to_string());
                    }
                }
            }
        }
        if verbose {
            println!("rs-notify-bridge: {} remembered id(s)", seen.len());
        }
        State { seen, pending: VecDeque::new(), seen_dirty: false, seen_path }
    }

    fn is_seen(&self, key: &str) -> bool {
        self.seen.iter().any(|k| k.as_str() == key)
    }

    fn mark_seen(&mut self, key: String) {
        if self.is_seen(&key) {
            return;
        }
        self.seen.push_back(key);
        while self.seen.len() > SEEN_MAX {
            self.seen.pop_front();
        }
        self.seen_dirty = true;
    }

    fn save(&mut self) {
        if !self.seen_dirty {
            return;
        }
        let Some(path) = &self.seen_path else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let text: String = self.seen.iter().map(|k| format!("{k}\n")).collect();
        if fs::write(path, text).is_ok() {
            self.seen_dirty = false;
        }
    }
}

fn process(args: &Args, state: &mut State, items: Vec<Native>, bar_up: &mut bool, start: Instant) {
    let now = now_ticks();
    let grace = START_GRACE.as_secs() as i64 * TICKS_PER_SEC;
    for n in items {
        let key = format!("{}|{}|{}", n.aumid, n.id, n.created);
        if !args.all && state.is_seen(&key) {
            continue;
        }
        // It may already be waiting for the bar from an earlier poll; do not queue the same toast twice.
        if state.pending.iter().any(|(k, _)| k.as_str() == key) {
            continue;
        }
        // Older notifications are history: remember them, do not raise a toast for a message from last week.
        let age = if n.created > 0 { now.saturating_sub(n.created) } else { 0 };
        let fresh = args.all || n.created == 0 || age <= grace;
        if !fresh {
            state.mark_seen(key);
            if args.verbose {
                log(start, &format!("skip old: {} / {}", n.app, n.title));
            }
            continue;
        }
        let wire = to_wire(&n, &key);
        if args.verbose {
            log(start, &format!("new: {} / {} / {}", n.app, n.title, n.body.replace('\n', " ")));
        }
        match send(&wire) {
            Send::Sent => {
                state.mark_seen(key);
                *bar_up = true;
                if args.verbose {
                    log(start, &format!("sent: {} / {}", n.app, n.title));
                }
            }
            Send::NoBar => {
                if *bar_up {
                    eprintln!("rs-notify-bridge: the WinRust bar is not listening on {PIPE}; keeping notifications until it is");
                    *bar_up = false;
                }
                queue(state, key, wire);
            }
            Send::Failed(why) => {
                eprintln!("rs-notify-bridge: write failed ({why}); keeping the notification for the next poll");
                *bar_up = false;
                queue(state, key, wire);
            }
        }
    }
    state.save();
}

/// Keeps the newest notifications when the bar has been gone for a while, instead of growing forever.
fn queue(state: &mut State, key: String, wire: NotifyWire) {
    if state.pending.len() >= PENDING_MAX {
        state.pending.pop_front();
    }
    state.pending.push_back((key, wire));
}

fn drain_pending(args: &Args, state: &mut State, bar_up: &mut bool, start: Instant) {
    loop {
        let Some((key, wire)) = state.pending.pop_front() else { break };
        match send(&wire) {
            Send::Sent => {
                let app = wire.app();
                let title = wire.title();
                state.mark_seen(key);
                if !*bar_up {
                    eprintln!("rs-notify-bridge: the WinRust bar is listening again");
                    *bar_up = true;
                }
                if args.verbose {
                    log(start, &format!("sent (retry): {app} / {title}"));
                }
            }
            _ => {
                // Still no bar: put it back at the front and stop until the next poll.
                state.pending.push_front((key, wire));
                *bar_up = false;
                break;
            }
        }
    }
    state.save();
}

fn to_wire(n: &Native, key: &str) -> NotifyWire {
    let mut w = NotifyWire::new();
    w.set_app(&n.app);
    w.set_title(&n.title);
    w.set_body(&n.body);
    w.set_icon(&icon_for(&n.aumid));
    // Stable, short, and 63 UTF-16 units fit: the bar uses the id to replace a toast instead of stacking duplicates.
    w.set_id(&format!("nr-{:016x}", fnv1a(key.as_bytes())));
    w
}

enum Send {
    Sent,
    NoBar,
    Failed(String),
}

/// Opens the bar's pipe and writes one record. Retries only while every instance is busy or the bar is between two
/// instances, exactly like `rs-notify`; a missing pipe is reported so the caller can keep the notification.
fn send(w: &NotifyWire) -> Send {
    let Some(h) = connect() else { return Send::NoBar };
    let bytes = w.as_bytes();
    let mut written = 0u32;
    let ok = unsafe { WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut written, null_mut()) };
    let err = if ok == 0 { Some(unsafe { GetLastError() }) } else { None };
    unsafe {
        CloseHandle(h);
    }
    match err {
        None if written as usize == bytes.len() => Send::Sent,
        None => Send::Failed(format!("short write ({written} of {} bytes)", bytes.len())),
        Some(e) => Send::Failed(format!("WriteFile error {e}")),
    }
}

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

// ---------------------------------------------------------------- icons

/// The icon the shell should draw for an AUMID. A user table in `%LOCALAPPDATA%\WinRust\notify-icons.txt` (`prefix=path`
/// per line) wins, then a small built-in table. An unknown app gets the bar's own glyph, which is what `rs-notify` does
/// when it has no icon either.
fn icon_for(aumid: &str) -> String {
    let a = aumid.to_ascii_lowercase();
    if let Some(p) = user_icon(&a) {
        return p;
    }
    if a.contains("discord") {
        if let Some(p) = newest_discord() {
            return p;
        }
    }
    if a.contains("steam") {
        if let Some(p) = first_existing(&["ProgramFiles(x86)/Steam/steam.exe", "ProgramFiles/Steam/steam.exe"]) {
            return p;
        }
    }
    if a.contains("serein") || a.contains("artcraft") {
        if let Some(p) = first_existing(&[
            "LOCALAPPDATA/Programs/Serein/Serein.exe",
            "LOCALAPPDATA/Serein/Serein.exe",
            "LOCALAPPDATA/Programs/ArtCraft Launcher/ArtCraft Launcher.exe",
        ]) {
            return p;
        }
        if let Some(p) = first_exe_in("LOCALAPPDATA/Programs/Serein") {
            return p;
        }
    }
    String::new()
}

fn user_icon(aumid: &str) -> Option<String> {
    let path = data_dir()?.join("notify-icons.txt");
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((prefix, path)) = line.split_once('=') else { continue };
        if aumid.contains(&prefix.trim().to_ascii_lowercase()) {
            let path = expand(path.trim());
            if Path::new(&path).is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// `%ProgramFiles(x86)%/Steam/steam.exe` style candidates: a `NAME/rest` first component is an environment variable.
fn first_existing(candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|c| {
        let p = expand(c);
        Path::new(&p).is_file().then_some(p)
    })
}

fn expand(spec: &str) -> String {
    let spec = spec.replace('\\', "/");
    let (head, rest) = spec.split_once('/').unwrap_or((spec.as_str(), ""));
    let base = match head {
        "LOCALAPPDATA" => env::var("LOCALAPPDATA").ok(),
        "APPDATA" => env::var("APPDATA").ok(),
        "PROGRAMFILES" | "ProgramFiles" => env::var("ProgramFiles").ok(),
        "PROGRAMFILES(X86)" | "ProgramFiles(x86)" => env::var("ProgramFiles(x86)").ok(),
        _ => return spec,
    };
    match base {
        Some(b) if !rest.is_empty() => format!("{b}\\{}", rest.replace('/', "\\")),
        Some(b) => b,
        None => spec,
    }
}

/// Squirrel installs Discord as `%LOCALAPPDATA%\Discord\app-<version>\Discord.exe`; pick the highest version by name.
fn newest_discord() -> Option<String> {
    let root = env::var("LOCALAPPDATA").ok().map(|b| PathBuf::from(b).join("Discord"))?;
    let mut apps: Vec<PathBuf> = fs::read_dir(&root)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("app-")))
        .collect();
    apps.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
    apps.into_iter().rev().find_map(|d| {
        let exe = d.join("Discord.exe");
        exe.is_file().then(|| exe.to_string_lossy().into_owned())
    })
}

fn first_exe_in(dir: &str) -> Option<String> {
    let dir = expand(dir);
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))
        .map(|p| p.to_string_lossy().into_owned())
}

fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(env::var("LOCALAPPDATA").ok()?).join("WinRust"))
}

// ---------------------------------------------------------------- Windows bits

/// Hides the console window only when this process owns it: a logon task would otherwise flash a black window, while a
/// run from a terminal shares the console with the shell and stays visible.
fn hide_own_console() {
    unsafe {
        let mut pids = [0u32; 2];
        if GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) != 1 {
            return;
        }
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_HIDE);
        }
    }
}

fn single_instance() -> Option<HANDLE> {
    let name: Vec<u16> = "Local\\WinRustNotifyBridge".encode_utf16().chain([0]).collect();
    let h = unsafe { CreateMutexW(null(), 0, name.as_ptr()) };
    if h.is_null() {
        return Some(null_mut()); // the mutex failed: run anyway, duplicate protection is best effort
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            CloseHandle(h);
        }
        None
    } else {
        Some(h)
    }
}

fn now_ticks() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64 * TICKS_PER_SEC + FILETIME_UNIX_EPOCH)
        .unwrap_or(0)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn log(start: Instant, msg: &str) {
    println!("[{:7.2}s] {msg}", start.elapsed().as_secs_f64());
}

// ---------------------------------------------------------------- --db fallback

/// The SQLite path is deliberately not wired into the listener flow. It is here to *diagnose* the fallback described in
/// tools\notify-bridge.md: probe the live database through `sqlite3.exe` when the user has it, print what is in it, and
/// otherwise say exactly what is missing. Reading a live WAL is fragile, so this never runs on its own.
fn db_diagnostic(explicit: Option<&str>) -> i32 {
    let db = match explicit {
        Some(p) => PathBuf::from(p),
        None => match env::var("LOCALAPPDATA") {
            Ok(b) => PathBuf::from(b).join(r"Microsoft\Windows\Notifications\wpndatabase.db"),
            Err(_) => {
                eprintln!("rs-notify-bridge: --db needs a path (LOCALAPPDATA is not set)");
                return EXIT_DB;
            }
        },
    };
    if !db.is_file() {
        eprintln!("rs-notify-bridge: no notification database at {}", db.display());
        return EXIT_DB;
    }
    println!("rs-notify-bridge: fallback database {}", db.display());

    let sqlite = std::process::Command::new("sqlite3")
        .arg("-readonly")
        .arg(&db)
        .arg("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name;")
        .output();
    let out = match sqlite {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            eprintln!(
                "rs-notify-bridge: sqlite3 could not read the database: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return EXIT_DB;
        }
        Err(_) => {
            eprintln!(
                "rs-notify-bridge: sqlite3.exe is not on PATH. The listener path works (see tools\\notify-bridge.md),\n\
                 so the database fallback is not needed; install the SQLite command-line tools only to inspect\n\
                 wpndatabase.db. The fallback design is in that file under \"Fallback: wpndatabase.db\"."
            );
            return EXIT_DB;
        }
    };
    let tables = String::from_utf8_lossy(&out.stdout);
    print!("{tables}");
    if tables.lines().any(|t| t.eq_ignore_ascii_case("Notification")) {
        if let Ok(o) = std::process::Command::new("sqlite3")
            .arg("-readonly")
            .arg(&db)
            .arg("PRAGMA table_info(Notification);")
            .output()
        {
            println!("Notification columns:\n{}", String::from_utf8_lossy(&o.stdout));
        }
    } else {
        println!("rs-notify-bridge: no Notification table: this database is not in the expected shape");
    }
    EXIT_OK
}
