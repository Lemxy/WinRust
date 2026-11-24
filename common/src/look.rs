//! The look settings the bar and the explorer share: the blur behind the bar and behind the context menu.
//!
//! One file, `%ProgramData%\WinRust\look.txt`, `key=value` per line, so either program can change a value and the other
//! picks it up (the bar re-reads on its next paint, the menu when it opens).
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering::Relaxed},
        Mutex,
    },
};

fn file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join("WinRust").join("look.txt"))
}

fn parse(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

static AT: AtomicU64 = AtomicU64::new(0); // 0 = read again now
static MAP: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn now() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

fn invalidate() {
    if let Ok(mut m) = MAP.lock() {
        *m = None;
    }
    AT.store(0, Relaxed);
}

/// Parsed once a second: the bar asks on every frame it repaints.
fn get(key: &str) -> Option<String> {
    let t = now();
    let mut map = MAP.lock().unwrap_or_else(|e| e.into_inner());
    if map.is_none() || t.saturating_sub(AT.load(Relaxed)) > 1000 {
        *map = Some(file().and_then(|f| std::fs::read_to_string(f).ok()).map(|s| parse(&s)).unwrap_or_default());
        AT.store(t, Relaxed);
    }
    map.as_ref().and_then(|m| m.get(key).cloned())
}

fn num(key: &str, default: i32, min: i32, max: i32) -> i32 {
    get(key).and_then(|v| v.parse::<i32>().ok()).map_or(default, |v| v.clamp(min, max))
}

fn flag(key: &str, default: bool) -> bool {
    get(key).and_then(|v| v.parse::<i32>().ok()).map_or(default, |v| v != 0)
}

/// Writes one value, keeping the others.
pub fn set(key: &str, value: impl ToString) {
    let Some(f) = file() else { return };
    let mut map = f.exists().then(|| std::fs::read_to_string(&f).ok()).flatten().map(|s| parse(&s)).unwrap_or_default();
    map.insert(key.to_string(), value.to_string());
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let text: String = keys.iter().map(|k| format!("{k}={}\n", map[*k])).collect();
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, text);
    invalidate();
}

/// Blur behind the bar. Falls back to the old `blur.txt` (`1`/`0`) when `look.txt` has no opinion yet.
pub fn bar_blur() -> bool {
    if let Some(v) = get("bar-blur") {
        return v != "0";
    }
    std::env::var_os("ProgramData")
        .map(|p| PathBuf::from(p).join("WinRust").join("blur.txt"))
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map_or(true, |s| s.trim() != "0")
}

pub fn bar_blur_set(on: bool) {
    set("bar-blur", i32::from(on));
}

/// Blur radius behind the bar, in pixels before scaling.
pub fn bar_radius() -> i32 {
    num("bar-radius", 8, 2, 20)
}

pub fn bar_radius_set(r: i32) {
    set("bar-radius", r.clamp(2, 20));
}

/// The search button in the bar.
pub fn bar_search() -> bool {
    flag("bar-search", true)
}

pub fn bar_search_set(on: bool) {
    set("bar-search", i32::from(on));
}

/// The Task View button in the bar.
pub fn bar_taskview() -> bool {
    flag("bar-taskview", true)
}

pub fn bar_taskview_set(on: bool) {
    set("bar-taskview", i32::from(on));
}

/// Blur behind the context menu.
pub fn menu_blur() -> bool {
    flag("menu-blur", true)
}

pub fn menu_blur_set(on: bool) {
    set("menu-blur", i32::from(on));
}

/// How solid the context menu looks: 100 = opaque, 67 = the default frost.
pub fn menu_alpha() -> u8 {
    num("menu-alpha", 67, 25, 100) as u8
}

pub fn menu_alpha_set(v: u8) {
    set("menu-alpha", v.clamp(25, 100));
}

/// Which monitor the bar lives on: 0 = the first monitor (the primary in the usual layout, default), 1.. = the others
/// in left-to-right order, -1 = one bar on every monitor (the primary's bar is the leader).
pub fn bar_monitor() -> i32 {
    num("bar-monitor", 0, -1, 16)
}

pub fn bar_monitor_set(i: i32) {
    set("bar-monitor", i.clamp(-1, 16));
}

/// Blur radius behind the context menu, in pixels before scaling.
pub fn menu_radius() -> i32 {
    num("menu-radius", 18, 4, 40)
}

pub fn menu_radius_set(r: i32) {
    set("menu-radius", r.clamp(4, 40));
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_key_values() {
        let m = parse("bar-blur=1\nmenu-alpha = 70\n\n");
        assert_eq!(m.get("bar-blur").map(String::as_str), Some("1"));
        assert_eq!(m.get("menu-alpha").map(String::as_str), Some("70"));
        assert!(m.get("nope").is_none());
    }
}

/// Start menu on the left instead of centered.
pub fn start_left() -> bool {
    flag("start-left", false)
}

pub fn start_left_set(on: bool) {
    set("start-left", i32::from(on));
}

/// The Start menu look: 0 = Windows 11 (pinned grid), 1 = Windows 10 (tiles), 2 = Windows 7 (two columns).
pub fn start_style() -> i32 {
    num("start-style", 0, 0, 2)
}

pub fn start_style_set(v: i32) {
    set("start-style", v.clamp(0, 2));
}
/// Start menu size: 100, 115 or 130 % of the style's own size.
pub fn start_scale() -> i32 {
    match num("start-scale", 1, 0, 2) {
        0 => 90,
        2 => 115,
        _ => 100,
    }
}

pub fn start_scale_set(v: i32) {
    set("start-scale", v.clamp(0, 2));
}

/// Rank the Start menu's search results by how often each app was launched (`%APPDATA%\topbar-usage.txt`). Off: the
/// matches come out in the list's own order, A to Z.
pub fn start_ranked() -> bool {
    flag("start-ranked", true)
}

pub fn start_ranked_set(on: bool) {
    set("start-ranked", i32::from(on));
}
/// Frost behind the windows of the apps (Explorer, Settings, Task Manager). Off by default: the tint needed to keep
/// text readable on a bright wallpaper makes it look washed out; turn it on to try it.
pub fn win_blur() -> bool {
    flag("win-blur", false)
}

pub fn win_blur_set(on: bool) {
    set("win-blur", i32::from(on));
}

/// The notification pop-ups of the bar.
pub fn notify() -> bool {
    flag("notify", true)
}

pub fn notify_set(on: bool) {
    set("notify", i32::from(on));
}

/// How long a notification stays on screen, in seconds (progress toasts never close by themselves).
pub fn notify_timeout() -> i32 {
    num("notify-timeout", 5, 1, 60)
}

pub fn notify_timeout_set(secs: i32) {
    set("notify-timeout", secs.clamp(1, 60));
}

/// Every value in this file back to what a fresh install has. Used by the settings window's "reset everything"; the
/// programs that are running pick the defaults up on their next read, like any other change made there.
pub fn reset_all() {
    for (key, value) in [
        ("bar-blur", 1),
        ("bar-radius", 8),
        ("bar-search", 1),
        ("bar-taskview", 1),
        ("bar-monitor", 0),
        ("menu-blur", 1),
        ("menu-alpha", 67),
        ("menu-radius", 18),
        ("start-left", 0),
        ("start-style", 0),
        ("start-scale", 1),
        ("start-ranked", 1),
        ("win-blur", 0),
        ("notify", 1),
        ("notify-timeout", 5),
    ] {
        set(key, value);
    }
}