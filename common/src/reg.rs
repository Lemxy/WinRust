//! The registry: native reads and writes, and the parser for `reg query` output (the installer still goes through reg.exe).

/// The value line as (type, data), e.g. ("REG_DWORD", "0x1"). None = no value in the output.
/// reg.exe writes it as `    <name>    <type>    <data>`, four spaces apart; names and data may contain spaces themselves.
pub fn value(out: &str) -> Option<(&str, &str)> {
    out.lines().find_map(|l| {
        let mut f = l.strip_prefix("    ")?.splitn(3, "    ");
        let (_, ty) = (f.next()?, f.next()?);
        ty.starts_with("REG_").then(|| (ty, f.next().unwrap_or("").trim_end()))
    })
}

/// Native read, no reg.exe process: `path` like `HKLM\SOFTWARE\X` or `HKCU\...`; `name` None = the key itself.
/// true when the key (or the value in it) exists.
pub fn exists(path: &str, name: Option<&str>) -> bool {
    raw(path, name).is_some()
}

/// A REG_DWORD read natively; None when missing or of another type.
pub fn get_dword(path: &str, name: &str) -> Option<u32> {
    raw(path, Some(name)).and_then(|(ty, b)| (ty == 4 && b.len() >= 4).then(|| u32::from_le_bytes(b[..4].try_into().unwrap())))
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// `HKLM\x` -> (HKEY_LOCAL_MACHINE, "x" as UTF-16). None for an unknown root.
fn split(path: &str) -> Option<(windows_sys::Win32::System::Registry::HKEY, Vec<u16>)> {
    use windows_sys::Win32::System::Registry::*;
    let (root, sub) = path.split_once('\\').unwrap_or((path, ""));
    let root = match root.to_ascii_uppercase().as_str() {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        "HKU" | "HKEY_USERS" => HKEY_USERS,
        "HKCR" | "HKEY_CLASSES_ROOT" => HKEY_CLASSES_ROOT,
        _ => return None,
    };
    Some((root, w(sub)))
}

/// Writes a REG_DWORD natively, creating the key when missing. true on success.
pub fn set_dword(path: &str, name: &str, v: u32) -> bool {
    use windows_sys::Win32::System::Registry::*;
    let Some((root, sub)) = split(path) else { return false };
    let b = v.to_le_bytes();
    unsafe { RegSetKeyValueW(root, sub.as_ptr(), w(name).as_ptr(), REG_DWORD, b.as_ptr().cast(), 4) == 0 }
}

/// Deletes a value natively. true when it is gone (also when it never existed).
pub fn delete_value(path: &str, name: &str) -> bool {
    use windows_sys::Win32::{Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND}, System::Registry::*};
    let Some((root, sub)) = split(path) else { return false };
    let rc = unsafe { RegDeleteKeyValueW(root, sub.as_ptr(), w(name).as_ptr()) };
    rc == 0 || rc == ERROR_FILE_NOT_FOUND || rc == ERROR_PATH_NOT_FOUND
}

/// Every value of a key as (name, type, data); empty when the key is missing.
pub fn values(path: &str) -> Vec<(String, u32, Vec<u8>)> {
    use windows_sys::Win32::{Foundation::ERROR_MORE_DATA, System::Registry::*};
    let mut v = vec![];
    let Some((root, sub)) = split(path) else { return v };
    unsafe {
        let mut k = std::ptr::null_mut();
        if RegOpenKeyExW(root, sub.as_ptr(), 0, KEY_READ, &mut k) != 0 {
            return v;
        }
        let (mut name, mut data) = (vec![0u16; 16384], vec![0u8; 4096]); // 16383 chars = the longest value name
        let mut i = 0;
        loop {
            let (mut nl, mut dl, mut ty) = (name.len() as u32, data.len() as u32, 0u32);
            let rc = RegEnumValueW(k, i, name.as_mut_ptr(), &mut nl, std::ptr::null(), &mut ty, data.as_mut_ptr(), &mut dl);
            if rc == ERROR_MORE_DATA {
                data.resize(dl as usize + 2, 0); // a big value: grow and read this index again
                continue;
            }
            if rc != 0 {
                break; // ERROR_NO_MORE_ITEMS
            }
            v.push((String::from_utf16_lossy(&name[..nl as usize]), ty, data[..dl as usize].to_vec()));
            i += 1;
        }
        RegCloseKey(k);
    }
    v
}

/// REG_SZ / REG_EXPAND_SZ data as text (as stored: %VARS% not expanded); None for other types.
pub fn text(ty: u32, b: &[u8]) -> Option<String> {
    (ty == 1 || ty == 2).then(|| {
        let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u).trim_end_matches('\0').to_string()
    })
}

/// A REG_SZ / REG_EXPAND_SZ read natively (not expanded); None when missing or of another type.
pub fn get_text(path: &str, name: &str) -> Option<String> {
    raw(path, Some(name)).and_then(|(ty, b)| text(ty, &b))
}

/// `path` aimed at the signed-in user the installer passed (see `user_args`): `HKCU\x` -> `HKU\<sid>\x`.
pub fn user_path(path: &str) -> String {
    user_args(&[path]).remove(0)
}

fn raw(path: &str, name: Option<&str>) -> Option<(u32, Vec<u8>)> {
    use windows_sys::Win32::System::Registry::*;
    let (root, sub) = split(path)?;
    unsafe {
        let mut k = std::ptr::null_mut();
        if RegOpenKeyExW(root, sub.as_ptr(), 0, KEY_READ, &mut k) != 0 {
            return None;
        }
        let r = match name {
            None => Some((0, vec![])),
            Some(n) => {
                let n = w(n);
                let (mut ty, mut len) = (0u32, 0u32);
                if RegQueryValueExW(k, n.as_ptr(), std::ptr::null(), &mut ty, std::ptr::null_mut(), &mut len) != 0 {
                    None
                } else {
                    let mut b = vec![0u8; len as usize];
                    let ok = RegQueryValueExW(k, n.as_ptr(), std::ptr::null(), &mut ty, b.as_mut_ptr(), &mut len) == 0;
                    b.truncate(len as usize);
                    ok.then_some((ty, b))
                }
            }
        };
        RegCloseKey(k);
        r
    }
}

/// The value as a number when it is a REG_DWORD; None when missing or of another type.
pub fn dword(out: &str) -> Option<u32> {
    match value(out)? {
        ("REG_DWORD", d) => u32::from_str_radix(d.trim_start_matches("0x"), 16).ok(),
        _ => None,
    }
}

/// Set by the installer for its children when the signed-in user is not the elevated account (UAC was answered with another
/// administrator): the SID of the signed-in user. Per-user keys then go to `HKU\<sid>`, not to the elevated account's HKCU.
pub const SID_ENV: &str = "WINRUST_SID";

pub fn valid_sid(s: &str) -> bool {
    s.starts_with("S-1-") && s.len() > 4 && s.chars().all(|c| c.is_ascii_digit() || c == '-' || c == 'S')
}

/// A reg.exe key argument aimed at the user `sid`: `HKCU\x` becomes `HKU\<sid>\x`. Anything else (HKLM, values, flags) is unchanged.
pub fn for_user(arg: &str, sid: &str) -> String {
    let rest = ["HKCU", "HKEY_CURRENT_USER"].iter().find_map(|p| arg.strip_prefix(p).filter(|r| r.is_empty() || r.starts_with('\\')));
    rest.map_or_else(|| arg.to_string(), |r| format!("HKU\\{sid}{r}"))
}

/// `for_user` for the SID the installer passed in the environment; the arguments as they are when it passed none.
pub fn user_args(args: &[&str]) -> Vec<String> {
    let sid = std::env::var(SID_ENV).ok().filter(|s| valid_sid(s));
    args.iter().map(|a| sid.as_deref().map_or_else(|| a.to_string(), |s| for_user(a, s))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aims_hkcu_at_the_given_user() {
        assert_eq!(for_user(r"HKCU\Software\X", "S-1-5-21-1-2-3-1001"), r"HKU\S-1-5-21-1-2-3-1001\Software\X");
        assert_eq!(for_user("HKEY_CURRENT_USER", "S-1-5-21-9"), r"HKU\S-1-5-21-9");
        assert_eq!(for_user(r"HKLM\SOFTWARE\X", "S-1-5-21-9"), r"HKLM\SOFTWARE\X");
        assert_eq!(for_user(r"HKCUX\a", "S-1-5-21-9"), r"HKCUX\a");
        assert_eq!(for_user("/v", "S-1-5-21-9"), "/v");
        assert!(valid_sid("S-1-5-21-1-2-3-1001") && !valid_sid("S-1-5-x; calc") && !valid_sid(""));
    }

    #[test]
    fn reads_reg_query_output() {
        let key = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\REG_X\r\n";
        assert_eq!(dword(&format!("{key}    AllowTelemetry    REG_DWORD    0x1a\r\n\r\n")), Some(26));
        assert_eq!(value(&format!("{key}    Debugger    REG_SZ    \"C:\\Program Files\\x.exe\" /e\r\n")), Some(("REG_SZ", "\"C:\\Program Files\\x.exe\" /e")));
        assert_eq!(value("    (Default)    REG_SZ    \r\n"), Some(("REG_SZ", "")));
        assert_eq!(dword("    V    REG_SZ    0x1\r\n"), None);
        assert_eq!(value("ERROR: The system was unable to find the specified registry key or value."), None);
    }

    #[test]
    fn native_write_enumerate_delete() {
        let k = r"HKCU\Software\winrust-reg-test-5c1d";
        assert!(set_dword(k, "a", 7) && get_dword(k, "a") == Some(7));
        assert!(set_dword(k, "a", 0x8000_0001) && get_dword(k, "a") == Some(0x8000_0001));
        assert_eq!(values(k), [("a".to_string(), 4, 0x8000_0001u32.to_le_bytes().to_vec())]);
        assert!(delete_value(k, "a") && get_dword(k, "a").is_none());
        assert!(delete_value(k, "a"), "already gone counts as done");
        unsafe { windows_sys::Win32::System::Registry::RegDeleteTreeW(windows_sys::Win32::System::Registry::HKEY_CURRENT_USER, w(r"Software\winrust-reg-test-5c1d").as_ptr()) };
        assert!(values(r"HKCU\Software\no-such-key-winrust").is_empty());
        let sz: Vec<u8> = w(r"C:\x.exe /s").iter().flat_map(|c| c.to_le_bytes()).collect();
        assert_eq!(text(1, &sz).as_deref(), Some(r"C:\x.exe /s"));
        assert_eq!(text(3, &[1, 2]), None);
        assert!(values(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion").iter().any(|(n, t, _)| n == "ProductName" && *t == 1));
    }

    #[test]
    fn native_read() {
        assert!(exists(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion", Some("ProductName")));
        assert!(exists(r"HKLM\SOFTWARE\Microsoft", None));
        assert!(!exists(r"HKLM\SOFTWARE\no-such-key-winrust", None));
        assert!(!exists(r"BAD\x", None));
        assert!(get_dword(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentMajorVersionNumber").is_some_and(|v| v >= 10));
        assert_eq!(get_dword(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName"), None, "REG_SZ is not a DWORD");
    }
}
