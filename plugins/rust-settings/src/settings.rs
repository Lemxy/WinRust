//! Settings in Rust (Win+I / `shellbits --settings`): Windows 11's layout (320px nav with icons,
//! page title, rounded #2B2B2B cards with a control on the right) and controls that change the real
//! system setting. Every value is read from Windows when a page opens; nothing is hard-coded.
//! Pages not rebuilt yet are one click away through each page's "More ... settings" row.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering::Relaxed};
use std::sync::Mutex;

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::flyout::{glyph, rect, rrect, utf16};
use crate::quick::rgb;

pub static WIN: AtomicIsize = AtomicIsize::new(0);
const WM_VALUES: u32 = WM_APP + 120;
const NAV: f32 = 320.0;
const ROW_H: f32 = 68.0;

#[derive(Clone, Copy)]
enum Kind {
    Toggle(fn() -> bool, fn(bool)),
    Slider(fn() -> f32, fn(f32)), // 0..1
    Volumes(fn() -> Vec<crate::qsx::Item>, fn(&str, f32)), // one card per app with its own volume slider
    Info(fn() -> String),
    Link(&'static str), // opens this page of Windows' own Settings
    Sub(&'static str),  // opens one of our own sub-pages
    Expand(&'static str), // a Link drawn with a down chevron (a section the real page unfolds in place)
    Header,             // a section title between cards, not a card
    Section,            // the same, with the tighter spacing some pages use (Privacy & security)
    Choice(fn() -> String, fn() -> Vec<String>, fn(usize)), // drop-down: current value, the options, pick one
    State(fn() -> bool, &'static str), // shows On/Off with a switch; changing it happens on that Windows page
    Flat(fn() -> String, fn() -> Vec<String>, fn(usize)), // like Choice, but shown as plain text with a chevron
    Big(fn() -> String),                   // the large battery percentage at the top of Power & battery
    Note(fn() -> bool, &'static str),      // an information bar, shown only while its condition holds
    Tiles(fn() -> Vec<(String, String, bool, String)>), // spec tiles: label, big value, -, bottom line
    Facts(fn() -> Vec<(String, String, bool, String)>), // a card of "name  value" lines with a Copy button
    RenamePc,                                            // device name + model with the Rename this PC button
    ToggleMore(fn() -> bool, fn(bool), &'static str), // a switch plus a chevron that opens more settings
    NetHero(fn() -> (String, bool)),       // the connection summary at the top of Network & internet: name, wired?
    BtTiles(fn() -> Vec<crate::qsx::Item>), // paired devices as tiles, then an "Add device" tile
    Center(&'static str, &'static str),    // a thin bar with a centred link: text, page it opens
    Space(f32),                            // extra air between cards
    LinkBtn(&'static str, &'static str, &'static str), // a Link with an accent button before the chevron: page, button text, button's page
    ThemeHero(fn() -> Vec<(String, String, bool, String)>), // desktop preview + Windows' themes to pick from: theme file, its wallpaper, is current
    TimeHero(fn() -> Vec<(String, String, bool, String)>), // clock, date, time zone and region at the top of Time & language
    UpdateHero(fn() -> bool),              // Windows Update's status block; the value is "a restart is waiting"
    When(fn() -> bool, &'static str),      // an expander card that only exists while its condition holds
    HomeHero(fn() -> Vec<(String, String, bool, String)>),    // this PC, its connection and update state, across the top of Home
    AccountHero(fn() -> Vec<(String, String, bool, String)>), // picture, name, email and account type at the top of Accounts
    SortBy,                                // "Sort by:" with its drop-down, above the app list
    Senders(fn() -> Vec<crate::qsx::Item>, fn(&str, bool)), // one card per item, each with its own switch: the list, and what flipping one does
    Actions(fn() -> Vec<crate::qsx::Item>, &'static str, fn(&str, &str)), // one card per item with a button: the list, the button's text, what it does (item id, item name)
    Devices(fn() -> Vec<crate::qsx::Item>, &'static str), // sound devices to pick from, then a "pair a new device" line with this label
}

/// Added to a Row's icon when the glyph comes from Settings' own icon font instead of Segoe Fluent Icons.
const SET: u32 = 0x10_0000;
thread_local! {
    // "Settings Fluent Icons": the font the real Settings app draws its card icons with (not installed system-wide)
    static SET_FACE: std::cell::OnceCell<Option<IDWriteFontFace>> = const { std::cell::OnceCell::new() };
}
unsafe fn settings_face(dw: &IDWriteFactory) -> Option<IDWriteFontFace> {
    SET_FACE.with(|c| {
        c.get_or_init(|| {
            let path = format!("{}\\SystemResources\\Windows.UI.SettingsAppThreshold\\SystemSettings\\Assets\\Fonts\\SettingsIcons.ttf", std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()));
            let file = dw.CreateFontFileReference(&HSTRING::from(path), None).ok()?;
            dw.CreateFontFace(DWRITE_FONT_FACE_TYPE_TRUETYPE, &[Some(file)], 0, DWRITE_FONT_SIMULATIONS_NONE).ok()
        })
        .clone()
    })
}

struct Row {
    icon: u32, // Segoe Fluent Icons glyph, 0 = none
    label: &'static str,
    sub: &'static str,
    kind: Kind,
}

struct Page {
    name: &'static str,
    icon: u32,
    parent: &'static str, // "" = shown in the left navigation
    rows: Vec<Row>,
}

#[derive(Clone, Default)]
struct Val {
    on: bool,
    f: f32,
    s: String,
    list: Vec<(String, String, bool, String)>, // devices: name, adapter, is the default, endpoint id
}

/// "Speakers (Realtek(R) Audio)" -> ("Speakers", "Realtek(R) Audio")
pub fn split_device(name: &str) -> (String, String) {
    match name.split_once(" (") {
        Some((a, b)) => (a.to_string(), b.strip_suffix(')').unwrap_or(b).to_string()),
        None => (name.to_string(), String::new()),
    }
}

const PUSH: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\PushNotifications");
const SENDERS: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings";
static BY_NAME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn toasts() -> bool {
    reg_u32(HKEY_CURRENT_USER, PUSH, w!("ToastEnabled")) != Some(0)
}
fn set_toasts(on: bool) {
    let v = on as u32;
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, PUSH, w!("ToastEnabled"), REG_DWORD.0, Some(&v as *const _ as *const c_void), 4);
    }
}
/// Most recent first (or by name), given (name, last notification time).
pub fn sort_senders(v: &mut [(String, u64)], by_name: bool) {
    if by_name {
        v.sort_by_key(|a| a.0.to_lowercase());
    } else {
        v.sort_by(|a, b| b.1.cmp(&a.1));
    }
}
/// Apps that have sent notifications, with whether each is allowed to: Windows' own per-app list.
fn senders() -> Vec<crate::qsx::Item> {
    use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY};
    let mut found: Vec<(String, u64)> = Vec::new();
    unsafe {
        let mut k = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(SENDERS), Some(0), KEY_READ, &mut k).is_err() {
            return Vec::new();
        }
        for i in 0.. {
            let mut name = [0u16; 512];
            let mut n = name.len() as u32;
            if RegEnumKeyExW(k, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None).is_err() {
                break;
            }
            let id = String::from_utf16_lossy(&name[..n as usize]);
            let (mut last, mut sz) = (0u64, 8u32);
            let _ = RegGetValueW(HKEY_CURRENT_USER, &HSTRING::from(format!("{SENDERS}\\{id}")), w!("LastNotificationAddedTime"), RRF_RT_REG_QWORD, None, Some(&mut last as *mut _ as *mut c_void), Some(&mut sz));
            found.push((id, last));
        }
        let _ = RegCloseKey(k);
        // only the ones Windows can name (the rest are internal senders the real page hides too)
        let mut named: Vec<(String, u64, String)> = found
            .into_iter()
            .filter_map(|(id, last)| {
                let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(format!("shell:AppsFolder\\{id}")), None).ok()?;
                let pw = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
                let name = pw.to_string().unwrap_or_default();
                windows::Win32::System::Com::CoTaskMemFree(Some(pw.0 as *const c_void));
                (!name.is_empty()).then_some((name, last, id))
            })
            .collect();
        let mut order: Vec<(String, u64)> = named.iter().map(|n| (n.2.clone(), n.1)).collect();
        if BY_NAME.load(Relaxed) {
            named.sort_by_key(|a| a.0.to_lowercase());
        } else {
            sort_senders(&mut order, false);
            named.sort_by_key(|n| order.iter().position(|o| o.0 == n.2));
        }
        named
            .into_iter()
            .map(|(name, _, id)| {
                let on = reg_u32(HKEY_CURRENT_USER, PCWSTR(HSTRING::from(format!("{SENDERS}\\{id}")).as_ptr()), w!("Enabled")) != Some(0);
                crate::qsx::Item { name, key: id, on, ..Default::default() }
            })
            .collect()
    }
}
/// Where a list item's icon comes from: a startup entry names its program, anything else is an app id.
fn icon_source(id: &str) -> String {
    if id.starts_with("wifi	") {
        return String::new();
    }
    match id.split('\t').nth(1).filter(|_| id.starts_with("startup\t") || id.starts_with("app\t")) {
        Some(file) => file.to_string(),
        None => format!("shell:AppsFolder\\{id}"),
    }
}

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved";

/// Windows' "is this startup item switched on" record: 12 bytes whose first byte is even when it is
/// on (02, 06) and odd when it was switched off (03, 07). No record at all means on.
pub fn startup_on(record: &[u8]) -> bool {
    record.first().is_none_or(|b| b & 1 == 0)
}

/// The program a startup command line starts: `"C:\a b\x.exe" --flag` -> `C:\a b\x.exe`.
pub fn exe_of(command: &str) -> String {
    let c = command.trim();
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("").to_string();
    }
    match c.to_lowercase().find(".exe") {
        Some(i) => c[..i + 4].to_string(),
        None => c.split_whitespace().next().unwrap_or("").to_string(),
    }
}

unsafe fn reg_values(root: HKEY, key: &str) -> Vec<(String, String)> {
    use windows::Win32::System::Registry::RegEnumValueW;
    let mut out = Vec::new();
    let mut k = HKEY::default();
    if RegOpenKeyExW(root, &HSTRING::from(key), Some(0), KEY_READ, &mut k).is_err() {
        return out;
    }
    for i in 0.. {
        let (mut name, mut data) = ([0u16; 512], [0u8; 4096]);
        let (mut n, mut d) = (name.len() as u32, data.len() as u32);
        if RegEnumValueW(k, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut n, None, None, Some(data.as_mut_ptr()), Some(&mut d)).is_err() {
            break;
        }
        let text: Vec<u16> = data[..d as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
        out.push((String::from_utf16_lossy(&name[..n as usize]), String::from_utf16_lossy(&text)));
    }
    let _ = RegCloseKey(k);
    out
}

unsafe fn approved(root: HKEY, list: &str, name: &str) -> bool {
    use windows::Win32::System::Registry::RRF_RT_REG_BINARY;
    let (mut rec, mut n) = ([0u8; 16], 16u32);
    let found = RegGetValueW(root, &HSTRING::from(format!("{APPROVED}\\{list}")), &HSTRING::from(name), RRF_RT_REG_BINARY, None, Some(rec.as_mut_ptr() as *mut c_void), Some(&mut n)).is_ok();
    startup_on(if found { &rec[..n as usize] } else { &[] })
}

/// What starts when you sign in: your own Run entries, your Startup folder, and the PC-wide Run
/// entries, each with Windows' own on/off record (the one Task Manager and Settings flip).
// ponytail: packaged (Store) apps' startup tasks are not listed; they need the StartupTask API per package
pub(crate) fn startup_apps() -> Vec<crate::qsx::Item> {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let mut out = Vec::new();
    unsafe {
        for (root, tag, note) in [(HKEY_CURRENT_USER, "cu", ""), (HKEY_LOCAL_MACHINE, "lm", "All users")] {
            for (name, command) in reg_values(root, RUN) {
                let on = approved(root, "Run", &name);
                let sub = match (on, note) { (true, "") => "On".to_string(), (false, "") => "Off".to_string(), (true, n) => format!("On - {n}"), (false, n) => format!("Off - {n}") };
                out.push(crate::qsx::Item { key: format!("startup\t{}\t{tag}:{name}", exe_of(&command)), name, sub, on, ..Default::default() });
            }
        }
        let folder = format!("{}\\Microsoft\\Windows\\Start Menu\\Programs\\Startup", std::env::var("APPDATA").unwrap_or_default());
        for f in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let file = f.file_name().to_string_lossy().to_string();
            if file.eq_ignore_ascii_case("desktop.ini") || f.path().is_dir() {
                continue;
            }
            let on = approved(HKEY_CURRENT_USER, "StartupFolder", &file);
            let name = f.path().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(file.clone());
            out.push(crate::qsx::Item { key: format!("startup\t{}\tfolder:{file}", f.path().display()), name, sub: if on { "On".into() } else { "Off".into() }, on, ..Default::default() });
        }
    }
    out.sort_by_key(|i| i.name.to_lowercase());
    out
}

/// Flip one startup item, the way Windows does: write its 12-byte record (02 = on, 03 = off).
/// PC-wide entries need administrator rights; without them the write fails and nothing changes.
pub(crate) fn set_startup(id: &str, on: bool) {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, REG_BINARY};
    let Some((kind, name)) = id.rsplit('\t').next().and_then(|k| k.split_once(':')) else { return };
    let (root, list) = match kind { "cu" => (HKEY_CURRENT_USER, "Run"), "lm" => (HKEY_LOCAL_MACHINE, "Run"), _ => (HKEY_CURRENT_USER, "StartupFolder") };
    let mut rec = [0u8; 12];
    rec[0] = if on { 2 } else { 3 };
    unsafe {
        let _ = RegSetKeyValueW(root, &HSTRING::from(format!("{APPROVED}\\{list}")), &HSTRING::from(name), REG_BINARY.0, Some(rec.as_ptr() as *const c_void), 12);
    }
}

const UNINSTALL: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
const UNINSTALL32: &str = "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

/// The second line of an app's card, like Windows': "1.2.3  |  Publisher  |  12/31/2026  |  45.2 MB",
/// leaving out whatever the app did not record. `date` is the registry's YYYYMMDD.
pub fn app_line(version: &str, publisher: &str, date: &str, size_kb: u32) -> String {
    let mut parts: Vec<String> = [version, publisher].iter().filter(|p| !p.trim().is_empty()).map(|p| p.trim().to_string()).collect();
    if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) {
        parts.push(format!("{}/{}/{}", date[4..6].trim_start_matches('0'), date[6..8].trim_start_matches('0'), &date[..4]));
    }
    if size_kb >= 512 { // smaller than half a megabyte would read "0 MB"
        parts.push(size_label(size_kb as u64 * 1024));
    }
    parts.join("  |  ")
}

/// The command that removes an app. Windows Installer entries are recorded as "install/repair"
/// (`MsiExec.exe /I{...}`); removing is the same command with /X.
pub fn uninstall_command(recorded: &str) -> String {
    let c = recorded.trim();
    if c.to_lowercase().starts_with("msiexec") { c.replacen("/I{", "/X{", 1).replacen("/i{", "/X{", 1) } else { c.to_string() }
}

/// A DisplayIcon value as a file: `"C:\x.exe",0` -> `C:\x.exe`.
pub fn icon_file(display_icon: &str) -> String {
    let t = display_icon.trim().trim_matches('"');
    match t.rsplit_once(',') {
        Some((file, index)) if index.trim().trim_start_matches('-').bytes().all(|b| b.is_ascii_digit()) => file.trim().trim_matches('"').to_string(),
        _ => t.to_string(),
    }
}

/// The programs Windows lists under Installed apps, from its uninstall records (this user's and the
/// PC's, 64- and 32-bit), without system components and patches, by name.
fn installed_apps() -> Vec<crate::qsx::Item> {
    let mut v = desktop_apps();
    for a in store_apps() {
        if !v.iter().any(|d| d.name == a.name) {
            v.push(a);
        }
    }
    sort_apps(&mut v, APPS_SORT.load(std::sync::atomic::Ordering::Relaxed));
    v
}

/// Store (packaged) apps installed for you, from Windows' package manager: name, version, publisher,
/// install date, logo. Frameworks, resource packs and Windows' own system packages are left out, as
/// Windows' list does.
fn store_apps() -> Vec<crate::qsx::Item> {
    use windows::ApplicationModel::PackageSignatureKind;
    use windows::Management::Deployment::PackageManager;
    let mut out = Vec::new();
    let Ok(pm) = PackageManager::new() else { return out };
    let Ok(list) = pm.FindPackagesByUserSecurityId(&HSTRING::new()) else { return out };
    for pkg in list {
        let keep = (|| -> windows::core::Result<bool> {
            Ok(!pkg.IsFramework()? && !pkg.IsResourcePackage()? && !pkg.IsBundle().unwrap_or(false) && pkg.SignatureKind()? != PackageSignatureKind::System)
        })()
        .unwrap_or(false);
        if !keep {
            continue;
        }
        let name = pkg.DisplayName().map(|s| s.to_string()).unwrap_or_default();
        if name.is_empty() || name.starts_with("ms-resource:") {
            continue;
        }
        let publisher = pkg.PublisherDisplayName().map(|s| s.to_string()).unwrap_or_default();
        let version = pkg.Id().and_then(|i| i.Version()).map(|v| format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision)).unwrap_or_default();
        // DateTime: 100 ns since 1601 -> YYYYMMDD, the uninstall records' format
        let date = pkg.InstalledDate().map(|d| {
            let ft = windows::Win32::Foundation::FILETIME { dwLowDateTime: d.UniversalTime as u32, dwHighDateTime: (d.UniversalTime >> 32) as u32 };
            let mut st = windows::Win32::Foundation::SYSTEMTIME::default();
            unsafe { let _ = windows::Win32::System::Time::FileTimeToSystemTime(&ft, &mut st); }
            format!("{:04}{:02}{:02}", st.wYear, st.wMonth, st.wDay)
        }).unwrap_or_default();
        let logo = pkg.Logo().and_then(|u| u.RawUri()).map(|u| u.to_string()).unwrap_or_default();
        let logo = logo.strip_prefix("file:///").map(|p| p.replace('/', "\\").replace("%20", " ")).unwrap_or(logo);
        // its icon is its Start entry's (the logo file alone only gives the PNG file-type icon)
        let aumid = pkg.GetAppListEntries().ok().and_then(|l| l.into_iter().next()).and_then(|e| e.AppUserModelId().ok()).map(|s| s.to_string());
        let logo = aumid.map(|a| format!(r"shell:AppsFolder\{a}")).unwrap_or_else(|| scaled_asset(&logo));
        let full = pkg.Id().and_then(|i| i.FullName()).map(|s| s.to_string()).unwrap_or_default();
        out.push(crate::qsx::Item {
            sub: app_line(&version, &publisher, &date, 0),
            key: format!("app\t{logo}\tpkg:{full}\t{date}"),
            name,
            ..Default::default()
        });
    }
    out
}

/// A package's logo as a file on disk: the manifest names "Assets\StoreLogo.png", the package holds
/// "StoreLogo.scale-100.png" (or targetsize-, or another scale): take the closest that exists.
pub fn scaled_asset(path: &str) -> String {
    let p = std::path::Path::new(path);
    if p.is_file() {
        return path.to_string();
    }
    let (Some(dir), Some(stem)) = (p.parent(), p.file_stem().map(|s| s.to_string_lossy().to_lowercase())) else { return path.to_string() };
    // WindowsApps folders cannot be listed by a user, but their files can be read: try the usual names
    let ext = p.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_else(|| "png".into());
    for q in ["scale-100", "scale-125", "scale-150", "scale-200", "targetsize-32", "targetsize-48", "scale-400"] {
        let c = dir.join(format!("{stem}.{q}.{ext}"));
        if c.is_file() {
            return c.display().to_string();
        }
    }
    let mut found: Vec<String> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| { let l = n.to_lowercase(); l.starts_with(&format!("{stem}.")) && (l.ends_with(".png") || l.ends_with(".jpg")) })
        .collect();
    // prefer the 100% scale, then the smallest size
    found.sort_by_key(|n| (!n.to_lowercase().contains("scale-100"), n.len(), n.clone()));
    found.first().map(|n| dir.join(n).display().to_string()).unwrap_or_else(|| path.to_string())
}

/// The programs in Windows' uninstall records (this user's and the PC's, 64- and 32-bit), without
/// system components and patches.
fn desktop_apps() -> Vec<crate::qsx::Item> {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let mut out: Vec<crate::qsx::Item> = Vec::new();
    unsafe {
        for (root, base) in [(HKEY_LOCAL_MACHINE, UNINSTALL), (HKEY_LOCAL_MACHINE, UNINSTALL32), (HKEY_CURRENT_USER, UNINSTALL)] {
            let mut k = HKEY::default();
            if RegOpenKeyExW(root, &HSTRING::from(base), Some(0), KEY_READ, &mut k).is_err() {
                continue;
            }
            for i in 0.. {
                let mut name = [0u16; 512];
                let mut n = name.len() as u32;
                if RegEnumKeyExW(k, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None).is_err() {
                    break;
                }
                let key = HSTRING::from(format!("{base}\\{}", String::from_utf16_lossy(&name[..n as usize])));
                let text = |v: PCWSTR| reg_str(root, PCWSTR(key.as_ptr()), v);
                let num = |v: PCWSTR| reg_u32(root, PCWSTR(key.as_ptr()), v).unwrap_or(0);
                let (title, remove) = (text(w!("DisplayName")), text(w!("UninstallString")));
                if title.is_empty() || remove.is_empty() || num(w!("SystemComponent")) == 1 || !text(w!("ParentKeyName")).is_empty() || out.iter().any(|a| a.name == title) {
                    continue;
                }
                out.push(crate::qsx::Item {
                    sub: app_line(&text(w!("DisplayVersion")), &text(w!("Publisher")), &text(w!("InstallDate")), num(w!("EstimatedSize"))),
                    key: format!("app\t{}\t{}\t{}", icon_file(&text(w!("DisplayIcon"))), uninstall_command(&remove), text(w!("InstallDate"))),
                    name: title,
                    signal: num(w!("EstimatedSize")), // KB, for sorting by size
                    ..Default::default()
                });
            }
            let _ = RegCloseKey(k);
        }
    }
    out
}

/// Installed apps' "Sort by", as Windows offers it.
static APPS_SORT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const APPS_SORTS: [&str; 4] = ["Name (A to Z)", "Name (Z to A)", "Size (large to small)", "Date installed"];
fn apps_sort() -> String { APPS_SORTS[APPS_SORT.load(std::sync::atomic::Ordering::Relaxed).min(3)].to_string() }
fn apps_sorts() -> Vec<String> { APPS_SORTS.iter().map(|s| s.to_string()).collect() }
fn set_apps_sort(i: usize) { APPS_SORT.store(i.min(3), std::sync::atomic::Ordering::Relaxed) }

/// Order the list: by name either way, biggest first, or newest install first (the date rides at the
/// end of the key as YYYYMMDD).
pub fn sort_apps(v: &mut [crate::qsx::Item], how: usize) {
    let date = |a: &crate::qsx::Item| a.key.rsplit('\t').next().unwrap_or("").to_string();
    match how {
        1 => v.sort_by_key(|a| std::cmp::Reverse(a.name.to_lowercase())),
        2 => v.sort_by_key(|a| std::cmp::Reverse(a.signal)),
        3 => v.sort_by_key(|a| std::cmp::Reverse(date(a))),
        _ => v.sort_by_key(|a| a.name.to_lowercase()),
    }
}

/// Uninstall: ask first (as Windows does), then start the app's own uninstaller, which shows its own steps.
fn uninstall_app(id: &str, name: &str) {
    use windows::Win32::System::Threading::{CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW};
    let Some(command) = id.split('\t').nth(2).filter(|c| !c.is_empty()) else { return };
    unsafe {
        let ask = format!("This app and its related info will be uninstalled.\n\n{name}");
        if crate::dialog::ask(None, "Uninstall", &ask, &["Uninstall", "Cancel"], 1) != Some(0) {
            return;
        }
        // a Store app: Windows' package manager removes it (as Settings does)
        if let Some(full) = command.strip_prefix("pkg:") {
            let full = HSTRING::from(full);
            std::thread::spawn(move || {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                if let Ok(pm) = windows::Management::Deployment::PackageManager::new() {
                    let _ = pm.RemovePackageAsync(&full).and_then(|op| op.get());
                }
            });
            return;
        }
        let mut cmd: Vec<u16> = command.encode_utf16().chain([0]).collect();
        let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        if CreateProcessW(None, Some(windows::core::PWSTR(cmd.as_mut_ptr())), None, None, false, PROCESS_CREATION_FLAGS(0), None, None, &si, &mut pi).is_ok() {
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
        } else {
            // uninstallers that need administrator rights cannot be started this way: let the shell ask
            windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("cmd.exe"), &HSTRING::from(format!("/c {command}")), PCWSTR::null(), SW_HIDE);
        }
    }
}

/// "Download and install": Windows' update service does it (as Settings' button does); it needs an
/// administrator, so Windows asks first.
fn wu_install_item() -> Vec<crate::qsx::Item> {
    vec![crate::qsx::Item { key: "wu-install".into(), name: "Download and install".into(), sub: "Windows Update downloads and installs what it found, in the background".into(), ..Default::default() }]
}
fn wu_install(_: &str, _: &str) {
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("runas"), w!("UsoClient.exe"), w!("StartInteractiveScan"), PCWSTR::null(), SW_HIDE); }
}

/// Energy recommendations' "Apply all": screen off after 10 minutes and sleep after 30 at most (plugged
/// in), dark mode, no screen saver - what Windows' button applies.
fn energy_item() -> Vec<crate::qsx::Item> {
    vec![crate::qsx::Item { key: "energy".into(), name: "Apply all recommendations".into(), sub: "Screen off after 10 minutes, sleep after 30, dark mode, no screen saver".into(), ..Default::default() }]
}
fn energy_apply(_: &str, _: &str) {
    let screen = power_value(SUB_VIDEO, VIDEOIDLE, false).unwrap_or(0);
    if screen == 0 || screen > 600 {
        set_power_value(SUB_VIDEO, VIDEOIDLE, false, 600);
    }
    let sleep = power_value(SUB_SLEEP, STANDBYIDLE, false).unwrap_or(0);
    if sleep == 0 || sleep > 1800 {
        set_power_value(SUB_SLEEP, STANDBYIDLE, false, 1800);
    }
    let theme = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
    set_hkcu_u32(theme, "AppsUseLightTheme", 0);
    set_hkcu_u32(theme, "SystemUsesLightTheme", 0);
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, w!("Control Panel\\Desktop"), w!("SCRNSAVE.EXE"));
        let _ = SystemParametersInfoW(SPI_SETSCREENSAVEACTIVE, 0, None, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
        // apps follow the new colours
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(w!("ImmersiveColorSet").as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None);
    }
}

fn set_sender(id: &str, on: bool) {
    let v = on as u32;
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(format!("{SENDERS}\\{id}")), w!("Enabled"), REG_DWORD.0, Some(&v as *const _ as *const c_void), 4);
    }
}


static VALUES: Mutex<(usize, Vec<Val>)> = Mutex::new((usize::MAX, Vec::new()));

// ======================= backends =======================

fn reg_u32(root: HKEY, key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let (mut v, mut n) = (0u32, 4u32);
    unsafe { RegGetValueW(root, key, name, RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut c_void), Some(&mut n)).ok().ok()? };
    Some(v)
}

fn reg_str(root: HKEY, key: PCWSTR, name: PCWSTR) -> String {
    let mut buf = [0u16; 256];
    let mut n = (buf.len() * 2) as u32;
    unsafe {
        if RegGetValueW(root, key, name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut n)).is_err() {
            return String::new();
        }
    }
    String::from_utf16_lossy(&buf[..(n as usize / 2).saturating_sub(1)]).trim().to_string()
}

// ---- settings that are one number in the registry (this user's): switches and drop-downs ----

fn hkcu_u32(key: &str, name: &str) -> Option<u32> {
    reg_u32(HKEY_CURRENT_USER, PCWSTR(HSTRING::from(key).as_ptr()), PCWSTR(HSTRING::from(name).as_ptr()))
}

/// Write the number and tell running programs (Explorer's taskbar and Start listen for this).
fn set_hkcu_u32(key: &str, name: &str, v: u32) {
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name), REG_DWORD.0, Some(&v as *const _ as *const c_void), 4);
        let what = w!("TraySettings");
        let _ = SendNotifyMessageW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(what.as_ptr() as isize));
    }
}

/// Which option a stored number stands for; a number that is none of them (or no number at all) is `missing`.
pub fn choice_index(value: Option<u32>, values: &[u32], missing: usize) -> usize {
    value.and_then(|v| values.iter().position(|x| *x == v)).unwrap_or(missing)
}

/// A switch kept as one number: `on` / `off` are the stored values, `missing` what Windows assumes
/// when nothing is stored.
macro_rules! reg_switch {
    ($get:ident, $set:ident, $key:expr, $name:expr, on = $on:expr, off = $off:expr, missing = $missing:expr) => {
        fn $get() -> bool {
            hkcu_u32($key, $name).map_or($missing, |v| v == $on)
        }
        fn $set(on: bool) {
            set_hkcu_u32($key, $name, if on { $on } else { $off })
        }
    };
}

/// A drop-down kept as one number: the labels, the number each stands for, and the default.
macro_rules! reg_choice {
    ($get:ident, $list:ident, $set:ident, $key:expr, $name:expr, [$(($label:expr, $value:expr)),+], missing = $missing:expr) => {
        fn $get() -> String {
            [$($label),+][choice_index(hkcu_u32($key, $name), &[$($value),+], $missing)].to_string()
        }
        fn $list() -> Vec<String> {
            [$($label),+].iter().map(|s| s.to_string()).collect()
        }
        fn $set(i: usize) {
            if let Some(v) = [$($value),+].get(i) {
                set_hkcu_u32($key, $name, *v)
            }
        }
    };
}

const ADV: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced";

// Accessibility > Visual effects
reg_switch!(always_scrollbars, set_always_scrollbars, r"Control Panel\Accessibility", "DynamicScrollbars", on = 0, off = 1, missing = false);
fn spi_u32(get: u32) -> u32 {
    let mut v = 0u32;
    unsafe { let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(get), 0, Some(&mut v as *mut _ as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    v
}
fn set_spi(set: u32, v: u32) {
    // these two take the value in pvParam, not uiParam
    unsafe { let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(set), 0, Some(v as usize as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); }
}
fn animations() -> bool {
    spi_u32(0x1042) != 0 // SPI_GETCLIENTAREAANIMATION
}
fn set_animations(on: bool) {
    set_spi(0x1043, on as u32)
}
const DURATIONS: [(&str, u32); 6] = [("5 seconds", 5), ("7 seconds", 7), ("15 seconds", 15), ("30 seconds", 30), ("1 minute", 60), ("5 minutes", 300)];
fn toast_time() -> String {
    let v = spi_u32(0x2016); // SPI_GETMESSAGEDURATION
    DURATIONS[choice_index(Some(v), &DURATIONS.map(|d| d.1), 0)].0.to_string()
}
fn toast_times() -> Vec<String> {
    DURATIONS.iter().map(|d| d.0.to_string()).collect()
}
fn set_toast_time(i: usize) {
    if let Some(d) = DURATIONS.get(i) {
        set_spi(0x2017, d.1)
    }
}

// Privacy & security > Recommendations & offers
reg_switch!(ad_id, set_ad_id, r"Software\Microsoft\Windows\CurrentVersion\AdvertisingInfo", "Enabled", on = 1, off = 0, missing = false);
reg_switch!(local_content, set_local_content, r"Control Panel\International\User Profile", "HttpAcceptLanguageOptOut", on = 0, off = 1, missing = true);
reg_switch!(track_launches, set_track_launches, ADV, "Start_TrackProgs", on = 1, off = 0, missing = true);
reg_switch!(settings_tips, set_settings_tips, r"Software\Microsoft\Windows\CurrentVersion\ContentDeliveryManager", "SubscribedContent-338393Enabled", on = 1, off = 0, missing = true);

// Multitasking
fn snap_on() -> bool {
    let mut on = windows::core::BOOL(0);
    unsafe { let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(0x0082), 0, Some(&mut on as *mut _ as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); } // SPI_GETWINARRANGING
    on.as_bool()
}
fn set_snap(on: bool) {
    unsafe { let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(0x0083), on as u32, None, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); } // SPI_SETWINARRANGING
}
reg_switch!(snap_flyout, set_snap_flyout, ADV, "EnableSnapAssistFlyout", on = 1, off = 0, missing = true);
reg_switch!(snap_bar, set_snap_bar, ADV, "EnableSnapBar", on = 1, off = 0, missing = true);
reg_switch!(snap_suggest, set_snap_suggest, ADV, "SnapAssist", on = 1, off = 0, missing = true);
reg_switch!(shake, set_shake, ADV, "DisallowShaking", on = 0, off = 1, missing = false);
reg_choice!(alt_tab, alt_tabs, set_alt_tab, ADV, "MultiTaskingAltTabFilter",
    [("Open windows and 20 most recent tabs", 0), ("Open windows and 5 most recent tabs", 1), ("Open windows and 3 most recent tabs", 2), ("Open windows only", 3)], missing = 3);

// Clipboard
reg_switch!(clip_history, set_clip_history, "Software\\Microsoft\\Clipboard", "EnableClipboardHistory", on = 1, off = 0, missing = false);

// Taskbar (Windows' own taskbar reads these)
reg_choice!(tb_align, tb_aligns, set_tb_align, ADV, "TaskbarAl", [("Left", 0), ("Center", 1)], missing = 1);
reg_choice!(tb_search, tb_searches, set_tb_search, "Software\\Microsoft\\Windows\\CurrentVersion\\Search", "SearchboxTaskbarMode",
    [("Hide", 0), ("Search icon only", 1), ("Search box", 2), ("Search icon and label", 3)], missing = 2);
reg_choice!(tb_combine, tb_combines, set_tb_combine, ADV, "TaskbarGlomLevel", [("Always", 0), ("When taskbar is full", 1), ("Never", 2)], missing = 0);
reg_switch!(tb_taskview, set_tb_taskview, ADV, "ShowTaskViewButton", on = 1, off = 0, missing = true);
reg_switch!(tb_seconds, set_tb_seconds, ADV, "ShowSecondsInSystemClock", on = 1, off = 0, missing = false);
reg_switch!(tb_endtask, set_tb_endtask, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced\\TaskbarDeveloperSettings", "TaskbarEndTask", on = 1, off = 0, missing = false);

// Start
reg_choice!(start_layout, start_layouts, set_start_layout, ADV, "Start_Layout", [("Default", 0), ("More pins", 1), ("More recommendations", 2)], missing = 0);
reg_switch!(start_most_used, set_start_most_used, ADV, "Start_TrackProgs", on = 1, off = 0, missing = true);
reg_switch!(start_recent, set_start_recent, ADV, "Start_TrackDocs", on = 1, off = 0, missing = true);
reg_switch!(start_tips, set_start_tips, ADV, "Start_IrisRecommendations", on = 1, off = 0, missing = true);
reg_switch!(start_account, set_start_account, ADV, "Start_AccountNotifications", on = 1, off = 0, missing = true);

// Mouse
fn primary_button() -> String {
    if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 { "Right".into() } else { "Left".into() }
}
fn primary_buttons() -> Vec<String> {
    vec!["Left".into(), "Right".into()]
}
fn set_primary_button(i: usize) {
    unsafe {
        let _ = SystemParametersInfoW(SPI_SETMOUSEBUTTONSWAP, (i == 1) as u32, None, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
    }
}
/// "Enhance pointer precision" is the third number of the mouse acceleration settings.
fn precision() -> bool {
    let mut v = [0i32; 3];
    unsafe { let _ = SystemParametersInfoW(SPI_GETMOUSE, 0, Some(v.as_mut_ptr() as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    v[2] != 0
}
fn set_precision(on: bool) {
    // Windows' own values: 6 / 10 with acceleration on, 0 / 0 with it off
    let mut v: [i32; 3] = if on { [6, 10, 1] } else { [0, 0, 0] };
    unsafe { let _ = SystemParametersInfoW(SPI_SETMOUSE, 0, Some(v.as_mut_ptr() as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); }
}
const WHEEL_STEPS: [u32; 6] = [1, 3, 5, 10, 20, 0xFFFF_FFFF];
pub fn wheel_label(lines: u32) -> String {
    match lines { 0xFFFF_FFFF => "One screen at a time".into(), 1 => "1 line".into(), n => format!("{n} lines") }
}
fn wheel_lines() -> String {
    let mut v = 3u32;
    unsafe { let _ = SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut v as *mut _ as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    wheel_label(v)
}
fn wheel_choices() -> Vec<String> {
    WHEEL_STEPS.iter().map(|n| wheel_label(*n)).collect()
}
fn set_wheel_lines(i: usize) {
    if let Some(n) = WHEEL_STEPS.get(i) {
        unsafe { let _ = SystemParametersInfoW(SPI_SETWHEELSCROLLLINES, *n, None, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); }
    }
}
reg_switch!(scroll_inactive, set_scroll_inactive, "Control Panel\\Desktop", "MouseWheelRouting", on = 2, off = 0, missing = true);

// Date & time
fn now_text() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let day = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"][t.wDayOfWeek as usize % 7];
    let month = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"][(t.wMonth as usize + 11) % 12];
    let (h, ampm) = (if t.wHour % 12 == 0 { 12 } else { t.wHour % 12 }, if t.wHour < 12 { "AM" } else { "PM" });
    format!("{h}:{:02} {ampm}, {day}, {month} {}, {}", t.wMinute, t.wDay, t.wYear)
}
/// Windows sets the clock itself when its time service uses an internet time server (NTP).
fn auto_time() -> bool {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    !reg_str(HKEY_LOCAL_MACHINE, w!("SYSTEM\\CurrentControlSet\\Services\\W32Time\\Parameters"), w!("Type")).eq_ignore_ascii_case("NoSync")
}
/// The time-zone updater is a service: running unless its start type is "disabled" (4).
fn auto_zone() -> bool {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    reg_u32(HKEY_LOCAL_MACHINE, w!("SYSTEM\\CurrentControlSet\\Services\\tzautoupdate"), w!("Start")) != Some(4)
}
fn time_server() -> String {
    use windows::Win32::System::Registry::HKEY_LOCAL_MACHINE;
    let s = reg_str(HKEY_LOCAL_MACHINE, w!("SYSTEM\\CurrentControlSet\\Services\\W32Time\\Parameters"), w!("NtpServer"));
    s.split([',', ' ']).next().unwrap_or("").to_string()
}
fn zone_text() -> String {
    let mut tz = windows::Win32::System::Time::TIME_ZONE_INFORMATION::default();
    unsafe { windows::Win32::System::Time::GetTimeZoneInformation(&mut tz) };
    utc_offset(-tz.Bias) + "  " + &time_zone()
}
/// "(UTC+05:30)" from minutes east of UTC.
pub fn utc_offset(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    format!("(UTC{sign}{:02}:{:02})", minutes.abs() / 60, minutes.abs() % 60)
}
reg_switch!(tray_time, set_tray_time, ADV, "ShowSystrayDateTimeValueName", on = 1, off = 0, missing = true);

// Wi-Fi
fn wifi_list() -> Vec<crate::qsx::Item> {
    crate::qsx::wifi_scan();
    crate::qsx::wifi_networks().into_iter().map(|mut n| {
        n.sub = format!("{}  |  signal {}%", n.sub, n.signal);
        n.key = format!("wifi	{}", n.key); // not an app: no app icon is looked up for it
        n
    }).collect()
}
/// Connect (saved networks) or disconnect; a network without a saved password opens Windows' list.
fn wifi_pick(_id: &str, name: &str) {
    if let Some(n) = crate::qsx::wifi_networks().into_iter().find(|n| n.name == name) {
        crate::qsx::wifi_connect(&n);
        std::thread::sleep(std::time::Duration::from_secs(4)); // let it connect before the list is read again
    }
}

// Background
fn wallpaper_now() -> String {
    let mut buf = [0u16; 520];
    unsafe { let _ = SystemParametersInfoW(SPI_GETDESKWALLPAPER, buf.len() as u32, Some(buf.as_mut_ptr() as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    String::from_utf16_lossy(&buf[..buf.iter().position(|c| *c == 0).unwrap_or(0)])
}
fn wallpaper_text() -> String {
    let p = wallpaper_now();
    if p.is_empty() { "Solid color".into() } else { std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(p) }
}
fn set_wallpaper(path: &str) {
    let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
    unsafe { let _ = SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(w.as_ptr() as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); }
}
/// The pictures you used recently (Windows keeps five), the current one first.
fn recent_wallpapers() -> Vec<crate::qsx::Item> {
    let now = wallpaper_now().to_lowercase();
    let mut out: Vec<crate::qsx::Item> = Vec::new();
    for i in 0..5 {
        let p = reg_str(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Wallpapers"), PCWSTR(HSTRING::from(format!("BackgroundHistoryPath{i}")).as_ptr()));
        if p.is_empty() || !std::path::Path::new(&p).exists() || out.iter().any(|o| o.key.ends_with(&p)) {
            continue;
        }
        let on = p.to_lowercase() == now;
        let name = std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(p.clone());
        out.push(crate::qsx::Item { name, sub: if on { "Current background".into() } else { p.clone() }, on, key: format!("app\t{p}\t{p}"), ..Default::default() });
    }
    out
}
fn use_wallpaper(id: &str, _name: &str) {
    if let Some(p) = id.split('\t').nth(2) {
        set_wallpaper(p);
    }
}
/// "Choose a fit": Windows stores it as two numbers (WallpaperStyle, TileWallpaper).
const FITS: [(&str, &str, &str); 6] = [("Fill", "10", "0"), ("Fit", "6", "0"), ("Stretch", "2", "0"), ("Tile", "0", "1"), ("Center", "0", "0"), ("Span", "22", "0")];
pub fn fit_index(style: &str, tile: &str) -> usize {
    FITS.iter().position(|f| f.1 == style.trim() && f.2 == tile.trim()).unwrap_or(0)
}
fn fit() -> String {
    let k = w!("Control Panel\\Desktop");
    FITS[fit_index(&reg_str(HKEY_CURRENT_USER, k, w!("WallpaperStyle")), &reg_str(HKEY_CURRENT_USER, k, w!("TileWallpaper")))].0.to_string()
}
fn fits() -> Vec<String> {
    FITS.iter().map(|f| f.0.to_string()).collect()
}
fn set_fit(i: usize) {
    let Some(f) = FITS.get(i) else { return };
    for (name, v) in [("WallpaperStyle", f.1), ("TileWallpaper", f.2)] {
        let w: Vec<u16> = v.encode_utf16().chain([0]).collect();
        unsafe { let _ = RegSetKeyValueW(HKEY_CURRENT_USER, w!("Control Panel\\Desktop"), &HSTRING::from(name), REG_SZ.0, Some(w.as_ptr() as *const c_void), (w.len() * 2) as u32); }
    }
    set_wallpaper(&wallpaper_now()); // setting the same picture again makes Windows redraw it with the new fit
}
fn video_wallpaper() -> String {
    match crate::wallpaper::video_path() {
        Some(p) => format!("Playing {}", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
        None => "None - put wallpaper.mp4 next to shellbits.exe".into(),
    }
}

// Themes
fn theme_list() -> Vec<crate::qsx::Item> {
    themes().into_iter().map(|(path, _wall, on, look)| {
        let name = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let mode = if look.starts_with("dark") { "Dark mode" } else { "Light mode" };
        crate::qsx::Item { name: theme_name(&name), sub: if on { format!("{mode}  |  in use") } else { mode.into() }, on, key: format!("app\t{path}\t{path}"), ..Default::default() }
    }).collect()
}
/// File names of Windows' own themes as Settings shows them: "aero" -> "Windows (light)".
pub fn theme_name(file: &str) -> String {
    match file.to_lowercase().as_str() {
        "aero" => "Windows (light)".into(),
        "dark" => "Windows (dark)".into(),
        "spotlight" => "Windows spotlight".into(),
        "themea" => "Glow".into(),
        "themeb" => "Captured Motion".into(),
        "themec" => "Sunrise".into(),
        "themed" => "Flow".into(),
        other => { let mut c = other.chars(); c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default() }
    }
}
/// Opening a .theme file is how Windows applies a theme.
fn apply_theme_file(id: &str, _name: &str) {
    if let Some(p) = id.split('\t').nth(2) {
        unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &HSTRING::from(p), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL); }
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

// Lock screen
reg_switch!(lock_tips, set_lock_tips, "Software\\Microsoft\\Windows\\CurrentVersion\\ContentDeliveryManager", "RotatingLockScreenOverlayEnabled", on = 1, off = 0, missing = true);
fn lock_background() -> String {
    let spotlight = hkcu_u32("Software\\Microsoft\\Windows\\CurrentVersion\\ContentDeliveryManager", "RotatingLockScreenEnabled") != Some(0);
    if spotlight { "Windows spotlight".into() } else { "Picture".into() }
}

// Bluetooth & devices > Devices
fn bt_list() -> Vec<crate::qsx::Item> {
    crate::qsx::bt_devices().into_iter().map(|mut d| { d.key = format!("wifi\t{}", d.key); d }).collect() // "wifi" = no app icon
}
/// Connect / disconnect a paired audio device here (Windows' Sound settings call); anything else (a mouse,
/// a keyboard) goes to Windows' Bluetooth page.
fn bt_manage(_id: &str, name: &str) {
    let Some(d) = crate::qsx::bt_devices().into_iter().find(|d| d.name == name) else { return };
    let (key, on) = (d.key, d.on);
    std::thread::spawn(move || {
        let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
        if !crate::qsx::bt_toggle(&key, !on) {
            // not an audio device: Windows' own Bluetooth page can connect it (ours would only loop back here)
            let _ = unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("ms-settings:bluetooth"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
        }
    });
}

// Printers
const DEVICES: &str = "Software\\Microsoft\\Windows NT\\CurrentVersion\\Devices";
/// The default printer is the first part of "name,winspool,Ne00:".
pub fn default_printer_of(device_value: &str) -> String {
    device_value.split(',').next().unwrap_or("").trim().to_string()
}
fn printers() -> Vec<crate::qsx::Item> {
    let default = default_printer_of(&reg_str(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows NT\\CurrentVersion\\Windows"), w!("Device")));
    let mut out: Vec<crate::qsx::Item> = unsafe { reg_values(HKEY_CURRENT_USER, DEVICES) }
        .into_iter()
        .map(|(name, port)| {
            let on = name == default;
            let kind = if port.to_lowercase().contains("nul:") { "Saves to a file" } else { "Printer" };
            crate::qsx::Item { sub: if on { format!("{kind}  |  Default") } else { kind.into() }, on, key: format!("wifi\t{name}"), name, ..Default::default() }
        })
        .collect();
    out.sort_by(|a, b| b.on.cmp(&a.on).then(a.name.cmp(&b.name)));
    out
}
/// Windows' own printer tool sets the default printer (`/y`).
fn make_default_printer(_id: &str, name: &str) {
    let mut cmd: Vec<u16> = format!("rundll32.exe printui.dll,PrintUIEntry /y /n \"{name}\"").encode_utf16().chain([0]).collect();
    unsafe {
        use windows::Win32::System::Threading::*;
        let si = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        if CreateProcessW(None, Some(windows::core::PWSTR(cmd.as_mut_ptr())), None, None, false, PROCESS_CREATION_FLAGS(0), None, None, &si, &mut pi).is_ok() {
            let _ = WaitForSingleObject(pi.hProcess, 8000);
            let _ = CloseHandle(pi.hThread);
            let _ = CloseHandle(pi.hProcess);
        }
    }
}
reg_switch!(printer_auto_default, set_printer_auto_default, "Software\\Microsoft\\Windows NT\\CurrentVersion\\Windows", "LegacyDefaultPrinterMode", on = 0, off = 1, missing = true);

// Touchpad (Windows' precision touchpad settings for this user)
const PTP: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\PrecisionTouchPad";
reg_switch!(tp_taps, set_tp_taps, PTP, "TapsEnabled", on = 1, off = 0, missing = true);
reg_switch!(tp_two_finger_tap, set_tp_two_finger_tap, PTP, "TwoFingerTapEnabled", on = 1, off = 0, missing = true);
reg_switch!(tp_drag, set_tp_drag, PTP, "TapAndDrag", on = 1, off = 0, missing = true);
reg_switch!(tp_right_zone, set_tp_right_zone, PTP, "RightClickZoneEnabled", on = 1, off = 0, missing = true);
reg_switch!(tp_scroll, set_tp_scroll, PTP, "PanEnabled", on = 1, off = 0, missing = true);
reg_switch!(tp_zoom, set_tp_zoom, PTP, "ZoomEnabled", on = 1, off = 0, missing = true);
reg_switch!(tp_with_mouse, set_tp_with_mouse, PTP, "LeaveOnWithMouse", on = 1, off = 0, missing = true);
reg_choice!(tp_sensitivity, tp_sensitivities, set_tp_sensitivity, PTP, "AAPThreshold",
    [("Most sensitive", 0), ("High sensitivity", 1), ("Medium sensitivity", 2), ("Low sensitivity", 3)], missing = 2);

// Gaming
reg_switch!(game_mode, set_game_mode, "Software\\Microsoft\\GameBar", "AutoGameModeEnabled", on = 1, off = 0, missing = true);
reg_switch!(game_bar, set_game_bar, "Software\\Microsoft\\GameBar", "UseNexusForGameBarEnabled", on = 1, off = 0, missing = true);
reg_switch!(game_dvr, set_game_dvr, "System\\GameConfigStore", "GameDVR_Enabled", on = 1, off = 0, missing = true);

// Typing
const TIP: &str = "Software\\Microsoft\\TabletTip\\1.7";
reg_switch!(hw_suggest, set_hw_suggest, "Software\\Microsoft\\Input\\Settings", "EnableHwkbTextPrediction", on = 1, off = 0, missing = false);
reg_switch!(multilingual, set_multilingual, "Software\\Microsoft\\Input\\Settings", "MultilingualEnabled", on = 1, off = 0, missing = false);
reg_switch!(autocorrect, set_autocorrect, TIP, "EnableAutocorrection", on = 1, off = 0, missing = true);
reg_switch!(spellcheck, set_spellcheck, TIP, "EnableSpellchecking", on = 1, off = 0, missing = true);

// Language & region
fn intl(name: &str) -> String {
    reg_str(HKEY_CURRENT_USER, w!("Control Panel\\International"), PCWSTR(HSTRING::from(name).as_ptr()))
}
/// A locale's name in its own words, e.g. "en-US" -> "English (United States)".
fn locale_display(tag: &str) -> String {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SLOCALIZEDDISPLAYNAME};
    let mut buf = [0u16; 128];
    let n = unsafe { GetLocaleInfoEx(&HSTRING::from(tag), LOCALE_SLOCALIZEDDISPLAYNAME, Some(&mut buf)) };
    if n > 1 { String::from_utf16_lossy(&buf[..n as usize - 1]) } else { tag.to_string() }
}
fn display_language() -> String {
    use windows::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
    let mut buf = [0u16; 85];
    let n = unsafe { LCIDToLocaleName(GetUserDefaultUILanguage() as u32, Some(&mut buf), 0) };
    locale_display(&String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]))
}
fn regional_format() -> String {
    locale_display(&intl("LocaleName"))
}
fn country() -> String {
    use windows::Win32::Globalization::{GetGeoInfoEx, GetUserDefaultGeoName, GEO_FRIENDLYNAME};
    let mut code = [0u16; 16];
    let n = unsafe { GetUserDefaultGeoName(&mut code) };
    let code = String::from_utf16_lossy(&code[..(n.max(1) - 1) as usize]);
    let mut name = [0u16; 128];
    let m = unsafe { GetGeoInfoEx(&HSTRING::from(code.as_str()), GEO_FRIENDLYNAME, Some(&mut name)) };
    if m > 1 { String::from_utf16_lossy(&name[..m as usize - 1]) } else { code }
}
/// Today written the way the regional format writes dates and times.
fn format_sample() -> String {
    format!("Short date {}  |  Time {}", intl("sShortDate"), intl("sTimeFormat"))
}
pub fn first_day(v: &str) -> String {
    // Windows counts from Monday (0) to Sunday (6)
    ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"].get(v.trim().parse::<usize>().unwrap_or(6)).unwrap_or(&"Sunday").to_string()
}
fn week_start() -> String {
    first_day(&intl("iFirstDayOfWeek"))
}

// Accounts
fn user_name() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}
fn pc_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}
reg_switch!(dynamic_lock, set_dynamic_lock, "Software\\Microsoft\\Windows NT\\CurrentVersion\\Winlogon", "EnableGoodbye", on = 1, off = 0, missing = false);

// Storage
reg_switch!(storage_sense, set_storage_sense, "Software\\Microsoft\\Windows\\CurrentVersion\\StorageSense\\Parameters\\StoragePolicy", "01", on = 1, off = 0, missing = false);

/// One line about a drive: "312 GB used of 476 GB  |  164 GB free".
pub fn drive_line(total: u64, free: u64) -> String {
    format!("{} used of {}  |  {} free", size_label(total.saturating_sub(free)), size_label(total), size_label(free))
}

/// This PC's fixed drives, each with its name, how full it is, and its own icon.
fn drive_list() -> Vec<crate::qsx::Item> {
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    let mut out = Vec::new();
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26u8 {
            let letter = (b'A' + i) as char;
            let root = HSTRING::from(format!("{letter}:\\"));
            if mask & (1 << i) == 0 || GetDriveTypeW(&root) != 3 {
                continue;
            }
            let (mut free, mut all) = (0u64, 0u64);
            if GetDiskFreeSpaceExW(&root, None, Some(&mut all), Some(&mut free)).is_err() {
                continue;
            }
            let mut label = [0u16; 64];
            let _ = GetVolumeInformationW(&root, Some(&mut label), None, None, None, None);
            let label = String::from_utf16_lossy(&label[..label.iter().position(|c| *c == 0).unwrap_or(0)]);
            out.push(crate::qsx::Item {
                name: format!("{} ({letter}:)", if label.is_empty() { "Local Disk" } else { &label }),
                sub: drive_line(all, free),
                key: format!("app\t{letter}:\\\t{letter}:\\"),
                ..Default::default()
            });
        }
    }
    out
}

/// A drive's Open button: show it in File Explorer.
fn open_drive(id: &str, _name: &str) {
    if let Some(path) = id.split('\t').nth(2) {
        crate::files::request(path);
    }
}

/// How much is in this user's temporary-files folder.
// ponytail: stops counting after 50,000 files so a huge folder cannot hold the page up
fn temp_size() -> String {
    fn walk(dir: &std::path::Path, left: &mut u32) -> u64 {
        let mut sum = 0;
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if *left == 0 {
                break;
            }
            *left -= 1;
            match e.metadata() {
                Ok(m) if m.is_dir() => sum += walk(&e.path(), left),
                Ok(m) => sum += m.len(),
                _ => {}
            }
        }
        sum
    }
    let mut left = 50_000;
    let bytes = walk(&std::env::temp_dir(), &mut left);
    if left == 0 { format!("More than {}", size_label(bytes)) } else { size_label(bytes) }
}

const PERSONALIZE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

fn set_personalize(name: PCWSTR, v: u32) {
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, PERSONALIZE, name, REG_DWORD.0, Some(&v as *const _ as *const c_void), 4);
        // tell every app the colours changed, like Settings does
        let what = w!("ImmersiveColorSet");
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(what.as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None);
    }
}

/// Value of `key=` in an .ini-style theme file's text.
pub fn ini_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        k.trim().eq_ignore_ascii_case(key).then(|| v.trim().to_string())
    })
}
/// Windows' built-in themes: (theme file, wallpaper file, is the current theme, "").
fn themes() -> Vec<(String, String, bool, String)> {
    let root = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let current = reg_str(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes"), w!("CurrentTheme")).to_lowercase();
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(format!("{root}\\Resources\\Themes")).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("theme"))).collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|f| {
            // theme files are UTF-16 or ANSI
            let bytes = std::fs::read(&f).ok()?;
            let text = if bytes.starts_with(&[0xFF, 0xFE]) { String::from_utf16_lossy(&bytes[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect::<Vec<_>>()) } else { String::from_utf8_lossy(&bytes).to_string() };
            let wall = ini_value(&text, "Wallpaper")?.replace("%SystemRoot%", &root).replace("%systemroot%", &root);
            let path = f.display().to_string();
            let dark = ini_value(&text, "SystemMode").is_some_and(|m| m.eq_ignore_ascii_case("dark"));
            Some((path.clone(), wall, path.to_lowercase() == current, format!("{}:{}", if dark { "dark" } else { "light" }, ini_value(&text, "ColorizationColor").map(|c| c[c.len().saturating_sub(6)..].to_string()).unwrap_or_default())))
        })
        .collect()
}
fn color_mode() -> String {
    if dark() { "Dark".into() } else { "Light".into() }
}
fn color_modes() -> Vec<String> {
    vec!["Light".into(), "Dark".into()]
}
fn set_color_mode(i: usize) {
    set_dark(i == 1);
}

fn dark() -> bool {
    reg_u32(HKEY_CURRENT_USER, PERSONALIZE, w!("AppsUseLightTheme")) == Some(0)
}
fn set_dark(on: bool) {
    set_personalize(w!("AppsUseLightTheme"), !on as u32);
    set_personalize(w!("SystemUsesLightTheme"), !on as u32);
}
fn transparency() -> bool {
    reg_u32(HKEY_CURRENT_USER, PERSONALIZE, w!("EnableTransparency")) != Some(0)
}
fn set_transparency(on: bool) {
    set_personalize(w!("EnableTransparency"), on as u32);
}

fn volume() -> f32 {
    unsafe { crate::quick::endpoint().and_then(|e| e.GetMasterVolumeLevelScalar()).unwrap_or(0.0) }
}
fn set_volume(v: f32) {
    unsafe {
        if let Ok(e) = crate::quick::endpoint() {
            let _ = e.SetMasterVolumeLevelScalar(v.clamp(0.0, 1.0), std::ptr::null());
        }
    }
}
fn muted() -> bool {
    unsafe { crate::quick::endpoint().and_then(|e| e.GetMute()).map(|b| b.as_bool()).unwrap_or(false) }
}
fn set_muted(on: bool) {
    unsafe {
        if let Ok(e) = crate::quick::endpoint() {
            let _ = e.SetMute(on, std::ptr::null());
        }
    }
}
const SCALES: [u32; 12] = [100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

/// Where the recommended scale sits in SCALES, given the current scale and how many steps Windows
/// says it is away from the recommended one.
pub fn recommended_index(current_percent: u32, steps_from_recommended: i32) -> usize {
    let cur = SCALES.iter().position(|s| *s >= current_percent).unwrap_or(0) as i32;
    (cur - steps_from_recommended).clamp(0, SCALES.len() as i32 - 1) as usize
}
fn scale_now() -> (u32, usize) {
    unsafe {
        let pct = windows::Win32::UI::HiDpi::GetDpiForSystem() * 100 / 96;
        let mut rel = 0i32;
        // 0x9E / 0x9F: the scale override Windows' own Settings page reads and writes (steps from recommended)
        let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(0x9E), 0, Some(&mut rel as *mut _ as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        (pct, recommended_index(pct, rel))
    }
}
fn scale_label(i: usize, rec: usize) -> String {
    format!("{}%{}", SCALES[i], if i == rec { " (Recommended)" } else { "" })
}
fn scale() -> String {
    let (pct, rec) = scale_now();
    scale_label(SCALES.iter().position(|s| *s >= pct).unwrap_or(0), rec)
}
fn scales() -> Vec<String> {
    let rec = scale_now().1;
    (0..=(rec + 3).min(SCALES.len() - 1)).map(|i| scale_label(i, rec)).collect()
}
fn set_scale(i: usize) {
    let rec = scale_now().1 as i32;
    unsafe {
        let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(0x9F), (i as i32 - rec) as u32, None, SPIF_UPDATEINIFILE);
    }
}

fn current_mode() -> DEVMODEW {
    let mut m = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    unsafe {
        let _ = EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut m);
    }
    m
}
/// Every size the main display offers, biggest first.
fn modes() -> Vec<(u32, u32)> {
    let mut v = Vec::new();
    let mut m = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    let mut i = 0;
    unsafe {
        while EnumDisplaySettingsW(PCWSTR::null(), ENUM_DISPLAY_SETTINGS_MODE(i), &mut m).as_bool() {
            if !v.contains(&(m.dmPelsWidth, m.dmPelsHeight)) {
                v.push((m.dmPelsWidth, m.dmPelsHeight));
            }
            i += 1;
        }
    }
    v.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)));
    v
}
fn resolution() -> String {
    let m = current_mode();
    format!("{} \u{D7} {}", m.dmPelsWidth, m.dmPelsHeight)
}
fn resolutions() -> Vec<String> {
    modes().iter().map(|(w_, h)| format!("{w_} \u{D7} {h}")).collect()
}
/// Try a display change the way Windows does: apply it, ask, and go back unless you say keep (15 s).
fn try_mode(m: DEVMODEW) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxTimeoutW(hwnd: *mut c_void, text: PCWSTR, caption: PCWSTR, kind: u32, lang: u16, ms: u32) -> i32;
    }
    unsafe {
        if ChangeDisplaySettingsExW(PCWSTR::null(), Some(&m), None, CDS_TYPE(0), None) != DISP_CHANGE_SUCCESSFUL {
            return;
        }
        let keep = MessageBoxTimeoutW(std::ptr::null_mut(), w!("Keep these display settings?\n\nReverting to the previous settings in 15 seconds."), w!("Settings"), 0x4 | 0x20 | 0x40000 | 0x100, 0, 15000) == 6;
        if keep {
            let _ = ChangeDisplaySettingsExW(PCWSTR::null(), Some(&m), None, CDS_UPDATEREGISTRY, None);
        } else {
            let _ = ChangeDisplaySettingsExW(PCWSTR::null(), None, None, CDS_TYPE(0), None);
        }
    }
}
fn set_resolution(i: usize) {
    if let Some((w_, h)) = modes().get(i).copied() {
        let mut m = current_mode();
        if (m.dmPelsWidth, m.dmPelsHeight) != (w_, h) {
            (m.dmPelsWidth, m.dmPelsHeight) = (w_, h);
            m.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT;
            try_mode(m);
        }
    }
}
const ORIENTATIONS: [&str; 4] = ["Landscape", "Portrait", "Landscape (flipped)", "Portrait (flipped)"];
fn orientation_index() -> usize {
    unsafe { current_mode().Anonymous1.Anonymous2.dmDisplayOrientation.0 as usize % 4 }
}
fn orientation() -> String {
    ORIENTATIONS[orientation_index()].to_string()
}
fn orientations() -> Vec<String> {
    ORIENTATIONS.iter().map(|s| s.to_string()).collect()
}
fn set_orientation(i: usize) {
    let old = orientation_index();
    if i == old || i > 3 {
        return;
    }
    let mut m = current_mode();
    if (i + old) % 2 == 1 {
        (m.dmPelsWidth, m.dmPelsHeight) = (m.dmPelsHeight, m.dmPelsWidth); // turning a quarter swaps width and height
    }
    m.Anonymous1.Anonymous2.dmDisplayOrientation = DEVMODE_DISPLAY_ORIENTATION(i as u32);
    m.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYORIENTATION;
    try_mode(m);
}
/// Night light on/off, read from where Windows keeps it.
/// ponytail: read-only; the switch opens Windows' night light page, because writing this blob is undocumented
fn night_light() -> bool {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\CloudStore\\Store\\DefaultAccount\\Current\\default$windows.data.bluelightreduction.bluelightreductionstate\\windows.data.bluelightreduction.bluelightreductionstate");
    let mut buf = [0u8; 128];
    let mut n = 128u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key, w!("Data"), RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut n)).is_ok() && n > 18 && buf[18] == 0x15 }
}

fn mouse_speed() -> f32 {
    let mut v = 10i32;
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETMOUSESPEED, 0, Some(&mut v as *mut _ as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    (v - 1) as f32 / 19.0
}
fn set_mouse_speed(f: f32) {
    let v = (1.0 + f * 19.0).round() as isize;
    unsafe {
        let _ = SystemParametersInfoW(SPI_SETMOUSESPEED, 0, Some(v as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
    }
}

fn radio(kind: windows::Devices::Radios::RadioKind) -> bool {
    use windows::Devices::Radios::RadioState;
    crate::quick::radios().unwrap_or_default().iter().any(|r| r.Kind().ok() == Some(kind) && r.State().ok() == Some(RadioState::On))
}
fn wifi() -> bool {
    radio(windows::Devices::Radios::RadioKind::WiFi)
}
fn set_wifi(on: bool) {
    crate::quick::set_radio(windows::Devices::Radios::RadioKind::WiFi, on);
}
fn bluetooth() -> bool {
    radio(windows::Devices::Radios::RadioKind::Bluetooth)
}
fn set_bluetooth(on: bool) {
    crate::quick::set_radio(windows::Devices::Radios::RadioKind::Bluetooth, on);
}
fn airplane() -> bool {
    crate::qsx::airplane() == Some(true)
}
/// The connection carrying your internet: its name (Wi-Fi network or adapter) and whether it is wired.
/// Empty name = not connected.
fn connection() -> (String, bool) {
    use windows::Win32::NetworkManagement::IpHelper::{GetAdaptersAddresses, GAA_FLAG_INCLUDE_GATEWAYS, IP_ADAPTER_ADDRESSES_LH};
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    let mut size = 64 * 1024u32;
    let mut buf = vec![0u8; size as usize];
    unsafe {
        if GetAdaptersAddresses(0, GAA_FLAG_INCLUDE_GATEWAYS, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size) != 0 {
            return Default::default();
        }
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            if a.OperStatus == IfOperStatusUp && !a.FirstGatewayAddress.is_null() {
                let wifi = a.IfType == 71;
                let ssid = if wifi { crate::quick::wifi_ssid() } else { String::new() };
                return (if ssid.is_empty() { a.FriendlyName.to_string().unwrap_or_default() } else { ssid }, !wifi);
            }
            p = a.Next;
        }
    }
    Default::default()
}
/// "Discoverable as "NAME"" - built once, the PC name doesn't change while we run.
fn discoverable() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| format!("Discoverable as \"{}\"", device_name()))
}
#[allow(dead_code)]
fn bt_device() -> String {
    let d = crate::quick::bt_device();
    if d.is_empty() { "No device connected".into() } else { d }
}

fn saver() -> bool {
    crate::qsx::saver_on()
}
fn set_saver(on: bool) {
    crate::qsx::set_saver(on);
}
fn battery() -> String {
    crate::quick::battery().map(|(p, _)| format!("{p}%")).unwrap_or_else(|| "No battery".into())
}
/// Windows shows this bar when power settings are set by policy.
fn power_managed() -> bool {
    let mut k = HKEY::default();
    unsafe {
        let ok = RegOpenKeyExW(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Policies\\Microsoft\\Power\\PowerSettings"), Some(0), KEY_READ, &mut k).is_ok();
        if ok {
            let _ = RegCloseKey(k);
        }
        ok
    }
}
// Windows' three power modes (overlay schemes), in the order its drop-down lists them
const POWER_MODES: [(&str, u128); 3] = [("Best Power Efficiency", 0x961cc777_2547_4f9d_8174_7d86181b8a7a), ("Balanced", 0), ("Best Performance", 0xded574b5_45a0_4f42_8737_46345c09c238)];
#[link(name = "powrprof", kind = "raw-dylib")]
unsafe extern "system" {
    fn PowerGetEffectiveOverlayScheme(scheme: *mut windows::core::GUID) -> u32;
    fn PowerSetActiveOverlayScheme(scheme: *const windows::core::GUID) -> u32;
}
fn power_mode() -> String {
    let mut g = windows::core::GUID::zeroed();
    unsafe { PowerGetEffectiveOverlayScheme(&mut g) };
    POWER_MODES.iter().find(|m| m.1 == g.to_u128()).map(|m| m.0).unwrap_or("Balanced").to_string()
}
fn power_modes() -> Vec<String> {
    POWER_MODES.iter().map(|m| m.0.to_string()).collect()
}
fn set_power_mode(i: usize) {
    if let Some(m) = POWER_MODES.get(i) {
        unsafe { PowerSetActiveOverlayScheme(&windows::core::GUID::from_u128(m.1)) };
    }
}
fn battery_percent_shown() -> bool {
    reg_u32(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"), w!("IsBatteryPercentageEnabled")) == Some(1)
}
fn set_battery_percent(on: bool) {
    let v = on as u32;
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced"), w!("IsBatteryPercentageEnabled"), REG_DWORD.0, Some(&v as *const _ as *const c_void), 4);
    }
}
/// "On", or the battery level at which energy saver switches itself on.
fn saver_text() -> String {
    use windows::Win32::System::Power::{PowerGetActiveScheme, PowerReadDCValueIndex};
    if saver() {
        return "On".into();
    }
    unsafe {
        let mut scheme = std::ptr::null_mut();
        let mut pct = 0u32;
        if PowerGetActiveScheme(None, &mut scheme).is_ok() {
            let sub = windows::core::GUID::from_u128(0xde830923_a562_41af_a086_e3a2c6bad2da);
            let key = windows::core::GUID::from_u128(0xe69653ca_cf7f_4f05_aa73_cb833fa90ad4);
            let _ = PowerReadDCValueIndex(None, Some(scheme), Some(&sub), Some(&key), &mut pct);
            let _ = LocalFree(Some(HLOCAL(scheme as *mut c_void)));
        }
        match pct {
            0 => "Never".into(),
            100 => "Always on".into(),
            p => format!("Turns on at {p}%"),
        }
    }
}
fn saver_choices() -> Vec<String> {
    vec![if saver() { "Turn off now".into() } else { "Turn on now".into() }]
}
fn toggle_saver(_: usize) {
    set_saver(!saver());
}

fn access(i: usize) -> bool {
    crate::qsx::access_states().get(i).is_some_and(|a| a.on)
}
fn sticky() -> bool {
    access(5)
}
fn set_sticky(on: bool) {
    crate::qsx::toggle_access(5, !on);
}
fn magnifier() -> bool { access(0) }
fn set_magnifier(on: bool) { crate::qsx::toggle_access(0, !on); }
fn color_filters() -> bool { access(1) }
fn set_color_filters(on: bool) { crate::qsx::toggle_access(1, !on); }
fn narrator() -> bool { access(2) }
fn set_narrator(on: bool) { crate::qsx::toggle_access(2, !on); }

// Accessibility > Keyboard: the classic accessibility switches, through SystemParametersInfo
fn spi_flags(get: u32, size: u32) -> u32 {
    // FILTERKEYS / TOGGLEKEYS both start with cbSize then dwFlags
    let mut buf = [0u32; 8]; // MOUSEKEYS is 7 DWORDs: room for the largest asked for
    buf[0] = size;
    unsafe { let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(get), size, Some(buf.as_mut_ptr() as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    buf[1]
}
fn set_spi_flag(get: u32, set: u32, size: u32, on: bool) {
    let mut buf = [0u32; 8]; // MOUSEKEYS is 7 DWORDs: room for the largest asked for
    buf[0] = size;
    unsafe {
        let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(get), size, Some(buf.as_mut_ptr() as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        buf[1] = if on { buf[1] | 1 } else { buf[1] & !1 }; // the *_ON bit
        let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(set), size, Some(buf.as_mut_ptr() as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
    }
}
fn filter_keys() -> bool { spi_flags(0x0032, 24) & 1 != 0 } // SPI_GETFILTERKEYS, FILTERKEYS is 24 bytes
fn set_filter_keys(on: bool) { set_spi_flag(0x0032, 0x0033, 24, on) }
fn toggle_keys() -> bool { spi_flags(0x0034, 8) & 1 != 0 } // SPI_GETTOGGLEKEYS
fn set_toggle_keys(on: bool) { set_spi_flag(0x0034, 0x0035, 8, on) }
fn access_keys() -> bool { spi_u32(0x100A) != 0 } // SPI_GETKEYBOARDCUES
fn set_access_keys(on: bool) { set_spi(0x100B, on as u32) }
reg_switch!(prtscn_snip, set_prtscn_snip, r"Control Panel\Keyboard", "PrintScreenKeyForSnippingEnabled", on = 1, off = 0, missing = true);

// Privacy & security > Camera / Microphone / Location: this user's "let apps use ..." switch
const CONSENT: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\";
fn consent(what: &str) -> bool {
    reg_str(HKEY_CURRENT_USER, PCWSTR(HSTRING::from(format!("{CONSENT}{what}")).as_ptr()), w!("Value")) != "Deny"
}
fn set_consent(what: &str, on: bool) {
    let v: Vec<u16> = (if on { "Allow" } else { "Deny" }).encode_utf16().chain([0]).collect();
    unsafe { let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(format!("{CONSENT}{what}")), w!("Value"), REG_SZ.0, Some(v.as_ptr() as *const c_void), (v.len() * 2) as u32); }
}
fn camera_ok() -> bool { consent("webcam") }
fn set_camera_ok(on: bool) { set_consent("webcam", on) }
fn mic_ok() -> bool { consent("microphone") }
fn set_mic_ok(on: bool) { set_consent("microphone", on) }
fn location_ok() -> bool { consent("location") }
fn set_location_ok(on: bool) { set_consent("location", on) }

// Accessibility > Text size: Windows' text scale (100-225%); apps pick it up when they next start
reg_choice!(text_size, text_sizes, set_text_size, r"Software\Microsoft\Accessibility", "TextScaleFactor",
    [("100%", 100), ("110%", 110), ("125%", 125), ("150%", 150), ("175%", 175), ("200%", 200), ("225%", 225)], missing = 0);
// Accessibility > Mouse pointer size, 1 to 15: the base size Windows draws cursors at (32 px plus 16 a
// step), then the cursors reloaded so it shows at once
fn pointer_size() -> String {
    format!("{}", hkcu_u32(r"Software\Microsoft\Accessibility", "CursorSize").unwrap_or(1))
}
fn pointer_size_v() -> f32 {
    (hkcu_u32(r"Software\Microsoft\Accessibility", "CursorSize").unwrap_or(1).clamp(1, 15) - 1) as f32 / 14.0
}
fn set_pointer_size_v(v: f32) {
    let n = (1.0 + v.clamp(0.0, 1.0) * 14.0).round() as u32;
    set_hkcu_u32(r"Software\Microsoft\Accessibility", "CursorSize", n);
    set_hkcu_u32(r"Control Panel\Cursors", "CursorBaseSize", 32 + (n - 1) * 16);
    unsafe { let _ = SystemParametersInfoW(SPI_SETCURSORS, 0, None, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); }
}
fn pointer_style() -> String {
    match hkcu_u32(r"Software\Microsoft\Accessibility", "CursorType").unwrap_or(0) { 1 => "Black", 2 => "Inverted", 6 => "Custom", _ => "White" }.into()
}
// Accessibility > Contrast themes: on or off (choosing a theme stays in Windows' page)
fn high_contrast() -> bool {
    spi_flags(0x0042, 16) & 1 != 0 // SPI_GETHIGHCONTRAST, HCF_HIGHCONTRASTON
}
fn contrast_text() -> String {
    if high_contrast() { "On".into() } else { "None".into() }
}
// Accessibility > Mouse: mouse keys (move the pointer with the number pad)
fn mouse_keys() -> bool { spi_flags(0x0036, 28) & 1 != 0 } // SPI_GETMOUSEKEYS, MOUSEKEYS is 28 bytes
fn set_mouse_keys(on: bool) { set_spi_flag(0x0036, 0x0037, 28, on) }
/// MOUSEKEYS: cbSize, dwFlags, iMaxSpeed (10-360), iTimeToMaxSpeed (1000-5000 ms), iCtrlSpeed, 2 reserved.
fn mk_get() -> [u32; 7] {
    let mut b = [0u32; 7];
    b[0] = 28;
    unsafe { let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(0x0036), 28, Some(b.as_mut_ptr() as *mut c_void), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)); }
    b
}
fn mk_set(f: impl FnOnce(&mut [u32; 7])) {
    let mut b = mk_get();
    f(&mut b);
    unsafe { let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(0x0037), 28, Some(b.as_mut_ptr() as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); }
}
fn mk_speed() -> f32 { (mk_get()[2] as f32 - 10.0) / 350.0 }
fn set_mk_speed(v: f32) { mk_set(|b| b[2] = (10.0 + v.clamp(0.0, 1.0) * 350.0).round() as u32) }
// acceleration: further right reaches top speed sooner (less time)
fn mk_accel() -> f32 { 1.0 - (mk_get()[3] as f32 - 1000.0) / 4000.0 }
fn set_mk_accel(v: f32) { mk_set(|b| b[3] = (5000.0 - v.clamp(0.0, 1.0) * 4000.0).round() as u32) }
fn mk_ctrl() -> bool { mk_get()[1] & 0x40 != 0 } // MKF_MODIFIERS
fn set_mk_ctrl(on: bool) { mk_set(|b| b[1] = if on { b[1] | 0x40 } else { b[1] & !0x40 }) }
fn mk_numlock() -> bool { mk_get()[1] & 0x80 != 0 } // MKF_REPLACENUMBERS
fn set_mk_numlock(on: bool) { mk_set(|b| b[1] = if on { b[1] | 0x80 } else { b[1] & !0x80 }) }
// Captions: caption style in use (read only)
fn caption_style() -> String {
    let v = reg_str(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\ClosedCaptioning"), w!("CaptionStyle"));
    if v.is_empty() { "Default".into() } else { v }
}
// Privacy & security > Speech: online speech recognition
reg_switch!(online_speech, set_online_speech, r"Software\Microsoft\Speech_OneCore\Settings\OnlineSpeechPrivacy", "HasAccepted", on = 1, off = 0, missing = false);

// Windows Update: pause state, active hours, other switches (machine-wide: read here, changed in Windows' page)
const WU_UX: PCWSTR = w!("SOFTWARE\\Microsoft\\WindowsUpdate\\UX\\Settings");
fn pause_state() -> String {
    let until = reg_str(HKEY_LOCAL_MACHINE, WU_UX, w!("PauseUpdatesExpiryTime"));
    match until.get(..10) {
        Some(d) if !until.is_empty() => format!("Paused until {d}"),
        _ => "Not paused".into(),
    }
}
fn active_hours() -> String {
    match (reg_u32(HKEY_LOCAL_MACHINE, WU_UX, w!("ActiveHoursStart")), reg_u32(HKEY_LOCAL_MACHINE, WU_UX, w!("ActiveHoursEnd"))) {
        (Some(a), Some(b)) => format!("{a}:00 to {b}:00"),
        _ => "Set automatically".into(),
    }
}
fn other_products() -> bool { reg_u32(HKEY_LOCAL_MACHINE, WU_UX, w!("AllowMUUpdateService")).unwrap_or(0) == 1 }
fn restart_notify() -> bool { reg_u32(HKEY_LOCAL_MACHINE, WU_UX, w!("RestartNotificationsAllowed2")).unwrap_or(0) == 1 }
fn metered_updates() -> bool { reg_u32(HKEY_LOCAL_MACHINE, WU_UX, w!("AllowAutoWindowsUpdateDownloadOverMeteredNetwork")).unwrap_or(0) == 1 }

/// Windows Update's own history, newest first: title, then "Installed on 13/11/2026" / "Failed ...".
fn update_history() -> Vec<crate::qsx::Item> {
    use windows::Win32::System::UpdateAgent::*;
    let mut out = Vec::new();
    unsafe {
        let Ok(session) = windows::Win32::System::Com::CoCreateInstance::<_, IUpdateSession>(&UpdateSession, None, windows::Win32::System::Com::CLSCTX_INPROC_SERVER) else { return out };
        let Ok(searcher) = session.CreateUpdateSearcher() else { return out };
        let n = searcher.GetTotalHistoryCount().unwrap_or(0).min(60);
        let Ok(list) = searcher.QueryHistory(0, n) else { return out };
        for i in 0..list.Count().unwrap_or(0) {
            let Ok(e) = list.get_Item(i) else { continue };
            let title = e.Title().map(|t| t.to_string()).unwrap_or_default();
            if title.is_empty() { continue; }
            let (y, m, d) = civil(e.Date().unwrap_or(0.0) as i64 - 25569); // OLE dates count from 1899-12-30
            let state = match e.ResultCode().map(|r| r.0).unwrap_or(0) { 2 => "Successfully installed", 3 => "Installed with errors", 4 => "Failed to install", 5 => "Cancelled", _ => "In progress" };
            out.push(crate::qsx::Item { key: format!("wu\t{i}"), name: title, sub: format!("{state} on {d}/{m}/{y}"), ..Default::default() });
        }
    }
    out
}
fn copy_update(_: &str, name: &str) {
    copy_text(name);
}


// ======================= System pages, part 2 (PLAN-600 S1-S40) =======================

// S1 brightness (built-in screens; WMI)
fn brightness_get() -> f32 { unsafe { crate::quick::get_brightness() }.unwrap_or(0) as f32 / 100.0 }
fn brightness_set(v: f32) { crate::quick::set_brightness((v * 100.0).round() as u32) }

// S6 refresh rate of the main display (same resolution and colour depth)
fn rates() -> Vec<u32> {
    let cur = current_mode();
    let mut v = Vec::new();
    let mut m = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
    let mut i = 0;
    unsafe {
        while EnumDisplaySettingsW(PCWSTR::null(), ENUM_DISPLAY_SETTINGS_MODE(i), &mut m).as_bool() {
            if (m.dmPelsWidth, m.dmPelsHeight, m.dmBitsPerPel) == (cur.dmPelsWidth, cur.dmPelsHeight, cur.dmBitsPerPel) && m.dmDisplayFrequency > 1 && !v.contains(&m.dmDisplayFrequency) {
                v.push(m.dmDisplayFrequency);
            }
            i += 1;
        }
    }
    v.sort_unstable_by(|a, b| b.cmp(a));
    v
}
fn refresh() -> String { format!("{} Hz", current_mode().dmDisplayFrequency) }
fn refreshes() -> Vec<String> { rates().iter().map(|r| format!("{r} Hz")).collect() }
fn set_refresh(i: usize) {
    if let Some(r) = rates().get(i).copied() {
        let mut m = current_mode();
        if m.dmDisplayFrequency != r {
            m.dmDisplayFrequency = r;
            m.dmFields = DM_DISPLAYFREQUENCY;
            try_mode(m);
        }
    }
}
fn display_facts() -> Vec<Line> {
    let m = current_mode();
    let line = |k: &str, v: String| (k.to_string(), v, false, String::new());
    let mut name = String::new();
    unsafe {
        let mut dd = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if EnumDisplayDevicesW(PCWSTR::null(), 0, &mut dd, 0).as_bool() {
            let adapter = String::from_utf16_lossy(&dd.DeviceName[..dd.DeviceName.iter().position(|&c| c == 0).unwrap_or(32)]);
            let mut mon = DISPLAY_DEVICEW { cb: size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if EnumDisplayDevicesW(&HSTRING::from(adapter), 0, &mut mon, 0).as_bool() {
                name = String::from_utf16_lossy(&mon.DeviceString[..mon.DeviceString.iter().position(|&c| c == 0).unwrap_or(128)]);
            }
            let gpu = String::from_utf16_lossy(&dd.DeviceString[..dd.DeviceString.iter().position(|&c| c == 0).unwrap_or(128)]);
            return vec![
                line("Display", name),
                line("Connected to", gpu),
                line("Desktop mode", format!("{} \u{00D7} {}, {} Hz", m.dmPelsWidth, m.dmPelsHeight, m.dmDisplayFrequency)),
                line("Bit depth", format!("{}-bit", (m.dmBitsPerPel / 4).clamp(6, 12))),
            ];
        }
    }
    vec![line("Desktop mode", format!("{} \u{00D7} {}, {} Hz", m.dmPelsWidth, m.dmPelsHeight, m.dmDisplayFrequency))]
}

// S7 per-app graphics preference (what Windows' Graphics page writes)
const GPU_PREF: &str = r"Software\Microsoft\DirectX\UserGpuPreferences";
pub fn gpu_label(data: &str) -> &'static str {
    match data.split(';').find_map(|p| p.strip_prefix("GpuPreference=")).and_then(|v| v.trim().parse::<u32>().ok()) {
        Some(1) => "Power saving",
        Some(2) => "High performance",
        _ => "Let Windows decide",
    }
}
fn gpu_apps() -> Vec<crate::qsx::Item> {
    unsafe { reg_values(HKEY_CURRENT_USER, GPU_PREF) }.into_iter().filter(|(n, _)| n.contains('\\')).map(|(n, d)| {
        let file = n.rsplit('\\').next().unwrap_or(&n).to_string();
        crate::qsx::Item { key: format!("gpu\t{n}"), name: file, sub: gpu_label(&d).into(), ..Default::default() }
    }).collect()
}
/// "Change": the next of the three preferences.
fn gpu_next(id: &str, _: &str) {
    let Some(path) = id.strip_prefix("gpu\t") else { return };
    let now = unsafe { reg_values(HKEY_CURRENT_USER, GPU_PREF) }.into_iter().find(|(n, _)| n == path).map(|(_, d)| d).unwrap_or_default();
    let next = match gpu_label(&now) { "Let Windows decide" => 1, "Power saving" => 2, _ => 0 };
    let v: Vec<u16> = format!("GpuPreference={next};").encode_utf16().chain([0]).collect();
    unsafe { let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(GPU_PREF), &HSTRING::from(path), REG_SZ.0, Some(v.as_ptr() as *const c_void), (v.len() * 2) as u32); }
}

// S8 multiple displays: duplicate / extend / one screen (shown only with two or more)
const TOPOLOGIES: [(&str, u32); 4] = [("Duplicate these displays", 0x2), ("Extend these displays", 0x4), ("Show only on 1", 0x1), ("Show only on 2", 0x8)];
fn several_displays() -> bool { unsafe { GetSystemMetrics(SM_CMONITORS) > 1 } }
fn topology() -> String {
    use windows::Win32::Devices::Display::*;
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_DATABASE_CURRENT, &mut np, &mut nm).is_err() { return String::new(); }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        let mut topo = DISPLAYCONFIG_TOPOLOGY_ID(0);
        if QueryDisplayConfig(QDC_DATABASE_CURRENT, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), Some(&mut topo)).is_err() { return String::new(); }
        TOPOLOGIES.iter().find(|t| t.1 == topo.0 as u32).map(|t| t.0.to_string()).unwrap_or_default()
    }
}
fn topologies() -> Vec<String> { TOPOLOGIES.iter().map(|t| t.0.to_string()).collect() }
fn set_topology(i: usize) {
    use windows::Win32::Devices::Display::*;
    if let Some(t) = TOPOLOGIES.get(i) {
        let flag = match t.1 { 0x2 => SDC_TOPOLOGY_CLONE, 0x4 => SDC_TOPOLOGY_EXTEND, 0x1 => SDC_TOPOLOGY_INTERNAL, _ => SDC_TOPOLOGY_EXTERNAL };
        unsafe { let _ = SetDisplayConfig(None, None, SDC_APPLY | flag); }
    }
}

// S18 Focus: do not disturb with the shell's own switch

fn set_dnd(on: bool) { crate::notify::dnd_set(on) }

// S21 / S25 power plan values of the active scheme (AC: plugged in, DC: on battery)
fn power_value(sub: u128, key: u128, dc: bool) -> Option<u32> {
    use windows::Win32::System::Power::*;
    unsafe {
        let mut scheme = std::ptr::null_mut();
        PowerGetActiveScheme(None, &mut scheme).ok().ok()?;
        let (s, k) = (windows::core::GUID::from_u128(sub), windows::core::GUID::from_u128(key));
        let mut v = 0u32;
        let r = if dc { PowerReadDCValueIndex(None, Some(scheme), Some(&s), Some(&k), &mut v) == 0 } else { PowerReadACValueIndex(None, Some(scheme), Some(&s), Some(&k), &mut v).is_ok() };
        let _ = LocalFree(Some(HLOCAL(scheme as *mut c_void)));
        r.then_some(v)
    }
}
fn set_power_value(sub: u128, key: u128, dc: bool, v: u32) {
    use windows::Win32::System::Power::*;
    unsafe {
        let mut scheme = std::ptr::null_mut();
        if PowerGetActiveScheme(None, &mut scheme).is_err() { return; }
        let (s, k) = (windows::core::GUID::from_u128(sub), windows::core::GUID::from_u128(key));
        if dc { let _ = PowerWriteDCValueIndex(None, &*scheme, Some(&s), Some(&k), v); } else { let _ = PowerWriteACValueIndex(None, &*scheme, Some(&s), Some(&k), v); }
        let _ = PowerSetActiveScheme(None, Some(scheme)); // makes the change take effect now
        let _ = LocalFree(Some(HLOCAL(scheme as *mut c_void)));
    }
}
const IDLE: [(&str, u32); 16] = [("1 minute", 60), ("2 minutes", 120), ("3 minutes", 180), ("5 minutes", 300), ("10 minutes", 600), ("15 minutes", 900), ("20 minutes", 1200), ("25 minutes", 1500), ("30 minutes", 1800), ("45 minutes", 2700), ("1 hour", 3600), ("2 hours", 7200), ("3 hours", 10800), ("4 hours", 14400), ("5 hours", 18000), ("Never", 0)];
/// A timeout in seconds as Settings words it (the nearest listed value).
pub fn idle_label(secs: u32) -> String {
    IDLE.iter().find(|i| i.1 == secs).map(|i| i.0.to_string()).unwrap_or_else(|| if secs == 0 { "Never".into() } else { format!("{} minutes", secs / 60) })
}
const SUB_VIDEO: u128 = 0x7516b95f_f776_4464_8c53_06167f40cc99;
const VIDEOIDLE: u128 = 0x3c0bc021_c8a8_4e07_a973_6b14cbcb2b7e;
const SUB_SLEEP: u128 = 0x238c9fa8_0aad_41ed_83f4_97be242c8f20;
const STANDBYIDLE: u128 = 0x29f6c1db_86da_48c5_9fdb_f2b67b1f44da;
const SUB_BUTTONS: u128 = 0x4f971e89_eebd_4455_a8de_9e59040e7347;
const LIDACTION: u128 = 0x5ca83367_6e45_459f_a27b_476b1d01c936;
const PBUTTONACTION: u128 = 0x7648efa3_dd9c_4e3e_b566_50f929386280;
const SBUTTONACTION: u128 = 0x96996bc0_ad50_47ec_923b_6f41874dd9eb;
macro_rules! idle_choice {
    ($get:ident, $list:ident, $set:ident, $sub:expr, $key:expr, $dc:expr) => {
        fn $get() -> String { power_value($sub, $key, $dc).map(idle_label).unwrap_or_default() }
        fn $list() -> Vec<String> { IDLE.iter().map(|i| i.0.to_string()).collect() }
        fn $set(i: usize) { if let Some(v) = IDLE.get(i) { set_power_value($sub, $key, $dc, v.1) } }
    };
}
idle_choice!(screen_ac, screen_acs, set_screen_ac, SUB_VIDEO, VIDEOIDLE, false);
idle_choice!(screen_dc, screen_dcs, set_screen_dc, SUB_VIDEO, VIDEOIDLE, true);
idle_choice!(sleep_ac, sleep_acs, set_sleep_ac, SUB_SLEEP, STANDBYIDLE, false);
idle_choice!(sleep_dc, sleep_dcs, set_sleep_dc, SUB_SLEEP, STANDBYIDLE, true);
const BUTTON_ACTIONS: [&str; 5] = ["Do nothing", "Sleep", "Hibernate", "Shut down", "Turn off the display"];
macro_rules! button_choice {
    ($get:ident, $list:ident, $set:ident, $key:expr, $n:expr) => {
        fn $get() -> String { power_value(SUB_BUTTONS, $key, false).and_then(|v| BUTTON_ACTIONS.get(v as usize)).unwrap_or(&"").to_string() }
        fn $list() -> Vec<String> { BUTTON_ACTIONS[..$n].iter().map(|s| s.to_string()).collect() }
        fn $set(i: usize) { set_power_value(SUB_BUTTONS, $key, false, i as u32); set_power_value(SUB_BUTTONS, $key, true, i as u32) }
    };
}
button_choice!(lid_action, lid_actions, set_lid_action, LIDACTION, 4);
button_choice!(power_button, power_buttons, set_power_button, PBUTTONACTION, 5);
button_choice!(sleep_button, sleep_buttons, set_sleep_button, SBUTTONACTION, 4);
fn has_battery() -> bool {
    let mut st = windows::Win32::System::Power::SYSTEM_POWER_STATUS::default();
    unsafe { windows::Win32::System::Power::GetSystemPowerStatus(&mut st).is_ok() && st.BatteryFlag != 128 }
}

// S29 temporary files: remove what has not been touched for a week (like Windows' cleanup), after asking
fn temp_item() -> Vec<crate::qsx::Item> {
    vec![crate::qsx::Item { key: "temp".into(), name: "Temporary files".into(), sub: format!("{} - files older than 7 days are removed", temp_size()), ..Default::default() }]
}
fn clean_temp(_: &str, _: &str) {
    unsafe {
        if crate::dialog::ask(None, "Temporary files", "Remove temporary files that have not been used for 7 days?\n\nFiles in use are skipped.", &["Remove", "Cancel"], 1) != Some(0) {
            return;
        }
    }
    let week = std::time::Duration::from_secs(7 * 86400);
    fn walk(dir: &std::path::Path, week: std::time::Duration) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > week);
            if p.is_dir() {
                walk(&p, week);
                if old { let _ = std::fs::remove_dir(&p); } // only once it is empty
            } else if old {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
    walk(&std::env::temp_dir(), week);
}

// S31 nearby sharing (who can share with this PC)
reg_choice!(near_share, near_shares, set_near_share, r"Software\Microsoft\Windows\CurrentVersion\CDP", "NearShareChannelUserAuthzPolicy",
    [("Off", 0), ("My devices only", 1), ("Everyone nearby", 2)], missing = 0);
// S32 desktops in Multitasking
reg_choice!(vd_taskbar, vd_taskbars, set_vd_taskbar, ADV, "VirtualDesktopTaskbarFilter", [("Only on the desktop I'm using", 1), ("On all desktops", 0)], missing = 0);
reg_choice!(vd_alttab, vd_alttabs, set_vd_alttab, ADV, "VirtualDesktopAltTabFilter", [("Only on the desktop I'm using", 1), ("On all desktops", 0)], missing = 0);

// S35 Recovery: advanced startup restarts the PC into the recovery menu, after asking
fn recovery_items() -> Vec<crate::qsx::Item> {
    vec![crate::qsx::Item { key: "advstart".into(), name: "Advanced startup".into(), sub: "Restart from a device or disc, change firmware settings, Windows startup settings".into(), ..Default::default() }]
}
fn advanced_startup(_: &str, _: &str) {
    unsafe {
        if crate::dialog::ask(None, "Advanced startup", "Restart now into the recovery menu?\n\nSave your work first: every app closes.", &["Restart now", "Cancel"], 1) == Some(0) {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("shutdown.exe").args(["/r", "/o", "/t", "0"]).creation_flags(0x0800_0000).spawn();
        }
    }
}
// S37 Remote Desktop (machine-wide: read here)
fn remote_desktop() -> bool { reg_u32(HKEY_LOCAL_MACHINE, w!("SYSTEM\\CurrentControlSet\\Control\\Terminal Server"), w!("fDenyTSConnections")) == Some(0) }

// S38 clipboard: clear the history (pinned items stay, as in Windows)
fn clip_items() -> Vec<crate::qsx::Item> {
    vec![crate::qsx::Item { key: "clip".into(), name: "Clear clipboard data".into(), sub: "Clear everything (except pinned items) on this device".into(), ..Default::default() }]
}
fn clear_clip(_: &str, _: &str) {
    unsafe {
        if crate::dialog::ask(None, "Clipboard", "Clear your clipboard history?\n\nPinned items stay.", &["Clear", "Cancel"], 1) == Some(0) {
            let _ = windows::ApplicationModel::DataTransfer::Clipboard::ClearHistory();
        }
    }
}
// S5 colour profiles installed
fn color_profiles() -> Vec<crate::qsx::Item> {
    let dir = format!(r"{}\System32\spool\drivers\color", std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()));
    let mut v: Vec<crate::qsx::Item> = std::fs::read_dir(&dir).into_iter().flatten().flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("icm") || x.eq_ignore_ascii_case("icc")))
        .map(|e| crate::qsx::Item { key: format!("icc\t{}", e.path().display()), name: e.file_name().to_string_lossy().to_string(), sub: size_label(e.metadata().map_or(0, |m| m.len())), ..Default::default() })
        .collect();
    v.sort_by_key(|i| i.name.to_lowercase());
    v
}
fn open_profiles(_: &str, _: &str) {
    let dir = format!(r"{}\System32\spool\drivers\color", std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()));
    crate::files::request(&dir);
    unsafe {
        if let Ok(h) = FindWindowW(w!("shellbits_apps"), None) {
            let _ = PostMessageW(Some(h), WM_HOTKEY, WPARAM(crate::FILES_ID as usize), LPARAM(0));
        }
    }
}


// ======================= Devices, network, personalization (PLAN-600 S41-S86) =======================

/// Every network adapter: (name, description, kind, up, IPv4, IPv6, DNS, gateway, MAC, speed Mbps).
pub struct Adapter { pub name: String, pub desc: String, pub kind: u32, pub up: bool, pub v4: Vec<String>, pub v6: Vec<String>, pub dns: Vec<String>, pub gw: Vec<String>, pub mac: String, pub mbps: u64 }
fn sockaddr_text(sa: *const windows::Win32::Networking::WinSock::SOCKADDR) -> Option<String> {
    use windows::Win32::Networking::WinSock::*;
    unsafe {
        let a = sa.as_ref()?;
        match a.sa_family {
            AF_INET => {
                let v = &*(sa as *const SOCKADDR_IN);
                let b = v.sin_addr.S_un.S_un_b;
                Some(format!("{}.{}.{}.{}", b.s_b1, b.s_b2, b.s_b3, b.s_b4))
            }
            AF_INET6 => {
                let v = &*(sa as *const SOCKADDR_IN6);
                Some(std::net::Ipv6Addr::from(v.sin6_addr.u.Byte).to_string())
            }
            _ => None,
        }
    }
}
fn adapters() -> Vec<Adapter> {
    use windows::Win32::NetworkManagement::IpHelper::*;
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    let mut out = Vec::new();
    let mut size = 128 * 1024u32;
    let mut buf = vec![0u8; size as usize];
    unsafe {
        if GetAdaptersAddresses(0, GAA_FLAG_INCLUDE_GATEWAYS, None, Some(buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size) != 0 {
            return out;
        }
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while let Some(a) = p.as_ref() {
            if a.IfType != 24 { // not the loopback
                let mut ad = Adapter { name: a.FriendlyName.to_string().unwrap_or_default(), desc: a.Description.to_string().unwrap_or_default(), kind: a.IfType, up: a.OperStatus == IfOperStatusUp, v4: vec![], v6: vec![], dns: vec![], gw: vec![], mac: String::new(), mbps: a.TransmitLinkSpeed / 1_000_000 };
                let mut u = a.FirstUnicastAddress;
                while let Some(x) = u.as_ref() {
                    if let Some(t) = sockaddr_text(x.Address.lpSockaddr) { if t.contains(':') { ad.v6.push(t) } else { ad.v4.push(t) } }
                    u = x.Next;
                }
                let mut d = a.FirstDnsServerAddress;
                while let Some(x) = d.as_ref() {
                    if let Some(t) = sockaddr_text(x.Address.lpSockaddr) { ad.dns.push(t) }
                    d = x.Next;
                }
                let mut g = a.FirstGatewayAddress;
                while let Some(x) = g.as_ref() {
                    if let Some(t) = sockaddr_text(x.Address.lpSockaddr) { ad.gw.push(t) }
                    g = x.Next;
                }
                ad.mac = a.PhysicalAddress[..a.PhysicalAddressLength as usize].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-");
                out.push(ad);
            }
            p = a.Next;
        }
    }
    out
}
fn adapter_facts(a: &Adapter) -> Vec<Line> {
    let line = |k: &str, v: String| (k.to_string(), v, false, String::new());
    let mut v = vec![line("Description", a.desc.clone())];
    if a.mbps > 0 && a.mbps < 1_000_000 { v.push(line("Link speed", format!("{} Mbps", a.mbps))); } // unknown speed reads as all ones
    if !a.v4.is_empty() { v.push(line("IPv4 address", a.v4.join(", "))); }
    if !a.v6.is_empty() { v.push(line("IPv6 address", a.v6.join(", "))); }
    if !a.gw.is_empty() { v.push(line("Default gateway", a.gw.join(", "))); }
    if !a.dns.is_empty() { v.push(line("DNS servers", a.dns.join(", "))); }
    v.push(line("Physical address (MAC)", a.mac.clone()));
    v
}
/// S57 / S68: properties of the connected Wi-Fi adapter.
fn wifi_facts() -> Vec<Line> {
    // the real radio, not Windows' "Wi-Fi Direct Virtual Adapter" entries
    let all: Vec<Adapter> = adapters().into_iter().filter(|a| a.kind == 71 && !a.desc.contains("Virtual")).collect();
    all.iter().find(|a| a.up).or(all.first()).map(adapter_facts).unwrap_or_else(|| vec![("Wi-Fi".into(), "No Wi-Fi adapter".into(), false, String::new())])
}
/// S60: properties of the wired adapter.
fn ethernet_facts() -> Vec<Line> {
    let all = adapters();
    all.iter().find(|a| a.kind == 6 && a.up).or(all.iter().find(|a| a.kind == 6)).map(adapter_facts).unwrap_or_else(|| vec![("Ethernet".into(), "No wired adapter".into(), false, String::new())])
}
/// S66: every adapter with its state.
fn adapter_list() -> Vec<crate::qsx::Item> {
    adapters().into_iter().map(|a| crate::qsx::Item { key: format!("nic\t{}", a.name), name: a.name, sub: format!("{}  |  {}", if a.up { "Connected" } else { "Not connected" }, a.desc), ..Default::default() }).collect()
}
fn open_adapters(_: &str, _: &str) {
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("control.exe"), w!("ncpa.cpl"), PCWSTR::null(), SW_SHOWNORMAL); }
}

/// S56 Wi-Fi profiles Windows has saved ("netsh wlan show profiles", names only).
pub fn wlan_profiles(netsh: &str) -> Vec<String> {
    netsh.lines().filter_map(|l| l.split_once(':').filter(|(k, _)| k.contains("Profile")).map(|(_, v)| v.trim().to_string())).filter(|v| !v.is_empty()).collect()
}
fn known_networks() -> Vec<crate::qsx::Item> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("netsh").args(["wlan", "show", "profiles"]).creation_flags(0x0800_0000).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    wlan_profiles(&out).into_iter().map(|n| crate::qsx::Item { key: format!("wlan\t{n}"), name: n, sub: "Saved network".into(), ..Default::default() }).collect()
}
fn forget_network(_: &str, name: &str) {
    unsafe {
        if crate::dialog::ask(None, "Wi-Fi", &format!("Forget \"{name}\"?\n\nYou will need its password to connect again."), &["Forget", "Cancel"], 1) != Some(0) {
            return;
        }
    }
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("netsh").args(["wlan", "delete", "profile", &format!("name={name}")]).creation_flags(0x0800_0000).status();
}

/// S61 VPN connections from the phone book (rasphone.pbk); Connect opens Windows' own dialer for that entry.
pub fn pbk_entries(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| l.trim().strip_prefix('[').and_then(|x| x.strip_suffix(']')).map(String::from)).collect()
}
/// Each phonebook entry with its Type= (1 phone, 2 VPN, 3 direct, 4/5 broadband; 0 when not written).
pub fn pbk_typed(text: &str) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for l in text.lines().map(str::trim) {
        if let Some(n) = l.strip_prefix('[').and_then(|x| x.strip_suffix(']')) {
            out.push((n.to_string(), 0));
        } else if let (Some(t), Some(last)) = (l.strip_prefix("Type="), out.last_mut()) {
            last.1 = t.trim().parse().unwrap_or(0);
        }
    }
    out
}
fn pbk_items(want: &[u32], sub: &str) -> Vec<crate::qsx::Item> {
    let pbk = format!(r"{}\Microsoft\Network\Connections\Pbk\rasphone.pbk", std::env::var("APPDATA").unwrap_or_default());
    pbk_typed(&std::fs::read_to_string(pbk).unwrap_or_default()).into_iter().filter(|e| want.contains(&e.1))
        .map(|(n, _)| crate::qsx::Item { key: format!("vpn\t{n}"), name: n, sub: sub.into(), ..Default::default() }).collect()
}
fn vpn_list() -> Vec<crate::qsx::Item> {
    pbk_items(&[2, 0], "VPN")
}
fn dialup_list() -> Vec<crate::qsx::Item> {
    pbk_items(&[1, 3, 4, 5], "Dial-up / broadband")
}

// S84 device usage: what you told Windows you use the PC for (it tailors tips and ads to it)
const INTENT: &str = r"Software\Microsoft\Windows\CurrentVersion\CloudExperienceHost\Intent";
fn intent_get(k: &str) -> bool { hkcu_u32(&format!(r"{INTENT}\{k}"), "Intent") == Some(1) }
fn intent_set(k: &str, on: bool) { set_hkcu_u32(&format!(r"{INTENT}\{k}"), "Intent", on as u32) }
fn use_gaming() -> bool { intent_get("gaming") }
fn set_use_gaming(on: bool) { intent_set("gaming", on) }
fn use_family() -> bool { intent_get("family") }
fn set_use_family(on: bool) { intent_set("family", on) }
fn use_creative() -> bool { intent_get("creative") }
fn set_use_creative(on: bool) { intent_set("creative", on) }
fn use_school() -> bool { intent_get("schoolwork") }
fn set_use_school(on: bool) { intent_set("schoolwork", on) }
fn use_fun() -> bool { intent_get("entertainment") }
fn set_use_fun(on: bool) { intent_set("entertainment", on) }
fn use_business() -> bool { intent_get("business") }
fn set_use_business(on: bool) { intent_set("business", on) }
fn vpn_connect(_: &str, name: &str) {
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("rasphone.exe"), &HSTRING::from(format!("-d \"{name}\"")), PCWSTR::null(), SW_SHOWNORMAL); }
}

// S64 proxy (this user's Internet settings)
const INET: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
reg_switch!(proxy_on, set_proxy_on, INET, "ProxyEnable", on = 1, off = 0, missing = false);
fn proxy_server() -> String {
    let v = reg_str(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings"), w!("ProxyServer"));
    if v.is_empty() { "None".into() } else { v }
}
fn proxy_script() -> String {
    let v = reg_str(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings"), w!("AutoConfigURL"));
    if v.is_empty() { "Off".into() } else { v }
}

// S53 AutoPlay, S54 USB
reg_switch!(autoplay, set_autoplay, r"Software\Microsoft\Windows\CurrentVersion\Explorer\AutoplayHandlers", "DisableAutoplay", on = 0, off = 1, missing = true);
reg_switch!(usb_notify, set_usb_notify, r"Software\Microsoft\Shell\USB", "NotifyOnUsbErrors", on = 1, off = 0, missing = true);

// S49 cameras: video capture devices present now (device tree, camera and image classes)
fn cameras() -> Vec<crate::qsx::Item> {
    use std::os::windows::process::CommandExt;
    // ponytail: PnP listing through PowerShell's Get-PnpDevice; the setup API version can replace it if this is slow
    let out = std::process::Command::new("powershell.exe").args(["-NoProfile", "-Command", "Get-PnpDevice -PresentOnly -Class Camera,Image -ErrorAction SilentlyContinue | ForEach-Object { $_.FriendlyName + '|' + $_.Status }"]).creation_flags(0x0800_0000).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    out.lines().filter_map(|l| l.split_once('|')).map(|(n, st)| crate::qsx::Item { key: format!("cam\t{n}"), name: n.trim().into(), sub: if st.trim() == "OK" { "Enabled".into() } else { st.trim().into() }, ..Default::default() }).collect()
}
fn camera_settings(_: &str, _: &str) {
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("ms-settings:camera"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL); }
}

// S71 / S72 background: browse for a picture, or a solid colour
fn bg_items() -> Vec<crate::qsx::Item> {
    vec![
        crate::qsx::Item { key: "bg\tphoto".into(), name: "Choose a photo".into(), sub: "Any picture file on this PC".into(), ..Default::default() },
        crate::qsx::Item { key: "bg\tcolor".into(), name: "Solid color".into(), sub: "One colour, no picture".into(), ..Default::default() },
    ]
}
fn bg_pick(id: &str, _: &str) {
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    unsafe {
        if id.ends_with("photo") {
            let Ok(d) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_INPROC_SERVER) else { return };
            let spec = [windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC { pszName: w!("Pictures"), pszSpec: w!("*.jpg;*.jpeg;*.png;*.bmp;*.gif;*.webp;*.jfif") }];
            let _ = d.SetFileTypes(&spec);
            if d.Show(None).is_err() { return; }
            let Ok(p) = d.GetResult().and_then(|i| i.GetDisplayName(SIGDN_FILESYSPATH)) else { return };
            let path = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const c_void));
            let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
            let _ = SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(w.as_ptr() as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
        } else if let Some(c) = pick_color() {
            // no picture, the desktop colour instead (what Settings' Solid color does)
            let empty = [0u16];
            let _ = SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(empty.as_ptr() as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
            let (el, col) = ([COLOR_BACKGROUND.0 as i32], [COLORREF(c)]);
            let _ = SetSysColors(1, el.as_ptr(), col.as_ptr());
            let rgb: Vec<u16> = format!("{} {} {}", c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF).encode_utf16().chain([0]).collect();
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, w!("Control Panel\\Colors"), w!("Background"), REG_SZ.0, Some(rgb.as_ptr() as *const c_void), (rgb.len() * 2) as u32);
        }
    }
}
/// Windows' colour dialog; the colour as COLORREF (0x00BBGGRR).
fn pick_color() -> Option<u32> {
    use windows::Win32::UI::Controls::Dialogs::*;
    let mut custom = [COLORREF(0x00FFFFFF); 16];
    let mut cc = CHOOSECOLORW { lStructSize: size_of::<CHOOSECOLORW>() as u32, lpCustColors: custom.as_mut_ptr(), Flags: CC_FULLOPEN | CC_RGBINIT, ..Default::default() };
    unsafe { ChooseColorW(&mut cc).as_bool().then_some(cc.rgbResult.0) }
}

// S74 / S75 accent colour
fn accent_items() -> Vec<crate::qsx::Item> {
    vec![crate::qsx::Item { key: "accent".into(), name: "Custom accent color".into(), sub: "Pick any colour".into(), ..Default::default() }]
}
fn accent_pick(_: &str, _: &str) {
    let Some(c) = pick_color() else { return };
    // DWM keeps it as 0xAABBGGRR; Explorer's own copy is AccentColorMenu. Automatic accent goes off.
    let abgr = 0xFF00_0000 | c;
    set_hkcu_u32(r"Software\Microsoft\Windows\DWM", "AccentColor", abgr);
    set_hkcu_u32(r"Software\Microsoft\Windows\DWM", "ColorizationColor", 0xC400_0000 | ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF));
    set_hkcu_u32(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "AccentColorMenu", abgr);
    unsafe {
        let wide: Vec<u16> = "ImmersiveColorSet".encode_utf16().chain([0]).collect();
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(wide.as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None);
    }
}
reg_switch!(accent_on_start, set_accent_on_start, r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "ColorPrevalence", on = 1, off = 0, missing = false);
reg_switch!(accent_on_titles, set_accent_on_titles, r"Software\Microsoft\Windows\DWM", "ColorPrevalence", on = 1, off = 0, missing = false);

// S80 which tray icons show on the taskbar (Windows' own per-app switch)
fn tray_icons() -> Vec<crate::qsx::Item> {
    let mut out = Vec::new();
    unsafe {
        let mut root = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, w!("Control Panel\\NotifyIconSettings"), Some(0), KEY_READ, &mut root).is_err() {
            return out;
        }
        let mut i = 0;
        loop {
            let mut name = [0u16; 64];
            let mut n = name.len() as u32;
            if windows::Win32::System::Registry::RegEnumKeyExW(root, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None).is_err() { break; }
            i += 1;
            let id = String::from_utf16_lossy(&name[..n as usize]);
            let sub = HSTRING::from(id.as_str());
            let mut path = [0u16; 520];
            let mut sz = (path.len() * 2) as u32;
            if RegGetValueW(root, &sub, w!("ExecutablePath"), RRF_RT_REG_SZ, None, Some(path.as_mut_ptr() as *mut c_void), Some(&mut sz)).is_err() { continue; }
            let exe = String::from_utf16_lossy(&path[..(sz as usize / 2).saturating_sub(1)]);
            let file = exe.rsplit('\\').next().unwrap_or(&exe).to_string();
            let mut v = 0u32;
            let mut vs = 4u32;
            let on = RegGetValueW(root, &sub, w!("IsPromoted"), RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut c_void), Some(&mut vs)).is_ok() && v == 1;
            if !out.iter().any(|x: &crate::qsx::Item| x.name.eq_ignore_ascii_case(&file)) {
                out.push(crate::qsx::Item { key: format!("tray\t{id}"), name: file, sub: if on { "Shown on the taskbar".into() } else { "In the ^ overflow".into() }, on, ..Default::default() });
            }
        }
        let _ = RegCloseKey(root);
    }
    out.sort_by_key(|x| x.name.to_lowercase());
    out
}
fn set_tray_icon(id: &str, on: bool) {
    let Some(key) = id.strip_prefix("tray\t") else { return };
    let v = on as u32;
    unsafe { let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(format!("Control Panel\\NotifyIconSettings\\{key}")), w!("IsPromoted"), REG_DWORD.0, Some(&v as *const _ as *const c_void), 4); }
}

// S82 fonts installed for everyone and for this user
fn font_list() -> Vec<crate::qsx::Item> {
    let mut v: Vec<crate::qsx::Item> = Vec::new();
    for (root, key) in [(HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Fonts"), (HKEY_CURRENT_USER, r"Software\Microsoft\Windows NT\CurrentVersion\Fonts")] {
        for (name, file) in unsafe { reg_values(root, key) } {
            let full = if file.contains('\\') { file.clone() } else { format!(r"{}\Fonts\{file}", std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())) };
            let shown = name.trim_end_matches(" (TrueType)").trim_end_matches(" (OpenType)").to_string();
            v.push(crate::qsx::Item { key: format!("font\t{full}"), name: shown, sub: file.rsplit('\\').next().unwrap_or(&file).to_string(), ..Default::default() });
        }
    }
    v.sort_by_key(|i| i.name.to_lowercase());
    v
}
fn open_font(id: &str, _: &str) {
    if let Some(path) = id.strip_prefix("font\t") {
        unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL); }
    }
}


// ======================= Privacy & security pages (PLAN-600 S141-S175) =======================

/// Sub-keys of a registry key.
unsafe fn reg_subkeys(root: HKEY, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut k = HKEY::default();
    if RegOpenKeyExW(root, &HSTRING::from(key), Some(0), KEY_READ, &mut k).is_err() {
        return out;
    }
    let mut i = 0;
    loop {
        let mut name = [0u16; 512];
        let mut n = name.len() as u32;
        if windows::Win32::System::Registry::RegEnumKeyExW(k, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None).is_err() {
            break;
        }
        out.push(String::from_utf16_lossy(&name[..n as usize]));
        i += 1;
    }
    let _ = RegCloseKey(k);
    out
}

/// A consent store entry's name as a person reads it: "Microsoft.WindowsCamera_8wekyb3d8bbwe" -> "WindowsCamera",
/// "C:#Program Files#App#app.exe" -> "app.exe".
pub fn consent_name(sub: &str) -> String {
    if sub.contains('#') {
        return sub.rsplit('#').next().unwrap_or(sub).to_string();
    }
    let base = sub.split('_').next().unwrap_or(sub);
    base.rsplit('.').next().unwrap_or(base).to_string()
}

/// The apps that asked for a capability: Store apps, then desktop apps, each with its Allow / Deny.
fn consent_apps(cap: &str) -> Vec<crate::qsx::Item> {
    let mut v = Vec::new();
    unsafe {
        for (prefix, sub) in [("", format!("{CONSENT}{cap}")), ("NonPackaged\\", format!("{CONSENT}{cap}\\NonPackaged"))] {
            for k in reg_subkeys(HKEY_CURRENT_USER, &sub) {
                if k == "NonPackaged" { continue; }
                let on = reg_str(HKEY_CURRENT_USER, PCWSTR(HSTRING::from(format!("{sub}\\{k}")).as_ptr()), w!("Value")) != "Deny";
                v.push(crate::qsx::Item { key: format!("consent\t{cap}\t{prefix}{k}"), name: consent_name(&k), sub: if prefix.is_empty() { "App".into() } else { "Desktop app".into() }, on, ..Default::default() });
            }
        }
    }
    v.sort_by_key(|i| i.name.to_lowercase());
    v
}
fn set_consent_app(id: &str, on: bool) {
    let mut parts = id.splitn(3, '\t');
    let (_, Some(cap), Some(sub)) = (parts.next(), parts.next(), parts.next()) else { return };
    let v: Vec<u16> = (if on { "Allow" } else { "Deny" }).encode_utf16().chain([0]).collect();
    unsafe { let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(format!("{CONSENT}{cap}\\{sub}")), w!("Value"), REG_SZ.0, Some(v.as_ptr() as *const c_void), (v.len() * 2) as u32); }
}
macro_rules! consent_page {
    ($get:ident, $set:ident, $apps:ident, $cap:expr) => {
        fn $get() -> bool { consent($cap) }
        fn $set(on: bool) { set_consent($cap, on) }
        fn $apps() -> Vec<crate::qsx::Item> { consent_apps($cap) }
    };
}
consent_page!(cam_ok2, set_cam_ok2, cam_apps, "webcam");
consent_page!(mic_ok2, set_mic_ok2, mic_apps, "microphone");
consent_page!(loc_ok2, set_loc_ok2, loc_apps, "location");
consent_page!(c_notif, set_c_notif, apps_notif, "userNotificationListener");
consent_page!(c_account, set_c_account, apps_account, "userAccountInformation");
consent_page!(c_contacts, set_c_contacts, apps_contacts, "contacts");
consent_page!(c_calendar, set_c_calendar, apps_calendar, "appointments");
consent_page!(c_calls, set_c_calls, apps_calls, "phoneCall");
consent_page!(c_history, set_c_history, apps_history, "phoneCallHistory");
consent_page!(c_email, set_c_email, apps_email, "email");
consent_page!(c_tasks, set_c_tasks, apps_tasks, "userDataTasks");
consent_page!(c_messaging, set_c_messaging, apps_messaging, "chat");
consent_page!(c_radios, set_c_radios, apps_radios, "radios");
consent_page!(c_otherdev, set_c_otherdev, apps_otherdev, "bluetoothSync");
consent_page!(c_diag, set_c_diag, apps_diag, "appDiagnostics");
consent_page!(c_docs, set_c_docs, apps_docs, "documentsLibrary");
consent_page!(c_downloads, set_c_downloads, apps_downloads, "downloadsFolder");
consent_page!(c_music, set_c_music, apps_music, "musicLibrary");
consent_page!(c_pics, set_c_pics, apps_pics, "picturesLibrary");
consent_page!(c_videos, set_c_videos, apps_videos, "videosLibrary");
consent_page!(c_fs, set_c_fs, apps_fs, "broadFileSystemAccess");
consent_page!(c_borders, set_c_borders, apps_borders, "graphicsCaptureWithoutBorder");
consent_page!(c_shots, set_c_shots, apps_shots, "graphicsCaptureProgrammatic");
consent_page!(c_genai, set_c_genai, apps_genai, "systemAIModels");
consent_page!(c_presence, set_c_presence, apps_presence, "humanPresence");

// S145 inking & typing, S146 diagnostics, S148 search permissions
reg_switch!(ink_personal, set_ink_personal, r"Software\Microsoft\InputPersonalization", "RestrictImplicitTextCollection", on = 0, off = 1, missing = true);
reg_switch!(tailored, set_tailored, r"Software\Microsoft\Windows\CurrentVersion\Privacy", "TailoredExperiencesWithDiagnosticDataEnabled", on = 1, off = 0, missing = false);
reg_choice!(feedback_freq, feedback_freqs, set_feedback_freq, r"Software\Microsoft\Siuf\Rules", "NumberOfSIUFInPeriod", [("Automatically (recommended)", 99), ("Never", 0), ("Always", 1)], missing = 0);
reg_choice!(safe_search, safe_searches, set_safe_search, r"Software\Microsoft\Windows\CurrentVersion\SearchSettings", "SafeSearchMode", [("Strict", 2), ("Moderate", 1), ("Off", 0)], missing = 1);
reg_switch!(msa_cloud, set_msa_cloud, r"Software\Microsoft\Windows\CurrentVersion\SearchSettings", "IsMSACloudSearchEnabled", on = 1, off = 0, missing = true);
reg_switch!(aad_cloud, set_aad_cloud, r"Software\Microsoft\Windows\CurrentVersion\SearchSettings", "IsAADCloudSearchEnabled", on = 1, off = 0, missing = true);
reg_switch!(search_history, set_search_history, r"Software\Microsoft\Windows\CurrentVersion\SearchSettings", "IsDeviceSearchHistoryEnabled", on = 1, off = 0, missing = true);
fn diag_level() -> String {
    match reg_u32(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\DataCollection"), w!("AllowTelemetry")) {
        Some(0) | Some(1) => "Required diagnostic data".into(),
        Some(_) => "Optional diagnostic data".into(),
        None => "Set by Windows".into(),
    }
}
// S144 for developers (machine-wide parts read only)
fn dev_mode() -> bool { reg_u32(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AppModelUnlock"), w!("AllowDevelopmentWithoutDevLicense")) == Some(1) }
reg_switch!(show_ext, set_show_ext, ADV, "HideFileExt", on = 0, off = 1, missing = false);
reg_switch!(show_hidden, set_show_hidden, ADV, "Hidden", on = 1, off = 2, missing = false);
reg_switch!(show_full_path, set_show_full_path, r"Software\Microsoft\Windows\CurrentVersion\Explorer\CabinetState", "FullPath", on = 1, off = 0, missing = false);
// S141 Windows Security: each area's health from the Security Center
fn security_facts() -> Vec<Line> {
    #[link(name = "wscapi", kind = "raw-dylib")]
    unsafe extern "system" { fn WscGetSecurityProviderHealth(providers: u32, health: *mut i32) -> i32; }
    let area = |name: &str, bit: u32| {
        let mut h = 0i32;
        let ok = unsafe { WscGetSecurityProviderHealth(bit, &mut h) } == 0;
        (name.to_string(), if !ok { "Unknown".into() } else { match h { 0 => "No actions needed".into(), 1 => "Not monitored".into(), 2 => "Needs attention".into(), _ => "At risk".into() } }, false, String::new())
    };
    vec![area("Virus & threat protection", 4), area("Firewall & network protection", 1), area("Automatic updates", 2), area("Internet settings", 8), area("User Account Control", 16), area("Security service", 32)]
}
// S105 sign-in options: restart apps
reg_switch!(restart_apps, set_restart_apps, r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon", "RestartApps", on = 1, off = 0, missing = false);
// S102 email & accounts: Microsoft accounts known to this user
fn ms_accounts() -> Vec<crate::qsx::Item> {
    unsafe { reg_subkeys(HKEY_CURRENT_USER, r"Software\Microsoft\IdentityCRL\UserExtendedProperties") }.into_iter()
        .map(|e| crate::qsx::Item { key: format!("msa\t{e}"), name: e, sub: "Microsoft account".into(), ..Default::default() }).collect()
}
fn manage_account(_: &str, _: &str) {
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("https://account.microsoft.com"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL); }
}
// S107 other users on this PC
fn local_users() -> Vec<crate::qsx::Item> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell.exe").args(["-NoProfile", "-Command", "Get-LocalUser | ForEach-Object { $_.Name + '|' + $_.Enabled + '|' + $_.Description }"]).creation_flags(0x0800_0000).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    out.lines().filter_map(|l| { let p: Vec<&str> = l.splitn(3, '|').collect(); (p.len() == 3).then(|| crate::qsx::Item { key: format!("user\t{}", p[0]), name: p[0].trim().into(), sub: format!("{}{}", if p[1].trim() == "True" { "Enabled" } else { "Disabled" }, if p[2].trim().is_empty() { String::new() } else { format!("  |  {}", p[2].trim()) }), ..Default::default() }) }).collect()
}
fn manage_users(_: &str, _: &str) {
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("ms-settings:otherusers"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL); }
}
// S109 work or school accounts joined to this PC
fn work_accounts() -> Vec<crate::qsx::Item> {
    let base = r"SYSTEM\CurrentControlSet\Control\CloudDomainJoin\JoinInfo";
    unsafe { reg_subkeys(HKEY_LOCAL_MACHINE, base) }.into_iter().map(|k| {
        let mail = reg_str(HKEY_LOCAL_MACHINE, PCWSTR(HSTRING::from(format!("{base}\\{k}")).as_ptr()), w!("UserEmail"));
        crate::qsx::Item { key: format!("work\t{k}"), name: if mail.is_empty() { "Work or school account".into() } else { mail }, sub: "Connected".into(), ..Default::default() }
    }).collect()
}
// S112 time zone: Windows' list, changed with tzutil (allowed for standard users)
fn zone_list() -> Vec<(String, String)> {
    let base = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Time Zones";
    let mut v: Vec<(String, String)> = unsafe { reg_subkeys(HKEY_LOCAL_MACHINE, base) }.into_iter()
        .map(|k| { let d = reg_str(HKEY_LOCAL_MACHINE, PCWSTR(HSTRING::from(format!("{base}\\{k}")).as_ptr()), w!("Display")); (k, d) }).filter(|z| !z.1.is_empty()).collect();
    // Windows sorts by UTC offset: "(UTC-08:00)" before "(UTC+05:30)"
    v.sort_by_key(|z| (zone_offset(&z.1), z.1.clone()));
    v
}
/// Minutes east of UTC in a display name like "(UTC+05:30) Chennai, ...".
pub fn zone_offset(display: &str) -> i32 {
    let Some(t) = display.strip_prefix("(UTC").and_then(|t| t.split(')').next()) else { return 0 };
    if t.is_empty() { return 0; }
    let sign = if t.starts_with('-') { -1 } else { 1 };
    let (h, m) = t[1..].split_once(':').unwrap_or((&t[1..], "0"));
    sign * (h.parse::<i32>().unwrap_or(0) * 60 + m.parse::<i32>().unwrap_or(0))
}
fn zone_now() -> String {
    let mut tz = windows::Win32::System::Time::DYNAMIC_TIME_ZONE_INFORMATION::default();
    unsafe { windows::Win32::System::Time::GetDynamicTimeZoneInformation(&mut tz); }
    let key = String::from_utf16_lossy(&tz.TimeZoneKeyName[..tz.TimeZoneKeyName.iter().position(|&c| c == 0).unwrap_or(128)]);
    zone_list().into_iter().find(|z| z.0 == key).map(|z| z.1).unwrap_or(key)
}
fn zones() -> Vec<String> { zone_list().into_iter().map(|z| z.1).collect() }
fn set_zone(i: usize) {
    if let Some((key, _)) = zone_list().get(i) {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("tzutil.exe").args(["/s", key]).creation_flags(0x0800_0000).status();
    }
}
// S114 additional clocks
fn extra_clocks() -> Vec<Line> {
    (1..=2).filter_map(|n| {
        let key = HSTRING::from(format!(r"Control Panel\TimeDate\AdditionalClocks\{n}"));
        (reg_u32(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), w!("Enable")) == Some(1)).then(|| (reg_str(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), w!("DisplayName")), reg_str(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()), w!("TzRegKeyName")), false, String::new()))
    }).collect()
}
// S120 speech voices installed
fn voices() -> Vec<Line> {
    let mut v = Vec::new();
    for base in [r"SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens", r"SOFTWARE\Microsoft\Speech\Voices\Tokens"] {
        for k in unsafe { reg_subkeys(HKEY_LOCAL_MACHINE, base) } {
            let name = reg_str(HKEY_LOCAL_MACHINE, PCWSTR(HSTRING::from(format!("{base}\\{k}")).as_ptr()), PCWSTR::null());
            if !name.is_empty() && !v.iter().any(|x: &Line| x.0 == name) {
                v.push((name, "Installed".into(), false, String::new()));
            }
        }
    }
    v
}
// S121 captures
reg_switch!(cap_background, set_cap_background, r"Software\Microsoft\Windows\CurrentVersion\GameDVR", "HistoricalCaptureEnabled", on = 1, off = 0, missing = false);
reg_switch!(cap_audio, set_cap_audio, r"Software\Microsoft\Windows\CurrentVersion\GameDVR", "AudioCaptureEnabled", on = 1, off = 0, missing = true);
reg_switch!(cap_cursor, set_cap_cursor, r"Software\Microsoft\Windows\CurrentVersion\GameDVR", "CursorCaptureEnabled", on = 1, off = 0, missing = false);
fn capture_folder() -> String {
    format!(r"{}\Videos\Captures", std::env::var("USERPROFILE").unwrap_or_default())
}
// S122 Game Bar shortcuts (Windows' defaults)
fn gamebar_keys() -> Vec<Line> {
    [("Open Game Bar", "Windows logo key + G"), ("Record that", "Windows logo key + Alt + G"), ("Start/stop recording", "Windows logo key + Alt + R"), ("Take screenshot", "Windows logo key + Alt + Print Screen"), ("Microphone on/off", "Windows logo key + Alt + M")]
        .iter().map(|(a, b)| (a.to_string(), b.to_string(), false, String::new())).collect()
}
// S128 text cursor thickness
fn caret_width() -> String { format!("{}", spi_u32(0x2006).max(1)) } // SPI_GETCARETWIDTH
fn caret_widths() -> Vec<String> { (1..=20).map(|n| n.to_string()).collect() }
fn set_caret_width(i: usize) {
    unsafe { let _ = SystemParametersInfoW(SYSTEM_PARAMETERS_INFO_ACTION(0x2007), 0, Some((i + 1) as usize as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE); }
}
// S129 Magnifier options
reg_choice!(mag_level, mag_levels, set_mag_level, r"Software\Microsoft\ScreenMagnifier", "Magnification", [("100%", 100), ("200%", 200), ("300%", 300), ("400%", 400), ("800%", 800), ("1600%", 1600)], missing = 1);
reg_choice!(mag_step, mag_steps, set_mag_step, r"Software\Microsoft\ScreenMagnifier", "ZoomIncrement", [("25%", 25), ("50%", 50), ("100%", 100), ("150%", 150), ("200%", 200), ("400%", 400)], missing = 2);
reg_choice!(mag_view, mag_views, set_mag_view, r"Software\Microsoft\ScreenMagnifier", "MagnificationMode", [("Full screen", 2), ("Docked", 1), ("Lens", 3)], missing = 0);
// S130 colour filter type
reg_choice!(filter_type, filter_types, set_filter_type, r"Software\Microsoft\ColorFiltering", "FilterType",
    [("Grayscale", 0), ("Inverted", 1), ("Grayscale inverted", 2), ("Red-green (green weak, deuteranopia)", 3), ("Red-green (red weak, protanopia)", 4), ("Blue-yellow (tritanopia)", 5)], missing = 0);
// S133 Narrator speed
reg_choice!(narrator_speed, narrator_speeds, set_narrator_speed, r"Software\Microsoft\Narrator", "SpeechSpeed", [("Slow", 5), ("Normal", 10), ("Fast", 15), ("Fastest", 20)], missing = 1);
// S137 on-screen keyboard
fn osk_items() -> Vec<crate::qsx::Item> {
    vec![crate::qsx::Item { key: "osk".into(), name: "On-screen keyboard".into(), sub: "Windows logo key + Ctrl + O".into(), ..Default::default() }]
}
fn open_osk(_: &str, _: &str) {
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("osk.exe"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL); }
}


// ======================= Apps and Windows Update (PLAN-600 S87-S100, S176-S185) =======================

/// S92 / S93 the app each common file and link type opens with (changing it stays in Windows' page:
/// Windows protects these choices with a hash only it can write).
fn default_apps() -> Vec<Line> {
    use windows::Win32::UI::Shell::*;
    let kinds = [".txt", ".pdf", ".jpg", ".png", ".gif", ".mp4", ".mkv", ".mp3", ".zip", ".html", "http", "https", "mailto"];
    kinds.iter().map(|k| {
        let mut buf = [0u16; 260];
        let mut n = buf.len() as u32;
        let ok = unsafe { AssocQueryStringW(ASSOCF_NONE, ASSOCSTR_FRIENDLYAPPNAME, &HSTRING::from(*k), PCWSTR::null(), Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut n) }.is_ok();
        (k.to_string(), if ok { String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)]) } else { "Not set".into() }, false, String::new())
    }).collect()
}

/// S98 app execution aliases: the small .exe stubs Store apps put in WindowsApps.
fn app_aliases() -> Vec<Line> {
    let dir = format!(r"{}\Microsoft\WindowsApps", std::env::var("LOCALAPPDATA").unwrap_or_default());
    let mut v: Vec<Line> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))
        .map(|e| (e.file_name().to_string_lossy().to_string(), "On".into(), false, String::new())).collect();
    v.sort_by_key(|l| l.0.to_lowercase());
    v
}

/// S176-S180 Windows Update: updates waiting, searched in the background (a search takes seconds).
static WU_FOUND: std::sync::Mutex<Option<Vec<crate::qsx::Item>>> = std::sync::Mutex::new(None);
static WU_SEARCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn wu_search() {
    if WU_SEARCHING.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    std::thread::spawn(|| unsafe {
        use windows::Win32::System::UpdateAgent::*;
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let mut out = Vec::new();
        if let Ok(session) = windows::Win32::System::Com::CoCreateInstance::<_, IUpdateSession>(&UpdateSession, None, windows::Win32::System::Com::CLSCTX_INPROC_SERVER) {
            if let Ok(searcher) = session.CreateUpdateSearcher() {
                if let Ok(res) = searcher.Search(&windows::core::BSTR::from("IsInstalled=0 and IsHidden=0")) {
                    if let Ok(list) = res.Updates() {
                        for i in 0..list.Count().unwrap_or(0) {
                            let Ok(u) = list.get_Item(i) else { continue };
                            let title = u.Title().map(|t| t.to_string()).unwrap_or_default();
                            let optional = windows::core::Interface::cast::<IUpdate3>(&u).and_then(|u3| u3.BrowseOnly()).map(|b| b.as_bool()).unwrap_or(false);
                            let downloaded = u.IsDownloaded().map(|b| b.as_bool()).unwrap_or(false);
                            out.push(crate::qsx::Item { key: format!("wu\t{i}"), name: title, sub: format!("{}{}", if optional { "Optional" } else { "Ready to install" }, if downloaded { ", downloaded" } else { "" }), ..Default::default() });
                        }
                    }
                }
            }
        }
        *WU_FOUND.lock().unwrap() = Some(out);
        WU_SEARCHING.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}
fn wu_available() -> Vec<crate::qsx::Item> {
    match WU_FOUND.lock().unwrap().clone() {
        Some(v) if v.is_empty() => vec![crate::qsx::Item { key: "wu\tnone".into(), name: "You're up to date".into(), sub: "Checked just now".into(), ..Default::default() }],
        Some(v) => v,
        None => {
            wu_search();
            vec![crate::qsx::Item { key: "wu\twait".into(), name: "Checking for updates...".into(), sub: "Open this page again in a moment for the result".into(), ..Default::default() }]
        }
    }
}
fn wu_check(_: &str, _: &str) {
    *WU_FOUND.lock().unwrap() = None;
    wu_search();
}
/// S184 Insider channel, if this PC is in one.
fn insider() -> String {
    let b = reg_str(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\WindowsSelfHost\\Applicability"), w!("BranchName"));
    if b.is_empty() { "Not joined".into() } else { b }
}


// ======================= more pages (PLAN-600: S27, S33, S50-S51, S85, S115, S118, S172) =======================

// S27 Storage Sense schedule and clean-up ages (its own policy values)
const SENSE: &str = r"Software\Microsoft\Windows\CurrentVersion\StorageSense\Parameters\StoragePolicy";
reg_choice!(sense_run, sense_runs, set_sense_run, SENSE, "2048", [("During low free disk space (default)", 0), ("Every day", 1), ("Every week", 7), ("Every month", 30)], missing = 0);
reg_choice!(sense_bin, sense_bins, set_sense_bin, SENSE, "256", [("Never", 0), ("1 day", 1), ("14 days", 14), ("30 days (default)", 30), ("60 days", 60)], missing = 3);
reg_choice!(sense_downloads, sense_downloadss, set_sense_downloads, SENSE, "512", [("Never (default)", 0), ("1 day", 1), ("14 days", 14), ("30 days", 30), ("60 days", 60)], missing = 0);
reg_switch!(sense_temp, set_sense_temp, SENSE, "04", on = 1, off = 0, missing = true);

// S33 edition and product id (the activation state itself stays in Windows' page)
fn activation_facts() -> Vec<Line> {
    let nt = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    let line = |k: &str, v: String| (k.to_string(), v, false, String::new());
    vec![
        line("Edition", reg_str(HKEY_LOCAL_MACHINE, nt, w!("ProductName")).replace("Windows 10", "Windows 11")),
        line("Edition ID", reg_str(HKEY_LOCAL_MACHINE, nt, w!("EditionID"))),
        line("Product ID", reg_str(HKEY_LOCAL_MACHINE, nt, w!("ProductId"))),
    ]
}

// S50 / S51 touchpad scrolling direction and gestures (the values Windows' page writes)
reg_choice!(tp_scroll_dir, tp_scroll_dirs, set_tp_scroll_dir, PTP, "ScrollDirection", [("Down motion scrolls up", 0), ("Down motion scrolls down", 4294967295)], missing = 0);
reg_choice!(tp_three_swipe, tp_three_swipes, set_tp_three_swipe, PTP, "ThreeFingerSlideEnabled", [("Nothing", 0), ("Switch apps and show desktop", 1), ("Switch desktops and show desktop", 2), ("Change audio and volume", 3)], missing = 1);
reg_choice!(tp_three_tap, tp_three_taps, set_tp_three_tap, PTP, "ThreeFingerTapEnabled", [("Nothing", 0), ("Open search", 1), ("Notification center", 2), ("Play/pause", 3), ("Middle mouse button", 4)], missing = 1);
reg_choice!(tp_four_swipe, tp_four_swipes, set_tp_four_swipe, PTP, "FourFingerSlideEnabled", [("Nothing", 0), ("Switch apps and show desktop", 1), ("Switch desktops and show desktop", 2), ("Change audio and volume", 3)], missing = 2);
reg_choice!(tp_four_tap, tp_four_taps, set_tp_four_tap, PTP, "FourFingerTapEnabled", [("Nothing", 0), ("Open search", 1), ("Notification center", 2), ("Play/pause", 3), ("Middle mouse button", 4)], missing = 2);

// S85 Dynamic Lighting
reg_switch!(dyn_light, set_dyn_light, r"Software\Microsoft\Lighting", "AmbientLightingEnabled", on = 1, off = 0, missing = true);
reg_switch!(dyn_light_apps, set_dyn_light_apps, r"Software\Microsoft\Lighting", "ControlledByForegroundApp", on = 1, off = 0, missing = true);

// S115 your languages, in your order
fn languages() -> Vec<Line> {
    unsafe {
        let mut buf = vec![0u16; 2048];
        let mut sz = (buf.len() * 2) as u32;
        if RegGetValueW(HKEY_CURRENT_USER, w!("Control Panel\\International\\User Profile"), w!("Languages"), RRF_RT_REG_MULTI_SZ, None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut sz)).is_err() {
            return Vec::new();
        }
        let n = (sz / 2) as usize;
        String::from_utf16_lossy(&buf[..n]).split('\0').filter(|t| !t.is_empty()).enumerate().map(|(i, tag)| {
            let mut name = [0u16; 128];
            let k = windows::Win32::Globalization::GetLocaleInfoEx(&HSTRING::from(tag), windows::Win32::Globalization::LOCALE_SLOCALIZEDDISPLAYNAME, Some(&mut name));
            let shown = if k > 1 { String::from_utf16_lossy(&name[..k as usize - 1]) } else { tag.to_string() };
            (shown, if i == 0 { "Windows display language and first in the list".into() } else { tag.to_string() }, false, String::new())
        }).collect()
    }
}

// S118 touch keyboard
reg_switch!(tk_sound, set_tk_sound, TIP, "EnableKeyAudioFeedback", on = 1, off = 0, missing = true);
reg_switch!(tk_caps, set_tk_caps, TIP, "EnableAutoShiftEngage", on = 1, off = 0, missing = true);
reg_switch!(tk_space, set_tk_space, TIP, "EnableDoubleTapSpace", on = 1, off = 0, missing = true);
reg_switch!(tk_capslock, set_tk_capslock, TIP, "EnableShiftLock", on = 1, off = 0, missing = true);

// S172 voice activation
reg_switch!(voice_apps, set_voice_apps, r"Software\Microsoft\Speech_OneCore\Settings\VoiceActivation\UserPreferenceForAllApps", "AgentActivationEnabled", on = 1, off = 0, missing = false);


// ======================= more System pages + ms-settings links (PLAN-600 S2, S4, S9, S12-S15, S24, S26, S28, S34, S46, S193) =======================

/// The Rust page for a Windows Settings address, when there is one.
pub fn page_for_uri(uri: &str) -> Option<&'static str> {
    let page = uri.strip_prefix("ms-settings:")?.split(['?', '#']).next()?;
    Some(match page {
        "display" => "Display", "nightlight" => "Display", "display-advanced" => "Advanced display", "display-advancedgraphics" => "Graphics",
        "sound" => "Sound", "notifications" => "Notifications", "quiethours" | "quietmomentshome" => "Focus",
        "powersleep" | "batterysaver" | "batterysaver-settings" => "Power & battery", "storagesense" => "Storage", "multitasking" => "Multitasking",
        "clipboard" => "Clipboard", "about" => "About", "recovery" => "Recovery", "remotedesktop" => "Remote Desktop", "activation" => "Activation",
        "bluetooth" | "devices" => "Bluetooth & devices", "connecteddevices" => "Devices", "printers" => "Printers & scanners", "mousetouchpad" => "Mouse",
        "devices-touchpad" => "Touchpad", "camera" => "Cameras", "autoplay" => "AutoPlay", "usb" => "USB",
        "network" | "network-status" => "Network & internet", "network-wifi" => "Wi-Fi", "network-wifisettings" => "Known networks", "network-ethernet" => "Ethernet",
        "network-vpn" => "VPN", "network-proxy" => "Proxy", "network-advancedsettings" => "Advanced network settings",
        "personalization" => "Personalization", "personalization-background" => "Background", "colors" | "personalization-colors" => "Colors",
        "themes" => "Themes", "lockscreen" => "Lock screen", "taskbar" => "Taskbar", "personalization-start" => "Start", "fonts" => "Fonts",
        "personalization-lighting" => "Dynamic Lighting", "appsfeatures" => "Installed apps", "defaultapps" => "Default apps", "startupapps" => "Startup",
        "appsfeatures-app" => "Advanced app settings", "yourinfo" => "Your info", "signinoptions" => "Sign-in options", "otherusers" => "Other users",
        "emailandaccounts" => "Email & accounts", "workplace" => "Access work or school", "dateandtime" => "Date & time", "regionlanguage" => "Language & region",
        "typing" => "Typing", "speech" => "Speech", "gaming-gamemode" => "Game Mode", "gaming-gamebar" => "Game Bar", "gaming-gamedvr" => "Captures",
        "easeofaccess-display" => "Text size", "easeofaccess-visualeffects" => "Visual effects", "easeofaccess-mousepointer" => "Mouse pointer and touch",
        "easeofaccess-cursor" => "Text cursor", "easeofaccess-magnifier" => "Magnifier", "easeofaccess-colorfilter" => "Color filters",
        "easeofaccess-highcontrast" => "Contrast themes", "easeofaccess-narrator" => "Narrator", "easeofaccess-audio" => "Audio",
        "easeofaccess-closedcaptioning" => "Captions", "easeofaccess-keyboard" => "Keyboard", "easeofaccess-mouse" => "Mouse keys",
        "privacy" => "Privacy & security", "privacy-webcam" => "Camera", "privacy-microphone" => "Microphone", "privacy-location" => "Location",
        "privacy-speech" => "Speech privacy", "privacy-feedback" => "Diagnostics & feedback", "privacy-speechtyping" => "Inking & typing personalization",
        "developers" => "For developers", "windowsupdate" => "Windows Update", "windowsupdate-history" => "Update history",
        "system" => "System", "apps" => "Apps", "accounts" => "Accounts", "time-language" => "Time & language", "gaming" => "Gaming",
        "easeofaccess" | "accessibility" => "Accessibility", "focus" => "Focus", "home" => "Home",
        "windowsupdate-action" => "Check for updates",
        "datausage" => "Data usage", "videoplayback" => "Video playback", "backup" => "Windows backup", "storagerecommendations" => "Cleanup recommendations",
        "maps" => "Offline maps", "findmydevice" => "Find my device", "network-dialup" => "Dial-up", "energyrecommendations" => "Energy recommendations",
        "network-mobilehotspot" => "Mobile hotspot", "savedpasskeys" => "Passkeys", "crossdevice" => "Resume", "mobile-devices" => "Mobile devices",
        "easeofaccess-speechrecognition" => "Voice access", "personalization-textinput" => "Text input", "family-group" => "Family", "apps-volume" => "Volume mixer",
        "windowsupdate-options" => "Advanced options", "windowsinsider" => "Windows Insider Program", "troubleshoot" => "Troubleshoot",
        _ => return None,
    })
}
fn request_file() -> std::path::PathBuf {
    std::env::temp_dir().join("shellbits-settings-page.txt")
}
/// Leave a page name for the Settings window (another process opens it).
pub fn request(page: &str) {
    let _ = std::fs::write(request_file(), page);
}
fn take_request() -> Option<String> {
    let s = std::fs::read_to_string(request_file()).ok()?;
    let _ = std::fs::remove_file(request_file());
    Some(s.trim().to_string()).filter(|s| !s.is_empty())
}

// S2 night light on / off (the state blob Windows keeps; the flip follows Windows' own format:
// on = byte 18 is 0x15 and two bytes 10 00 sit at 23; the timestamp at 10.. is bumped so it applies)
const NIGHT: &str = r"Software\Microsoft\Windows\CurrentVersion\CloudStore\Store\DefaultAccount\Current\default$windows.data.bluelightreduction.bluelightreductionstate\windows.data.bluelightreduction.bluelightreductionstate";
pub fn night_blob(data: &[u8], on: bool) -> Option<Vec<u8>> {
    if data.len() < 25 { return None; }
    let now_on = data[18] == 0x15;
    if now_on == on { return Some(data.to_vec()); }
    let mut v: Vec<u8> = if on {
        let mut x = data[..23].to_vec();
        x.extend_from_slice(&[0x10, 0x00]);
        x.extend_from_slice(&data[23..]);
        x
    } else {
        let mut x = data[..23].to_vec();
        x.extend_from_slice(&data[25..]);
        x
    };
    v[18] = if on { 0x15 } else { 0x13 };
    for b in v.iter_mut().take(15).skip(10) {
        if *b != 0xFF { *b += 1; break; }
    }
    Some(v)
}
fn set_night_light(on: bool) {
    unsafe {
        let key = HSTRING::from(NIGHT);
        let mut buf = [0u8; 256];
        let mut n = buf.len() as u32;
        if RegGetValueW(HKEY_CURRENT_USER, &key, w!("Data"), RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut n)).is_err() { return; }
        if let Some(v) = night_blob(&buf[..n as usize], on) {
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, &key, w!("Data"), REG_BINARY.0, Some(v.as_ptr() as *const c_void), v.len() as u32);
        }
    }
}

// S4 HDR (advanced colour) of the main display
unsafe fn main_target() -> Option<(windows::Win32::Foundation::LUID, u32)> {
    use windows::Win32::Devices::Display::*;
    let (mut np, mut nm) = (0u32, 0u32);
    if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).is_err() { return None; }
    let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
    let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
    if QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).is_err() { return None; }
    paths.first().map(|p| (p.targetInfo.adapterId, p.targetInfo.id))
}
fn hdr_state() -> (bool, bool) {
    use windows::Win32::Devices::Display::*;
    unsafe {
        let Some((adapter, id)) = main_target() else { return (false, false) };
        let mut info = DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO::default();
        info.header = DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, size: std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32, adapterId: adapter, id };
        if DisplayConfigGetDeviceInfo(&mut info.header) != 0 { return (false, false); }
        let bits = info.Anonymous.value;
        (bits & 1 != 0, bits & 2 != 0) // supported, enabled
    }
}
fn hdr_on() -> bool { hdr_state().1 }
fn set_hdr(on: bool) {
    use windows::Win32::Devices::Display::*;
    unsafe {
        let Some((adapter, id)) = main_target() else { return };
        let mut st = DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE::default();
        st.header = DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: DISPLAYCONFIG_DEVICE_INFO_SET_ADVANCED_COLOR_STATE, size: std::mem::size_of::<DISPLAYCONFIG_SET_ADVANCED_COLOR_STATE>() as u32, adapterId: adapter, id };
        st.Anonymous.value = on as u32;
        let _ = DisplayConfigSetDeviceInfo(&st.header);
    }
}
fn hdr_text() -> String {
    match hdr_state() { (false, _) => "Not supported by this display".into(), (true, true) => "On".into(), (true, false) => "Off".into() }
}

// S9 identify: a big number on each screen for two seconds
fn identify_items() -> Vec<crate::qsx::Item> {
    vec![crate::qsx::Item { key: "identify".into(), name: "Identify displays".into(), sub: "Show each screen's number".into(), ..Default::default() }]
}
fn identify(_: &str, _: &str) {
    std::thread::spawn(|| unsafe {
        unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> windows::core::BOOL {
            (*(lp.0 as *mut Vec<HMONITOR>)).push(m);
            true.into()
        }
        unsafe extern "system" fn wproc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
            if msg == WM_PAINT {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(h, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(h, &mut rc);
                let br = CreateSolidBrush(COLORREF(0x202020));
                FillRect(dc, &rc, br);
                let _ = DeleteObject(br.into());
                let font = CreateFontW(-160, 0, 0, 0, 600, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI Variable Display"));
                let old = SelectObject(dc, font.into());
                SetTextColor(dc, COLORREF(0xFFFFFF));
                SetBkMode(dc, TRANSPARENT);
                let mut t: Vec<u16> = (GetWindowLongPtrW(h, GWLP_USERDATA) as usize).to_string().encode_utf16().collect();
                DrawTextW(dc, &mut t, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(dc, old);
                let _ = DeleteObject(font.into());
                let _ = EndPaint(h, &ps);
                return LRESULT(0);
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        let mut mons: Vec<HMONITOR> = Vec::new();
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut mons as *mut _ as isize));
        let inst: HINSTANCE = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default().into();
        let wc = WNDCLASSW { lpfnWndProc: Some(wproc), hInstance: inst, lpszClassName: w!("shellbits_identify"), ..Default::default() };
        RegisterClassW(&wc);
        let mut wins = Vec::new();
        for (k, m) in mons.iter().enumerate() {
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(*m, &mut mi);
            let r = mi.rcMonitor;
            if let Ok(h) = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, w!("shellbits_identify"), w!(""), WS_POPUP, r.left + 40, r.bottom - 300, 200, 240, None, None, Some(inst), None) {
                SetWindowLongPtrW(h, GWLP_USERDATA, (k + 1) as isize);
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                wins.push(h);
            }
        }
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut msg = MSG::default();
        while std::time::Instant::now() < until {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() { let _ = TranslateMessage(&msg); DispatchMessageW(&msg); }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        for h in wins { let _ = DestroyWindow(h); }
    });
}

// S12 / S13 the default output and input devices' format; S14 communications ducking
fn device_format(render: bool) -> Vec<Line> {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return Vec::new() };
        let Ok(dev) = en.GetDefaultAudioEndpoint(if render { eRender } else { eCapture }, eConsole) else { return vec![("Device".into(), "None".into(), false, String::new())] };
        let Ok(client) = dev.Activate::<IAudioClient>(CLSCTX_ALL, None) else { return Vec::new() };
        let Ok(fmt) = client.GetMixFormat() else { return Vec::new() };
        let f = std::ptr::read_unaligned(fmt);
        let (ch, bits, rate) = (f.nChannels, f.wBitsPerSample, f.nSamplesPerSec);
        let v = vec![
            ("Format".into(), format!("{} channel{}, {} bit, {} Hz", ch, if ch == 1 { "" } else { "s" }, bits, rate), false, String::new()),
            ("Quality".into(), if rate >= 48000 { "Studio quality".into() } else { "CD quality".into() }, false, String::new()),
        ];
        CoTaskMemFree(Some(fmt as *const c_void));
        v
    }
}
fn output_format() -> Vec<Line> { device_format(true) }
fn input_format() -> Vec<Line> { device_format(false) }
reg_choice!(ducking, duckings, set_ducking, r"Software\Microsoft\Multimedia\Audio", "UserDuckingPreference",
    [("Mute all other sounds", 0), ("Reduce the volume of other sounds by 80%", 1), ("Reduce the volume of other sounds by 50%", 2), ("Do nothing", 3)], missing = 1);

// S15 / S34 troubleshooters: Windows' Get Help pages
fn troubleshooters() -> Vec<crate::qsx::Item> {
    [("Audio", "ms-contact-support://?ActivationType=NormalLaunch&invoker=Settings&AppId=AudioTroubleshooter"), ("Network and Internet", "ms-contact-support://?ActivationType=NormalLaunch&invoker=Settings&AppId=NetworkTroubleshooter"), ("Printer", "ms-contact-support://?ActivationType=NormalLaunch&invoker=Settings&AppId=PrinterTroubleshooter"), ("Windows Update", "ms-contact-support://?ActivationType=NormalLaunch&invoker=Settings&AppId=WindowsUpdateTroubleshooter"), ("Camera", "ms-contact-support://?ActivationType=NormalLaunch&invoker=Settings&AppId=CameraTroubleshooter"), ("Bluetooth", "ms-contact-support://?ActivationType=NormalLaunch&invoker=Settings&AppId=BluetoothTroubleshooter")]
        .iter().map(|(n, u)| crate::qsx::Item { key: format!("ts\t{u}"), name: n.to_string(), sub: "Get Help troubleshooter".into(), ..Default::default() }).collect()
}
fn run_troubleshooter(id: &str, _: &str) {
    if let Some(u) = id.strip_prefix("ts\t") {
        unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &HSTRING::from(u), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL); }
    }
}

// S24 energy recommendations: what Windows suggests, and whether this PC already follows it
fn energy_facts() -> Vec<Line> {
    let ok = |b: bool| if b { "Applied".to_string() } else { "Not applied".into() };
    let screen = power_value(SUB_VIDEO, VIDEOIDLE, false).unwrap_or(0);
    let sleep = power_value(SUB_SLEEP, STANDBYIDLE, false).unwrap_or(0);
    vec![
        ("Turn off my screen after 10 minutes or less".into(), ok(screen != 0 && screen <= 600), false, String::new()),
        ("Put my device to sleep after 30 minutes or less".into(), ok(sleep != 0 && sleep <= 1800), false, String::new()),
        ("Use dark mode".into(), ok(hkcu_u32(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme") == Some(0)), false, String::new()),
        ("Turn off my screen saver".into(), ok(reg_str(HKEY_CURRENT_USER, w!("Control Panel\\Desktop"), w!("SCRNSAVE.EXE")).is_empty()), false, String::new()),
    ]
}

// S26 storage used by each of your folders (counted in the background, then cached)
static FOLDER_SIZES: std::sync::Mutex<Option<Vec<Line>>> = std::sync::Mutex::new(None);
static SIZING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn folder_sizes() -> Vec<Line> {
    if let Some(v) = FOLDER_SIZES.lock().unwrap().clone() {
        return v;
    }
    if !SIZING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        std::thread::spawn(|| {
            fn size(dir: &std::path::Path, left: &mut u32) -> u64 {
                let mut sum = 0;
                for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                    if *left == 0 { break; }
                    *left -= 1;
                    let Ok(m) = e.metadata() else { continue };
                    if m.is_dir() { sum += size(&e.path(), left); } else { sum += m.len(); }
                }
                sum
            }
            let home = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
            let v: Vec<Line> = ["Documents", "Pictures", "Videos", "Music", "Downloads", "Desktop"].iter().map(|f| {
                let mut left = 200_000u32; // ponytail: stops counting after 200,000 items in one folder
                let n = size(&home.join(f), &mut left);
                (f.to_string(), format!("{}{}", size_label(n), if left == 0 { "+" } else { "" }), false, String::new())
            }).collect();
            *FOLDER_SIZES.lock().unwrap() = Some(v);
            SIZING.store(false, std::sync::atomic::Ordering::Relaxed);
        });
    }
    vec![("Counting".into(), "Open this page again in a moment".into(), false, String::new())]
}

// S28 large files in Downloads (100 MB and up), biggest first
fn big_downloads() -> Vec<crate::qsx::Item> {
    let dir = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Downloads");
    let mut v: Vec<(u64, String)> = std::fs::read_dir(&dir).into_iter().flatten().flatten()
        .filter_map(|e| { let m = e.metadata().ok()?; (m.is_file() && m.len() >= 100 << 20).then(|| (m.len(), e.file_name().to_string_lossy().to_string())) }).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0));
    v.into_iter().take(20).map(|(n, f)| crate::qsx::Item { key: format!("dl\t{f}"), name: f, sub: size_label(n), ..Default::default() }).collect()
}
fn open_downloads(_: &str, _: &str) {
    let dir = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Downloads");
    crate::files::request(&dir.to_string_lossy());
    unsafe {
        if let Ok(h) = FindWindowW(w!("shellbits_apps"), None) {
            let _ = PostMessageW(Some(h), WM_HOTKEY, WPARAM(crate::FILES_ID as usize), LPARAM(0));
        }
    }
}

// S46 a printer's queue (Windows' own queue window)
fn open_queue(_: &str, name: &str) {
    unsafe { windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("rundll32.exe"), &HSTRING::from(format!("printui.dll,PrintUIEntry /o /n \"{name}\"")), PCWSTR::null(), SW_SHOWNORMAL); }
}

fn mono() -> bool {
    access(3)
}
fn set_mono(on: bool) {
    crate::qsx::toggle_access(3, !on);
}

const CPU0: PCWSTR = w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0");
const NT: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
type Line = (String, String, bool, String);

// ---- Mobile hotspot: Windows' own tethering (WinRT), on the connection that has the internet
fn tethering() -> Option<windows::Networking::NetworkOperators::NetworkOperatorTetheringManager> {
    let profile = windows::Networking::Connectivity::NetworkInformation::GetInternetConnectionProfile().ok()?;
    windows::Networking::NetworkOperators::NetworkOperatorTetheringManager::CreateFromConnectionProfile(&profile).ok()
}
fn hotspot_on() -> bool {
    tethering().and_then(|m| m.TetheringOperationalState().ok()) == Some(windows::Networking::NetworkOperators::TetheringOperationalState::On)
}
fn set_hotspot(on: bool) {
    // starting takes a second or two: done on a worker so the window keeps drawing
    std::thread::spawn(move || {
        let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
        if let Some(m) = tethering() {
            let _ = if on { m.StartTetheringAsync().and_then(|a| a.get()).map(|_| ()) } else { m.StopTetheringAsync().and_then(|a| a.get()).map(|_| ()) };
        }
    });
}
fn hotspot_facts() -> Vec<Line> {
    let Some(m) = tethering() else {
        return vec![("Not available".into(), "This connection can't be shared".into(), false, String::new())];
    };
    let mut v = Vec::new();
    if let Ok(c) = m.GetCurrentAccessPointConfiguration() {
        v.push(("Name".into(), c.Ssid().map(|s| s.to_string()).unwrap_or_default(), false, String::new()));
        v.push(("Password".into(), c.Passphrase().map(|s| s.to_string()).unwrap_or_default(), false, String::new()));
    }
    let n = m.ClientCount().unwrap_or(0);
    let max = m.MaxClientCount().unwrap_or(8);
    v.push(("Devices connected".into(), format!("{n} of {max}"), false, String::new()));
    v
}

// ---- Passkeys saved on this device (Windows Hello), from Windows' WebAuthn API
fn passkeys() -> Vec<Line> {
    use windows::Win32::Networking::WindowsWebServices::*;
    let opts = WEBAUTHN_GET_CREDENTIALS_OPTIONS { dwVersion: WEBAUTHN_GET_CREDENTIALS_OPTIONS_CURRENT_VERSION, ..Default::default() };
    let mut v = Vec::new();
    unsafe {
        if let Ok(list) = WebAuthNGetPlatformCredentialList(&opts) {
            if !list.is_null() {
                let items = std::slice::from_raw_parts((*list).ppCredentialDetails, (*list).cCredentialDetails as usize);
                for d in items.iter().filter(|d| !d.is_null()) {
                    let d = &**d;
                    let text = |p: PCWSTR| if p.is_null() { String::new() } else { p.to_string().unwrap_or_default() };
                    let site = if d.pRpInformation.is_null() { String::new() } else { text((*d.pRpInformation).pwszId) };
                    let user = if d.pUserInformation.is_null() { String::new() } else { text((*d.pUserInformation).pwszName) };
                    v.push((site, user, false, String::new()));
                }
                WebAuthNFreePlatformCredentialList(list);
            }
        }
    }
    if v.is_empty() {
        v.push(("No passkeys saved on this device".into(), String::new(), false, String::new()));
    }
    v
}

reg_switch!(resume_on, set_resume_on, r"Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration", "IsResumeAllowed", on = 1, off = 0, missing = true);
reg_switch!(resume_onedrive, set_resume_onedrive, r"Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration", "IsOneDriveResumeAllowed", on = 1, off = 0, missing = true);
reg_switch!(resume_whatsapp, set_resume_whatsapp, r"Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration", "IsWhatsAppResumeAllowed", on = 1, off = 0, missing = true);
reg_switch!(resume_spotify, set_resume_spotify, r"Software\Microsoft\Windows\CurrentVersion\CrossDeviceResume\Configuration", "IsSpotifyResumeAllowed", on = 1, off = 0, missing = true);
reg_switch!(mobile_access, set_mobile_access, r"Software\Microsoft\Windows\CurrentVersion\Mobility", "CrossDeviceEnabled", on = 1, off = 0, missing = false);
reg_switch!(phone_link, set_phone_link, r"Software\Microsoft\Windows\CurrentVersion\Mobility", "PhoneLinkEnabled", on = 1, off = 0, missing = false);

// ---- Voice access: running or not (Windows keeps its state here)
fn voice_access() -> bool {
    hkcu_u32(r"Software\Microsoft\VoiceAccess", "RunningState") == Some(1)
}
fn set_voice_access(on: bool) {
    unsafe {
        if on {
            windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("voiceaccess.exe"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        } else {
            windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("taskkill.exe"), w!("/im voiceaccess.exe"), PCWSTR::null(), SW_HIDE);
        }
    }
}

/// Data usage: what each network adapter has sent and received since the PC started.
// ponytail: counters since start-up; Windows' 30-day totals live in its SRUM database
fn data_usage() -> Vec<Line> {
    use windows::Win32::NetworkManagement::IpHelper::*;
    let mut v = Vec::new();
    unsafe {
        let mut t: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut t).is_ok() && !t.is_null() {
            let rows = std::slice::from_raw_parts((*t).Table.as_ptr(), (*t).NumEntries as usize);
            for r in rows {
                // real adapters only (not the filter layers stacked on each)
                if r.InterfaceAndOperStatusFlags._bitfield & 1 == 0 || r.Type == 24 || r.InOctets + r.OutOctets == 0 {
                    continue;
                }
                let n = r.Alias.iter().position(|&c| c == 0).unwrap_or(r.Alias.len());
                v.push((String::from_utf16_lossy(&r.Alias[..n]), format!("{} received, {} sent", size_label(r.InOctets), size_label(r.OutOctets)), false, String::new()));
            }
            FreeMibTable(t as *const c_void);
        }
    }
    if v.is_empty() {
        v.push(("No network adapters have been used".into(), String::new(), false, String::new()));
    }
    v
}

reg_switch!(video_enhance, set_video_enhance, r"Software\Microsoft\Windows\CurrentVersion\VideoSettings", "EnableAutoEnhanceDuringPlayback", on = 1, off = 0, missing = false);
reg_switch!(video_lowres, set_video_lowres, r"Software\Microsoft\Windows\CurrentVersion\VideoSettings", "AllowLowResolution", on = 1, off = 0, missing = false);

/// Windows backup: which of your folders OneDrive keeps (a folder moved into OneDrive is backed up).
fn backup_facts() -> Vec<Line> {
    let od = std::env::var("OneDrive").unwrap_or_default();
    let mut v: Vec<Line> = [("Desktop", "Desktop"), ("Documents", "Personal"), ("Pictures", "My Pictures"), ("Music", "My Music"), ("Videos", "My Video")].iter().map(|(label, name)| {
        let path = reg_str(HKEY_CURRENT_USER, w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\User Shell Folders"), PCWSTR(HSTRING::from(*name).as_ptr()));
        let on = !od.is_empty() && path.to_lowercase().contains("onedrive");
        (label.to_string(), if on { "Backed up to OneDrive" } else { "Not backed up" }.to_string(), false, String::new())
    }).collect();
    v.insert(0, ("OneDrive".into(), if od.is_empty() { "Not signed in".into() } else { "Signed in".into() }, false, String::new()));
    v
}

/// Cleanup recommendations: what can go - temporary files, the Recycle Bin, big downloads.
fn cleanup_facts() -> Vec<Line> {
    fn size(dir: &std::path::Path, left: &mut u32) -> u64 {
        let mut sum = 0;
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if *left == 0 { break; }
            *left -= 1;
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() { sum += size(&e.path(), left); } else { sum += m.len(); }
        }
        sum
    }
    let mut left = 50_000u32; // ponytail: stops counting after 50,000 items so the page opens at once
    let temp = size(&std::env::temp_dir(), &mut left);
    let mut bin = windows::Win32::UI::Shell::SHQUERYRBINFO { cbSize: std::mem::size_of::<windows::Win32::UI::Shell::SHQUERYRBINFO>() as u32, ..Default::default() };
    let _ = unsafe { windows::Win32::UI::Shell::SHQueryRecycleBinW(PCWSTR::null(), &mut bin) };
    let big = big_downloads();
    vec![
        ("Temporary files".into(), format!("{}{}", size_label(temp), if left == 0 { "+" } else { "" }), false, String::new()),
        ("Recycle Bin".into(), format!("{} ({} items)", size_label(bin.i64Size as u64), bin.i64NumItems), false, String::new()),
        ("Large files in Downloads".into(), format!("{} files of 100 MB or more", big.len()), false, String::new()),
    ]
}

/// Offline maps: where they are kept and whether they update themselves.
fn maps_facts() -> Vec<Line> {
    let dir = std::path::PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("Microsoft\\MapData");
    let n: u64 = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum();
    let auto = reg_u32(HKEY_LOCAL_MACHINE, w!("SYSTEM\\Maps"), w!("AutoUpdateEnabled")).unwrap_or(1) != 0;
    vec![
        ("Storage location".into(), dir.display().to_string(), false, String::new()),
        ("Maps downloaded".into(), if n == 0 { "None".into() } else { size_label(n) }, false, String::new()),
        ("Update maps automatically".into(), if auto { "On".into() } else { "Off".into() }, false, String::new()),
    ]
}

/// Find my device: on when Windows sends this PC's location.
fn find_my_device() -> String {
    let on = reg_u32(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\Settings\\FindMyDevice"), w!("LocationSyncEnabled")).unwrap_or(0) != 0;
    if on { "On - Windows saves this device's location so you can find it from account.microsoft.com/devices".into() } else { "Off".into() }
}

fn cpu_speed() -> String {
    // Windows rounds to a tenth and prints two decimals ("3.20 GHz")
    format!("{:.2} GHz", (reg_u32(HKEY_LOCAL_MACHINE, CPU0, w!("~MHz")).unwrap_or(0) as f64 / 100.0).round() / 10.0)
}
/// Installed and usable memory in GB.
fn ram_gb() -> (f64, f64) {
    use windows::Win32::System::SystemInformation::{GetPhysicallyInstalledSystemMemory, GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    let mut kb = 0u64;
    unsafe {
        let _ = GlobalMemoryStatusEx(&mut m);
        let _ = GetPhysicallyInstalledSystemMemory(&mut kb);
    }
    let usable = m.ullTotalPhys as f64 / 1073741824.0;
    (if kb > 0 { kb as f64 / 1048576.0 } else { usable }, usable)
}
pub fn size_label(bytes: u64) -> String {
    if bytes >= 1 << 30 { format!("{} GB", (bytes as f64 / 1073741824.0).round()) } else { format!("{} MB", (bytes as f64 / 1048576.0).round()) }
}
/// Graphics adapters and their dedicated memory, biggest first.
fn gpus() -> Vec<(String, u64)> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
    let mut v: Vec<(String, u64)> = Vec::new();
    unsafe {
        if let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() {
            let mut i = 0;
            while let Ok(a) = f.EnumAdapters1(i) {
                i += 1;
                if let Ok(d) = a.GetDesc1() {
                    let name = String::from_utf16_lossy(&d.Description[..d.Description.iter().position(|c| *c == 0).unwrap_or(0)]);
                    if d.Flags & 2 == 0 && !v.iter().any(|g| g.0 == name) {
                        v.push((name, d.DedicatedVideoMemory as u64)); // 2 = software adapter
                    }
                }
            }
        }
    }
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v
}
/// Used and total bytes across the fixed drives.
fn storage() -> (u64, u64) {
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
    let (mut used, mut total) = (0u64, 0u64);
    unsafe {
        let mask = GetLogicalDrives();
        for i in 0..26 {
            let root = HSTRING::from(format!("{}:\\", (b'A' + i) as char));
            if mask & (1 << i) != 0 && GetDriveTypeW(&root) == 3 {
                let (mut free, mut all) = (0u64, 0u64);
                if GetDiskFreeSpaceExW(&root, None, Some(&mut all), Some(&mut free)).is_ok() {
                    used += all - free;
                    total += all;
                }
            }
        }
    }
    (used, disks().max(total))
}
/// Size of every fixed disk added up (what Windows calls your storage, partitions you can't see included).
fn disks() -> u64 {
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    let mut total = 0u64;
    for i in 0..8 {
        unsafe {
            let Ok(h) = CreateFileW(&HSTRING::from(format!(r"\\.\PhysicalDrive{i}")), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) else { break };
            let mut out = [0u8; 64]; // DISK_GEOMETRY_EX: media type at 8, disk size at 24
            let mut got = 0u32;
            if windows::Win32::System::IO::DeviceIoControl(h, 0x700A0, None, 0, Some(out.as_mut_ptr() as *mut c_void), 64, Some(&mut got), None).is_ok() && out[8] == 12 {
                total += u64::from_le_bytes(out[24..32].try_into().unwrap());
            }
            let _ = CloseHandle(h);
        }
    }
    total
}
fn storage_text() -> String {
    let (used, total) = storage();
    format!("{} of {} used", size_label(used), size_label(total))
}
fn spec_tiles() -> Vec<Line> {
    let g = gpus();
    let (installed, _) = ram_gb();
    let mut v = vec![("Processor".to_string(), processor().trim().to_string(), false, cpu_speed()), ("Installed RAM".to_string(), format!("{installed:.1} GB"), false, String::new())];
    if let Some(top) = g.first() {
        v.push(("Graphics card".to_string(), size_label(top.1), false, if g.len() > 1 { "Multiple GPUs installed".to_string() } else { top.0.clone() }));
    }
    v.push(("Storage".to_string(), size_label(storage().1), false, storage_text()));
    v
}
fn device_info() -> Vec<Line> {
    let (installed, usable) = ram_gb();
    let line = |k: &str, v: String| (k.to_string(), v, false, String::new());
    vec![
        line("Device name", device_name().to_lowercase()),
        line("Processor", format!("{} ({})", processor().trim(), cpu_speed())),
        line("Installed RAM", format!("{installed:.1} GB ({usable:.1} GB usable)")),
        line("Graphics card", gpus().iter().map(|g| format!("{} ({})", g.0, size_label(g.1))).collect::<Vec<_>>().join("\n")),
        line("Storage", storage_text()),
        line("Device ID", reg_str(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\SQMClient"), w!("MachineId")).trim_matches(|c| c == '{' || c == '}').to_string()),
        line("Product ID", reg_str(HKEY_LOCAL_MACHINE, NT, w!("ProductId"))),
        line("System type", format!("64-bit operating system, {}-based processor", if std::env::var("PROCESSOR_ARCHITECTURE").is_ok_and(|a| a == "ARM64") { "ARM" } else { "x64" })),
    ]
}
/// Days since 1970 -> (year, month, day).
pub fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}
fn windows_specs() -> Vec<Line> {
    let line = |k: &str, v: String| (k.to_string(), v, false, String::new());
    let (y, m, d) = civil(reg_u32(HKEY_LOCAL_MACHINE, NT, w!("InstallDate")).unwrap_or(0) as i64 / 86400);
    vec![
        line("Edition", reg_str(HKEY_LOCAL_MACHINE, NT, w!("ProductName")).replace("Windows 10", "Windows 11")),
        line("Version", reg_str(HKEY_LOCAL_MACHINE, NT, w!("DisplayVersion"))),
        line("Installed on", format!("{m}/{d}/{y}")),
        line("OS build", format!("{}.{}", reg_str(HKEY_LOCAL_MACHINE, NT, w!("CurrentBuild")), reg_u32(HKEY_LOCAL_MACHINE, NT, w!("UBR")).unwrap_or(0))),
    ]
}
/// Put text on the clipboard (the Copy buttons).
pub(crate) fn copy_text(text: &str) {
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    let w: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        if OpenClipboard(None).is_ok() {
            let _ = EmptyClipboard();
            if let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, w.len() * 2) {
                let p = GlobalLock(h) as *mut u16;
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(w.as_ptr(), p, w.len());
                    let _ = GlobalUnlock(h);
                    let _ = SetClipboardData(13, Some(HANDLE(h.0))); // CF_UNICODETEXT
                }
            }
            let _ = CloseClipboard();
        }
    }
}

fn device_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}
fn processor() -> String {
    reg_str(HKEY_LOCAL_MACHINE, w!("HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0"), w!("ProcessorNameString"))
}
/// [(time, date), ("Time zone", display name), ("Region", country)] in your own formats.
fn time_summary() -> Vec<(String, String, bool, String)> {
    use windows::Win32::Globalization::{GetDateFormatEx, GetGeoInfoEx, GetTimeFormatEx, GetUserDefaultGeoName, DATE_LONGDATE, GEO_FRIENDLYNAME, TIME_NOSECONDS};
    let text = |buf: &[u16], n: i32| String::from_utf16_lossy(&buf[..(n.max(1) - 1) as usize]);
    let mut buf = [0u16; 128];
    unsafe {
        let n = GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, None, PCWSTR::null(), Some(&mut buf));
        let time = text(&buf, n);
        let n = GetDateFormatEx(PCWSTR::null(), DATE_LONGDATE, None, PCWSTR::null(), Some(&mut buf), PCWSTR::null());
        let date = text(&buf, n);
        let key = reg_str(HKEY_LOCAL_MACHINE, w!("SYSTEM\\CurrentControlSet\\Control\\TimeZoneInformation"), w!("TimeZoneKeyName"));
        let zone = reg_str(HKEY_LOCAL_MACHINE, PCWSTR(HSTRING::from(format!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\Time Zones\\{key}")).as_ptr()), w!("Display"));
        let mut geo = [0u16; 16];
        let _ = GetUserDefaultGeoName(&mut geo);
        let n = GetGeoInfoEx(PCWSTR(geo.as_ptr()), GEO_FRIENDLYNAME, Some(&mut buf));
        let region = text(&buf, n);
        vec![(time, date, false, String::new()), ("Time zone".into(), if zone.is_empty() { time_zone() } else { zone }, false, String::new()), ("Region".into(), region, false, String::new())]
    }
}
/// Windows has installed updates and is waiting for a restart.
fn restart_pending() -> bool {
    let mut k = HKEY::default();
    unsafe {
        let ok = RegOpenKeyExW(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\WindowsUpdate\\Auto Update\\RebootRequired"), Some(0), KEY_READ, &mut k).is_ok();
        if ok {
            let _ = RegCloseKey(k);
        }
        // newer builds stage the update and offer "Update and restart" instead of setting that key
        ok || reg_u32(HKEY_LOCAL_MACHINE, w!(r"SOFTWARE\Microsoft\WindowsUpdate\Orchestrator"), w!("EnhancedShutdownEnabled")) == Some(1)
    }
}
fn early_updates() -> bool {
    reg_u32(HKEY_LOCAL_MACHINE, w!("SOFTWARE\\Microsoft\\WindowsUpdate\\UX\\Settings"), w!("IsContinuousInnovationOptedIn")) == Some(1)
}
fn time_zone() -> String {
    let mut tz = windows::Win32::System::Time::TIME_ZONE_INFORMATION::default();
    unsafe { windows::Win32::System::Time::GetTimeZoneInformation(&mut tz) };
    String::from_utf16_lossy(&tz.StandardName[..tz.StandardName.iter().position(|&c| c == 0).unwrap_or(0)])
}
/// "Administrator" or "Standard user", from how Windows issued this sign-in's token.
fn account_type() -> String {
    use windows::Win32::Security::{GetTokenInformation, TokenElevationType, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut tok = HANDLE::default();
        let (mut kind, mut n) = (0u32, 0u32);
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok).is_ok() {
            let _ = GetTokenInformation(tok, TokenElevationType, Some(&mut kind as *mut _ as *mut c_void), 4, &mut n);
            let _ = CloseHandle(tok);
        }
        // 2 = full administrator token, 3 = an administrator's limited token, 1 = an ordinary account
        if kind >= 2 { "Administrator".into() } else { "Standard user".into() }
    }
}
fn account_summary() -> Vec<(String, String, bool, String)> {
    vec![(crate::launcher::display_name(), account_email(), false, account_type())]
}
fn home_summary() -> Vec<(String, String, bool, String)> {
    let (net, _) = connection();
    let line = |a: String, b: String| (a, b, false, String::new());
    vec![
        line(device_name().to_lowercase(), model()),
        line(if net.is_empty() { "Not connected".into() } else { net.clone() }, if net.is_empty() { String::new() } else { "Connected".into() }),
        line("Windows Update".into(), if restart_pending() { "Restart required".into() } else { "You're up to date".into() }),
    ]
}
fn rust_ui_ram() -> String {
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    let mut c = PROCESS_MEMORY_COUNTERS::default();
    unsafe {
        let _ = K32GetProcessMemoryInfo(windows::Win32::System::Threading::GetCurrentProcess(), &mut c, size_of::<PROCESS_MEMORY_COUNTERS>() as u32);
    }
    format!("{:.0} MB in use, {:.0} MB committed", c.WorkingSetSize as f64 / 1048576.0, c.PagefileUsage as f64 / 1048576.0)
}
fn is_shell() -> bool {
    crate::session::installed()
}
fn set_shell(on: bool) {
    let _ = if on { crate::session::install() } else { crate::session::restore() };
}

fn model() -> String {
    reg_str(HKEY_LOCAL_MACHINE, w!("HARDWARE\\DESCRIPTION\\System\\BIOS"), w!("SystemProductName"))
}

/// The Microsoft account this PC is signed in with (shown under your name), if any.
fn account_email() -> String {
    unsafe {
        let mut k = HKEY::default();
        // the account Windows itself is signed in with (other apps' accounts are stored elsewhere)
        if RegOpenKeyExW(HKEY_USERS, w!(".DEFAULT\\Software\\Microsoft\\IdentityCRL\\StoredIdentities"), Some(0), KEY_READ, &mut k).is_err() {
            return String::new();
        }
        let mut name = [0u16; 256];
        let mut n = name.len() as u32;
        let ok = RegEnumKeyExW(k, 0, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut n, None, None, None, None).is_ok();
        let _ = RegCloseKey(k);
        if ok { String::from_utf16_lossy(&name[..n as usize]) } else { String::new() }
    }
}

fn dnd() -> bool {
    crate::notify::dnd_get()
}

fn pages() -> Vec<Page> {
    use Kind::*;
    let r = |icon, label, sub, kind| Row { icon, label, sub, kind };
    let top = |name, icon, rows| Page { name, icon, parent: "", rows };
    let sub = |name, parent, rows| Page { name, icon: 0, parent, rows };
    vec![
        top("Home", 0xE80F, vec![
            r(0, "", "", HomeHero(home_summary)),
            r(0, "Recommended settings", "", Header),
            r(0xEDA2, "Storage", "", Link("ms-settings:storagesense")),
            r(0xE71D, "Installed apps", "", Link("ms-settings:appsfeatures")),
            r(0, "Rust UI", "", Header),
            r(0xE950, "Use Rust UI as the Windows shell", "Replaces Explorer's taskbar and desktop from your next sign-in", Toggle(is_shell, set_shell)),
            r(0xE9D9, "Memory used by Rust UI", "", Info(rust_ui_ram)),
        ]),
        top("System", 0xE770, vec![
            r(0xE7F8, "Display", "Monitors, brightness, night light, display profile", Sub("Display")),
            r(0xE767, "Sound", "Volume levels, output, input, sound devices", Sub("Sound")),
            r(0xEA8F, "Notifications", "Alerts from apps and system, do not disturb", Sub("Notifications")),
            r(SET | 0xE5C4, "Focus", "Reduce distractions", Sub("Focus")),
            r(0xE7E8, "Power & battery", "Screen and sleep, battery usage, power mode, energy saver", Sub("Power & battery")),
            r(0xEDA2, "Storage", "Storage space, drives, configuration rules", Sub("Storage")),
            r(0xE72D, "Share", "Share files, photos, links and more", Sub("Share")),
            r(0xE7C4, "Multitasking", "Snap windows, desktops, task switching", Sub("Multitasking")),
            r(0xE77F, "Clipboard", "Cut and copy history, sync, clear", Sub("Clipboard")),
            r(0xE90F, "Troubleshoot", "Recommended troubleshooters, preferences, history", Sub("Troubleshoot")),
            r(0xE777, "Recovery", "Reset, advanced startup, go back", Sub("Recovery")),
            r(0xE8AF, "Remote Desktop", "Remote Desktop users, connection permissions", Sub("Remote Desktop")),
            r(0xE946, "About", "Device specifications, rename PC, Windows specifications", Sub("About")),
        ]),
        sub("Display", "System", {
            // ponytail: no Brightness card here, like the real page on this PC (it lives in Quick Settings);
            // add it back when we know the rule Windows uses to show it
            let mut v = vec![r(0, "Brightness & color", "", Header)];
            if unsafe { crate::quick::get_brightness() }.is_some() {
                v.push(r(0xE706, "Brightness", "Adjust the brightness of the built-in display", Slider(brightness_get, brightness_set)));
            }
            v.extend([
                r(0xF08C, "Night light", "Use warmer colors to help block blue light", Toggle(night_light, set_night_light)),
                r(0xEF3C, "Color profile", "Determine how colors appear on your screen", Sub("Color profile")),
                r(SET | 0xEA7F, "HDR", "Brighter, more vivid picture on displays that support it", if hdr_state().0 { Toggle(hdr_on, set_hdr) } else { Info(hdr_text) }),
                r(0, "Scale & layout", "", Header),
                r(SET | 0xE58E, "Scale", "Change the size of text, apps, and other items", Choice(scale, scales, set_scale)),
                r(0xE799, "Display resolution", "Adjust the resolution to fit your connected display", Choice(resolution, resolutions, set_resolution)),
                r(0xE8B4, "Display orientation", "", Choice(orientation, orientations, set_orientation)),
                r(0xEBC6, "Multiple displays", "Choose the presentation mode for your displays", if several_displays() { Choice(topology, topologies, set_topology) } else { Expand("ms-settings:display") }),
                r(0, "Related settings", "", Header),
                r(0xE7F4, "Advanced display", "Display information, refresh rate", Sub("Advanced display")),
                r(0xE7FC, "Graphics", "", Sub("Graphics")),
                r(0, "", "", Actions(identify_items, "Identify", identify)),
            ]);
            v
        }),
        sub("Sound", "System", vec![
            r(0, "Output", "", Header),
            r(0xE767, "Choose where to play sound", "Apps might have their own settings", Devices(crate::qsx::outputs, "Pair a new output device")),
            r(0, "Volume", "", Slider(volume, set_volume)),
            r(0, "Mono audio", "Combine left and right audio channels into one", Toggle(mono, set_mono)),
            r(0, "Input", "", Header),
            r(0xE720, "Choose a device for speaking or recording", "Apps might have their own settings", Devices(crate::qsx::inputs, "Pair a new input device")),
            r(0, "Advanced", "", Header),
            r(0xE74F, "Mute", "", Toggle(muted, set_muted)),
            r(0xE9E9, "Volume mixer", "App volume mix, app input & output devices", Sub("Volume mixer")),
            r(0xE767, "Output format", "", Facts(output_format)),
            r(0xE720, "Input format", "", Facts(input_format)),
            r(0xE717, "When Windows detects communications activity", "", Choice(ducking, duckings, set_ducking)),
            r(0, "Troubleshoot common sound problems", "", Header),
            r(0, "", "", Actions(troubleshooters, "Run", run_troubleshooter)),
            r(0, "More sound settings", "", Link("ms-settings:sound")),
        ]),
        sub("Notifications", "System", vec![
            r(0xEA8F, "Notifications", "Get notifications from apps and other senders", Toggle(toasts, set_toasts)),
            r(0xF4BD, "Do not disturb", "Notifications will be sent directly to notification center", Toggle(dnd, crate::notify::dnd_set)),
            r(SET | 0xECC1, "Turn on do not disturb automatically", "", Expand("ms-settings:notifications")),
            r(0xE8D0, "Set priority notifications", "", Link("ms-settings:notifications")),
            r(SET | 0xE5C4, "Focus", "Session duration, hide badges on apps", Link("ms-settings:quiethours")),
            r(0, "Notifications from apps and other senders", "", Header),
            r(0, "Sort by:", "", SortBy),
            r(0, "", "", Senders(senders, set_sender)),
        ]),
        sub("Power & battery", "System", vec![
            r(0, "", "", Big(battery)),
            r(0xE946, "", "", Note(power_managed, "Some of these settings are managed by your organization.")),
            r(0xE8BE, "Energy recommendations", "Lower your carbon footprint by applying these recommendations", Sub("Energy recommendations")),
            r(0xEC48, "Power Mode", "Optimize your device based on power use and performance", Flat(power_mode, power_modes, set_power_mode)),
            r(0xEE3F, "Screen, sleep, & hibernate timeouts", "Choose what happens when your device is idle for a specified time", Sub("Screen and sleep")),
            r(0xEBB2, "Battery percentage", "Display battery percentage on taskbar", Toggle(battery_percent_shown, set_battery_percent)),
            r(0xEBBA, "Energy saver", "Reduce power consumption and increase battery life by limiting some background activities", Flat(saver_text, saver_choices, toggle_saver)),
            r(SET | 0xE9E6, "Battery usage", "", Expand("ms-settings:batterysaver-usagedetails")),
            r(0xE762, "Lid & power button controls", "Choose what happens when you interact with your device's physical controls", Sub("Lid & power button controls")),
        ]),
        sub("About", "System", vec![
            r(0, "", "", Tiles(spec_tiles)),
            r(0, "", "", RenamePc),
            r(0xE946, "Device info", "", Facts(device_info)),
            r(0xE770, "Windows specifications", "", Facts(windows_specs)),
            r(0, "Product key and activation", "Change product key or upgrade your edition of Windows", Sub("Activation")),
        ]),
        top("Bluetooth & devices", 0xE702, vec![
            r(0, "", "", BtTiles(crate::qsx::bt_devices)),
            r(0, "", "", Center("View more devices", "ms-settings:connecteddevices")),
            r(0xE702, "Bluetooth", discoverable(), Toggle(bluetooth, set_bluetooth)),
            r(0, "", "", Space(16.33)),
            r(0xE772, "Devices", "Mouse, keyboard, pen, audio, displays and docks, other devices", LinkBtn("Devices", "Add device", "ms-settings-connectabledevices:devicediscovery")),
            r(0xE749, "Printers & scanners", "Preferences", Sub("Printers & scanners")),
            r(0xE1C9, "Mobile devices", "Instantly access your mobile devices from your PC", Link("ms-settings:mobile-devices")),
            r(0xE722, "Cameras", "Connected cameras, default image settings", Sub("Cameras")),
            r(0xEC31, "AutoPlay", "Defaults for removable drives and memory cards", Sub("AutoPlay")),
            r(0xE88E, "USB", "Notifications, USB battery saver", Sub("USB")),
            r(0xE962, "Mouse", "Buttons, mouse pointer speed, scrolling", Sub("Mouse")),
            r(0xE962, "Mouse pointer speed", "", Slider(mouse_speed, set_mouse_speed)),
            r(0xEFA5, "Touchpad", "Taps, gestures, scrolling, zooming", Sub("Touchpad")),
        ]),
        top("Network & internet", 0xE701, vec![
            r(0, "", "", NetHero(connection)),
            r(0xE701, "Wi-Fi", "Connect, manage known networks, metered network", Sub("Wi-Fi")),
            r(0xE839, "Ethernet", "Authentication, IP and DNS settings, metered network", Sub("Ethernet")),
            r(0xE705, "VPN", "Add, connect, manage", Sub("VPN")),
            r(0xE704, "Mobile hotspot", "Share your internet connection", Link("ms-settings:network-mobilehotspot")),
            r(0xE709, "Airplane mode", "Stop wireless communication", ToggleMore(airplane, crate::qsx::set_airplane, "Airplane mode")),
            r(0xE968, "Proxy", "Proxy server for Wi-Fi and Ethernet connections", Sub("Proxy")),
            r(0xE83C, "Dial-up", "Set up a dial-up internet connection", Sub("Dial-up")),
            r(0xE774, "Advanced network settings", "View all network adapters, network reset", Sub("Advanced network settings")),
        ]),
        top("Personalization", 0xE771, vec![
            r(0, "", "", ThemeHero(themes)),
            r(0xEB9F, "Background", "Background image, color, slideshow", Sub("Background")),
            r(0xE790, "Colors", "Accent color, transparency effects, color theme", Sub("Colors")),
            r(0xE771, "Themes", "Install, create, manage", Sub("Themes")),
            r(SET | 0xF431, "Dynamic Lighting", "Connected devices, effects, app settings", Sub("Dynamic Lighting")),
            r(0xEE3F, "Lock screen", "Lock screen images, apps, animations", Sub("Lock screen")),
            r(0xE144, "Text input", "Touch keyboard, voice typing, emoji and more, input method editor", Link("ms-settings:personalization-textinput")),
            r(0xE8FC, "Start", "Recent apps and items, folders", Sub("Start")),
            r(0xE90E, "Taskbar", "Taskbar behaviors, system pins", Sub("Taskbar")),
            r(0xE8D2, "Fonts", "Install, manage", Sub("Fonts")),
            r(0xE7F8, "Device usage", "Select all the ways you plan to use your device", Sub("Device usage")),
        ]),
        sub("Colors", "Personalization", vec![
            r(0xE790, "Choose your mode", "Change the colors that appear in Windows and your apps", Choice(color_mode, color_modes, set_color_mode)),
            r(0xEF1F, "Transparency effects", "Windows and surfaces appear translucent", Toggle(transparency, set_transparency)),
            r(0, "Accent color", "", Header),
            r(0, "", "", Actions(accent_items, "Pick", accent_pick)),
            r(0xE790, "Show accent color on Start and taskbar", "", Toggle(accent_on_start, set_accent_on_start)),
            r(0xE790, "Show accent color on title bars and window borders", "", Toggle(accent_on_titles, set_accent_on_titles)),
        ]),
        sub("Game Mode", "Gaming", vec![
            r(0xE7FC, "Game Mode", "Turns off things in the background, like Windows updates and app notifications", Toggle(game_mode, set_game_mode)),
            r(0xE7F4, "Graphics", "Choose which graphics card each app uses, in Windows' own page", Link("ms-settings:display-advancedgraphics")),
        ]),
        sub("Game Bar", "Gaming", vec![
            r(SET | 0xF192, "Allow your controller to open Game Bar", "Press the Xbox button, or Windows logo key + G", Toggle(game_bar, set_game_bar)),
            r(0xE714, "Record what happened", "Captures in the background while you play", Toggle(game_dvr, set_game_dvr)),
        ]),
        sub("Typing", "Time & language", vec![
            r(0xE144, "Show text suggestions when typing on the physical keyboard", "", Toggle(hw_suggest, set_hw_suggest)),
            r(0xE144, "Multilingual text suggestions", "", Toggle(multilingual, set_multilingual)),
            r(0xE144, "Autocorrect misspelled words", "", Toggle(autocorrect, set_autocorrect)),
            r(0xE144, "Highlight misspelled words", "", Toggle(spellcheck, set_spellcheck)),
            r(0xE765, "Touch keyboard", "Sounds, capitals, double-tap space", Sub("Touch keyboard")),
            r(0xE765, "Advanced keyboard settings", "In Windows' own page", Link("ms-settings:typing")),
        ]),
        sub("Language & region", "Time & language", vec![
            r(0, "Language", "", Header),
            r(0xF2B7, "Windows display language", "Windows features like Settings and File Explorer appear in this language", Info(display_language)),
            r(0xF2B7, "Preferred languages", "", Facts(languages)),
            r(0xF2B7, "Add a language or change the order", "In Windows' own page", Link("ms-settings:regionlanguage")),
            r(0, "Region", "", Header),
            r(0xE909, "Country or region", "Windows and apps might use your country or region to give you local content", Info(country)),
            r(0xE8EF, "Regional format", "Windows and some apps format dates and times based on your regional format", Info(regional_format)),
            r(0xE787, "Formats", "", Info(format_sample)),
            r(0xE787, "First day of week", "", Info(week_start)),
        ]),
        sub("Your info", "Accounts", vec![
            r(0xE77B, "User name", "", Info(user_name)),
            r(0xE715, "Microsoft account", "", Info(account_email)),
            r(0xE7EF, "Account type", "", Info(account_type)),
            r(0xE7F4, "Device name", "", Info(pc_name)),
            r(0xE722, "Adjust your photo, manage your account online", "In Windows' own page", Link("ms-settings:yourinfo")),
        ]),
        sub("Sign-in options", "Accounts", vec![
            r(0xE192, "Ways to sign in: face, fingerprint, PIN, security key, password", "Changing them needs Windows' own page, which asks you to confirm who you are", Link("ms-settings:signinoptions")),
            r(0, "Additional settings", "", Header),
            r(0xE72E, "Dynamic lock", "Automatically lock your device when you're away, using a paired phone", Toggle(dynamic_lock, set_dynamic_lock)),
        ]),
        sub("Devices", "Bluetooth & devices", vec![
            r(0, "Bluetooth devices", "", Header),
            r(0, "", "", Actions(bt_list, "Connect", bt_manage)),
            r(0xE710, "Add device", "Pair a new Bluetooth or other device, in Windows' own page", Link("ms-settings-connectabledevices:devicediscovery")),
        ]),
        sub("Printers & scanners", "Bluetooth & devices", vec![
            r(0, "", "", Actions(printers, "Set as default", make_default_printer)),
            r(0, "Print queues", "", Header),
            r(0, "", "", Actions(printers, "Open queue", open_queue)),
            r(0, "Printer preferences", "", Header),
            r(0xE749, "Let Windows manage my default printer", "The default is the printer you used most recently at your current location", Toggle(printer_auto_default, set_printer_auto_default)),
            r(0xE710, "Add a printer or scanner", "In Windows' own page", Link("ms-settings:printers")),
        ]),
        sub("Touchpad", "Bluetooth & devices", vec![
            r(0xEFA5, "Leave touchpad on when a mouse is connected", "", Toggle(tp_with_mouse, set_tp_with_mouse)),
            r(0, "Taps", "", Header),
            r(0xEFA5, "Touchpad sensitivity", "", Choice(tp_sensitivity, tp_sensitivities, set_tp_sensitivity)),
            r(0xEFA5, "Tap with a single finger to single-click", "", Toggle(tp_taps, set_tp_taps)),
            r(0xEFA5, "Tap with two fingers to right-click", "", Toggle(tp_two_finger_tap, set_tp_two_finger_tap)),
            r(0xEFA5, "Tap twice and drag to multi-select", "", Toggle(tp_drag, set_tp_drag)),
            r(0xEFA5, "Press the lower right corner of the touchpad to right-click", "", Toggle(tp_right_zone, set_tp_right_zone)),
            r(0, "Scroll & zoom", "", Header),
            r(0xEC8F, "Drag two fingers to scroll", "", Toggle(tp_scroll, set_tp_scroll)),
            r(0xE8A3, "Pinch to zoom", "", Toggle(tp_zoom, set_tp_zoom)),
            r(0xEC8F, "Scrolling direction", "", Choice(tp_scroll_dir, tp_scroll_dirs, set_tp_scroll_dir)),
            r(0, "Three-finger gestures", "", Header),
            r(0xEFA5, "Swipes", "", Choice(tp_three_swipe, tp_three_swipes, set_tp_three_swipe)),
            r(0xEFA5, "Taps", "", Choice(tp_three_tap, tp_three_taps, set_tp_three_tap)),
            r(0, "Four-finger gestures", "", Header),
            r(0xEFA5, "Swipes", "", Choice(tp_four_swipe, tp_four_swipes, set_tp_four_swipe)),
            r(0xEFA5, "Taps", "", Choice(tp_four_tap, tp_four_taps, set_tp_four_tap)),
            r(0xEFA5, "Touchpad changes", "Windows applies changes made here the next time the touchpad driver reads them; Windows' own page applies them at once", Link("ms-settings:devices-touchpad")),
        ]),
        sub("Background", "Personalization", vec![
            r(0xEB9F, "Current picture", "", Info(wallpaper_text)),
            r(0xE9A6, "Choose a fit for your desktop image", "", Choice(fit, fits, set_fit)),
            r(0, "Recent images", "", Header),
            r(0, "", "", Actions(recent_wallpapers, "Use", use_wallpaper)),
            r(0, "Rust UI", "", Header),
            r(0xE714, "Video wallpaper", "Rust UI plays this over the picture above", Info(video_wallpaper)),
            r(0, "Personalize your background", "", Header),
            r(0, "", "", Actions(bg_items, "Choose", bg_pick)),
            r(0xE8B9, "Slideshow", "In Windows' own page", Link("ms-settings:personalization-background")),
        ]),
        sub("Themes", "Personalization", vec![
            r(0, "Windows themes", "", Header),
            r(0, "", "", Actions(theme_list, "Apply", apply_theme_file)),
            r(0xE771, "Get more themes from the Microsoft Store", "", Link("ms-windows-store://collection?CollectionId=WindowsThemes")),
        ]),
        sub("Lock screen", "Personalization", vec![
            r(0xEE3F, "Personalize your lock screen", "", Info(lock_background)),
            r(0xE82F, "Get fun facts, tips, tricks, and more on your lock screen", "", Toggle(lock_tips, set_lock_tips)),
            r(0xEE3F, "Change the picture, status and sign-in screen", "In Windows' own page", Link("ms-settings:lockscreen")),
        ]),
        sub("Mouse", "Bluetooth & devices", vec![
            r(0xE962, "Primary mouse button", "", Choice(primary_button, primary_buttons, set_primary_button)),
            r(0xE962, "Mouse pointer speed", "", Slider(mouse_speed, set_mouse_speed)),
            r(0xE7C9, "Enhance pointer precision", "", Toggle(precision, set_precision)),
            r(0, "Scrolling", "", Header),
            r(0xEC8F, "Lines to scroll at a time", "", Choice(wheel_lines, wheel_choices, set_wheel_lines)),
            r(0xEC8F, "Scroll inactive windows when hovering over them", "", Toggle(scroll_inactive, set_scroll_inactive)),
            r(0xE7C9, "Additional mouse settings", "Pointer schemes and more, in Windows' own page", Link("ms-settings:mousetouchpad")),
        ]),
        sub("Date & time", "Time & language", vec![
            r(0xE823, "Date and time", "", Info(now_text)),
            r(0xE823, "Set time automatically", "Your PC keeps its clock right using an internet time server", State(auto_time, "ms-settings:dateandtime")),
            r(0xE909, "Set time zone automatically", "", State(auto_zone, "ms-settings:dateandtime")),
            r(0xE909, "Time zone", "", Choice(zone_now, zones, set_zone)),
            r(0xE823, "Additional clocks", "", Facts(extra_clocks)),
            r(0xE774, "Time server", "", Info(time_server)),
            r(0xE823, "Show time and date in the System tray", "", Toggle(tray_time, set_tray_time)),
            r(0xE787, "Change the time, time zone or sync now", "These need administrator rights: Windows' own page asks for them", Link("ms-settings:dateandtime")),
        ]),
        sub("Wi-Fi", "Network & internet", vec![
            r(0xE701, "Wi-Fi", "", Toggle(wifi, set_wifi)),
            r(0, "Available networks", "", Header),
            r(0, "", "", Actions(wifi_list, "Connect", wifi_pick)),
            r(0xE8F1, "Manage known networks", "Add, remove, and edit networks, in Windows' own page", Sub("Known networks")),
            r(0xE7BA, "Hardware properties", "", Sub("Wi-Fi properties")),
        ]),
        sub("Storage", "System", vec![
            r(0, "Drives", "", Header),
            r(0, "", "", Actions(drive_list, "Open", open_drive)),
            r(0, "Storage management", "", Header),
            r(0xEDA2, "Storage Sense", "Automatically free up space, delete temporary files, and manage locally available cloud content", Toggle(storage_sense, set_storage_sense)),
            r(0, "", "", Actions(temp_item, "Remove files", clean_temp)),
            r(0xE9F5, "Cleanup recommendations", "Windows' own list of what can be removed", Link("ms-settings:storagerecommendations")),
            r(0xEDA2, "Storage Sense schedule", "When it runs and what it deletes", Sub("Storage Sense")),
            r(0, "Your folders", "", Header),
            r(0xEDA2, "Space used", "", Facts(folder_sizes)),
            r(0, "Large files in Downloads", "", Header),
            r(0, "", "", Actions(big_downloads, "Open folder", open_downloads)),
        ]),
        sub("Color profile", "Display", vec![
            r(0, "", "", Actions(color_profiles, "Open folder", open_profiles)),
        ]),
        sub("Advanced display", "Display", vec![
            r(0xE7F4, "Display information", "", Facts(display_facts)),
            r(0xE7F4, "Choose a refresh rate", "A higher rate gives smoother motion, but also uses more power", Choice(refresh, refreshes, set_refresh)),
        ]),
        sub("Graphics", "Display", vec![
            r(0, "Custom options for apps", "", Header),
            r(0, "", "", Actions(gpu_apps, "Change", gpu_next)),
            r(0xE713, "Add an app, default graphics settings", "In Windows' own page", Link("ms-settings:display-advancedgraphics")),
        ]),
        sub("Focus", "System", vec![
            r(0xE5C4, "Do not disturb", "Notifications are silenced; they still go to the notification center", Toggle(dnd, set_dnd)),
            r(0xE823, "Start a focus session", "Focus sessions run in the Clock app", Link("ms-clock:")),
            r(0xEA8F, "Notifications", "Banners, sounds and priority", Sub("Notifications")),
        ]),
        sub("Screen and sleep", "Power & battery", {
            let mut v = vec![
                r(0xE7F4, "When plugged in, turn off my screen after", "", Choice(screen_ac, screen_acs, set_screen_ac)),
                r(0xE708, "When plugged in, put my device to sleep after", "", Choice(sleep_ac, sleep_acs, set_sleep_ac)),
            ];
            if has_battery() {
                v.push(r(0xE7F4, "On battery power, turn off my screen after", "", Choice(screen_dc, screen_dcs, set_screen_dc)));
                v.push(r(0xE708, "On battery power, put my device to sleep after", "", Choice(sleep_dc, sleep_dcs, set_sleep_dc)));
            }
            v
        }),
        sub("Lid & power button controls", "Power & battery", {
            let mut v = vec![
                r(0xE7E8, "Pressing the power button will make my PC", "", Choice(power_button, power_buttons, set_power_button)),
                r(0xE708, "Pressing the sleep button will make my PC", "", Choice(sleep_button, sleep_buttons, set_sleep_button)),
            ];
            if has_battery() {
                v.push(r(0xE762, "Closing the lid will make my PC", "", Choice(lid_action, lid_actions, set_lid_action)));
            }
            v
        }),
        sub("Share", "System", vec![
            r(0xE72D, "Nearby sharing", "Discoverable as this PC's name", Choice(near_share, near_shares, set_near_share)),
        ]),
        sub("Recovery", "System", vec![
            r(0xE777, "Reset this PC", "Choose to keep or remove your personal files, then reinstall Windows", Link("ms-settings:recovery")),
            r(0, "", "", Actions(recovery_items, "Restart now", advanced_startup)),
        ]),
        sub("Remote Desktop", "System", vec![
            r(0xE8AF, "Remote Desktop", "Connect to and use this PC from another device (changing it needs an administrator)", State(remote_desktop, "ms-settings:remotedesktop")),
        ]),
        sub("Cameras", "Bluetooth & devices", vec![
            r(0, "Connected cameras", "", Header),
            r(0, "", "", Actions(cameras, "Settings", camera_settings)),
        ]),
        sub("AutoPlay", "Bluetooth & devices", vec![
            r(0xEC31, "Use AutoPlay for all media and devices", "", Toggle(autoplay, set_autoplay)),
        ]),
        sub("USB", "Bluetooth & devices", vec![
            r(0xE88E, "Connection notifications", "Notify me if there are issues connecting to USB devices", Toggle(usb_notify, set_usb_notify)),
        ]),
        sub("Ethernet", "Network & internet", vec![
            r(0xE839, "Properties", "", Facts(ethernet_facts)),
        ]),
        sub("Volume mixer", "Sound", vec![
            r(0, "System", "", Header),
            r(0xE767, "Volume", "", Slider(volume, set_volume)),
            r(0, "Apps", "", Header),
            r(0, "", "", Volumes(crate::qsx::mixer, crate::qsx::set_app_volume)),
            r(0xE713, "App input and output devices", "In Windows' own page", Link("ms-settings:apps-volume")),
        ]),
        sub("Mobile hotspot", "Network & internet", vec![
            r(0xE704, "Mobile hotspot", "Share my internet connection with other devices", Toggle(hotspot_on, set_hotspot)),
            r(0xE946, "Properties", "", Facts(hotspot_facts)),
        ]),
        sub("Passkeys", "Accounts", vec![
            r(0, "Saved passkeys", "", Header),
            r(0xE8D7, "Passkeys on this device", "Sign in to these sites with your face, fingerprint or PIN", Facts(passkeys)),
        ]),
        sub("Resume", "Apps", vec![
            r(0xEA6C, "Resume", "Continue what you were doing on your phone on this PC", Toggle(resume_on, set_resume_on)),
            r(0, "Apps", "", Header),
            r(0xE753, "OneDrive", "Files you opened on your phone", Toggle(resume_onedrive, set_resume_onedrive)),
            r(0xE8BD, "WhatsApp", "", Toggle(resume_whatsapp, set_resume_whatsapp)),
            r(0xE8D6, "Spotify", "", Toggle(resume_spotify, set_resume_spotify)),
        ]),
        sub("Mobile devices", "Bluetooth & devices", vec![
            r(0xE1C9, "Allow this PC to access your mobile devices", "Use your phone's camera, files and messages here", Toggle(mobile_access, set_mobile_access)),
            r(0xE8EA, "Phone Link", "Show my phone's notifications, messages and calls", Toggle(phone_link, set_phone_link)),
        ]),
        sub("Voice access", "Accessibility", vec![
            r(0xE720, "Voice access", "Control your PC and write text with your voice", Toggle(voice_access, set_voice_access)),
            r(0xE765, "Voice typing", "Windows logo key + H in any text box", Sub("Typing")),
        ]),
        sub("Text input", "Personalization", vec![
            r(0xE765, "Touch keyboard and typing", "Suggestions, autocorrect, touch keyboard", Sub("Typing")),
            r(0xF2B7, "Input method editor", "Languages and keyboards", Sub("Language & region")),
            r(0xE76E, "Emoji and more", "Windows logo key + period (.)", Info(|| "Emoji, GIFs, kaomoji, symbols and clipboard history".into())),
        ]),
        sub("Family", "Accounts", vec![
            r(0xE716, "Your family", "Add members, set screen time, content filters and spending limits", Link("https://family.microsoft.com")),
            r(0xE77B, "Other users", "Accounts on this PC", Sub("Other users")),
        ]),
        sub("Data usage", "Network & internet", vec![
            r(0, "Usage since start-up", "", Header),
            r(0xEB05, "Adapters", "", Facts(data_usage)),
        ]),
        sub("Video playback", "Apps", vec![
            r(0xE116, "Process video automatically to enhance it", "Depends on your device hardware", Toggle(video_enhance, set_video_enhance)),
            r(0xE83F, "Play video at a lower resolution when on battery", "Saves battery", Toggle(video_lowres, set_video_lowres)),
            r(0xE7F4, "HDR", "Stream HDR video and change display settings", Sub("Display")),
        ]),
        sub("Windows backup", "Accounts", vec![
            r(0xE753, "OneDrive folder syncing", "", Facts(backup_facts)),
        ]),
        sub("Cleanup recommendations", "Storage", vec![
            r(0xE9F5, "What can be removed", "", Facts(cleanup_facts)),
            r(0, "Large files in Downloads", "", Header),
            r(0, "", "", Actions(big_downloads, "Show", |_, n| unsafe {
                let p = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Downloads").join(n);
                windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("explorer.exe"), &HSTRING::from(format!("/select,\"{}\"", p.display())), PCWSTR::null(), SW_SHOWNORMAL);
            })),
        ]),
        sub("Offline maps", "Apps", vec![
            r(0xE826, "Maps", "", Facts(maps_facts)),
        ]),
        sub("Find my device", "Privacy & security", vec![
            r(0xE707, "Find my device", "", Info(find_my_device)),
            r(0xE81D, "Location", "Find my device needs location to be on", Sub("Location")),
        ]),
        sub("Dial-up", "Network & internet", vec![
            r(0, "Connections", "", Header),
            r(0, "", "", Actions(dialup_list, "Connect", vpn_connect)),
            r(0xE710, "Set up a new connection", "In Windows' own page", Link("ms-settings:network-dialup")),
        ]),
        sub("Device usage", "Personalization", vec![
            r(0, "Select all the ways you plan to use your device", "", Header),
            r(0xE7FC, "Gaming", "Play and stream games", Toggle(use_gaming, set_use_gaming)),
            r(0xE716, "Family", "Connect with family, edit safety settings and keep everyone's devices up to date", Toggle(use_family, set_use_family)),
            r(0xE790, "Creativity", "Bring ideas to life and create content", Toggle(use_creative, set_use_creative)),
            r(0xE7BE, "School", "Take notes, write papers and collaborate on projects", Toggle(use_school, set_use_school)),
            r(0xE8B2, "Entertainment", "Stream videos, browse the web and use social media", Toggle(use_fun, set_use_fun)),
            r(0xE821, "Business", "Track expenses and manage customers", Toggle(use_business, set_use_business)),
        ]),
        sub("VPN", "Network & internet", vec![
            r(0, "VPN connections", "", Header),
            r(0, "", "", Actions(vpn_list, "Connect", vpn_connect)),
            r(0xE710, "Add VPN", "In Windows' own page", Link("ms-settings:network-vpn")),
        ]),
        sub("Proxy", "Network & internet", vec![
            r(0, "Manual proxy setup", "", Header),
            r(0xE968, "Use a proxy server", "", Toggle(proxy_on, set_proxy_on)),
            r(0xE968, "Proxy address", "", Info(proxy_server)),
            r(0, "Automatic proxy setup", "", Header),
            r(0xE968, "Setup script", "", Info(proxy_script)),
            r(0xE713, "Edit the addresses", "In Windows' own page", Link("ms-settings:network-proxy")),
        ]),
        sub("Advanced network settings", "Network & internet", vec![
            r(0, "Network adapters", "", Header),
            r(0, "", "", Actions(adapter_list, "Properties", open_adapters)),
            r(0xE72C, "Network reset", "Reset all network adapters to factory settings (in Windows' own page)", Link("ms-settings:network-status")),
        ]),
        sub("Known networks", "Wi-Fi", vec![
            r(0, "", "", Actions(known_networks, "Forget", forget_network)),
        ]),
        sub("Wi-Fi properties", "Wi-Fi", vec![
            r(0xE701, "Properties", "", Facts(wifi_facts)),
        ]),
        sub("Taskbar icons", "Taskbar", vec![
            r(0, "Other system tray icons", "", Header),
            r(0, "", "", Senders(tray_icons, set_tray_icon)),
        ]),
        sub("Fonts", "Personalization", vec![
            r(0, "Available fonts", "", Header),
            r(0, "", "", Actions(font_list, "Open", open_font)),
        ]),
        sub("Notifications permission", "Privacy & security", vec![
            r(0xE72E, "Notification access", "Let apps access your notifications", Toggle(c_notif, set_c_notif)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_notif, set_consent_app)),
        ]),
        sub("Account info", "Privacy & security", vec![
            r(0xE72E, "Account information access", "Let apps access your account info", Toggle(c_account, set_c_account)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_account, set_consent_app)),
        ]),
        sub("Contacts", "Privacy & security", vec![
            r(0xE72E, "Contacts access", "Let apps access your contacts", Toggle(c_contacts, set_c_contacts)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_contacts, set_consent_app)),
        ]),
        sub("Calendar", "Privacy & security", vec![
            r(0xE72E, "Calendar access", "Let apps access your calendar", Toggle(c_calendar, set_c_calendar)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_calendar, set_consent_app)),
        ]),
        sub("Phone calls", "Privacy & security", vec![
            r(0xE72E, "Phone call access", "Let apps make phone calls", Toggle(c_calls, set_c_calls)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_calls, set_consent_app)),
        ]),
        sub("Call history", "Privacy & security", vec![
            r(0xE72E, "Call history access", "Let apps access your call history", Toggle(c_history, set_c_history)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_history, set_consent_app)),
        ]),
        sub("Email", "Privacy & security", vec![
            r(0xE72E, "Email access", "Let apps access your email", Toggle(c_email, set_c_email)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_email, set_consent_app)),
        ]),
        sub("Tasks", "Privacy & security", vec![
            r(0xE72E, "Tasks access", "Let apps access your tasks", Toggle(c_tasks, set_c_tasks)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_tasks, set_consent_app)),
        ]),
        sub("Messaging", "Privacy & security", vec![
            r(0xE72E, "Messaging access", "Let apps read or send messages", Toggle(c_messaging, set_c_messaging)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_messaging, set_consent_app)),
        ]),
        sub("Radios", "Privacy & security", vec![
            r(0xE72E, "Radio control access", "Let apps control device radios", Toggle(c_radios, set_c_radios)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_radios, set_consent_app)),
        ]),
        sub("Other devices", "Privacy & security", vec![
            r(0xE72E, "Communicate with unpaired devices", "Let apps automatically share and sync info with wireless devices", Toggle(c_otherdev, set_c_otherdev)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_otherdev, set_consent_app)),
        ]),
        sub("App diagnostics", "Privacy & security", vec![
            r(0xE72E, "App diagnostic access", "Let apps access diagnostic info about your other apps", Toggle(c_diag, set_c_diag)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_diag, set_consent_app)),
        ]),
        sub("Documents", "Privacy & security", vec![
            r(0xE72E, "Documents library access", "Let apps access your documents library", Toggle(c_docs, set_c_docs)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_docs, set_consent_app)),
        ]),
        sub("Downloads folder", "Privacy & security", vec![
            r(0xE72E, "Downloads folder access", "Let apps access your downloads folder", Toggle(c_downloads, set_c_downloads)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_downloads, set_consent_app)),
        ]),
        sub("Music library", "Privacy & security", vec![
            r(0xE72E, "Music library access", "Let apps access your music library", Toggle(c_music, set_c_music)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_music, set_consent_app)),
        ]),
        sub("Pictures", "Privacy & security", vec![
            r(0xE72E, "Pictures library access", "Let apps access your pictures library", Toggle(c_pics, set_c_pics)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_pics, set_consent_app)),
        ]),
        sub("Videos", "Privacy & security", vec![
            r(0xE72E, "Videos library access", "Let apps access your videos library", Toggle(c_videos, set_c_videos)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_videos, set_consent_app)),
        ]),
        sub("File system", "Privacy & security", vec![
            r(0xE72E, "File system access", "Let apps access your file system", Toggle(c_fs, set_c_fs)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_fs, set_consent_app)),
        ]),
        sub("Screenshot borders", "Privacy & security", vec![
            r(0xE72E, "Screenshot border settings", "Let apps turn off the screenshot border", Toggle(c_borders, set_c_borders)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_borders, set_consent_app)),
        ]),
        sub("Screenshots and apps", "Privacy & security", vec![
            r(0xE72E, "Screenshot access", "Let apps take screenshots of various windows or displays", Toggle(c_shots, set_c_shots)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_shots, set_consent_app)),
        ]),
        sub("Text and image generation", "Privacy & security", vec![
            r(0xE72E, "Text and image generation access", "Let apps use generative AI in Windows", Toggle(c_genai, set_c_genai)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_genai, set_consent_app)),
        ]),
        sub("Presence sensing", "Privacy & security", vec![
            r(0xE72E, "Presence sensor access", "Let apps access presence sensing", Toggle(c_presence, set_c_presence)),
            r(0, "Apps", "", Header),
            r(0, "", "", Senders(apps_presence, set_consent_app)),
        ]),
        sub("Windows Security", "Privacy & security", vec![
            r(0xEA18, "Protection areas", "", Facts(security_facts)),
            r(0xE713, "Open Windows Security", "Scans, firewall rules and protection history", Link("windowsdefender:")),
        ]),
        sub("Inking & typing personalization", "Privacy & security", vec![
            r(0xEADF, "Custom inking and typing dictionary", "Use your typing history and handwriting patterns to create a custom dictionary", Toggle(ink_personal, set_ink_personal)),
        ]),
        sub("Diagnostics & feedback", "Privacy & security", vec![
            r(0xE9D9, "Diagnostic data", "Set by Windows or your organization", Info(diag_level)),
            r(0xE9D9, "Tailored experiences", "Let Microsoft use your diagnostic data for personalized tips, ads, and recommendations", Toggle(tailored, set_tailored)),
            r(0xED15, "Feedback frequency", "Windows should ask for my feedback", Choice(feedback_freq, feedback_freqs, set_feedback_freq)),
        ]),
        sub("Search permissions", "Privacy & security", vec![
            r(0xE773, "SafeSearch", "Filter adult content in web previews", Choice(safe_search, safe_searches, set_safe_search)),
            r(0, "Cloud content search", "", Header),
            r(0xE753, "Microsoft account", "Search your Microsoft account content", Toggle(msa_cloud, set_msa_cloud)),
            r(0xE821, "Work or school account", "Search your work or school content", Toggle(aad_cloud, set_aad_cloud)),
            r(0, "History", "", Header),
            r(0xE81C, "Search history on this device", "Improve results using your search history on this device", Toggle(search_history, set_search_history)),
        ]),
        sub("For developers", "Privacy & security", vec![
            r(0xEC7A, "Developer Mode", "Install apps from any source, including loose files (needs an administrator to change)", State(dev_mode, "ms-settings:developers")),
            r(0, "File Explorer", "", Header),
            r(0xE8E5, "Show file extensions", "", Toggle(show_ext, set_show_ext)),
            r(0xE8E5, "Show hidden and system files", "", Toggle(show_hidden, set_show_hidden)),
            r(0xE8E5, "Show full path in title bar", "", Toggle(show_full_path, set_show_full_path)),
        ]),
        sub("Email & accounts", "Accounts", vec![
            r(0, "Accounts used by other apps", "", Header),
            r(0, "", "", Actions(ms_accounts, "Manage", manage_account)),
        ]),
        sub("Other users", "Accounts", vec![
            r(0, "Accounts on this PC", "", Header),
            r(0, "", "", Actions(local_users, "Manage", manage_users)),
        ]),
        sub("Access work or school", "Accounts", vec![
            r(0, "", "", Actions(work_accounts, "Manage", manage_users)),
        ]),
        sub("Speech", "Time & language", vec![
            r(0, "Installed voices", "", Header),
            r(0xEFA9, "Voices", "", Facts(voices)),
        ]),
        sub("Captures", "Gaming", vec![
            r(0xF7EE, "Captures location", "", Info(capture_folder)),
            r(0xE7FC, "Record what happened", "Record the last few minutes in the background", Toggle(cap_background, set_cap_background)),
            r(0xE767, "Capture audio when recording a game", "", Toggle(cap_audio, set_cap_audio)),
            r(0xE962, "Capture mouse cursor when recording a game", "", Toggle(cap_cursor, set_cap_cursor)),
        ]),
        sub("Text cursor", "Accessibility", vec![
            r(0xE330, "Text cursor thickness", "", Choice(caret_width, caret_widths, set_caret_width)),
        ]),
        sub("Storage Sense", "Storage", vec![
            r(0xEDA2, "Run Storage Sense", "", Choice(sense_run, sense_runs, set_sense_run)),
            r(0xE74D, "Delete temporary files that my apps aren't using", "", Toggle(sense_temp, set_sense_temp)),
            r(0xE74D, "Delete files in my recycle bin if they have been there for over", "", Choice(sense_bin, sense_bins, set_sense_bin)),
            r(0xE896, "Delete files in my Downloads folder if they haven't been opened for more than", "", Choice(sense_downloads, sense_downloadss, set_sense_downloads)),
        ]),
        sub("Activation", "About", vec![
            r(0xE946, "Windows", "", Facts(activation_facts)),
            r(0xE713, "Activation state, change product key", "In Windows' own page", Link("ms-settings:activation")),
        ]),
        sub("Dynamic Lighting", "Personalization", vec![
            r(0xF431, "Use Dynamic Lighting on my devices", "", Toggle(dyn_light, set_dyn_light)),
            r(0xF431, "Compatible apps in the foreground always control lighting", "", Toggle(dyn_light_apps, set_dyn_light_apps)),
        ]),
        sub("Touch keyboard", "Typing", vec![
            r(0xE767, "Play key sounds as I type", "", Toggle(tk_sound, set_tk_sound)),
            r(0xE765, "Capitalize the first letter of each sentence", "", Toggle(tk_caps, set_tk_caps)),
            r(0xE765, "Use all uppercase letters when I double-tap Shift", "", Toggle(tk_capslock, set_tk_capslock)),
            r(0xE765, "Add a period after I double-tap the Spacebar", "", Toggle(tk_space, set_tk_space)),
        ]),
        sub("Energy recommendations", "Power & battery", vec![
            r(0xE8BE, "Recommendations", "", Facts(energy_facts)),
            r(0, "", "", Actions(energy_item, "Apply all", energy_apply)),
        ]),
        sub("Troubleshoot", "System", vec![
            r(0, "", "", Actions(troubleshooters, "Run", run_troubleshooter)),
        ]),
        sub("Airplane mode", "Network & internet", vec![
            r(0xE709, "Airplane mode", "Stop all wireless communication", Toggle(airplane, crate::qsx::set_airplane)),
            r(0, "Wireless devices", "", Header),
            r(0xE701, "Wi-Fi", "", Toggle(wifi, set_wifi)),
            r(0xE702, "Bluetooth", "", Toggle(bluetooth, set_bluetooth)),
        ]),
        sub("Multitasking", "System", vec![
            r(0xE7C4, "Snap windows", "Automatically resize and arrange windows on your screen", Toggle(snap_on, set_snap)),
            r(0, "Show snap layouts when I hover over a window's maximize button", "", Toggle(snap_flyout, set_snap_flyout)),
            r(0, "Show snap layouts when I drag a window to the top of my screen", "", Toggle(snap_bar, set_snap_bar)),
            r(0, "When I snap a window, suggest what I can snap next to it", "", Toggle(snap_suggest, set_snap_suggest)),
            r(0xE8AB, "Show tabs from apps when snapping or pressing Alt+Tab", "", Choice(alt_tab, alt_tabs, set_alt_tab)),
            r(0xE7C4, "Title bar window shake", "When I grab a window's title bar and shake it, minimize all other windows", Toggle(shake, set_shake)),
            r(0, "Desktops", "", Header),
            r(0xE7C4, "On the taskbar, show all the open windows", "", Choice(vd_taskbar, vd_taskbars, set_vd_taskbar)),
            r(0xE7C4, "Show all open windows when I press Alt+Tab", "", Choice(vd_alttab, vd_alttabs, set_vd_alttab)),
        ]),
        sub("Magnifier", "Accessibility", vec![
            r(0xE8A3, "Magnifier", "Windows logo key + Plus to turn on, Windows logo key + Esc to turn off", Toggle(magnifier, set_magnifier)),
            r(0xE8A3, "Zoom level", "", Choice(mag_level, mag_levels, set_mag_level)),
            r(0xE8A3, "Zoom increment", "", Choice(mag_step, mag_steps, set_mag_step)),
            r(0xE8A3, "View", "", Choice(mag_view, mag_views, set_mag_view)),
        ]),
        sub("Color filters", "Accessibility", vec![
            r(0xE790, "Color filters", "Make photos and colors easier to see", Toggle(color_filters, set_color_filters)),
            r(0xE790, "Filter type", "", Choice(filter_type, filter_types, set_filter_type)),
        ]),
        sub("Narrator", "Accessibility", vec![
            r(0xED4D, "Narrator", "A screen reader that describes what's on your screen", Toggle(narrator, set_narrator)),
            r(0xED4D, "Voice speed", "", Choice(narrator_speed, narrator_speeds, set_narrator_speed)),
            r(0xE713, "Voice, verbosity and braille", "In Windows' own page", Link("ms-settings:easeofaccess-narrator")),
        ]),
        sub("Audio", "Accessibility", vec![
            r(0xE767, "Mono audio", "Combine left and right audio channels into one", Toggle(mono, set_mono)),
        ]),
        sub("Keyboard", "Accessibility", vec![
            r(0xE765, "Sticky keys", "Press one key at a time for keyboard shortcuts", Toggle(sticky, set_sticky)),
            r(0xE765, "Filter keys", "Ignore brief or repeated keystrokes", Toggle(filter_keys, set_filter_keys)),
            r(0xE765, "Toggle keys", "Play a sound when you press Caps lock, Num lock, or Scroll lock", Toggle(toggle_keys, set_toggle_keys)),
            r(0xE765, "Underline access keys", "Access keys are underlined even when not holding Alt", Toggle(access_keys, set_access_keys)),
            r(0xE722, "Use the Print screen key to open screen capture", "", Toggle(prtscn_snip, set_prtscn_snip)),
            r(0, "", "", Actions(osk_items, "Open", open_osk)),
        ]),
        sub("Camera", "Privacy & security", vec![
            r(0xE722, "Camera access", "Let apps use your camera", Toggle(camera_ok, set_camera_ok)),
            r(0, "Let apps access your camera", "", Header),
            r(0, "", "", Senders(cam_apps, set_consent_app)),
        ]),
        sub("Microphone", "Privacy & security", vec![
            r(0xE720, "Microphone access", "Let apps use your microphone", Toggle(mic_ok, set_mic_ok)),
            r(0, "Let apps access your microphone", "", Header),
            r(0, "", "", Senders(mic_apps, set_consent_app)),
        ]),
        sub("Location", "Privacy & security", vec![
            r(0xE81D, "Location services", "Let apps use your location (the device-wide switch needs an administrator)", Toggle(location_ok, set_location_ok)),
            r(0, "Let apps access your location", "", Header),
            r(0, "", "", Senders(loc_apps, set_consent_app)),
        ]),
        sub("Text size", "Accessibility", vec![
            r(0xE8E9, "Text size", "Apps pick a new size up when they next start", Choice(text_size, text_sizes, set_text_size)),
        ]),
        sub("Mouse pointer and touch", "Accessibility", vec![
            r(0xE7C9, "Mouse pointer style", "", Info(pointer_style)),
            r(0xE7C9, "Size", "1 to 15", Slider(pointer_size_v, set_pointer_size_v)),
            r(0xE713, "Change the pointer's style", "In Windows' own page", Link("ms-settings:easeofaccess-mousepointer")),
        ]),
        sub("Contrast themes", "Accessibility", vec![
            r(0xE7A1, "Contrast themes", "Left Alt + left Shift + Print Screen turns them on and off", Info(contrast_text)),
            r(0xE713, "Choose a contrast theme", "In Windows' own page", Link("ms-settings:easeofaccess-highcontrast")),
        ]),
        sub("Captions", "Accessibility", vec![
            r(0xE7F0, "Caption style", "", Info(caption_style)),
            r(0xE713, "Edit caption styles", "In Windows' own page. Live captions: Windows logo key + Ctrl + L", Link("ms-settings:easeofaccess-closedcaptioning")),
        ]),
        sub("Mouse keys", "Accessibility", vec![
            r(0xE962, "Mouse keys", "Use the number pad to move the mouse pointer", Toggle(mouse_keys, set_mouse_keys)),
            r(0xE962, "Only use mouse keys when Num lock is on", "", Toggle(mk_numlock, set_mk_numlock)),
            r(0xE962, "Hold the Ctrl key to speed up and the Shift key to slow down", "", Toggle(mk_ctrl, set_mk_ctrl)),
            r(0xE962, "Mouse keys speed", "", Slider(mk_speed, set_mk_speed)),
            r(0xE962, "Mouse keys acceleration", "", Slider(mk_accel, set_mk_accel)),
        ]),
        sub("Speech privacy", "Privacy & security", vec![
            r(0xEFA9, "Online speech recognition", "Use your voice for apps that use Microsoft's online speech recognition technology", Toggle(online_speech, set_online_speech)),
            r(0xE720, "Voice activation", "Let apps access voice activation services", Toggle(voice_apps, set_voice_apps)),
        ]),
        sub("Update history", "Windows Update", vec![
            r(0, "", "", Actions(update_history, "Copy name", copy_update)),
            r(0xE74D, "Uninstall updates", "In Windows' own page (needs an administrator)", Link("ms-settings:windowsupdate-history")),
        ]),
        sub("Advanced options", "Windows Update", vec![
            r(0xE789, "Receive updates for other Microsoft products", "", State(other_products, "ms-settings:windowsupdate-options")),
            r(0xE789, "Get me up to date", "Restart as soon as possible to finish updating", State(restart_notify, "ms-settings:windowsupdate-options")),
            r(0xE789, "Download updates over metered connections", "", State(metered_updates, "ms-settings:windowsupdate-options")),
            r(0xE823, "Active hours", "", Info(active_hours)),
            r(0xE713, "Delivery Optimization, optional updates, recovery", "In Windows' own page (machine-wide settings need an administrator)", Link("ms-settings:windowsupdate-options")),
        ]),
        sub("Visual effects", "Accessibility", vec![
            r(0xE794, "Always show scrollbars", "", Toggle(always_scrollbars, set_always_scrollbars)),
            r(0xE794, "Transparency effects", "", Toggle(transparency, set_transparency)),
            r(0xE794, "Animation effects", "", Toggle(animations, set_animations)),
            r(0xEA8F, "Dismiss notifications after this amount of time", "", Choice(toast_time, toast_times, set_toast_time)),
        ]),
        sub("Recommendations & offers", "Privacy & security", vec![
            r(0xE72E, "Personalized offers", "Let apps show me personalized ads by using my advertising ID", Toggle(ad_id, set_ad_id)),
            r(0xE774, "Language list", "Let websites show me locally relevant content by accessing my language list", Toggle(local_content, set_local_content)),
            r(0xE8FC, "Improve Start and search results", "Let Windows improve Start and search results by tracking app launches", Toggle(track_launches, set_track_launches)),
            r(0xE713, "Settings suggestions", "Show me suggested content in the Settings app", Toggle(settings_tips, set_settings_tips)),
        ]),
        sub("Clipboard", "System", vec![
            r(0xE77F, "Clipboard history", "Save multiple items to the clipboard to use later. Press Windows logo key + V to view your clipboard history and paste from it.", Toggle(clip_history, set_clip_history)),
            r(0, "", "", Actions(clip_items, "Clear", clear_clip)),
            r(0xE72E, "Sync across your devices", "On Windows' own Clipboard page", Link("ms-settings:clipboard")),
        ]),
        sub("Taskbar", "Personalization", vec![
            r(0, "Taskbar items", "", Header),
            r(0xE721, "Search", "", Choice(tb_search, tb_searches, set_tb_search)),
            r(0xE7C4, "Task view", "", Toggle(tb_taskview, set_tb_taskview)),
            r(0, "Taskbar behaviors", "", Header),
            r(0xE8E3, "Taskbar alignment", "", Choice(tb_align, tb_aligns, set_tb_align)),
            r(0xE8FD, "Combine taskbar buttons and hide labels", "", Choice(tb_combine, tb_combines, set_tb_combine)),
            r(0xE823, "Show seconds in system tray clock", "Uses more power", Toggle(tb_seconds, set_tb_seconds)),
            r(0xE71A, "End task", "Enable end task in taskbar by right click", Toggle(tb_endtask, set_tb_endtask)),
            r(0xE7C4, "Automatically hide the taskbar", "", Toggle(crate::taskbar::auto_hide, crate::taskbar::set_auto_hide)),
            r(0xE9F5, "Other system tray icons", "Show or hide additional system tray icons", Sub("Taskbar icons")),
            r(0xE946, "Windows' own taskbar settings", "Search, Task view, alignment, seconds and auto-hide are followed by the Rust taskbar", Link("ms-settings:taskbar")),
        ]),
        sub("Start", "Personalization", vec![
            r(0xE8FC, "Layout", "", Choice(start_layout, start_layouts, set_start_layout)),
            r(0xE71D, "Show most used apps", "", Toggle(start_most_used, set_start_most_used)),
            r(0xE823, "Show recommended files in Start, recent files in File Explorer, and items in Jump Lists", "", Toggle(start_recent, set_start_recent)),
            r(0xE82F, "Show recommendations for tips, shortcuts, new apps, and more", "", Toggle(start_tips, set_start_tips)),
            r(0xE77B, "Show account-related notifications", "", Toggle(start_account, set_start_account)),
            r(0xE8B7, "Folders", "These folders appear on Start next to the Power button", Link("ms-settings:personalization-start-places")),
        ]),
        sub("Installed apps", "Apps", vec![
            r(0xE8CB, "Sort by", "", Choice(apps_sort, apps_sorts, set_apps_sort)),
            r(0, "", "", Actions(installed_apps, "Uninstall", uninstall_app)),
        ]),
        sub("Default apps", "Apps", vec![
            r(0, "Defaults by file and link type", "", Header),
            r(0xE7AC, "Opens with", "", Facts(default_apps)),
            r(0xE713, "Change a default", "In Windows' own page (Windows protects these choices)", Link("ms-settings:defaultapps")),
        ]),
        sub("Advanced app settings", "Apps", vec![
            r(0xE756, "App execution aliases", "Names you can type to start Store apps", Facts(app_aliases)),
            r(0xE713, "Choose where to get apps, archive apps", "In Windows' own page", Link("ms-settings:appsfeatures-app")),
        ]),
        sub("Check for updates", "Windows Update", vec![
            r(0, "", "", Actions(wu_available, "Check again", wu_check)),
            r(0, "", "", Actions(wu_install_item, "Install", wu_install)),
        ]),
        sub("Windows Insider Program", "Windows Update", vec![
            r(0xF1AD, "Insider channel", "", Info(insider)),
            r(0xE713, "Join or leave", "In Windows' own page", Link("ms-settings:windowsinsider")),
        ]),
        sub("Startup", "Apps", vec![
            r(0, "Startup apps", "", Header),
            r(0, "", "", Senders(startup_apps, set_startup)),
        ]),
        top("Apps", 0xE71D, vec![
            r(0xE71D, "Installed apps", "Uninstall and manage apps on your PC", Sub("Installed apps")),
            r(SET | 0xEFDD, "Advanced app settings", "Choose where to get apps, archive apps, uninstall updates", Sub("Advanced app settings")),
            r(SET | 0xE42A, "Default apps", "Defaults for file and link types, other defaults", Sub("Default apps")),
            r(0xF277, "Actions", "Windows can recommend actions from these apps.", Link("ms-settings:appactions")),
            r(0xE826, "Offline maps", "Downloads, storage location, map updates", Link("ms-settings:maps")),
            r(SET | 0xF6AB, "Apps for websites", "Websites that can open in an app instead of a browser", Link("ms-settings:appsforwebsites")),
            r(0xE116, "Video playback", "Video adjustments, HDR streaming, battery options", Link("ms-settings:videoplayback")),
            r(0xE7B5, "Startup", "Apps that start automatically when you sign in", Sub("Startup")),
            r(0xEA6C, "Resume", "Continue work across devices", Link("ms-settings:crossdevice")),
        ]),
        top("Accounts", 0xE77B, vec![
            r(0, "", "", AccountHero(account_summary)),
            r(0, "Account settings", "", Header),
            r(0xE77B, "Your info", "Profile photo", Sub("Your info")),
            r(0xE192, "Sign-in options", "Windows Hello, security key, password, dynamic lock", Sub("Sign-in options")),
            r(0xE715, "Email & accounts", "Accounts used by email, calendar, and contacts", Sub("Email & accounts")),
            r(0xE716, "Family", "Manage your family group, edit account types and device permissions", Link("ms-settings:family-group")),
            r(0xE753, "Windows backup", "Back up your files, apps, preferences to restore them across devices", Link("ms-settings:backup")),
            r(0xE8FA, "Other users", "Device access, work or school users, kiosk assigned access", Sub("Other users")),
            r(0xE821, "Access work or school", "Organization resources like email, apps, and network", Sub("Access work or school")),
            r(0xE8D7, "Passkeys", "Use your face, fingerprint, or PIN to sign in to apps and websites", Link("ms-settings:savedpasskeys")),
        ]),
        top("Time & language", 0xE775, vec![
            r(0, "", "", TimeHero(time_summary)),
            r(SET | 0xE4AB, "Date & time", "Time zones, automatic clock settings, calendar display", Sub("Date & time")),
            r(0xF2B7, "Language & region", "Windows display language, preferred languages, regional formats", Sub("Language & region")),
            r(0xE144, "Typing", "Touch keyboard, text suggestions, preferences", Sub("Typing")),
            r(0xEFA9, "Speech", "Speech language, speech recognition microphone setup, voices", Link("ms-settings:speech")),
        ]),
        top("Gaming", 0xE7FC, vec![
            r(SET | 0xF28C, "XBOX mode", "Enable, accessibility control hints", Link("ms-settings:gaming-fullscreenexperience")),
            r(SET | 0xF192, "Game Bar", "Controller and keyboard shortcuts", Sub("Game Bar")),
            r(0xF7EE, "Captures", "Save location, recording preferences", Sub("Captures")),
            r(0xE7FC, "Game Mode", "Optimize your PC for play", Sub("Game Mode")),
        ]),
        top("Accessibility", 0xE776, vec![
            r(0, "Vision", "", Header),
            r(0xE8E9, "Text size", "Text size that appears throughout Windows and your apps", Sub("Text size")),
            r(0xE794, "Visual effects", "Scroll bars, transparency, animations, notification timeout", Sub("Visual effects")),
            r(0xE7C9, "Mouse pointer and touch", "Mouse pointer color, size", Sub("Mouse pointer and touch")),
            r(SET | 0xE330, "Text cursor", "Appearance and thickness, text cursor indicator", Sub("Text cursor")),
            r(0xE8A3, "Magnifier", "Magnifier reading, zoom increment", Link("ms-settings:easeofaccess-magnifier")),
            r(0xE790, "Color filters", "Colorblindness filters, grayscale, inverted", Sub("Color filters")),
            r(0xE81E, "Screen tint", "Add a subtle color overlay to make your screen more comfortable to view", Link("ms-settings:easeofaccess-colorfilter")),
            r(0xE7A1, "Contrast themes", "Color themes for low vision, light sensitivity", Sub("Contrast themes")),
            r(0xED4D, "Narrator", "Voice, verbosity, keyboard, braille", Sub("Narrator")),
            r(0, "Hearing", "", Header),
            r(0xE767, "Audio", "Mono audio, audio notifications", Sub("Audio")),
            r(0xE7F0, "Captions", "Styles, live captions", Sub("Captions")),
            r(0, "Interaction", "", Header),
            r(0xE720, "Speech", "Voice access, voice typing, Windows Speech Recognition", Link("ms-settings:easeofaccess-speechrecognition")),
            r(0xE765, "Keyboard", "Sticky, Filter, and Toggle keys, on-screen keyboard", Sub("Keyboard")),
            r(0xE765, "Sticky keys", "Press one key at a time for keyboard shortcuts", Toggle(sticky, set_sticky)),
            r(0xE962, "Mouse", "Mouse keys, speed, acceleration", Sub("Mouse keys")),
            r(0xE7B3, "Eye control", "Eye tracker, text-to-speech", Link("ms-settings:easeofaccess-eyecontrol")),
        ]),
        top("Privacy & security", 0xE72E, vec![
            r(0, "Security", "", Section),
            r(0xEA18, "Windows Security", "Antivirus, browser, firewall, and network protection for your device", Sub("Windows Security")),
            r(0xE707, "Find my device", "Track your device if you think you've lost it", Link("ms-settings:findmydevice")),
            r(0, "Windows permissions", "", Section),
            r(0xE72E, "Recommendations & offers", "Advertising ID, personalized offers, local content, app launches, settings suggestions, productivity tools", Sub("Recommendations & offers")),
            r(0xEFA9, "Speech", "Online speech recognition for dictation and other voice-based interactions", Sub("Speech privacy")),
            r(0xEADF, "Inking & typing personalization", "Custom dictionary, words in your dictionary", Sub("Inking & typing personalization")),
            r(0xE9D9, "Diagnostics & feedback", "Diagnostic data, inking and typing data, feedback frequency", Sub("Diagnostics & feedback")),
            r(0xE773, "Search", "Search history, search apps, cloud content search, search indexing", Sub("Search permissions")),
            r(0xEC7A, "For developers", "These settings are intended for development use only", Sub("For developers")),
            r(0, "App permissions", "", Section),
            r(0xE81D, "Location", "", Sub("Location")),
            r(0xE722, "Camera", "", Sub("Camera")),
            r(0xE720, "Microphone", "", Sub("Microphone")),
            r(0xEA8F, "Notifications", "", Sub("Notifications permission")),
            r(0xE72E, "Account info", "", Sub("Account info")),
            r(0xE72E, "Contacts", "", Sub("Contacts")),
            r(0xE72E, "Calendar", "", Sub("Calendar")),
            r(0xE72E, "Phone calls", "", Sub("Phone calls")),
            r(0xE72E, "Call history", "", Sub("Call history")),
            r(0xE72E, "Email", "", Sub("Email")),
            r(0xE72E, "Tasks", "", Sub("Tasks")),
            r(0xE72E, "Messaging", "", Sub("Messaging")),
            r(0xE72E, "Radios", "", Sub("Radios")),
            r(0xE72E, "Other devices", "", Sub("Other devices")),
            r(0xE72E, "App diagnostics", "", Sub("App diagnostics")),
            r(0xE72E, "Documents", "", Sub("Documents")),
            r(0xE72E, "Downloads folder", "", Sub("Downloads folder")),
            r(0xE72E, "Music library", "", Sub("Music library")),
            r(0xE72E, "Pictures", "", Sub("Pictures")),
            r(0xE72E, "Videos", "", Sub("Videos")),
            r(0xE72E, "File system", "", Sub("File system")),
            r(0xE72E, "Screenshot borders", "", Sub("Screenshot borders")),
            r(0xE72E, "Screenshots and apps", "", Sub("Screenshots and apps")),
            r(0xE72E, "Text and image generation", "", Sub("Text and image generation")),
            r(0xE72E, "Presence sensing", "", Sub("Presence sensing")),
        ]),
        top("Windows Update", 0xE895, vec![
            r(0, "", "", UpdateHero(restart_pending)),
            r(0, "Available updates", "These updates will complete automatically.", When(restart_pending, "ms-settings:windowsupdate")),
            r(0xE895, "Check for updates", "Search for updates now and see what is waiting", Sub("Check for updates")),
            r(0, "More options", "", Header),
            // ponytail: read-only; this switch is a machine-wide setting that needs administrator rights to change
            r(0xE789, "Get the latest updates as soon as they're available", "Be among the first to get the latest non-security updates, fixes, and improvements as they roll out.", State(early_updates, "ms-settings:windowsupdate")),
            r(0xE769, "Pause updates", "", Info(pause_state)),
            r(0xE81C, "Update history", "", Sub("Update history")),
            r(0xE9F5, "Advanced options", "Delivery optimization, optional updates, active hours, other update settings", Sub("Advanced options")),
            r(0xF1AD, "Windows Insider Program", "Get preview builds of Windows to share feedback on new features and updates", Sub("Windows Insider Program")),
        ]),
    ]
}

// ======================= window (measured from Windows' Settings at 100%) =======================

const TOPBAR: f32 = 48.0;
const CARD_X: f32 = 325.0;
const NAV_Y: f32 = 143.0;

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Nav(usize),    // page index
    Row(usize),
    Back,
    Win(u8),       // minimise / maximise / close
    Result(usize), // page index of a search result
    Rename,
    Radio(usize, usize), // row, device
    Pair(usize),
    Mute,
    Sender(usize, usize), // row, app
    Sort,
    Copy(usize),
    Open(&'static str), // opens this page of Windows' own Settings
    Tile(usize),        // a device tile's Connect button
    Theme(usize),       // a theme thumbnail
    Vol(usize, usize),  // row, app: a volume mixer card's slider
    Drop(usize),        // an option in an open drop-down list
    DropOff,            // anywhere else while a drop-down is open: closes it
}

#[derive(Default)]
struct Ui {
    page: usize,
    hover: Option<Hit>,
    drag: Option<usize>,
    drag_vol: Option<(usize, usize)>, // a mixer slider being dragged: row, app
    drop: Option<(usize, Vec<String>, usize)>, // an open drop-down: its row, the options, the chosen one
    focus: Option<usize>, // the row the keyboard is on (drawn with Windows' focus ring; a click hides it)
    vals: Vec<Val>,
    query: String,
    hits: Vec<(D2D_RECT_F, Hit)>,
    gfx: Option<(ID2D1DCRenderTarget, IDWriteFactory)>,
}

thread_local! {
    static SCROLL: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
    /// Where a wheel turn is gliding to (None when still).
    static GOAL: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
    static UI: RefCell<Ui> = RefCell::new(Ui { page: 1, ..Default::default() });
    static PAGES: Vec<Page> = pages();
}

fn page_index(name: &str) -> Option<usize> {
    PAGES.with(|p| p.iter().position(|x| x.name == name))
}

/// Read every value on a page off the UI thread (some of Windows' answers take a moment).
fn load(page: usize) {
    std::thread::spawn(move || unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let kinds: Vec<Kind> = pages().get(page).map(|p| p.rows.iter().map(|r| r.kind).collect()).unwrap_or_default();
        let mut slow = String::new(); // which value took long to read, for %TEMP%\shellbits-settings-load.txt
        let mut index = 0;
        let vals = kinds
            .into_iter()
            .map(|k| {
                let t = std::time::Instant::now();
                if std::env::var("SHELLBITS_TRACE").is_ok() {
                    let _ = std::fs::write(std::env::temp_dir().join("shellbits-settings-row.txt"), format!("page {page} reading row {index}"));
                }
                let v = read_value(k);
                if t.elapsed().as_millis() > 300 {
                    slow += &format!("page {page} row {index}: {} ms

", t.elapsed().as_millis());
                }
                index += 1;
                v
            })
            .collect();
        if !slow.is_empty() {
            let _ = std::fs::write(std::env::temp_dir().join("shellbits-settings-load.txt"), slow);
        }
        *VALUES.lock().unwrap() = (page, vals);
        let _ = PostMessageW(Some(HWND(WIN.load(Relaxed) as *mut c_void)), WM_VALUES, WPARAM(0), LPARAM(0));
    });
}

fn read_value(k: Kind) -> Val {
    match k {
        Kind::Toggle(get, _) | Kind::ToggleMore(get, ..) => Val { on: get(), ..Default::default() },
        Kind::NetHero(get) => {
            let (s, on) = get();
            Val { s, on, ..Default::default() }
        }
        Kind::Slider(get, _) => Val { f: get(), on: std::ptr::fn_addr_eq(get, volume as fn() -> f32) && muted(), ..Default::default() },
        Kind::Info(get) | Kind::Choice(get, ..) | Kind::Flat(get, ..) => Val { s: get(), ..Default::default() },
        Kind::Big(get) => Val { s: get(), on: crate::quick::battery().is_some_and(|b| b.1), ..Default::default() },
        Kind::Note(shown, _) => Val { list: if shown() { vec![Default::default()] } else { Vec::new() }, ..Default::default() },
        Kind::State(get, _) | Kind::UpdateHero(get) => Val { on: get(), ..Default::default() },
        Kind::When(shown, _) => Val { list: if shown() { vec![Default::default()] } else { Vec::new() }, ..Default::default() },
        Kind::Tiles(get) | Kind::Facts(get) | Kind::ThemeHero(get) | Kind::TimeHero(get) | Kind::HomeHero(get) | Kind::AccountHero(get) => Val { list: get(), ..Default::default() },
        Kind::BtTiles(get) => Val { list: get().into_iter().map(|d| (d.name, String::new(), d.on, d.key)).collect(), ..Default::default() },
        // the volume (0-100) rides in the second field, the app's name is also its key
        Kind::Volumes(get, _) => Val { list: get().into_iter().map(|d| (d.name.clone(), d.signal.to_string(), false, d.name)).collect(), ..Default::default() },
        Kind::Actions(get, ..) => Val { list: get().into_iter().map(|d| (d.name, d.sub, d.on, d.key)).collect(), ..Default::default() },
        Kind::Senders(get, _) => Val {
            list: get().into_iter().map(|d| (d.name, if !d.sub.is_empty() { d.sub } else if d.on { "Banners, Sounds".to_string() } else { "Off".to_string() }, d.on, d.key)).collect(),
            ..Default::default()
        },
        Kind::Devices(get, _) => Val {
            list: get().into_iter().map(|d| { let (a, b) = split_device(&d.name); (a, b, d.on, d.key) }).collect(),
            ..Default::default()
        },
        _ => Val::default(),
    }
}

fn first_row_y(page: &Page) -> f32 {
    // measured on the real pages: where the first row sits depends on what the page opens with
    let first = page.rows.first().map(|r| r.kind);
    match (page.name, page.parent.is_empty(), first) {
        ("System", ..) => 223.0,
        (_, true, Some(Kind::NetHero(_) | Kind::BtTiles(_) | Kind::ThemeHero(_))) => 112.0,
        (_, true, Some(Kind::UpdateHero(_))) => 117.0,
        (_, true, Some(Kind::AccountHero(_))) => 110.0,
        (_, true, Some(Kind::Header | Kind::Section)) => 128.0,
        (_, true, _) => 117.0,
        (_, false, Some(Kind::Header)) => 123.0,
        (_, false, _) => 112.0,
    }
}

/// Top of every row (before scrolling) and the y just past the last one. Cards are ROW_H tall with a
/// 4px gap; a section title takes 16px at the top of a page and 56px between cards, as in Settings.
fn row_tops(kinds: &[Kind], counts: &[usize], start: f32) -> (Vec<f32>, f32) {
    let mut y = start;
    let mut tops = Vec::new();
    for (i, k) in kinds.iter().enumerate() {
        tops.push(y);
        y += row_height(*k, i, counts) + 4.0;
    }
    (tops, y)
}

/// A card is ROW_H; a device card adds 70 per device and a 50px "pair" line; a section title is
/// 12 at the top of a page and 52 between cards.
fn row_height(k: Kind, i: usize, counts: &[usize]) -> f32 {
    match k {
        Kind::Header if i == 0 => 12.0,
        Kind::Header => 52.0,
        Kind::Section if i == 0 => 12.0,
        Kind::Section => 36.0,
        Kind::Devices(..) => ROW_H + 70.0 * counts.get(i).copied().unwrap_or(0) as f32 + 50.0,
        Kind::SortBy => 37.0,
        Kind::HomeHero(_) => 77.0,
        Kind::AccountHero(_) => 84.0,
        Kind::UpdateHero(_) => 101.0,
        Kind::When(..) => if counts.get(i).copied().unwrap_or(0) > 0 { ROW_H } else { -4.0 },
        Kind::TimeHero(_) => 88.0,
        Kind::ThemeHero(_) => 237.0,
        Kind::BtTiles(_) => 227.0, // 221 tall, 10 of air to the bar below
        Kind::Center(..) => 41.0, // 35 tall, 10 of air below
        Kind::Space(h) => h - 4.0,
        Kind::NetHero(_) => 115.0,
        // tiles are 140 tall, three to a row, 14 apart; then 52 of air before the next card
        Kind::Tiles(_) => (counts.get(i).copied().unwrap_or(0).div_ceil(3) as f32 * 154.0 - 14.0).max(0.0) + 52.0,
        // 68 for the heading, then 25 per line (a line that wraps onto more lines is counted by its newlines), 20 of padding
        Kind::Facts(_) => ROW_H + 25.0 * counts.get(i).copied().unwrap_or(0) as f32 + 20.0,
        Kind::Big(_) => 95.0,
        Kind::Toggle(..) | Kind::ToggleMore(..) | Kind::State(..) => ROW_H + 0.67, // cards holding a switch are two thirds of a pixel taller in Settings
        Kind::Note(..) => if counts.get(i).copied().unwrap_or(0) > 0 { 52.0 } else { -4.0 },
        Kind::Senders(..) | Kind::Actions(..) | Kind::Volumes(..) => (73.0 * counts.get(i).copied().unwrap_or(0) as f32 - 5.0).max(0.0),
        _ => ROW_H,
    }
}

thread_local! {
    // theme wallpapers, shrunk: width, height, pixels
    static THUMBS: RefCell<std::collections::HashMap<String, (u32, u32, Vec<u8>)>> = RefCell::new(std::collections::HashMap::new());
}
thread_local! {
    static APP_ICONS: RefCell<std::collections::HashMap<String, Vec<u8>>> = RefCell::new(std::collections::HashMap::new());
}
thread_local! {
    // how many devices each row of the open page lists (0 for ordinary rows)
    static COUNTS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}
fn set_counts(vals: &[Val]) {
    COUNTS.with(|c| *c.borrow_mut() = vals.iter().map(|v| v.list.len() + v.list.iter().map(|l| l.1.matches('\n').count()).sum::<usize>()).collect());
}

/// The next row the keyboard can land on (things you can click), from `now` forward or back.
pub fn next_focus(rows: &[Kind], now: Option<usize>, back: bool) -> Option<usize> {
    let can = |k: &Kind| matches!(k, Kind::Link(_) | Kind::LinkBtn(..) | Kind::Expand(_) | Kind::Sub(_) | Kind::Choice(..) | Kind::Flat(..) | Kind::State(..) | Kind::Toggle(..) | Kind::ToggleMore(..));
    let idx: Vec<usize> = rows.iter().enumerate().filter(|(_, k)| can(k)).map(|(i, _)| i).collect();
    match (now, back) {
        (None, false) => idx.first().copied(),
        (None, true) => idx.last().copied(),
        (Some(n), false) => idx.iter().copied().find(|&i| i > n).or(Some(n)),
        (Some(n), true) => idx.iter().rev().copied().find(|&i| i < n).or(Some(n)),
    }
}

fn row_rect(page: &Page, i: usize, cw: f32) -> D2D_RECT_F {
    let kinds: Vec<Kind> = page.rows.iter().map(|r| r.kind).collect();
    let counts = COUNTS.with(|c| c.borrow().clone());
    let y = row_tops(&kinds, &counts, first_row_y(page)).0[i] - SCROLL.with(|s| s.get());
    rect(CARD_X, y, cw - 25.0, y + row_height(kinds[i], i, &counts))
}

fn slider_x(r: &D2D_RECT_F) -> (f32, f32) {
    (r.right - 216.0, r.right - 16.0)
}

/// Rows whose label or description contains the query: (page index, row label, page name).
fn search(pages: &[Page], q: &str) -> Vec<(usize, &'static str, &'static str)> {
    let q = q.to_lowercase();
    pages
        .iter()
        .enumerate()
        .flat_map(|(i, p)| p.rows.iter().filter(|r| r.label.to_lowercase().contains(&q) || r.sub.to_lowercase().contains(&q)).map(move |r| (i, r.label, p.name)))
        .take(8)
        .collect()
}

unsafe fn draw(ui: &mut Ui, cw: f32, ch: f32, maximized: bool) -> windows::core::Result<()> {
    let Some((rt, dw)) = ui.gfx.clone() else { return Ok(()) };
    let mut hits: Vec<(D2D_RECT_F, Hit)> = Vec::new();
    rt.Clear(Some(&rgb(0x202020)));
    rt.SetTransform(&windows_numerics::Matrix3x2::identity()); // a page entrance cut short must not leave its offset
    let b = |c: u32| rt.CreateSolidColorBrush(&rgb(c), None);
    let white = b(0xFFFFFF)?;
    let dim = b(0xCFCFCF)?;
    let accent = b(0x4CC2FF)?;
    let fmt = |size: f32, weight, family: PCWSTR| -> windows::core::Result<IDWriteTextFormat> {
        let t = dw.CreateTextFormat(family, None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))?;
        t.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        t.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(t)
    };
    let ui_font = w!("Segoe UI Variable Text");
    let (n, sb) = (DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD);
    let body = fmt(14.0, n, ui_font)?;
    let body_sb = fmt(14.0, sb, ui_font)?;
    let small = fmt(12.0, n, w!("Segoe UI Variable Small"))?; // the 12px optical size, a touch wider, as Settings uses
    let title = fmt(28.0, sb, w!("Segoe UI Variable Display"))?;
    let h20 = fmt(20.0, sb, w!("Segoe UI Variable Display"))?;
    let icons = fmt(16.0, n, w!("Segoe Fluent Icons"))?;
    icons.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
    let icons20 = fmt(20.0, n, w!("Segoe Fluent Icons"))?;
    icons20.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
    let icons10 = fmt(10.0, n, w!("Segoe Fluent Icons"))?;
    let icons12 = fmt(12.0, n, w!("Segoe Fluent Icons"))?;
    icons12.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
    icons10.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
    let right = fmt(14.0, n, ui_font)?;
    let header = fmt(14.0, sb, ui_font)?;
    let h18 = fmt(18.0, sb, w!("Segoe UI Variable Display"))?;
    let big = fmt(54.0, sb, w!("Segoe UI Variable Display"))?;
    let icons40 = fmt(40.0, n, w!("Segoe Fluent Icons"))?;
    right.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING)?;
    let txt = |s: &str, f: &IDWriteTextFormat, r: D2D_RECT_F, br: &ID2D1SolidColorBrush| rt.DrawText(&utf16(s), f, &r, br, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    let ico = |cp: u32, f: &IDWriteTextFormat, r: D2D_RECT_F, br: &ID2D1SolidColorBrush| rt.DrawText(&glyph(cp), f, &r, br, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    let hover = ui.hover;
    let set_face = settings_face(&dw);
    // a card's icon, 20px, centred in `r`
    let card_icon = |cp: u32, r: D2D_RECT_F| {
        let face = if cp & SET != 0 { set_face.as_ref() } else { None };
        let Some(face) = face else { return ico(cp & !SET, &icons20, r, &white) };
        let code = cp & !SET;
        let mut gi = 0u16;
        let mut fm = DWRITE_FONT_METRICS::default();
        face.GetMetrics(&mut fm);
        if face.GetGlyphIndices(&code, 1, &mut gi).is_err() || gi == 0 {
            return;
        }
        let size = 20.0f32;
        let (advance, offset) = (0.0f32, DWRITE_GLYPH_OFFSET::default());
        let run = DWRITE_GLYPH_RUN { fontFace: std::mem::ManuallyDrop::new(Some(face.clone())), fontEmSize: size, glyphCount: 1, glyphIndices: &gi, glyphAdvances: &advance, glyphOffsets: &offset, isSideways: false.into(), bidiLevel: 0 };
        let origin = windows_numerics::Vector2 { X: ((r.left + r.right) / 2.0 - size / 2.0).round(), Y: ((r.top + r.bottom) / 2.0 - size / 2.0).round() + size * fm.ascent as f32 / fm.designUnitsPerEm as f32 };
        rt.DrawGlyphRun(origin, &run, &white, DWRITE_MEASURING_MODE_NATURAL);
        drop(std::mem::ManuallyDrop::into_inner(run.fontFace));
    };

    PAGES.with(|pages| -> windows::core::Result<()> {
        let page = &pages[ui.page];
        let top_of = if page.parent.is_empty() { ui.page } else { page_index(page.parent).unwrap_or(ui.page) };

        // ---- title bar: back, "Settings", search, window buttons ----
        let back = rect(8.0, 9.0, 40.0, 41.0);
        let can_back = !page.parent.is_empty() || !ui.query.is_empty();
        if can_back && hover == Some(Hit::Back) {
            rt.FillRoundedRectangle(&rrect(back, 4.0), &b(0x2D2D2D)?);
        }
        ico(0xE72B, &icons10, back, &b(if can_back { 0xFFFFFF } else { 0x6B6B6B })?);
        hits.push((back, Hit::Back));
        txt("Settings", &small, rect(49.0, 8.0, 200.0, 40.0), &white);
        let sbx = rect(CARD_X, 9.0, 839.0, 39.0);
        rt.FillRoundedRectangle(&rrect(sbx, 15.0), &b(0x2D2D2D)?);
        rt.DrawRoundedRectangle(&rrect(rect(sbx.left + 0.5, sbx.top + 0.5, sbx.right - 0.5, sbx.bottom - 0.5), 15.0), &b(0x353535)?, 1.0, None);
        ico(0xE721, &icons12, rect(sbx.left + 7.0, sbx.top - 1.0, sbx.left + 31.0, sbx.bottom - 1.0), &white);
        if ui.query.is_empty() {
            txt("Find a setting", &body, rect(sbx.left + 40.0, sbx.top - 2.0, sbx.right - 12.0, sbx.bottom - 2.0), &b(0x9A9A9A)?);
        } else {
            txt(&ui.query, &body, rect(sbx.left + 40.0, sbx.top, sbx.right - 12.0, sbx.bottom), &white);
            rt.FillRectangle(&rect(sbx.left + 14.0, sbx.bottom - 1.5, sbx.right - 14.0, sbx.bottom), &accent);
        }
        for (k, cp) in [0xE921u32, if maximized { 0xE923 } else { 0xE922 }, 0xE8BB].iter().enumerate() {
            let r = rect(cw - 46.0 * (3 - k) as f32, 0.0, cw - 46.0 * (2 - k) as f32, 32.0);
            if hover == Some(Hit::Win(k as u8)) {
                rt.FillRectangle(&r, &b(if k == 2 { 0xC42B1C } else { 0x2D2D2D })?);
            }
            ico(*cp, &icons10, r, &white);
            hits.push((r, Hit::Win(k as u8)));
        }

        // ---- profile + nav ----
        let c = windows_numerics::Vector2 { X: 46.0, Y: 87.0 };
        rt.FillEllipse(&D2D1_ELLIPSE { point: c, radiusX: 30.0, radiusY: 30.0 }, &b(0xFAFAFA)?);
        rt.FillEllipse(&D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: 46.0, Y: 79.0 }, radiusX: 9.0, radiusY: 9.0 }, &b(0x8A8A8A)?);
        rt.FillRoundedRectangle(&rrect(rect(32.0, 91.0, 60.0, 103.0), 5.0), &b(0x8A8A8A)?);
        let email = account_email();
        if email.is_empty() {
            txt(&crate::launcher::display_name(), &body_sb, rect(88.0, 70.0, 310.0, 104.0), &white);
        } else {
            txt(&crate::launcher::display_name(), &body_sb, rect(88.0, 69.0, 310.0, 89.0), &white);
            txt(&email, &fmt(12.0, n, ui_font)?, rect(88.0, 87.0, 310.0, 105.0), &white);
        }
        let mut k = 0;
        for (i, p) in pages.iter().enumerate() {
            if !p.parent.is_empty() {
                continue;
            }
            let r = rect(16.0, NAV_Y + k as f32 * 40.0, 296.0, NAV_Y + 36.0 + k as f32 * 40.0);
            k += 1;
            if (i == top_of && ui.query.is_empty()) || hover == Some(Hit::Nav(i)) {
                rt.FillRoundedRectangle(&rrect(r, 4.0), &b(0x2D2D2D)?);
            }
            if i == top_of && ui.query.is_empty() {
                rt.FillRoundedRectangle(&rrect(rect(r.left, r.top + 10.0, r.left + 3.0, r.bottom - 10.0), 1.5), &accent);
            }
            ico(p.icon, &icons, rect(r.left + 8.0, r.top, r.left + 32.0, r.bottom), &white);
            txt(p.name, &body, rect(r.left + 44.0, r.top, r.right - 8.0, r.bottom), &white);
            hits.push((r, Hit::Nav(i)));
        }

        // ---- search results ----
        if !ui.query.is_empty() {
            txt(&format!("Results for \u{201C}{}\u{201D}", ui.query), &title, rect(CARD_X, 49.0, cw - 40.0, 97.0), &white);
            let res = search(pages, &ui.query);
            if res.is_empty() {
                txt("No settings match. Press Esc to clear.", &body, rect(CARD_X, 123.0, cw - 25.0, 150.0), &dim);
            }
            for (j, (pi, label, pname)) in res.iter().enumerate() {
                let y = 123.0 + j as f32 * (ROW_H + 4.0);
                let r = rect(CARD_X, y, cw - 25.0, y + ROW_H);
                rt.FillRoundedRectangle(&rrect(r, 4.0), &b(if hover == Some(Hit::Result(*pi)) { 0x323232 } else { 0x2B2B2B })?);
                txt(label, &body, rect(r.left + 20.0, r.top + 12.0, r.right - 60.0, r.top + 34.0), &white);
                txt(pname, &small, rect(r.left + 20.0, r.top + 34.0, r.right - 60.0, r.top + 54.0), &dim);
                ico(0xE76C, &icons12, rect(r.right - 36.0, r.top, r.right - 12.0, r.bottom), &white);
                hits.push((r, Hit::Result(*pi)));
            }
            ui.hits = hits;
            return Ok(());
        }

        // a new page rises into place and fades in (Windows' page transition, ~250 ms)
        let enter = page_enter();
        if enter < 1.0 {
            rt.SetTransform(&windows_numerics::Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 28.0 * (1.0 - enter) });
        }

        // ---- page title (breadcrumb on sub-pages) ----
        if page.parent.is_empty() {
            txt(page.name, &title, rect(CARD_X, 49.0, cw - 40.0, 97.0), &white);
        } else {
            let l = dw.CreateTextLayout(&utf16(page.parent), &title, 600.0, 48.0)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            l.GetMetrics(&mut m)?;
            // the parent's name goes back to it, as in Windows (lighter under the mouse)
            let pr = rect(CARD_X, 55.0, CARD_X + m.width, 93.0);
            txt(page.parent, &title, rect(CARD_X, 49.0, cw - 40.0, 97.0), if hover == Some(Hit::Back) { &white } else { &dim });
            hits.push((pr, Hit::Back));
            ico(0xE76C, &icons, rect(CARD_X + m.width + 6.0, 53.0, CARD_X + m.width + 34.0, 97.0), &dim);
            txt(page.name, &title, rect(CARD_X + m.width + 40.0, 49.0, cw - 40.0, 97.0), &white);
        }
        if page.name == "System" {
            rt.FillRoundedRectangle(&rrect(rect(324.0, 123.0, 444.0, 191.0), 4.0), &b(0x000000)?);
            txt(&device_name().to_lowercase(), &h20, rect(460.0, 129.0, 800.0, 153.0), &white);
            txt(&model(), &body, rect(460.0, 152.0, 800.0, 172.0), &white);
            let rr = rect(461.0, 171.0, 521.0, 191.0);
            txt("Rename", &body, rr, &b(if hover == Some(Hit::Rename) { 0xB5F0FF } else { 0x99EBFF })?);
            hits.push((rr, Hit::Rename));
        }

        // ---- cards ----
        let clip_top = first_row_y(page) - 18.0;
        // measured: sub-pages sit their icons and text 2px further in; top-level card lists sit them 1px higher
        let dx = if page.parent.is_empty() { 0.0 } else { 2.0 };
        let dy = if page.parent.is_empty() && matches!(first_row_y(page) as i32, 117 | 128 | 223) && !matches!(page.rows.first().map(|r| r.kind), Some(Kind::UpdateHero(_))) { -1.0 } else { 0.0 };

        rt.PushAxisAlignedClip(&rect(NAV, clip_top, cw, ch), D2D1_ANTIALIAS_MODE_ALIASED);
        for (i, row) in page.rows.iter().enumerate() {
            // Settings lays rows out on fractions of a pixel: boxes snap to the nearest pixel, text snaps down
            let rf = row_rect(page, i, cw);
            let r = rect(rf.left, rf.top.round(), rf.right, rf.bottom.round());
            let dy = dy + rf.top.floor() - rf.top.round();
            if r.top > ch || r.bottom < clip_top {
                continue;
            }
            if matches!(row.kind, Kind::Header | Kind::Section) {
                // the title sits 17px above the next card
                txt(row.label, &header, rect(r.left + 1.0, r.bottom - 26.0, r.right, r.bottom - 4.0), &white);
                continue;
            }
            if let Kind::Big(_) = row.kind {
                let v = ui.vals.get(i).cloned().unwrap_or_default();
                let l = dw.CreateTextLayout(&utf16(&v.s), &big, 600.0, 80.0)?;
                let mut m = DWRITE_TEXT_METRICS::default();
                l.GetMetrics(&mut m)?;
                txt(&v.s, &big, rect(r.left - 2.0, r.top, r.left + 600.0, r.top + 78.0), &white);
                let level: u32 = v.s.trim_end_matches('%').parse().unwrap_or(0);
                ico(if v.on { 0xEBAB } else { 0xEBA0 } + (level + 5) / 10, &icons40, rect(r.left + m.width + 14.0, r.top + 6.0, r.left + m.width + 74.0, r.top + 78.0), &white);
                continue;
            }
            if let Kind::Note(_, text) = row.kind {
                if r.bottom - r.top > 10.0 {
                    let c = rect(r.left, r.top, r.right, r.top + 49.0);
                    rt.FillRoundedRectangle(&rrect(c, 4.0), &b(0x272727)?);
                    rt.DrawRoundedRectangle(&rrect(rect(c.left + 0.5, c.top + 0.5, c.right - 0.5, c.bottom - 0.5), 4.0), &b(0x1D1D1D)?, 1.0, None);
                    ico(0xF167, &icons, rect(c.left + 12.0, c.top, c.left + 36.0, c.bottom), &accent);
                    txt(text, &body, rect(c.left + 46.0, c.top, c.right - 20.0, c.bottom), &white);
                }
                continue;
            }
            if let Kind::Tiles(_) = row.kind {
                let list = ui.vals.get(i).map(|v| v.list.clone()).unwrap_or_default();
                let wrap = fmt(16.0, sb, ui_font)?;
                wrap.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                wrap.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
                for (k, (label, value, _, foot)) in list.iter().enumerate() {
                    let (x, y) = (r.left + (k % 3) as f32 * 212.0, r.top + (k / 3) as f32 * 154.0);
                    let c = rect(x, y, x + 198.0, y + 140.0);
                    rt.FillRoundedRectangle(&rrect(c, 6.0), &b(0x2B2B2B)?);
                    ico([0xE950, 0xE964, 0xE7F4, 0xEDA2][k.min(3)], &icons10, rect(x + 14.0, y + 12.0, x + 30.0, y + 32.0), &dim);
                    txt(label, &small, rect(x + 36.0, y + 12.0, x + 190.0, y + 32.0), &dim);
                    txt(value, &wrap, rect(x + 16.0, y + 38.0, x + 186.0, y + 100.0), &white);
                    txt(foot, &small, rect(x + 16.0, y + 106.0, x + 190.0, y + 128.0), &dim);
                }
                continue;
            }
            if matches!(row.kind, Kind::RenamePc) {
                rt.FillRoundedRectangle(&rrect(r, 4.0), &b(0x2B2B2B)?);
                rt.DrawRoundedRectangle(&rrect(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), 4.0), &b(0x1D1D1D)?, 1.0, None);
                txt(&device_name().to_lowercase(), &body, rect(r.left + 16.0, r.top + 15.0, r.right - 200.0, r.top + 37.0), &white);
                txt(&model(), &small, rect(r.left + 16.0, r.top + 35.0, r.right - 200.0, r.top + 53.0), &dim);
                let br = rect(r.right - 146.0, r.top + 18.0, r.right - 16.0, r.top + 50.0);
                rt.FillRoundedRectangle(&rrect(br, 4.0), &b(if hover == Some(Hit::Rename) { 0x3D3D3D } else { 0x373737 })?);
                let centered = fmt(14.0, n, ui_font)?;
                centered.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                txt("Rename this PC", &centered, br, &white);
                hits.push((br, Hit::Rename));
                continue;
            }
            if let Kind::Facts(_) = row.kind {
                let list = ui.vals.get(i).map(|v| v.list.clone()).unwrap_or_default();
                rt.FillRoundedRectangle(&rrect(r, 4.0), &b(0x272727)?);
                rt.FillRoundedRectangle(&rrect(rect(r.left, r.top, r.right, r.top + ROW_H), 4.0), &b(0x2B2B2B)?);
                rt.DrawRoundedRectangle(&rrect(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), 4.0), &b(0x1D1D1D)?, 1.0, None);
                rt.FillRectangle(&rect(r.left, r.top + ROW_H, r.right, r.top + ROW_H + 1.0), &b(0x1D1D1D)?);
                card_icon(row.icon, rect(r.left + 14.0 + dx, r.top + 1.0 + dy, r.left + 42.0 + dx, r.top + ROW_H + 1.0 + dy));
                txt(row.label, &body, rect(r.left + 58.0, r.top, r.right - 200.0, r.top + ROW_H), &white);
                let br = rect(r.right - 134.0, r.top + 18.0, r.right - 44.0, r.top + 50.0);
                rt.FillRoundedRectangle(&rrect(br, 4.0), &b(if hover == Some(Hit::Copy(i)) { 0x3D3D3D } else { 0x373737 })?);
                let centered = fmt(14.0, n, ui_font)?;
                centered.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                txt("Copy", &centered, br, &white);
                hits.push((br, Hit::Copy(i)));
                ico(0xE70E, &icons10, rect(r.right - 36.0, r.top, r.right - 12.0, r.top + ROW_H), &white);
                let top_align = fmt(14.0, n, ui_font)?;
                top_align.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
                let mut y = r.top + ROW_H + 9.0;
                // the label column fits the longest label (About keeps Windows' 112 px)
                let key_w = list.iter().map(|l| l.0.chars().count()).max().map_or(112.0, |n| (n as f32 * 7.4).clamp(112.0, 330.0));
                for (key, value, _, _) in &list {
                    let lines = 1 + value.matches('\n').count();
                    txt(key, &top_align, rect(r.left + 58.0, y, r.left + 58.0 + key_w, y + 22.0), &white);
                    for (k, part) in value.split('\n').enumerate() {
                        txt(part, &top_align, rect(r.left + 60.0 + key_w, y + 19.0 * k as f32, r.right - 20.0, y + 19.0 * k as f32 + 22.0), &dim);
                    }
                    y += 25.0 + 19.0 * (lines - 1) as f32;
                }
                continue;
            }
            if let Kind::NetHero(_) = row.kind {
                let v = ui.vals.get(i).cloned().unwrap_or_default();
                let up = !v.s.is_empty();
                let big_icon = fmt(72.0, n, w!("Segoe Fluent Icons"))?;
                ico(if !up { 0xF384 } else if v.on { 0xE839 } else { 0xE701 }, &big_icon, rect(r.left - 4.0, r.top - 6.0, r.left + 96.0, r.top + 94.0), &b(0x0078D4)?);
                txt(if up { &v.s } else { "Not connected" }, &h20, rect(r.left + 114.0, r.top + 18.0, r.left + 400.0, r.top + 46.0), &white);
                if up {
                    ico(0xE774, &icons12, rect(r.left + 112.0, r.top + 48.0, r.left + 132.0, r.top + 68.0), &b(0x4CC2FF)?);
                    txt("Connected", &body, rect(r.left + 134.0, r.top + 46.0, r.left + 400.0, r.top + 70.0), &dim);
                }
                for (x, glyph_, label, uri) in [(418.0, 0xE946, "Properties", if v.on { "ms-settings:network-ethernet" } else { "ms-settings:network-wifi" }), (636.0, 0xEB05, "Data usage", "ms-settings:datausage")] {
                    let hr = rect(r.left + x - 8.0, r.top + 14.0, r.left + x + 190.0, r.top + 74.0);
                    if hover == Some(Hit::Open(uri)) {
                        rt.FillRoundedRectangle(&rrect(hr, 4.0), &b(0x2B2B2B)?);
                    }
                    ico(glyph_, &icons, rect(r.left + x, r.top + 30.0, r.left + x + 24.0, r.top + 58.0), &white);
                    txt(label, &body_sb, rect(r.left + x + 32.0, r.top + 30.0, r.left + x + 190.0, r.top + 58.0), &white);
                    hits.push((hr, Hit::Open(uri)));
                }
                ico(0xE76C, &icons12, rect(r.right - 30.0, r.top + 30.0, r.right - 6.0, r.top + 58.0), &white);
                continue;
            }
            if let Kind::BtTiles(_) = row.kind {
                let list = ui.vals.get(i).map(|v| v.list.clone()).unwrap_or_default();
                let centered = fmt(14.0, n, ui_font)?;
                centered.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                let centered_sb = fmt(14.0, sb, ui_font)?;
                centered_sb.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                let big_icon = fmt(44.0, n, w!("Segoe Fluent Icons"))?;
                big_icon.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                // as many tiles as fit; the last one is always "Add device"
                let fit = (((r.right - r.left + 6.0) / 224.0).floor() as usize).max(1);
                let shown = list.len().min(fit - 1);
                for k in 0..=shown {
                    let x = r.left + k as f32 * 224.0;
                    let c = rect(x, r.top, x + 218.0, r.top + 221.0);
                    rt.FillRoundedRectangle(&rrect(c, 6.0), &b(0x2B2B2B)?);
                    rt.DrawRoundedRectangle(&rrect(rect(c.left + 0.5, c.top + 0.5, c.right - 0.5, c.bottom - 0.5), 6.0), &b(0x1D1D1D)?, 1.0, None);
                    if k == shown {
                        ico(0xE710, &big_icon, rect(c.left, c.top + 60.0, c.right, c.top + 130.0), &white);
                        txt("Add device", &centered_sb, rect(c.left, c.top + 128.0, c.right, c.top + 150.0), &white);
                        hits.push((c, Hit::Open("ms-settings-connectabledevices:devicediscovery")));
                        continue;
                    }
                    let (name, _, on, _) = &list[k];
                    ico(0xE712, &icons, rect(c.right - 40.0, c.top + 8.0, c.right - 10.0, c.top + 32.0), &white);
                    ico(0xE702, &big_icon, rect(c.left, c.top + 28.0, c.right, c.top + 92.0), &white);
                    txt(name, &centered_sb, rect(c.left + 8.0, c.top + 104.0, c.right - 8.0, c.top + 126.0), &white);
                    txt(if *on { "Connected" } else { "Not connected" }, &centered, rect(c.left + 8.0, c.top + 123.0, c.right - 8.0, c.top + 145.0), &dim);
                    if !*on {
                        let br = rect(c.left + 71.0, c.top + 153.0, c.left + 147.0, c.top + 185.0);
                        rt.FillRoundedRectangle(&rrect(br, 4.0), &b(if hover == Some(Hit::Tile(k)) { 0x3D3D3D } else { 0x373737 })?);
                        txt("Connect", &centered, br, &white);
                        hits.push((br, Hit::Tile(k)));
                    }
                }
                continue;
            }
            if let Kind::Center(text, uri) = row.kind {
                let r = rect(r.left, r.top, r.right, r.top + 35.0);
                rt.FillRoundedRectangle(&rrect(r, 4.0), &b(if hover == Some(Hit::Open(uri)) { 0x323232 } else { 0x2B2B2B })?);
                rt.DrawRoundedRectangle(&rrect(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), 4.0), &b(0x1D1D1D)?, 1.0, None);
                let centered = fmt(14.0, n, ui_font)?;
                centered.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                txt(text, &centered, r, &b(0x99EBFF)?);
                hits.push((r, Hit::Open(uri)));
                continue;
            }
            if matches!(row.kind, Kind::Space(_)) {
                continue;
            }
            if let Kind::ThemeHero(_) = row.kind {
                let list = ui.vals.get(i).map(|v| v.list.clone()).unwrap_or_default();
                // the desktop preview: a small window on your background
                let pv = rect(r.left - 1.0, r.top + 41.0, r.left + 249.0, r.top + 181.0);
                rt.FillRoundedRectangle(&rrect(pv, 8.0), &b(0x000000)?);
                let win = rect(pv.left + 137.0, pv.top + 16.0, pv.left + 233.0, pv.top + 111.0);
                rt.FillRectangle(&win, &b(0x2B2B2B)?);
                for k in 0..4 {
                    let y = win.top + 11.0 + 9.0 * k as f32;
                    rt.FillRectangle(&rect(win.left + 12.0, y, win.right - if k == 3 { 42.0 } else { 12.0 }, y + 1.0), &b(0xFFFFFF)?);
                }
                rt.FillRoundedRectangle(&rrect(rect(win.right - 36.0, win.bottom - 22.0, win.right - 12.0, win.bottom - 14.0), 2.0), &accent);
                txt("Select a theme to apply", &body_sb, rect(r.left + 297.0, r.top - 4.0, r.right, r.top + 20.0), &white);
                let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                for (k, (_, wall, current, tone)) in list.iter().take(6).enumerate() {
                    let (x, y) = (r.left + 300.0 + (k % 3) as f32 * 120.0, r.top + 32.0 + (k / 3) as f32 * 98.0);
                    let c = rect(x, y, x + 112.0, y + 90.0);
                    let (w_, h_, px) = THUMBS.with(|m| m.borrow_mut().entry(wall.clone()).or_insert_with(|| crate::files::image(wall, 224, windows::Win32::UI::Shell::SIIGBF_THUMBNAILONLY).unwrap_or_default()).clone());
                    if w_ > 0 && px.len() == (w_ * h_ * 4) as usize {
                        if let Ok(bm) = rt.CreateBitmap(D2D_SIZE_U { width: w_, height: h_ }, Some(px.as_ptr() as *const c_void), w_ * 4, &bp) {
                            // the real page squeezes the whole picture into the tile
                            let src = rect(0.0, 0.0, w_ as f32, h_ as f32);
                            rt.DrawBitmap(&bm, Some(&c), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, Some(&src));
                        }
                    } else {
                        rt.FillRoundedRectangle(&rrect(c, 4.0), &b(0x2B2B2B)?);
                    }
                    // the little window each theme shows, in that theme's light or dark tone
                    let mock = rect(c.left + 66.0, c.top + 44.0, c.left + 108.0, c.top + 86.0);
                    rt.FillRoundedRectangle(&rrect(mock, 4.0), &b(if tone.starts_with("dark") { 0x1F2A44 } else { 0xE9EEF6 })?);
                    let chip = tone.split_once(':').and_then(|t| u32::from_str_radix(t.1, 16).ok()).unwrap_or(0x0078D4); // the theme's own accent colour
                    rt.FillRoundedRectangle(&rrect(rect(mock.left + 9.0, mock.bottom - 15.0, mock.right - 9.0, mock.bottom - 8.0), 2.0), &b(chip)?);
                    if *current || hover == Some(Hit::Theme(k)) {
                        rt.DrawRoundedRectangle(&rrect(rect(c.left - 1.0, c.top - 1.0, c.right + 1.0, c.bottom + 1.0), 4.0), &accent, 2.0, None);
                    }
                    hits.push((c, Hit::Theme(k)));
                }
                continue;
            }
            if let Kind::TimeHero(_) = row.kind {
                let list = ui.vals.get(i).map(|v| v.list.clone()).unwrap_or_default();
                if let Some((time, date, _, _)) = list.first() {
                    let clock = fmt(40.0, sb, w!("Segoe UI Variable Display"))?;
                    txt(time, &clock, rect(r.left - 2.0, r.top - 6.0, r.left + 300.0, r.top + 46.0), &white);
                    txt(date, &body, rect(r.left, r.top + 51.0, r.left + 380.0, r.top + 73.0), &dim);
                }
                for (k, (label, value, _, _)) in list.iter().enumerate().skip(1) {
                    let x = r.left + if k == 1 { 400.0 } else { 646.0 };
                    ico(if k == 1 { 0xE121 } else { 0xE774 }, &icons, rect(x, r.top + 11.0, x + 24.0, r.top + 39.0), &white);
                    txt(label, &body_sb, rect(x + 36.0, r.top + 5.0, x + 240.0, r.top + 27.0), &white);
                    txt(value, &small, rect(x + 36.0, r.top + 26.0, x + 240.0, r.top + 44.0), &white);
                }
                continue;
            }
            if let Kind::UpdateHero(_) = row.kind {
                let v = ui.vals.get(i).cloned().unwrap_or_default();
                let big_icon = fmt(64.0, n, w!("Segoe Fluent Icons"))?;
                big_icon.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                ico(0xE895, &big_icon, rect(r.left, r.top - 2.0, r.left + 96.0, r.top + 86.0), &b(0x0078D4)?);
                txt(if v.on { "Restart required" } else { "You're up to date" }, &h18, rect(r.left + 114.0, r.top + 13.0, r.right - 200.0, r.top + 41.0), &white);
                txt(if v.on { "Your device will restart outside of active hours." } else { "Windows checks for updates on its own." }, &body, rect(r.left + 114.0, r.top + 38.0, r.right - 200.0, r.top + 60.0), &dim);
                let label = if v.on { "Restart now" } else { "Check for updates" };
                let l = dw.CreateTextLayout(&utf16(label), &body, 400.0, 32.0)?;
                let mut m = DWRITE_TEXT_METRICS::default();
                l.GetMetrics(&mut m)?;
                let br = rect(r.right - m.width.ceil() - 24.0, r.top + 27.0, r.right, r.top + 59.0);
                // goes to our Check for updates page (its address maps there)
                rt.FillRoundedRectangle(&rrect(br, 4.0), &b(if hover == Some(Hit::Open("ms-settings:windowsupdate-action")) { 0x47B1E8 } else { 0x4CC2FF })?);
                let centered = fmt(14.0, n, ui_font)?;
                centered.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                txt(label, &centered, br, &b(0x000000)?);
                hits.push((br, Hit::Open("ms-settings:windowsupdate-action")));
                continue;
            }
            if let Kind::When(_, uri) = row.kind {
                if r.bottom - r.top < 10.0 {
                    continue;
                }
                rt.FillRoundedRectangle(&rrect(r, 4.0), &b(if hover == Some(Hit::Open(uri)) { 0x323232 } else { 0x2B2B2B })?);
                rt.DrawRoundedRectangle(&rrect(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), 4.0), &b(0x1D1D1D)?, 1.0, None);
                txt(row.label, &body, rect(r.left + 16.0, r.top + 15.0, r.right - 60.0, r.top + 37.0), &white);
                txt(row.sub, &small, rect(r.left + 16.0, r.top + 35.0, r.right - 60.0, r.top + 53.0), &dim);
                ico(0xE70D, &icons10, rect(r.right - 36.0, r.top, r.right - 12.0, r.bottom), &white);
                hits.push((r, Hit::Open(uri)));
                continue;
            }
            if let Kind::HomeHero(_) = row.kind {
                let list = ui.vals.get(i).map(|v| v.list.clone()).unwrap_or_default();
                rt.FillRoundedRectangle(&rrect(rect(r.left, r.top + 2.0, r.left + 120.0, r.top + 70.0), 4.0), &b(0x000000)?);
                if let Some((name, model_, _, _)) = list.first() {
                    txt(name, &h20, rect(r.left + 136.0, r.top + 6.0, r.left + 420.0, r.top + 32.0), &white);
                    txt(model_, &body, rect(r.left + 136.0, r.top + 30.0, r.left + 420.0, r.top + 50.0), &white);
                    let rr = rect(r.left + 136.0, r.top + 49.0, r.left + 196.0, r.top + 69.0);
                    txt("Rename", &body, rr, &b(if hover == Some(Hit::Rename) { 0xB5F0FF } else { 0x99EBFF })?);
                    hits.push((rr, Hit::Rename));
                }
                for (k, (title_, state, _, _)) in list.iter().enumerate().skip(1) {
                    let x = r.left + if k == 1 { 460.0 } else { 610.0 };
                    let uri = if k == 1 { "ms-settings:network" } else { "ms-settings:windowsupdate" };
                    ico(if k == 1 { 0xE839 } else { 0xE895 }, &icons20, rect(x, r.top + 16.0, x + 28.0, r.top + 52.0), &b(0x4CC2FF)?);
                    txt(title_, &body_sb, rect(x + 38.0, r.top + 14.0, x + 240.0, r.top + 36.0), &white);
                    txt(state, &small, rect(x + 38.0, r.top + 34.0, x + 240.0, r.top + 52.0), &dim);
                    hits.push((rect(x, r.top + 8.0, x + 140.0, r.top + 60.0), Hit::Open(uri)));
                }
                continue;
            }
            if let Kind::AccountHero(_) = row.kind {
                if let Some((name, email, _, kind_)) = ui.vals.get(i).and_then(|v| v.list.first().cloned()) {
                    let c = windows_numerics::Vector2 { X: r.left + 49.0, Y: r.top + 49.0 };
                    rt.FillEllipse(&D2D1_ELLIPSE { point: c, radiusX: 48.0, radiusY: 48.0 }, &b(0xFAFAFA)?);
                    rt.FillEllipse(&D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: c.X, Y: c.Y - 12.0 }, radiusX: 15.0, radiusY: 15.0 }, &b(0x8A8A8A)?);
                    rt.FillRoundedRectangle(&rrect(rect(c.X - 23.0, c.Y + 7.0, c.X + 23.0, c.Y + 27.0), 8.0), &b(0x8A8A8A)?);
                    txt(&name.to_uppercase(), &h18, rect(r.left + 110.0, r.top + 14.0, r.right, r.top + 40.0), &white);
                    txt(&email, &body, rect(r.left + 110.0, r.top + 37.0, r.right, r.top + 57.0), &white);
                    txt(&kind_, &body, rect(r.left + 110.0, r.top + 56.0, r.right, r.top + 76.0), &white);
                }
                continue;
            }
            if matches!(row.kind, Kind::SortBy) {
                txt(row.label, &body, rect(r.left, r.top, r.left + 60.0, r.top + 30.0), &dim);
                let bx = rect(r.left + 53.0, r.top, r.left + 183.0, r.top + 30.0);
                rt.FillRoundedRectangle(&rrect(bx, 4.0), &b(if hover == Some(Hit::Sort) { 0x3D3D3D } else { 0x2D2D2D })?);
                txt(if BY_NAME.load(Relaxed) { "Name" } else { "Most recent" }, &body, rect(bx.left + 12.0, bx.top, bx.right - 30.0, bx.bottom), &white);
                ico(0xE70D, &icons10, rect(bx.right - 32.0, bx.top, bx.right - 8.0, bx.bottom), &dim);
                hits.push((bx, Hit::Sort));
                continue;
            }
            if let Kind::Volumes(..) = row.kind {
                // the volume mixer: app icon and name, then its own slider and number, like Windows' page
                let list = ui.vals.get(i).map(|v| v.list.clone()).unwrap_or_default();
                for (k, (name, vol, _, _)) in list.iter().enumerate() {
                    let c = rect(r.left, r.top + 73.0 * k as f32, r.right, r.top + 73.0 * k as f32 + ROW_H);
                    if c.top > ch || c.bottom < clip_top {
                        continue;
                    }
                    rt.FillRoundedRectangle(&rrect(c, 4.0), &b(0x2B2B2B)?);
                    rt.DrawRoundedRectangle(&rrect(rect(c.left + 0.5, c.top + 0.5, c.right - 0.5, c.bottom - 0.5), 4.0), &b(0x1D1D1D)?, 1.0, None);
                    ico(0xE767, &icons, rect(c.left + 16.0, c.top, c.left + 40.0, c.bottom), &white);
                    txt(name, &body, rect(c.left + 58.0, c.top, c.right - 330.0, c.bottom), &white);
                    let f = vol.parse::<f32>().unwrap_or(0.0) / 100.0;
                    let (x0, x1) = (c.right - 300.0, c.right - 24.0);
                    let y = (c.top + c.bottom) / 2.0;
                    let x = x0 + (x1 - x0) * f.clamp(0.0, 1.0);
                    rt.FillRoundedRectangle(&rrect(rect(x0, y - 2.0, x1, y + 2.0), 2.0), &b(0x9E9E9E)?);
                    rt.FillRoundedRectangle(&rrect(rect(x0, y - 2.0, x, y + 2.0), 2.0), &accent);
                    let pt = windows_numerics::Vector2 { X: x, Y: y };
                    rt.FillEllipse(&D2D1_ELLIPSE { point: pt, radiusX: 10.0, radiusY: 10.0 }, &b(0x454545)?);
                    rt.FillEllipse(&D2D1_ELLIPSE { point: pt, radiusX: 6.0, radiusY: 6.0 }, &accent);
                    txt(&format!("{:.0}", f * 100.0), &right, rect(x0 - 60.0, c.top, x0 - 14.0, c.bottom), &white);
                    hits.push((rect(x0 - 12.0, c.top, x1 + 12.0, c.bottom), Hit::Vol(i, k)));
                }
                continue;
            }
            if let Kind::Senders(..) | Kind::Actions(..) = row.kind {
                let button = if let Kind::Actions(_, label, _) = row.kind { Some(label) } else { None };
                let list = ui.vals.get(i).map(|v| v.list.clone()).unwrap_or_default();
                let bp = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
                for (k, (name, what, on, id)) in list.iter().enumerate() {
                    let c = rect(r.left, r.top + 73.0 * k as f32, r.right, r.top + 73.0 * k as f32 + ROW_H);
                    if c.top > ch || c.bottom < clip_top {
                        continue;
                    }
                    rt.FillRoundedRectangle(&rrect(c, 4.0), &b(if hover == Some(Hit::Sender(i, k)) { 0x323232 } else { 0x2B2B2B })?);
                    rt.DrawRoundedRectangle(&rrect(rect(c.left + 0.5, c.top + 0.5, c.right - 0.5, c.bottom - 0.5), 4.0), &b(0x1D1D1D)?, 1.0, None);
                    let px = APP_ICONS.with(|m| m.borrow_mut().entry(id.clone()).or_insert_with(|| Some(icon_source(id)).filter(|f| !f.is_empty()).and_then(|f| crate::files::image(&f, 24, windows::Win32::UI::Shell::SIIGBF_ICONONLY).filter(|i| i.0 == 24 && i.1 == 24)).map(|i| i.2).unwrap_or_default()).clone());
                    if px.len() == 24 * 24 * 4 {
                        if let Ok(bm) = rt.CreateBitmap(D2D_SIZE_U { width: 24, height: 24 }, Some(px.as_ptr() as *const c_void), 96, &bp) {
                            crate::flyout::draw_icon(&rt, &bm, c.left + 16.0, c.top + 22.0, 24.0);
                        }
                    }
                    txt(name, &body, rect(c.left + 58.0, c.top + 15.0, c.right - 140.0, c.top + 37.0), &white);
                    txt(what, &small, rect(c.left + 58.0, c.top + 35.0, c.right - 140.0, c.top + 53.0), &dim);
                    if let Some(label) = button {
                        let label = if label == "Connect" && *on { "Disconnect" } else { label };
                        // a quiet button at the right, like the buttons on Windows' own cards
                        let bx = rect(c.right - 112.0, c.top + 18.0, c.right - 16.0, c.top + 50.0);
                        rt.FillRoundedRectangle(&rrect(bx, 4.0), &b(if hover == Some(Hit::Sender(i, k)) { 0x3D3D3D } else { 0x373737 })?);
                        rt.DrawRoundedRectangle(&rrect(rect(bx.left + 0.5, bx.top + 0.5, bx.right - 0.5, bx.bottom - 0.5), 4.0), &b(0x424242)?, 1.0, None);
                        let mid = dw.CreateTextFormat(w!("Segoe UI Variable Text"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 14.0, w!(""))?;
                        mid.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                        mid.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                        txt(label, &mid, bx, &white);
                        hits.push((bx, Hit::Sender(i, k)));
                        continue;
                    }
                    txt(if *on { "On" } else { "Off" }, &right, rect(c.right - 190.0, c.top, c.right - 96.0, c.bottom), &white);
                    crate::quick::switch(&rt, c.right - 84.0, c.top + 24.0, *on)?;
                    ico(0xE76C, &icons12, rect(c.right - 36.0, c.top, c.right - 12.0, c.bottom), &white);
                    hits.push((c, Hit::Sender(i, k)));
                }
                continue;
            }
            let clickable = matches!(row.kind, Kind::Link(_) | Kind::LinkBtn(..) | Kind::Expand(_) | Kind::Sub(_) | Kind::Choice(..) | Kind::Flat(..) | Kind::State(..));
            let hov = hover == Some(Hit::Row(i)) && clickable;
            rt.FillRoundedRectangle(&rrect(r, 4.0), &b(if hov { 0x323232 } else { 0x2B2B2B })?);
            rt.DrawRoundedRectangle(&rrect(rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), 4.0), &b(0x1D1D1D)?, 1.0, None);
            let tx = if row.icon != 0 {
                card_icon(row.icon, rect(r.left + 14.0 + dx, r.top + 1.0 + dy, r.left + 42.0 + dx, r.top + ROW_H + 1.0 + dy));
                r.left + 58.0 + dx
            } else {
                r.left + 16.0
            };
            if row.sub.is_empty() {
                txt(row.label, &body, rect(tx, r.top + dy, r.right - 260.0, r.top + ROW_H + dy), &white);
            } else {
                txt(row.label, &body, rect(tx, r.top + 15.0 + dy, r.right - 60.0, r.top + 37.0 + dy), &white);
                txt(row.sub, &small, rect(tx, r.top + 35.0 + dy, r.right - 60.0, r.top + 53.0 + dy), &dim);
            }
            let v = ui.vals.get(i).cloned().unwrap_or_default();
            let loaded = i < ui.vals.len();
            match row.kind {
                Kind::Toggle(..) => {
                    txt(if v.on { "On" } else { "Off" }, &right, rect(r.right - 160.0, r.top, r.right - 71.0, r.bottom), &white);
                    crate::quick::switch_at(&rt, r.right - 59.0, r.top + 24.0, switch_pos(ui.page, i, v.on))?;
                }
                Kind::Slider(..) => {
                    let (x0, x1) = slider_x(&r);
                    let y = (r.top + r.bottom) / 2.0;
                    let x = x0 + (x1 - x0) * v.f.clamp(0.0, 1.0);
                    rt.FillRoundedRectangle(&rrect(rect(x0, y - 2.0, x1, y + 2.0), 2.0), &b(0x9E9E9E)?);
                    rt.FillRoundedRectangle(&rrect(rect(x0, y - 2.0, x, y + 2.0), 2.0), &accent);
                    let pt = windows_numerics::Vector2 { X: x, Y: y };
                    rt.FillEllipse(&D2D1_ELLIPSE { point: pt, radiusX: 10.0, radiusY: 10.0 }, &b(0x454545)?);
                    rt.FillEllipse(&D2D1_ELLIPSE { point: pt, radiusX: 6.0, radiusY: 6.0 }, &accent);
                    txt(&format!("{:.0}", v.f * 100.0), &right, rect(x0 - 60.0, r.top, x0 - 8.0, r.bottom), &white);
                    if row.label == "Volume" {
                        // the speaker beside the number mutes, like the real page
                        let mr = rect(x0 - 62.0, r.top + 18.0, x0 - 30.0, r.top + 50.0);
                        ico(if v.on { 0xE74F } else { 0xE767 }, &icons, mr, &white);
                        hits.push((r, Hit::Row(i)));
                        hits.push((mr, Hit::Mute));
                        continue;
                    }
                }
                Kind::Info(_) => txt(if loaded { &v.s } else { "\u{2026}" }, &right, rect(r.left + 260.0, r.top, r.right - 24.0, r.bottom), &dim),
                Kind::Link(_) | Kind::Sub(_) => ico(0xE76C, &icons12, rect(r.right - 36.0, r.top, r.right - 12.0, r.bottom), &white),
                Kind::Expand(_) => ico(0xE70D, &icons10, rect(r.right - 36.0, r.top, r.right - 12.0, r.bottom), &white),
                Kind::State(..) => {
                    txt(if v.on { "On" } else { "Off" }, &right, rect(r.right - 190.0, r.top, r.right - 96.0, r.bottom), &white);
                    crate::quick::switch(&rt, r.right - 84.0, r.top + 24.0, v.on)?;
                    ico(0xE76C, &icons12, rect(r.right - 36.0, r.top, r.right - 12.0, r.bottom), &white);
                }
                Kind::Choice(..) => {
                    let value = if loaded { v.s.as_str() } else { "\u{2026}" };
                    let l = dw.CreateTextLayout(&utf16(value), &body, 600.0, 32.0)?;
                    let mut m = DWRITE_TEXT_METRICS::default();
                    l.GetMetrics(&mut m)?;
                    let bx = rect(r.right - 16.0 - (m.width + 12.0 + 44.0).ceil(), r.top + 18.0, r.right - 16.0, r.top + 50.0);
                    rt.FillRoundedRectangle(&rrect(bx, 4.0), &b(if hov { 0x3D3D3D } else { 0x373737 })?);
                    txt(value, &body, rect(bx.left + 12.0, bx.top, bx.right - 32.0, bx.bottom), &white);
                    ico(0xE70D, &icons10, rect(bx.right - 34.0, bx.top, bx.right - 10.0, bx.bottom), &dim);
                }
                Kind::Flat(..) => {
                    txt(if loaded { &v.s } else { "\u{2026}" }, &right, rect(r.left + 300.0, r.top, r.right - 45.0, r.bottom), &dim);
                    ico(0xE70D, &icons10, rect(r.right - 36.0, r.top, r.right - 12.0, r.bottom), &white);
                }
                Kind::ToggleMore(..) => {
                    txt(if v.on { "On" } else { "Off" }, &right, rect(r.right - 190.0, r.top, r.right - 96.0, r.bottom), &white);
                    crate::quick::switch(&rt, r.right - 84.0, r.top + 24.0, v.on)?;
                    let cr = rect(r.right - 40.0, r.top, r.right, r.bottom);
                    ico(0xE76C, &icons12, rect(r.right - 36.0, r.top, r.right - 12.0, r.bottom), &white);
                    if let Kind::ToggleMore(_, _, uri) = row.kind {
                        hits.push((r, Hit::Row(i)));
                        hits.push((cr, Hit::Open(uri)));
                        continue;
                    }
                }
                Kind::LinkBtn(_, text, uri) => {
                    ico(0xE76C, &icons12, rect(r.right - 36.0, r.top, r.right - 12.0, r.bottom), &white);
                    let br = rect(r.right - 174.0, r.top + 18.0, r.right - 44.0, r.top + 50.0);
                    rt.FillRoundedRectangle(&rrect(br, 4.0), &b(if hover == Some(Hit::Open(uri)) { 0x47B1E8 } else { 0x4CC2FF })?);
                    let centered = fmt(14.0, n, ui_font)?;
                    centered.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                    txt(text, &centered, br, &b(0x000000)?);
                    hits.push((r, Hit::Row(i)));
                    hits.push((br, Hit::Open(uri)));
                    continue;
                }
                Kind::HomeHero(_) | Kind::AccountHero(_) | Kind::UpdateHero(_) | Kind::When(..) | Kind::TimeHero(_) | Kind::ThemeHero(_) | Kind::NetHero(_) | Kind::BtTiles(_) | Kind::Center(..) | Kind::Space(_) => {}
                Kind::Header | Kind::Section | Kind::SortBy | Kind::Senders(..) | Kind::Actions(..) | Kind::Volumes(..) | Kind::Big(_) | Kind::Note(..) | Kind::Tiles(_) | Kind::Facts(_) | Kind::RenamePc => {}
                Kind::Devices(_, pair) => {
                    let line = b(0x1D1D1D)?;
                    let inner = b(0x272727)?;
                    let current = v.list.iter().find(|d| d.2).map(|d| d.0.as_str()).unwrap_or("");
                    txt(current, &right, rect(r.left + 300.0, r.top, r.right - 45.0, r.top + ROW_H), &dim);
                    ico(0xE70E, &icons10, rect(r.right - 36.0, r.top, r.right - 12.0, r.top + ROW_H), &white);
                    let mut y = r.top + ROW_H;
                    for (k, (name, adapter, on, _)) in v.list.iter().enumerate() {
                        let dr = rect(r.left + 1.0, y, r.right - 1.0, y + 70.0);
                        rt.FillRectangle(&dr, &b(if hover == Some(Hit::Radio(i, k)) { 0x2F2F2F } else { 0x272727 })?);
                        rt.FillRectangle(&rect(r.left, y, r.right, y + 1.0), &line);
                        let c = windows_numerics::Vector2 { X: r.left + 68.0, Y: y + 26.0 };
                        if *on {
                            rt.FillEllipse(&D2D1_ELLIPSE { point: c, radiusX: 9.5, radiusY: 9.5 }, &accent);
                            rt.FillEllipse(&D2D1_ELLIPSE { point: c, radiusX: 4.5, radiusY: 4.5 }, &b(0x1C1C1C)?);
                        } else {
                            rt.DrawEllipse(&D2D1_ELLIPSE { point: c, radiusX: 9.0, radiusY: 9.0 }, &b(0xC5C5C5)?, 1.0, None);
                        }
                        txt(name, &body, rect(r.left + 86.0, y + 7.0, r.right - 60.0, y + 29.0), &white);
                        txt(adapter, &small, rect(r.left + 86.0, y + 27.0, r.right - 60.0, y + 45.0), &dim);
                        txt(if *on { "Default device" } else { "Enabled" }, &small, rect(r.left + 86.0, y + 43.0, r.right - 60.0, y + 61.0), &dim);
                        ico(0xE76C, &icons12, rect(r.right - 36.0, y, r.right - 12.0, y + 70.0), &white);
                        hits.push((r, Hit::Row(i)));
                        hits.push((dr, Hit::Radio(i, k)));
                        y += 70.0;
                    }
                    rt.FillRectangle(&rect(r.left + 1.0, y + 1.0, r.right - 1.0, r.bottom - 1.0), &inner);
                    rt.FillRectangle(&rect(r.left, y, r.right, y + 1.0), &line);
                    txt(pair, &body, rect(r.left + 58.0, y, r.right - 190.0, y + 50.0), &white);
                    let br = rect(r.right - 174.0, y + 9.0, r.right - 44.0, y + 41.0);
                    rt.FillRoundedRectangle(&rrect(br, 4.0), &b(if hover == Some(Hit::Pair(i)) { 0x3D3D3D } else { 0x373737 })?);
                    let centered = fmt(14.0, n, ui_font)?;
                    centered.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                    txt("Add device", &centered, br, &white);
                    hits.push((br, Hit::Pair(i)));
                    continue;
                }
            }
            hits.push((r, Hit::Row(i)));
        }
        if let Some(f) = ui.focus {
            // Windows' keyboard focus ring: 2px white, rounded, just outside the card
            let r = row_rect(page, f, cw);
            rt.DrawRoundedRectangle(&rrect(rect(r.left - 3.0, r.top - 3.0, r.right + 3.0, r.bottom + 3.0), 7.0), &white, 2.0, None);
        }
        rt.PopAxisAlignedClip();
        if enter < 1.0 {
            rt.SetTransform(&windows_numerics::Matrix3x2::identity());
            let fade = rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0x20 as f32 / 255.0, g: 0x20 as f32 / 255.0, b: 0x20 as f32 / 255.0, a: 1.0 - enter }, None)?;
            rt.FillRectangle(&rect(CARD_X - 20.0, 46.0, cw, ch), &fade);
        }
        // an open drop-down: Windows' list, laid over the control with the chosen option where the box was
        if let Some((i, opts, cur)) = ui.drop.clone() {
            hits.push((rect(0.0, 0.0, cw, ch), Hit::DropOff));
            let r = row_rect(page, i, cw);
            let widest = opts.iter().map(|o| { let l = dw.CreateTextLayout(&utf16(o), &body, 900.0, 32.0); l.map(|l| { let mut m = DWRITE_TEXT_METRICS::default(); let _ = l.GetMetrics(&mut m); m.width }).unwrap_or(100.0) }).fold(0f32, f32::max);
            let lw = (widest + 56.0).max(160.0);
            let lh = opts.len() as f32 * 36.0 + 8.0;
            let top = (r.top + 18.0 - 4.0 - cur as f32 * 36.0).clamp(48.0, (ch - lh - 8.0).max(48.0));
            let lr = rect(r.right - 16.0 - lw, top, r.right - 16.0, top + lh);
            rt.FillRoundedRectangle(&rrect(rect(lr.left - 1.0, lr.top + 2.0, lr.right + 1.0, lr.bottom + 4.0), 8.0), &rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.35 }, None)?);
            rt.FillRoundedRectangle(&rrect(lr, 8.0), &b(0x2C2C2C)?);
            rt.DrawRoundedRectangle(&rrect(rect(lr.left + 0.5, lr.top + 0.5, lr.right - 0.5, lr.bottom - 0.5), 8.0), &b(0x3B3B3B)?, 1.0, None);
            for (k, o) in opts.iter().enumerate() {
                let ir = rect(lr.left + 4.0, lr.top + 4.0 + k as f32 * 36.0, lr.right - 4.0, lr.top + 4.0 + k as f32 * 36.0 + 36.0);
                if k == cur || hover == Some(Hit::Drop(k)) {
                    rt.FillRoundedRectangle(&rrect(ir, 4.0), &b(if k == cur { 0x383838 } else { 0x333333 })?);
                }
                if k == cur {
                    rt.FillRoundedRectangle(&rrect(rect(ir.left, ir.top + 10.0, ir.left + 3.0, ir.bottom - 10.0), 1.5), &accent);
                }
                txt(o, &body, rect(ir.left + 12.0, ir.top, ir.right - 8.0, ir.bottom), &white);
                hits.push((ir, Hit::Drop(k)));
            }
        }
        ui.hits = hits;
        Ok(())
    })
}

unsafe fn paint(hwnd: HWND) {
    let t0 = std::time::Instant::now();
    paint_inner(hwnd);
    crate::flyout::paint_log("settings", t0);
}

unsafe fn paint_inner(hwnd: HWND) {
    let mut ps = PAINTSTRUCT::default();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let params = BP_PAINTPARAMS { cbSize: size_of::<BP_PAINTPARAMS>() as u32, dwFlags: BPPF_ERASE, ..Default::default() };
    let mut mem = HDC::default();
    let pb = BeginBufferedPaint(hdc, &rc, BPBF_TOPDOWNDIB, Some(&params), &mut mem);
    if !mem.is_invalid() {
        let dpi = GetDpiForWindow(hwnd) as f32;
        let max = IsZoomed(hwnd).as_bool();
        UI.with(|u| {
            let mut ui = u.borrow_mut();
            if ui.gfx.is_none() {
                ui.gfx = make_gfx().ok();
            }
            if let Some((rt, _)) = ui.gfx.clone() {
                let _ = rt.BindDC(mem, &rc);
                rt.SetDpi(dpi, dpi);
                rt.BeginDraw();
                let _ = draw(&mut ui, rc.right as f32 * 96.0 / dpi, rc.bottom as f32 * 96.0 / dpi, max);
                let _ = rt.EndDraw(None, None);
            }
        });
        let _ = BufferedPaintSetAlpha(pb, None, 255);
        let _ = EndBufferedPaint(pb, true);
    }
    let _ = EndPaint(hwnd, &ps);
}

unsafe fn make_gfx() -> windows::core::Result<(ID2D1DCRenderTarget, IDWriteFactory)> {
    let f: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE }, r#type: crate::flyout::rt_type(),
        ..Default::default()
    };
    let rt = f.CreateDCRenderTarget(&props)?;
    crate::flyout::crisp_text(&rt);
    rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
    Ok((rt, DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?))
}

/// A mixer slider moved to `x`: the card shows it at once; the app's volume follows (on its own thread).
fn set_vol(r: D2D_RECT_F, i: usize, k: usize, x: f32) {
    let (x0, x1) = (r.right - 300.0, r.right - 24.0);
    let f = ((x - x0) / (x1 - x0)).clamp(0.0, 1.0);
    let target = UI.with(|u| {
        let mut ui = u.borrow_mut();
        let page = ui.page;
        let name = ui.vals.get_mut(i).and_then(|v| v.list.get_mut(k)).map(|d| {
            d.1 = format!("{:.0}", f * 100.0);
            d.0.clone()
        });
        let set = PAGES.with(|p| if let Kind::Volumes(_, set) = p[page].rows[i].kind { Some(set) } else { None });
        name.zip(set)
    });
    if let Some((name, set)) = target {
        std::thread::spawn(move || {
            let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
            set(&name, f);
        });
    }
}

fn apply_slider(i: usize, f: f32, commit: bool) {
    UI.with(|u| {
        let mut ui = u.borrow_mut();
        if let Some(v) = ui.vals.get_mut(i) {
            v.f = f;
        }
        let (kind, label) = PAGES.with(|p| (p[ui.page].rows[i].kind, p[ui.page].rows[i].label));
        if let Kind::Slider(_, set) = kind {
            // volume follows the drag; slower settings (brightness) apply when you let go
            if commit || label == "Volume" {
                std::thread::spawn(move || set(f));
            }
        }
    });
}

const T_GLIDE: usize = 0x6C2;
const T_PAGE: usize = 0x6C5;
/// When the page on screen was opened (for its entrance).
static PAGE_T0: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
/// How far a newly opened page is into its entrance, 0..1 (eased); 1 when it is still.
fn page_enter() -> f32 {
    static STILL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *STILL.get_or_init(|| std::env::args().any(|a| a.starts_with("--snapshot"))) {
        return 1.0; // off-screen pictures show the page at rest
    }
    match *PAGE_T0.lock().unwrap() {
        Some(t0) => {
            let k = (t0.elapsed().as_millis() as f32 / 250.0).min(1.0);
            1.0 - (1.0 - k).powi(3)
        }
        None => 1.0,
    }
}
const T_SWITCH: usize = 0x6C4;
/// A switch's knob slides over this long when flipped (Windows' own takes about this).
const SWITCH_MS: u128 = 160;
/// The switch being flipped: page, row, when.
static SWITCH_ANIM: std::sync::Mutex<Option<(usize, usize, std::time::Instant)>> = std::sync::Mutex::new(None);

/// Where a switch's knob is (0 off .. 1 on): sliding toward `on` while it is the one just flipped.
fn switch_pos(page: usize, i: usize, on: bool) -> f32 {
    let target = if on { 1.0 } else { 0.0 };
    match *SWITCH_ANIM.lock().unwrap() {
        Some((p, r, t0)) if p == page && r == i => {
            let k = (t0.elapsed().as_millis() as f32 / SWITCH_MS as f32).min(1.0);
            let ease = 1.0 - (1.0 - k).powi(3); // fast then settling, like Windows' motion
            (1.0 - target) + (target - (1.0 - target)) * ease
        }
        _ => target,
    }
}

/// An ms-settings: link opens our own page when there is one (and it is not the page you are on - those
/// rows exist to reach what only Windows' page has); anything else goes to Windows.
fn open_link(uri: &str) {
    let here = UI.with(|u| u.borrow().page);
    match page_for_uri(&uri.to_lowercase()).and_then(page_index) {
        Some(p) if p != here => go(p),
        _ => unsafe {
            windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &HSTRING::from(uri), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        },
    }
}

fn go(page: usize) {
    *PAGE_T0.lock().unwrap() = Some(std::time::Instant::now());
    unsafe {
        let h = HWND(WIN.load(Relaxed) as *mut c_void);
        if !h.is_invalid() {
            SetTimer(Some(h), T_PAGE, 10, None);
        }
    }
    UI.with(|u| {
        let mut ui = u.borrow_mut();
        ui.page = page;
        ui.vals.clear();
        ui.query.clear();
        ui.drop = None;
        ui.focus = None;
    });
    SCROLL.with(|sc| sc.set(0.0));
    GOAL.with(|g| g.set(None));
    COUNTS.with(|c| c.borrow_mut().clear());
    load(page);
}

fn hit(x: f32, y: f32) -> Option<Hit> {
    UI.with(|u| u.try_borrow().ok().and_then(|ui| ui.hits.iter().rev().find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|(_, h)| *h)))
}

pub unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Re-entered while the window's state is in use: a handler called into Windows (a shell call, a
    // menu, a dialog) and that ran a message loop. Touching the state again would panic and close the
    // window, so this message gets default handling instead.
    if UI.with(|u| u.try_borrow_mut().is_err()) && !matches!(msg, WM_NCCREATE | WM_CREATE | WM_NCDESTROY | WM_DESTROY) {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let s = GetDpiForWindow(hwnd) as f32 / 96.0;
    let dips = |lp: LPARAM| ((lp.0 & 0xFFFF) as i16 as f32 / s, ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s);
    let cw = || {
        let mut rc = RECT::default();
        let _ = GetClientRect(hwnd, &mut rc);
        rc.right as f32 / s
    };
    let redraw = || {
        let _ = InvalidateRect(Some(hwnd), None, false);
    };
    let row_at = |i: usize| UI.with(|u| PAGES.with(|p| row_rect(&p[u.borrow().page], i, cw())));
    match msg {
        // our own title bar, like Settings': the whole window is client area
        WM_NCCALCSIZE if wp.0 == 1 => {
            if IsZoomed(hwnd).as_bool() {
                let p = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
                let f = GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                p.rgrc[0].left += f;
                p.rgrc[0].top += f;
                p.rgrc[0].right -= f;
                p.rgrc[0].bottom -= f;
            }
            return LRESULT(0);
        }
        WM_NCHITTEST => {
            let mut pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
            let _ = ScreenToClient(hwnd, &mut pt);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let e = (6.0 * s) as i32;
            if !IsZoomed(hwnd).as_bool() {
                let (l, t, r, bt) = (pt.x < e, pt.y < e, pt.x >= rc.right - e, pt.y >= rc.bottom - e);
                let code = match (l, t, r, bt) {
                    (true, true, _, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, _, true, true) => HTBOTTOMRIGHT,
                    (true, ..) => HTLEFT,
                    (_, true, ..) => HTTOP,
                    (_, _, true, _) => HTRIGHT,
                    (.., true) => HTBOTTOM,
                    _ => HTCLIENT,
                };
                if code != HTCLIENT {
                    return LRESULT(code as isize);
                }
            }
            let (x, y) = (pt.x as f32 / s, pt.y as f32 / s);
            if y < TOPBAR && hit(x, y).is_none() && !(CARD_X..839.0).contains(&x) {
                return LRESULT(HTCAPTION as isize);
            }
            return LRESULT(HTCLIENT as isize);
        }
        WM_VALUES => {
            let (page, vals) = VALUES.lock().unwrap().clone();
            UI.with(|u| {
                let mut ui = u.borrow_mut();
                if ui.page == page && ui.drag.is_none() {
                    set_counts(&vals);
                    ui.vals = vals;
                }
            });
            redraw();
        }
        WM_CHAR => {
            // typing anywhere searches, like the Find a setting box
            UI.with(|u| {
                let mut ui = u.borrow_mut();
                match wp.0 as u32 {
                    8 => {
                        ui.query.pop();
                    }
                    27 => ui.query.clear(),
                    c if c >= 32 => {
                        if let Some(ch) = char::from_u32(c) {
                            ui.query.push(ch);
                        }
                    }
                    _ => {}
                }
            });
            redraw();
        }
        WM_MOUSEMOVE => {
            let (x, y) = dips(lp);
            if let Some((i, k)) = UI.with(|u| u.borrow().drag_vol) {
                set_vol(row_at(i), i, k, x);
                redraw();
            } else if let Some(i) = UI.with(|u| u.borrow().drag) {
                let (x0, x1) = slider_x(&row_at(i));
                apply_slider(i, ((x - x0) / (x1 - x0)).clamp(0.0, 1.0), false);
                redraw();
            } else {
                let h = hit(x, y);
                if UI.with(|u| std::mem::replace(&mut u.borrow_mut().hover, h) != h) {
                    let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                    redraw();
                }
            }
        }
        WM_MOUSELEAVE => {
            UI.with(|u| u.borrow_mut().hover = None);
            redraw();
        }
        WM_LBUTTONDOWN => {
            let (x, y) = dips(lp);
            let _ = SetFocus(Some(hwnd));
            UI.with(|u| u.borrow_mut().focus = None);
            if let Some(Hit::Vol(i, k)) = hit(x, y) {
                SetCapture(hwnd);
                UI.with(|u| u.borrow_mut().drag_vol = Some((i, k)));
                set_vol(row_at(i), i, k, x);
                redraw();
                return LRESULT(0);
            }
            if let Some(Hit::Row(i)) = hit(x, y) {
                let kind = UI.with(|u| PAGES.with(|p| p[u.borrow().page].rows[i].kind));
                let (x0, x1) = slider_x(&row_at(i));
                if matches!(kind, Kind::Slider(..)) && x >= x0 - 12.0 && x <= x1 + 12.0 {
                    SetCapture(hwnd);
                    UI.with(|u| u.borrow_mut().drag = Some(i));
                    apply_slider(i, ((x - x0) / (x1 - x0)).clamp(0.0, 1.0), false);
                    redraw();
                }
            }
        }
        WM_LBUTTONUP => {
            let (x, y) = dips(lp);
            if let Some((i, k)) = UI.with(|u| u.borrow_mut().drag_vol.take()) {
                let _ = ReleaseCapture();
                set_vol(row_at(i), i, k, x);
                redraw();
                return LRESULT(0);
            }
            if let Some(i) = UI.with(|u| u.borrow_mut().drag.take()) {
                let _ = ReleaseCapture();
                let (x0, x1) = slider_x(&row_at(i));
                apply_slider(i, ((x - x0) / (x1 - x0)).clamp(0.0, 1.0), true);
                redraw();
                return LRESULT(0);
            }
            match hit(x, y) {
                Some(Hit::Nav(i)) | Some(Hit::Result(i)) => go(i),
                Some(Hit::Back) => {
                    let parent = UI.with(|u| {
                        let mut ui = u.borrow_mut();
                        if !ui.query.is_empty() {
                            ui.query.clear();
                            return None;
                        }
                        PAGES.with(|p| page_index(p[ui.page].parent))
                    });
                    if let Some(p) = parent {
                        go(p);
                    }
                }
                Some(Hit::Win(0)) => {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                Some(Hit::Win(1)) => {
                    let _ = ShowWindow(hwnd, if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                }
                Some(Hit::Win(_)) => {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                Some(Hit::Radio(i, k)) => {
                    let (page, id) = UI.with(|u| {
                        let ui = u.borrow();
                        (ui.page, ui.vals.get(i).and_then(|v| v.list.get(k)).map(|d| d.3.clone()))
                    });
                    if let Some(id) = id {
                        std::thread::spawn(move || {
                            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                            crate::qsx::set_output(&id);
                            load(page); // show what Windows actually did
                        });
                    }
                }
                // a name without ':' is one of our own pages
                Some(Hit::Open(uri)) if !uri.contains(':') => {
                    if let Some(p) = page_index(uri) { go(p); }
                }
                Some(Hit::Open(uri)) => open_link(uri),
                Some(Hit::Tile(k)) => {
                    // Connect on a device tile: audio devices connect here (the call Windows' Sound settings
                    // make); a device that cannot be connected that way opens Windows' Bluetooth page
                    if let Some(d) = crate::qsx::bt_devices().into_iter().nth(k) {
                        bt_manage("", &d.name);
                        let h = hwnd.0 as isize;
                        std::thread::spawn(move || { std::thread::sleep(std::time::Duration::from_millis(2500)); let _ = unsafe { InvalidateRect(Some(HWND(h as *mut _)), None, false) }; });
                    }
                }
                Some(Hit::Vol(..)) => {} // handled on the press
                Some(Hit::DropOff) => UI.with(|u| u.borrow_mut().drop = None),
                Some(Hit::Drop(k)) => {
                    let (page, picked) = UI.with(|u| {
                        let mut ui = u.borrow_mut();
                        let page = ui.page;
                        let picked = ui.drop.take().map(|(i, opts, _)| {
                            if let Some(v) = ui.vals.get_mut(i) {
                                v.s = opts.get(k).cloned().unwrap_or_default(); // shown at once
                            }
                            i
                        });
                        (page, picked)
                    });
                    if let Some(i) = picked {
                        if let Some(Kind::Choice(_, _, set)) = pages().get(page).and_then(|p| p.rows.get(i)).map(|r| r.kind) {
                            std::thread::spawn(move || {
                                set(k);
                                load(page); // show what Windows actually did
                            });
                        }
                    }
                }
                Some(Hit::Theme(k)) => {
                    // opening a .theme file is how Windows applies it
                    let file = UI.with(|u| u.borrow().vals.iter().find_map(|v| v.list.get(k).filter(|l| l.0.to_lowercase().ends_with(".theme")).map(|l| l.0.clone())));
                    if let Some(f) = file {
                        windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &HSTRING::from(f), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
                    }
                }
                Some(Hit::Copy(i)) => {
                    let text = UI.with(|u| u.borrow().vals.get(i).map(|v| v.list.iter().map(|l| format!("{}\t{}", l.0, l.1.replace('\n', ", "))).collect::<Vec<_>>().join("\r\n")).unwrap_or_default());
                    copy_text(&text);
                }
                Some(Hit::Sort) => {
                    BY_NAME.store(!BY_NAME.load(Relaxed), Relaxed);
                    load(UI.with(|u| u.borrow().page));
                }
                Some(Hit::Sender(i, k)) if matches!(pages().get(UI.with(|u| u.borrow().page)).and_then(|p| p.rows.get(i)).map(|r| r.kind), Some(Kind::Actions(..))) => {
                    // a card's button: do its action for that item, then read the list again
                    let (page, item) = UI.with(|u| { let ui = u.borrow(); (ui.page, ui.vals.get(i).and_then(|v| v.list.get(k)).map(|d| (d.3.clone(), d.0.clone()))) });
                    if let (Some((id, name)), Some(Kind::Actions(_, _, act))) = (item, pages().get(page).and_then(|p| p.rows.get(i)).map(|r| r.kind)) {
                        std::thread::spawn(move || {
                            act(&id, &name);
                            load(page);
                        });
                    }
                }
                Some(Hit::Sender(i, k)) => {
                    let (page, app) = UI.with(|u| {
                        let mut ui = u.borrow_mut();
                        let app = ui.vals.get_mut(i).and_then(|v| v.list.get_mut(k)).map(|d| {
                            d.2 = !d.2;
                            (d.3.clone(), d.2)
                        });
                        (ui.page, app)
                    });
                    let set = pages().get(page).and_then(|p| p.rows.get(i)).and_then(|r| if let Kind::Senders(_, set) = r.kind { Some(set) } else { None });
                    if let (Some((id, on)), Some(set)) = (app, set) {
                        std::thread::spawn(move || {
                            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                            set(&id, on);
                            load(page);
                        });
                    }
                }
                Some(Hit::Mute) => {
                    let page = UI.with(|u| u.borrow().page);
                    std::thread::spawn(move || {
                        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                        set_muted(!muted());
                        load(page);
                    });
                }
                Some(Hit::Pair(_)) => {
                    windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("ms-settings-connectabledevices:devicediscovery"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
                }
                Some(Hit::Rename) => {
                    // renaming the PC needs an administrator: Windows' own small rename dialog asks for that
                    // (and for the restart), rather than all of Windows' Settings app
                    windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), w!("SystemPropertiesComputerName.exe"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
                }
                Some(Hit::Row(i)) => {
                    let (page, kind, on) = UI.with(|u| {
                        let ui = u.borrow();
                        (ui.page, PAGES.with(|p| p[ui.page].rows[i].kind), ui.vals.get(i).is_some_and(|v| v.on))
                    });
                    match kind {
                        Kind::Toggle(_, set) | Kind::ToggleMore(_, set, _) => {
                            UI.with(|u| {
                                if let Some(v) = u.borrow_mut().vals.get_mut(i) {
                                    v.on = !on;
                                }
                            });
                            *SWITCH_ANIM.lock().unwrap() = Some((page, i, std::time::Instant::now()));
                            SetTimer(Some(hwnd), T_SWITCH, 10, None);
                            std::thread::spawn(move || {
                                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                                set(!on);
                                std::thread::sleep(std::time::Duration::from_millis(900));
                                load(page); // show what Windows actually did
                            });
                        }
                        // a card that names one of our own pages (not an ms-settings: address) opens that page
                        Kind::LinkBtn(name, ..) if !name.contains(':') => {
                            if let Some(p) = page_index(name) {
                                go(p);
                            }
                        }
                        Kind::Link(uri) | Kind::Expand(uri) | Kind::LinkBtn(uri, ..) => open_link(uri),
                        Kind::Sub(name) => {
                            if let Some(p) = page_index(name) {
                                go(p);
                            }
                        }
                        Kind::State(_, uri) => {
                            windows::Win32::UI::Shell::ShellExecuteW(None, w!("open"), &HSTRING::from(uri), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
                        }
                        Kind::Choice(_, options, _) => {
                            let now = UI.with(|u| u.borrow().vals.get(i).map(|v| v.s.clone()).unwrap_or_default());
                            let opts = options();
                            let cur = opts.iter().position(|o| *o == now).unwrap_or(0);
                            UI.with(|u| u.borrow_mut().drop = Some((i, opts, cur)));
                        }
                        Kind::Flat(_, options, set) => {
                            {
                                let now = UI.with(|u| u.borrow().vals.get(i).map(|v| v.s.clone()).unwrap_or_default());
                                let items: Vec<crate::menu::Item> = options().iter().enumerate().map(|(k, o)| crate::menu::Item::new(k as u32 + 1, 0, o).checked(*o == now)).collect();
                                let mut pt = POINT::default();
                                let _ = GetCursorPos(&mut pt);
                                let cmd = crate::menu::pick(hwnd, items, pt.x - 12, pt.y + 8).unwrap_or(0);
                                if cmd > 0 {
                                    std::thread::spawn(move || {
                                        set(cmd as usize - 1);
                                        load(page); // show what Windows actually did
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                }
                None => {}
            }
            redraw();
        }
        WM_KEYDOWN => {
            let vk = wp.0 as u16;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            match vk {
                0x1B => {
                    // Esc: an open list closes; else back to the parent page
                    let had = UI.with(|u| u.borrow_mut().drop.take().is_some());
                    if !had {
                        let parent = UI.with(|u| pages().get(u.borrow().page).map(|p| p.parent).unwrap_or(""));
                        if let Some(p) = page_index(parent) { go(p); }
                    }
                    redraw();
                }
                0x28 | 0x26 | 0x09 => {
                    // Down / Up / Tab / Shift+Tab: to the next row you can act on, kept in view
                    let back = vk == 0x26 || (vk == 0x09 && shift);
                    let next = UI.with(|u| {
                        let ui = u.borrow();
                        let rows: Vec<Kind> = pages().get(ui.page).map(|p| p.rows.iter().map(|r| r.kind).collect()).unwrap_or_default();
                        next_focus(&rows, ui.focus, back)
                    });
                    if let Some(f) = next {
                        UI.with(|u| u.borrow_mut().focus = Some(f));
                        let r = row_at(f);
                        let mut rc = RECT::default();
                        let _ = GetClientRect(hwnd, &mut rc);
                        let view = rc.bottom as f32 / s;
                        if r.top < 110.0 { SCROLL.with(|sc| sc.set((sc.get() - (110.0 - r.top)).max(0.0))); }
                        if r.bottom > view - 16.0 { SCROLL.with(|sc| sc.set(sc.get() + (r.bottom - (view - 16.0)))); }
                    }
                    redraw();
                }
                0x0D | 0x20 => {
                    // Enter / Space: what a click on that row does
                    if let Some(f) = UI.with(|u| u.borrow().focus) {
                        let r = row_at(f);
                        let (x, y) = (((r.left + 40.0) * s) as isize, (((r.top + r.bottom) / 2.0) * s) as isize);
                        let _ = PostMessageW(Some(hwnd), WM_LBUTTONUP, WPARAM(0), LPARAM((y << 16) | (x & 0xFFFF)));
                    }
                }
                _ => return DefWindowProcW(hwnd, msg, wp, lp),
            }
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            let d = ((wp.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let need = UI.with(|u| PAGES.with(|p| { let pg = &p[u.borrow().page]; row_tops(&pg.rows.iter().map(|r| r.kind).collect::<Vec<_>>(), &COUNTS.with(|c| c.borrow().clone()), first_row_y(pg)).1 + 24.0 }));
            let max = (need - rc.bottom as f32 / s).max(0.0);
            // glide there (~100 ms) instead of jumping 60 px a notch
            let goal = (GOAL.with(|g| g.get()).unwrap_or(SCROLL.with(|sc| sc.get())) - d * 60.0).clamp(0.0, max);
            GOAL.with(|g| g.set(Some(goal)));
            SetTimer(Some(hwnd), T_GLIDE, 8, None);
        }
        WM_TIMER if wp.0 == T_PAGE => {
            if page_enter() >= 1.0 {
                *PAGE_T0.lock().unwrap() = None;
                let _ = KillTimer(Some(hwnd), T_PAGE);
            }
            redraw();
        }
        WM_TIMER if wp.0 == T_SWITCH => {
            let done = SWITCH_ANIM.lock().unwrap().is_none_or(|a| a.2.elapsed().as_millis() >= SWITCH_MS);
            if done {
                *SWITCH_ANIM.lock().unwrap() = None;
                let _ = KillTimer(Some(hwnd), T_SWITCH);
            }
            redraw();
        }
        WM_TIMER if wp.0 == T_GLIDE => {
            match GOAL.with(|g| g.get()) {
                Some(goal) => {
                    let (next, done) = crate::files::glide(SCROLL.with(|sc| sc.get()), goal);
                    SCROLL.with(|sc| sc.set(next));
                    if done { GOAL.with(|g| g.set(None)); }
                }
                None => { let _ = KillTimer(Some(hwnd), T_GLIDE); }
            }
            redraw();
        }
        WM_SIZE => redraw(),
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => paint(hwnd),
        WM_CLOSE => {
            // where it was, how big, and which page: the next open comes back the same
            let mut wpl = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            if GetWindowPlacement(hwnd, &mut wpl).is_ok() {
                let r = wpl.rcNormalPosition;
                let page = UI.with(|u| u.borrow().page);
                let name = pages().get(page).map(|p| p.name).unwrap_or("");
                let _ = std::fs::write(place_file(), format!("{} {} {} {} {}\n{name}", r.left, r.top, r.right - r.left, r.bottom - r.top, (wpl.showCmd == SW_SHOWMAXIMIZED.0 as u32) as i32));
            }
            let _ = ShowWindow(hwnd, SW_HIDE);
            UI.with(|u| u.borrow_mut().gfx = None);
            // hand the memory back: the window is gone, the app keeps running
            let _ = windows::Win32::System::ProcessStatus::K32EmptyWorkingSet(windows::Win32::System::Threading::GetCurrentProcess());
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

fn place_file() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("shellbits").join("settings-place.txt")
}

pub unsafe fn show(inst: HINSTANCE) {
    let mut h = HWND(WIN.load(Relaxed) as *mut c_void);
    let first = h.is_invalid();
    if h.is_invalid() {
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: w!("shellbits_settings"), hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);
        let Ok(w_) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("shellbits_settings"), w!("Settings"), WS_OVERLAPPEDWINDOW, CW_USEDEFAULT, CW_USEDEFAULT, 1180, 790, None, None, Some(inst), None) else { return };
        h = w_;
        WIN.store(h.0 as isize, Relaxed);
        let dark = 1u32;
        let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, 4);
        let _ = DwmExtendFrameIntoClientArea(h, &MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 1, cyBottomHeight: 0 });
        let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
    }
    if !first && !IsWindowVisible(h).as_bool() {
        // closing only hid it: reopening starts at Home, as a new Windows Settings window does
        if let Some(i) = page_index("Home") { UI.with(|u| { let mut u = u.borrow_mut(); u.page = i; u.query.clear(); }); }
    }
    if first {
        // the size and place it had when it was last closed
        let saved = std::fs::read_to_string(place_file()).unwrap_or_default();
        let mut lines = saved.lines();
        if let Some((x, y, cw, ch, max)) = lines.next().and_then(crate::photos::parse_place) {
            let wpl = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, showCmd: if max { SW_SHOWMAXIMIZED.0 as u32 } else { SW_HIDE.0 as u32 }, rcNormalPosition: RECT { left: x, top: y, right: x + cw, bottom: y + ch }, ..Default::default() };
            let _ = SetWindowPlacement(h, &wpl);
        }
        // Windows' Settings always starts at Home: the page is not restored
        if let Some(i) = page_index("Home") {
            UI.with(|u| u.borrow_mut().page = i);
        }
    }
    // opened for a particular page (an ms-settings: link): go there
    if let Some(i) = take_request().and_then(|n| page_index(&n)) {
        UI.with(|u| { let mut u = u.borrow_mut(); u.page = i; u.query.clear(); });
    }
    load(UI.with(|u| u.borrow().page));
    crate::flyout::show_painted(h, SW_SHOW); // painted before it appears: never an empty frame
    crate::flyout::force_foreground(h);
}

/// `shellbits --snapshot-settings out.bmp [page name]`: one page with live values, drawn off-screen
/// at the size of the reference capture (1164 x 774).
pub unsafe fn snapshot(path: &str, page: &str) -> windows::core::Result<()> {
    let page = page_index(page).unwrap_or(1);
    load(page);
    for _ in 0..250 {
        // keep handling messages while we wait: some Windows queries answer through this thread
        let mut m = MSG::default();
        while PeekMessageW(&mut m, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        if VALUES.lock().unwrap().0 == page {
            break;
        }
    }
    let (w_, h) = (1164, 774);
    UI.with(|u| -> windows::core::Result<()> {
        let mut ui = u.borrow_mut();
        ui.page = page;
        ui.vals = VALUES.lock().unwrap().1.clone();
        set_counts(&ui.vals);
        ui.gfx = Some(make_gfx()?);
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w_, biHeight: -h, biPlanes: 1, biBitCount: 32, ..Default::default() },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let dc = CreateCompatibleDC(None);
        let old = SelectObject(dc, dib.into());
        let rt = ui.gfx.as_ref().unwrap().0.clone();
        rt.BindDC(dc, &RECT { left: 0, top: 0, right: w_, bottom: h })?;
        rt.BeginDraw();
        draw(&mut ui, w_ as f32, h as f32, false)?;
        rt.EndDraw(None, None)?;
        let _ = GdiFlush();
        let px = std::slice::from_raw_parts(bits as *const u8, (w_ * h * 4) as usize);
        let mut out = Vec::with_capacity(54 + px.len());
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&w_.to_le_bytes());
        out.extend_from_slice(&(-h).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&[0; 24]);
        for p in px.chunks(4) {
            out.extend_from_slice(&[p[0], p[1], p[2], 255]);
        }
        let _ = std::fs::write(path, out);
        SelectObject(dc, old);
        let _ = DeleteDC(dc);
        let _ = DeleteObject(dib.into());
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn settings_links_and_night_light() {
        assert_eq!(super::page_for_uri("ms-settings:display"), Some("Display"));
        assert_eq!(super::page_for_uri("ms-settings:privacy-webcam"), Some("Camera"));
        assert_eq!(super::page_for_uri("ms-settings:unknownpage"), None);
        assert_eq!(super::page_for_uri("calc.exe"), None);
        let mut off = vec![0u8; 40];
        off[18] = 0x13;
        let on = super::night_blob(&off, true).unwrap();
        assert_eq!((on.len(), on[18], on[23], on[24], on[10]), (42, 0x15, 0x10, 0x00, 1));
        let back = super::night_blob(&on, false).unwrap();
        assert_eq!((back.len(), back[18]), (40, 0x13));
    }

    #[test]
    fn store_package_names() {
    }

    #[test]
    fn privacy_and_time_helpers() {
        assert_eq!(super::consent_name("Microsoft.WindowsCamera_8wekyb3d8bbwe"), "WindowsCamera");
        assert_eq!(super::consent_name("C:#Program Files#Zoom#bin#Zoom.exe"), "Zoom.exe");
        assert_eq!(super::zone_offset("(UTC+05:30) Chennai, Kolkata, Mumbai, New Delhi"), 330);
        assert_eq!(super::zone_offset("(UTC-08:00) Pacific Time (US & Canada)"), -480);
        assert_eq!(super::zone_offset("(UTC) Coordinated Universal Time"), 0);
    }

    #[test]
    fn network_lists() {
        assert_eq!(super::wlan_profiles("Profiles on interface Wi-Fi:

User profiles
-------------
    All User Profile     : Home 5G
    All User Profile     : cafe
"), vec!["Home 5G", "cafe"]);
        assert_eq!(super::pbk_entries("[Work VPN]
MEDIA=rastapi
[Home]
"), vec!["Work VPN", "Home"]);
    }

    #[test]
    fn system_page_helpers() {
        assert_eq!(super::gpu_label("GpuPreference=2;"), "High performance");
        assert_eq!(super::gpu_label("SwapEffectUpgradeEnable=1;"), "Let Windows decide");
        assert_eq!(super::idle_label(300), "5 minutes");
        assert_eq!(super::idle_label(0), "Never");
        assert_eq!(super::idle_label(420), "7 minutes");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(super::civil(0), (1970, 1, 1));
        assert_eq!(super::civil(20405), (2025, 11, 13));
    }

    use super::*;

    #[test]
    fn week_start_names() {
        assert_eq!(first_day("0"), "Monday");
        assert_eq!(first_day("6"), "Sunday");
        assert_eq!(first_day("x"), "Sunday");
    }

    #[test]
    fn default_printer_value() {
        assert_eq!(default_printer_of("HPDE4E14 (HP Smart Tank 510 series),winspool,Ne00:"), "HPDE4E14 (HP Smart Tank 510 series)");
        assert_eq!(default_printer_of(""), "");
    }

    #[test]
    fn fits_and_theme_names() {
        assert_eq!(FITS[fit_index("10", "0")].0, "Fill");
        assert_eq!(FITS[fit_index("0", "1")].0, "Tile");
        assert_eq!(FITS[fit_index("22", "0")].0, "Span");
        assert_eq!(FITS[fit_index("", "")].0, "Fill"); // nothing stored
        assert_eq!(theme_name("aero"), "Windows (light)");
        assert_eq!(theme_name("themeA"), "Glow");
        assert_eq!(theme_name("custom"), "Custom");
    }

    #[test]
    fn mouse_and_time_labels() {
        assert_eq!(wheel_label(1), "1 line");
        assert_eq!(wheel_label(3), "3 lines");
        assert_eq!(wheel_label(0xFFFF_FFFF), "One screen at a time");
        assert_eq!(utc_offset(330), "(UTC+05:30)");
        assert_eq!(utc_offset(-300), "(UTC-05:00)");
        assert_eq!(utc_offset(0), "(UTC+00:00)");
    }

    #[test]
    fn stored_choices_and_drive_lines() {
        assert_eq!(choice_index(Some(2), &[0, 1, 2, 3], 3), 2);
        assert_eq!(choice_index(None, &[0, 1], 1), 1);       // nothing stored: Windows' default
        assert_eq!(choice_index(Some(9), &[0, 1], 1), 1);    // a number that is not one of the options
        assert_eq!(drive_line(500 * 1024 * 1024 * 1024, 100 * 1024 * 1024 * 1024), "400 GB used of 500 GB  |  100 GB free");
        assert_eq!(drive_line(10, 20), "0 MB used of 0 MB  |  0 MB free"); // never underflows
    }

    #[test]
    fn installed_app_records() {
        assert_eq!(app_line("1.2.3", "Acme", "20261105", 2048), "1.2.3  |  Acme  |  11/5/2026  |  2 MB");
        assert_eq!(app_line("", "Acme", "", 0), "Acme");
        assert_eq!(app_line("7", "", "2026", 0), "7"); // a date that is not YYYYMMDD is left out
        assert_eq!(uninstall_command("MsiExec.exe /I{ABC-1}"), "MsiExec.exe /X{ABC-1}");
        assert_eq!(uninstall_command(r#""C:\P F\unins000.exe" /SILENT"#), r#""C:\P F\unins000.exe" /SILENT"#);
        assert_eq!(icon_file(r#""C:\P F\app.exe",0"#), r"C:\P F\app.exe");
        assert_eq!(icon_file(r"C:\a,b\app.ico"), r"C:\a,b\app.ico"); // a comma that is part of the path
        assert_eq!(icon_source("app\tC:/x.ico\tC:/unins.exe"), "C:/x.ico");
    }

    #[test]
    fn startup_records_and_commands() {
        assert!(startup_on(&[])); // never switched: on
        assert!(startup_on(&[2, 0, 0, 0]) && startup_on(&[6, 0, 0, 0]));
        assert!(!startup_on(&[3, 0, 0, 0]) && !startup_on(&[7, 0, 0, 0]));
        assert_eq!(exe_of(r#""C:\Program Files\A B\x.exe" --hidden"#), r"C:\Program Files\A B\x.exe");
        assert_eq!(exe_of(r"C:\WINDOWS\system32\SecurityHealthSystray.exe"), r"C:\WINDOWS\system32\SecurityHealthSystray.exe");
        assert_eq!(exe_of(r"C:\tools\run.EXE -background"), r"C:\tools\run.EXE");
        assert_eq!(icon_source("startup\tC:/x.exe\tcu:X"), "C:/x.exe");
        assert_eq!(icon_source("Microsoft.App_1!App"), r"shell:AppsFolder\Microsoft.App_1!App");
    }
    #[test]
    fn layout_and_scale_math() {
        use Kind::*;
        fn s() -> String {
            String::new()
        }
        // Display page: title, 3 cards, title, card -> the positions measured on the real page (minus its 8px frame)
        let (tops, end) = row_tops(&[Header, Info(s), Info(s), Info(s), Header, Info(s)], &[], 123.0);
        assert_eq!(tops, vec![123.0, 139.0, 211.0, 283.0, 355.0, 411.0]);
        assert_eq!(end, 483.0);
        // Sound page: title, a device card listing 4 devices, then Volume -> 140..537 and 541, as measured
        fn none() -> Vec<crate::qsx::Item> {
            Vec::new()
        }
        let (tops, _) = row_tops(&[Header, Devices(none, ""), Info(s)], &[0, 4, 0], 124.0);
        assert_eq!(tops, vec![124.0, 140.0, 542.0]);
        assert_eq!(split_device("Speakers (Realtek(R) Audio)"), ("Speakers".to_string(), "Realtek(R) Audio".to_string()));
        assert_eq!(split_device("Headset"), ("Headset".to_string(), String::new()));
        let mut v = vec![("b".to_string(), 5), ("A".to_string(), 9), ("c".to_string(), 1)];
        sort_senders(&mut v, false);
        assert_eq!(v[0].0, "A");
        sort_senders(&mut v, true);
        assert_eq!(v.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["A", "b", "c"]);
        // Power page: battery %, the policy bar, first card -> 112, 212, 268 as measured; without the bar -> 212
        fn yes() -> bool {
            true
        }
        assert_eq!(row_tops(&[Big(s), Note(yes, ""), Info(s)], &[0, 1, 0], 112.0).0, vec![112.0, 211.0, 267.0]);
        assert_eq!(row_tops(&[Big(s), Note(yes, ""), Info(s)], &[0, 0, 0], 112.0).0, vec![112.0, 211.0, 211.0]);
        assert!(power_modes().contains(&power_mode()));
        // About page: 4 tiles (two rows), rename card, device info -> 112, 462, 534 as measured
        fn none4() -> Vec<Line> {
            Vec::new()
        }
        assert_eq!(row_tops(&[Tiles(none4), RenamePc, Facts(none4)], &[4, 0, 9], 112.0).0, vec![112.0, 462.0, 534.0]);
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19782), (2024, 2, 29));
        assert_eq!(size_label(6 << 30), "6 GB");
        assert_eq!(size_label(486 << 20), "486 MB");
        // Bluetooth page: tiles, the bar, Bluetooth switch, air, Devices -> 112, 343, 382, ~455, 472 (real: 112, 343, 388?, 477)
        fn devs() -> Vec<crate::qsx::Item> {
            Vec::new()
        }
        fn on() -> bool {
            true
        }
        fn set(_: bool) {}
        let t = row_tops(&[BtTiles(devs), Center("", ""), Toggle(on, set), Space(17.0), Info(s)], &[], 112.0).0;
        assert_eq!(&t[..3], &[112.0, 343.0, 388.0]);
        assert_eq!(ini_value("[Theme]\r\nDisplayName=x\r\nWallpaper=%SystemRoot%\\web\\a.jpg\r\n", "wallpaper").as_deref(), Some("%SystemRoot%\\web\\a.jpg"));
        assert!(themes().len() >= 2, "Windows ships at least the light and dark themes");
        assert_eq!(recommended_index(100, 0), 0);
        assert_eq!(recommended_index(150, 1), 1); // at 150%, one step above recommended -> 125% is recommended
        assert_eq!(recommended_index(100, -2), 2);
        assert!(resolutions().contains(&resolution())); // the live list offers the mode we're in
        assert!(scales().contains(&scale()));
    }
    #[test]
    fn every_sub_page_link_resolves() {
        let p = pages();
        for page in &p {
            assert!(page.parent.is_empty() || p.iter().any(|x| x.name == page.parent), "parent of {}", page.name);
            for r in &page.rows {
                if let Kind::Sub(name) = r.kind {
                    assert!(p.iter().any(|x| x.name == name), "missing page {name}");
                }
            }
        }
    }
    #[test]
    fn search_finds_rows() {
        let p = pages();
        assert!(search(&p, "night").iter().any(|r| r.1 == "Night light"));
        assert!(search(&p, "zzzz").is_empty());
    }
}

#[cfg(test)]
mod pbk_tests {
    #[test]
    fn phonebook_types() {
        let t = "[Work VPN]\nMEDIA=rastapi\nType=2\n\n[Home DSL]\nType=5\n[Old]\n";
        assert_eq!(super::pbk_typed(t), vec![("Work VPN".to_string(), 2), ("Home DSL".to_string(), 5), ("Old".to_string(), 0)]);
    }
}

#[cfg(test)]
mod apps_sort_tests {
    #[test]
    fn sorts_installed_apps() {
        let it = |n: &str, kb: u32, d: &str| crate::qsx::Item { name: n.into(), signal: kb, key: format!("app\ticon\tcmd\t{d}"), ..Default::default() };
        let mut v = vec![it("b", 10, "20240101"), it("A", 300, "20260101"), it("c", 50, "")];
        super::sort_apps(&mut v, 0);
        assert_eq!(v.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["A", "b", "c"]);
        super::sort_apps(&mut v, 2);
        assert_eq!(v[0].name, "A");
        super::sort_apps(&mut v, 3);
        assert_eq!(v.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["A", "b", "c"]);
        super::sort_apps(&mut v, 1);
        assert_eq!(v[0].name, "c");
    }
}

#[cfg(test)]
mod focus_tests {
    use super::Kind;
    #[test]
    fn keyboard_moves_between_actionable_rows() {
        let rows = [Kind::Header, Kind::Sub("A"), Kind::Header, Kind::Sub("B"), Kind::Sub("C")];
        assert_eq!(super::next_focus(&rows, None, false), Some(1));
        assert_eq!(super::next_focus(&rows, Some(1), false), Some(3));
        assert_eq!(super::next_focus(&rows, Some(4), false), Some(4));
        assert_eq!(super::next_focus(&rows, Some(3), true), Some(1));
        assert_eq!(super::next_focus(&rows, None, true), Some(4));
    }
}
