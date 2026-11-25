//! A per-app icon for the Start menu.
//!
//! `%ProgramData%\WinRust\icons.txt`, one `<app path><TAB><icon file>` line per override. The key is the same path the
//! Start menu uses for the entry (a `.lnk`, an `.exe` or `shell:AppsFolder\<id>`), so an app keeps its icon while its
//! shortcut stays where it is, and two apps never share one.
//!
//! Chosen from the Start menu's right-click menu and from WinRust Settings, which is a different process: the file is the
//! whole interface, re-read when its timestamp changes, and the bar picks the change up the next time the panel opens.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

use windows_sys::Win32::{
    Foundation::HWND,
    UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    },
};

use crate::t;

/// The overrides file, under the machine-wide WinRust folder (the same place as `lang.txt` and `dodge.txt`).
pub fn file() -> PathBuf {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join("WinRust").join("icons.txt")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn parse(text: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('\t') {
            let (k, v) = (k.trim(), v.trim());
            if !k.is_empty() && !v.is_empty() {
                m.insert(k.to_lowercase(), v.to_string());
            }
        }
    }
    m
}

fn read_map() -> HashMap<String, String> {
    std::fs::read_to_string(file()).map(|s| parse(&s)).unwrap_or_default()
}

fn stamp() -> u64 {
    std::fs::metadata(file())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Last read, with the timestamp it came from: the bar asks once per painted icon and must not touch the disk each time.
static CACHE: Mutex<Option<(u64, HashMap<String, String>)>> = Mutex::new(None);

/// The icon chosen for `key`, when the file still exists.
pub fn get(key: &str) -> Option<PathBuf> {
    let st = stamp();
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(c.as_ref(), Some((s, _)) if *s == st) {
        *c = Some((st, read_map()));
    }
    c.as_ref()?.1.get(&key.to_lowercase()).map(PathBuf::from).filter(|p| p.is_file())
}

fn write_map(m: &HashMap<String, String>) -> std::io::Result<()> {
    let path = file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut lines: Vec<String> = m.iter().map(|(k, v)| format!("{k}\t{v}")).collect();
    lines.sort();
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None; // next get re-reads
    Ok(())
}

/// Points `key` at `icon` (an `.ico`, an `.exe`, a `.dll`, an image).
pub fn set(key: &str, icon: &Path) -> std::io::Result<()> {
    let mut m = read_map();
    m.insert(key.to_lowercase(), icon.to_string_lossy().into_owned());
    write_map(&m)
}

/// Drops the override: the app gets its own icon back.
pub fn clear(key: &str) -> std::io::Result<()> {
    let mut m = read_map();
    m.remove(&key.to_lowercase());
    write_map(&m)
}

/// Asks for an icon file. The classic dialog renders without explorer.exe (the modern picker does not), which is the
/// same reason `rust-explorer` uses it for "Open with".
pub fn pick(owner: HWND) -> Option<PathBuf> {
    let mut buf = vec![0u16; 1024];
    let filter: Vec<u16> = t!("Иконки", "Icons", ja: "アイコン", pt: "Ícones", zh: "图标")
        .encode_utf16()
        .chain(wide("\0*.ico;*.exe;*.dll;*.png;*.jpg;*.bmp"))
        .chain(t!("Все файлы", "All files", ja: "すべてのファイル", pt: "Todos os arquivos", zh: "所有文件").encode_utf16())
        .chain(wide("\0*.*"))
        .chain([0u16])
        .collect();
    let title = wide(&t!("Выберите значок", "Choose an icon", ja: "アイコンを選択", pt: "Escolher um ícone", zh: "选择图标"));
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let s = String::from_utf16_lossy(&buf[..len]);
    (!s.is_empty()).then(|| PathBuf::from(s))
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn the_file_is_a_list_of_key_tab_icon_lines() {
        // the format is the interface between the bar and Settings: a line without its tab is dropped, not the file
        let m = parse("C:\\x\\App.lnk\tC:\\icons\\app.ico\nno tab here\n\nC:/x/b.exe\tD:\\b.ico\n");
        assert_eq!(m.get("c:\\x\\app.lnk").map(String::as_str), Some(r"C:\icons\app.ico"));
        assert_eq!(m.get("c:/x/b.exe").map(String::as_str), Some(r"D:\b.ico"));
        assert_eq!(m.len(), 2);
    }
}
