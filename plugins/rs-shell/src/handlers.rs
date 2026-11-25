//! Per-user URI handlers, so that WinRust opens its own programs when it is the shell.
//!
//! `ms-settings:xxx` -> `rust-settings.exe --system "%1"`, `ms-screenclip:` / `ms-snippingtool:` -> `rs-shot.exe`
//! (HKCU\Software\Classes, no `DelegateExecute`: with one, ShellExecute ignores the command and starts the packaged app
//! instead). Nothing is applied unless `%ProgramData%\WinRust\handlers-enabled` exists. Before the first write the old
//! values go to `%ProgramData%\WinRust\handlers.json`; `rs-shell --restore` puts them back and removes the file. A missing
//! rust-settings or rs-shot restores its handler, so the system's own `ms-settings:` keeps working.
//!
//! `control.exe` -> `rust-control` is **not** installed here: it is not a protocol but a system executable, so replacing it
//! needs HKLM (IFEO / App Paths) and belongs to the provisioning script (see `scripts/user-mode.ps1`).
use std::{
    os::windows::process::CommandExt,
    path::PathBuf,
    process::Command,
};
use crate::log; // %LOCALAPPDATA%\rs-shell\log.txt

const NO_WINDOW: u32 = 0x0800_0000;
const MS_SETTINGS: &str = r"HKCU\Software\Classes\ms-settings\shell\open\command";
const MS_SCREENCLIP: &str = r"HKCU\Software\Classes\ms-screenclip\shell\open\command";
const MS_SNIPPING: &str = r"HKCU\Software\Classes\ms-snippingtool\shell\open\command";

fn data_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("ProgramData")?).join("WinRust"))
}

fn marker() -> Option<PathBuf> {
    Some(data_dir()?.join("handlers-enabled"))
}

fn backup_file() -> Option<PathBuf> {
    Some(data_dir()?.join("handlers.json"))
}

/// The opt-in marker the installer (or a tester) creates: a byte, a line, anything.
pub fn enabled() -> bool {
    marker().is_some_and(|p| p.exists())
}

fn reg(args: &[&str]) -> Option<String> {
    let o = Command::new("reg").args(args).creation_flags(NO_WINDOW).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// One value of `key` as (type, data); `name` None = the default value.
fn query(key: &str, name: Option<&str>) -> Option<(String, String)> {
    let out = match name {
        Some(n) => reg(&["query", key, "/v", n])?,
        None => reg(&["query", key, "/ve"])?,
    };
    let (ty, data) = winrust_common::reg::value(&out)?;
    Some((ty.to_string(), data.to_string()))
}

/// `rust-settings.exe` / `rs-shot.exe`, in the installer's folder, beside rs-shell, or in the launcher's bin.
fn find_exe(name: &str, folder: &str) -> Option<PathBuf> {
    let mut v = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        v.push(PathBuf::from(pf).join(folder).join(name));
    }
    if let Ok(e) = std::env::current_exe() {
        if let Some(d) = e.parent() {
            v.push(d.join(name));
        }
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(la).join(r"plugin-launcher\bin").join(name));
    }
    v.into_iter().find(|p| p.is_file())
}

#[derive(Clone)]
struct Saved {
    key: String,
    existed: bool,
    default: Option<(String, String)>,  // (type, data)
    delegate: Option<(String, String)>,
}

fn snapshot(key: &str) -> Saved {
    let existed = reg(&["query", key]).is_some();
    Saved { key: key.to_string(), existed, default: query(key, None), delegate: query(key, Some("DelegateExecute")) }
}

// ---------- handlers.json (a small, strict JSON we write and read ourselves) ----------

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

fn field(obj: &str, name: &str) -> Option<String> {
    let pat = format!("\"{name}\":\"");
    let start = obj.find(&pat)? + pat.len();
    let mut out = String::new();
    let mut esc = false;
    for c in obj[start..].chars() {
        if esc {
            out.push(match c {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            esc = false;
            continue;
        }
        match c {
            '\\' => esc = true,
            '"' => return Some(out),
            c => out.push(c),
        }
    }
    None
}

fn field_bool(obj: &str, name: &str) -> bool {
    obj.contains(&format!("\"{name}\":true"))
}

fn serialize(saved: &[Saved]) -> String {
    let value = |v: &Option<(String, String)>| v.clone().unwrap_or_default();
    let mut out = String::new();
    for s in saved {
        let (dt, dd) = value(&s.default);
        let (gt, gd) = value(&s.delegate);
        out.push_str(&format!(
            "{{\"key\":\"{}\",\"existed\":{},\"default_type\":\"{}\",\"default_data\":\"{}\",\"delegate_type\":\"{}\",\"delegate_data\":\"{}\"}}\n",
            esc(&s.key),
            s.existed,
            esc(&dt),
            esc(&dd),
            esc(&gt),
            esc(&gd),
        ));
    }
    out
}

fn parse(text: &str) -> Vec<Saved> {
    let mut out = Vec::new();
    for line in text.lines().filter(|l| l.trim_start().starts_with('{')) {
        let Some(key) = field(line, "key") else { continue };
        let pair = |ty: &str, data: &str| {
            let (ty, data) = (field(line, ty).unwrap_or_default(), field(line, data).unwrap_or_default());
            (!ty.is_empty()).then_some((ty, data))
        };
        out.push(Saved {
            key,
            existed: field_bool(line, "existed"),
            default: pair("default_type", "default_data"),
            delegate: pair("delegate_type", "delegate_data"),
        });
    }
    out
}

fn load_backup() -> Vec<Saved> {
    backup_file().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse(&t)).unwrap_or_default()
}

/// Writes the backup; false when the folder is not writable. Without a backup the handlers must not be applied, or
/// `--restore` would have nothing to put back.
fn save_backup(saved: &[Saved]) -> bool {
    let Some(p) = backup_file() else { return false };
    if saved.is_empty() {
        return !p.exists() || std::fs::remove_file(&p).is_ok();
    }
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    std::fs::write(&p, serialize(saved)).is_ok()
}

/// Remembers a key the first time only: a later `ensure_handlers` must not save our own command as the "original".
fn remember(saved: &mut Vec<Saved>, key: &str) {
    if !saved.iter().any(|s| s.key.eq_ignore_ascii_case(key)) {
        saved.push(snapshot(key));
    }
}

// ---------- writing and restoring ----------

fn apply(key: &str, command: &str) {
    let _ = reg(&["add", key, "/ve", "/t", "REG_SZ", "/d", command, "/f"]);
    // With DelegateExecute present ShellExecute uses the packaged app instead of our command.
    let _ = reg(&["delete", key, "/v", "DelegateExecute", "/f"]);
}

fn restore_entry(e: &Saved) {
    if !e.existed {
        let _ = reg(&["delete", &e.key, "/f"]); // the key itself was ours
        return;
    }
    match &e.default {
        Some((ty, data)) => { let _ = reg(&["add", &e.key, "/ve", "/t", ty, "/d", data, "/f"]); }
        None => { let _ = reg(&["delete", &e.key, "/ve", "/f"]); }
    }
    match &e.delegate {
        Some((ty, data)) => { let _ = reg(&["add", &e.key, "/v", "DelegateExecute", "/t", ty, "/d", data, "/f"]); }
        None => { let _ = reg(&["delete", &e.key, "/v", "DelegateExecute", "/f"]); }
    }
}

/// Applies the handlers this build can serve, and gives back the ones it cannot. Only with the marker file.
pub fn ensure_handlers() {
    if !enabled() {
        return;
    }
    log("handlers: marker found");
    let hub = find_exe("rust-settings.exe", "rust-settings");
    let shot = find_exe("rs-shot.exe", "rs-shot");
    let mut saved = load_backup();
    let mut want: Vec<(&str, String)> = Vec::new();
    match &hub {
        Some(exe) => {
            remember(&mut saved, MS_SETTINGS);
            want.push((MS_SETTINGS, format!("\"{}\" --system \"%1\"", exe.display())));
        }
        None => {
            if let Some(e) = saved.iter().find(|s| s.key.eq_ignore_ascii_case(MS_SETTINGS)).cloned() {
                restore_entry(&e);
                saved.retain(|s| !s.key.eq_ignore_ascii_case(MS_SETTINGS));
            }
            log("handlers: rust-settings.exe not found, ms-settings left as it was");
        }
    }
    match &shot {
        Some(exe) => {
            for key in [MS_SCREENCLIP, MS_SNIPPING] {
                remember(&mut saved, key);
                want.push((key, format!("\"{}\"", exe.display())));
            }
        }
        None => {
            let mut changed = false;
            saved.retain(|s| {
                let ours = s.key.eq_ignore_ascii_case(MS_SCREENCLIP) || s.key.eq_ignore_ascii_case(MS_SNIPPING);
                if ours {
                    restore_entry(s);
                    changed = true;
                }
                !ours
            });
            if changed {
                log("handlers: rs-shot.exe not found, capture handlers restored");
            }
        }
    }
    // The backup goes first: if ProgramData\WinRust is not writable the handlers are not applied at all.
    if !want.is_empty() && !save_backup(&saved) {
        log("handlers: cannot write handlers.json, nothing applied");
        return;
    }
    save_backup(&saved);
    for (key, command) in &want {
        apply(key, command);
        log(&format!("handlers: {key} -> {command}"));
    }
}

/// `rs-shell --restore`: puts every saved value back and forgets the backup. Safe with nothing saved.
pub fn restore() -> bool {
    let saved = load_backup();
    if saved.is_empty() {
        log("handlers: nothing to restore");
        return false;
    }
    for e in &saved {
        restore_entry(e);
    }
    if let Some(p) = backup_file() {
        let _ = std::fs::remove_file(p);
    }
    log(&format!("handlers: restored {} key(s)", saved.len()));
    true
}

#[cfg(test)]
mod tests {
    use super::{Saved, parse, serialize};

    #[test]
    fn backup_round_trips_quotes_and_backslashes() {
        let s = vec![
            Saved {
                key: r"HKCU\Software\Classes\ms-settings\shell\open\command".into(),
                existed: true,
                default: Some(("REG_SZ".into(), r#""C:\Program Files\x\x.exe" --system "%1""#.into())),
                delegate: Some(("REG_SZ".into(), "{4ed3a719-cea8-4bd9-910d-e252f997afc2}".into())),
            },
            Saved { key: r"HKCU\Software\Classes\ms-screenclip\shell\open\command".into(), existed: false, default: None, delegate: None },
        ];
        let back = parse(&serialize(&s));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].key, s[0].key);
        assert!(back[0].existed);
        assert_eq!(back[0].default, s[0].default);
        assert_eq!(back[0].delegate, s[0].delegate);
        assert!(!back[1].existed);
        assert_eq!(back[1].default, None);
    }
}
