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
    flag_of(get(key).as_deref(), default)
}

/// The text of a boolean key: anything but `0` is on, a missing key takes `default`. Separate from `get` so the
/// back-compat rule (an old look.txt without the key keeps today's behaviour) is unit-tested.
fn flag_of(value: Option<&str>, default: bool) -> bool {
    value.and_then(|v| v.parse::<i32>().ok()).map_or(default, |v| v != 0)
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

/// An icon file for the bar's Start button (`.ico`, `.exe`, `.dll`). Empty = the grid drawn from the accent colour.
pub fn start_icon() -> String {
    get("start-icon").unwrap_or_default()
}

pub fn start_icon_set(path: &str) {
    set("start-icon", path.trim());
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

/// Which monitor the bar lives on: 0 = the first monitor (the primary in the usual layout), 1.. = the others in
/// left-to-right order, -1 = one bar on every monitor (the primary's bar is the leader). Still read for an existing
/// `look.txt`; new settings use [`bar_monitors`].
pub fn bar_monitor() -> i32 {
    num("bar-monitor", 0, -1, 16)
}

pub fn bar_monitor_set(i: i32) {
    set("bar-monitor", i.clamp(-1, 16));
}

/// The monitors that get a bar (`bar-monitors=0,2`), or `None` for all of them. The per-monitor choice is what the
/// settings window edits; `bar-monitor` (all = -1) is the old single-value form, still honoured when the new key is
/// missing so an existing look.txt keeps working.
pub fn bar_monitors() -> Option<Vec<usize>> {
    if let Some(v) = get("bar-monitors") {
        let list: Vec<usize> = v.split(',').filter_map(|s| s.trim().parse::<usize>().ok()).collect();
        if !list.is_empty() {
            return Some(list);
        }
    }
    match bar_monitor() {
        i if i < 0 => None,
        i => Some(vec![i as usize]),
    }
}

/// True when the monitor at index `i` should have its own bar.
pub fn bar_on_monitor(i: usize) -> bool {
    bar_monitors().map_or(true, |l| l.contains(&i))
}

/// Writes the per-monitor list (`0,2`). Sorted and without duplicates; an empty list is ignored by the bar, which keeps
/// the primary's so the shell never ends without one.
pub fn bar_monitors_set(list: &[usize]) {
    let mut v = list.to_vec();
    v.sort_unstable();
    v.dedup();
    set("bar-monitors", v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","));
}

/// Blur radius behind the context menu, in pixels before scaling.
pub fn menu_radius() -> i32 {
    num("menu-radius", 18, 4, 40)
}

pub fn menu_radius_set(r: i32) {
    set("menu-radius", r.clamp(4, 40));
}

/// Wallpaper Engine behind the desktop icons (`we=1`). Off by default: without the key an old look.txt, a fresh install
/// and a machine without WE all keep the picture wallpaper exactly as before.
pub fn we() -> bool {
    flag("we", false)
}

pub fn we_set(on: bool) {
    set("we", i32::from(on));
}

/// The wallpaper WE opens in its pop-out: a `project.json`, a `scene.pkg`, an `.mp4` or an `index.html`/folder. Empty =
/// ask WE for the wallpaper it is already showing (`-control getWallpaper`).
pub fn we_file() -> String {
    get("we-file").unwrap_or_default()
}

pub fn we_file_set(path: &str) {
    set("we-file", path.trim());
}

/// One WE pop-out across the whole virtual screen instead of one per monitor (the CLI takes one window per monitor, but
/// a scene may look wrong cut per screen). Off = one per monitor.
pub fn we_span() -> bool {
    flag("we-span", false)
}

pub fn we_span_set(on: bool) {
    set("we-span", i32::from(on));
}

#[cfg(test)]
mod tests {
    use super::{flag_of, parse};

    #[test]
    fn parses_key_values() {
        let m = parse("bar-blur=1\nmenu-alpha = 70\n\n");
        assert_eq!(m.get("bar-blur").map(String::as_str), Some("1"));
        assert_eq!(m.get("menu-alpha").map(String::as_str), Some("70"));
        assert!(m.get("nope").is_none());
    }

    #[test]
    fn a_missing_boolean_key_keeps_the_old_behaviour() {
        // look.txt written before a key existed (we, we-span, ...): the caller's default wins
        let old = parse("bar-blur=1\nbar-monitor=0\n");
        assert!(!flag_of(old.get("we").map(String::as_str), false));
        assert!(flag_of(old.get("bar-blur").map(String::as_str), false));
        assert_eq!(flag_of(Some("0"), true), false);
        assert_eq!(flag_of(Some("1"), false), true);
        assert_eq!(flag_of(Some("2"), false), true); // this file's booleans are "0 / anything else"
        assert_eq!(flag_of(Some("junk"), false), false); // a broken value falls back to the default (off for we)
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

/// How opaque the top-level windows of the apps are (Explorer, Settings, Control Panel): 255 = solid, 222 = 87 %.
/// Applied when a window is created; `window-opacity=255` turns it off.
pub fn window_opacity() -> u8 {
    num("window-opacity", 222, 0, 255) as u8
}

pub fn window_opacity_set(v: u8) {
    set("window-opacity", v.clamp(0, 255));
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
        ("window-opacity", 222),
        ("notify", 1),
        ("notify-timeout", 5),
        ("we", 0),
    ] {
        set(key, value);
    }
}