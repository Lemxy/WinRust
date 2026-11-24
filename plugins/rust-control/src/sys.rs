//! Small Win32 helpers: registry reads, launching Windows pages, confirmations and formatting.
//! No shell strings are built from user input: the only command lines started are the uninstall
//! commands a program itself registered.

use std::{
    ptr::{null, null_mut},
    sync::OnceLock,
};

use windows_sys::Win32::{
    Foundation::*,
    System::{
        Registry::*,
        Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW},
    },
    UI::{
        Shell::{IsUserAnAdmin, ShellExecuteW},
        WindowsAndMessaging::*,
    },
};

use crate::model::Target;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

pub fn dword(root: HKEY, key: &str, name: &str) -> Option<u32> {
    let (mut v, mut n) = (0u32, 4u32);
    let rc = unsafe {
        RegGetValueW(root, wide(key).as_ptr(), wide(name).as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut v as *mut u32 as _, &mut n)
    };
    (rc == ERROR_SUCCESS).then_some(v)
}

pub fn qword(root: HKEY, key: &str, name: &str) -> Option<u64> {
    let (mut v, mut n) = (0u64, 8u32);
    let rc = unsafe {
        RegGetValueW(root, wide(key).as_ptr(), wide(name).as_ptr(), RRF_RT_REG_QWORD, null_mut(), &mut v as *mut u64 as _, &mut n)
    };
    (rc == ERROR_SUCCESS).then_some(v)
}

/// A REG_SZ/REG_EXPAND_SZ of any length.
pub fn string_long(root: HKEY, key: &str, name: &str) -> Option<String> {
    let (k, v) = (wide(key), wide(name));
    let mut n = 0u32;
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    if unsafe { RegGetValueW(root, k.as_ptr(), v.as_ptr(), flags, null_mut(), null_mut(), &mut n) } != ERROR_SUCCESS || n < 2 {
        return None;
    }
    let mut buf = vec![0u16; n as usize / 2 + 1];
    let mut n2 = (buf.len() * 2) as u32;
    let rc = unsafe { RegGetValueW(root, k.as_ptr(), v.as_ptr(), flags, null_mut(), buf.as_mut_ptr() as _, &mut n2) };
    (rc == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf[..(n2 as usize / 2).saturating_sub(1)]))
}

/// A short REG_SZ (names such as `ProductName`); an empty or missing value is `None`.
pub fn string(root: HKEY, key: &str, name: &str) -> Option<String> {
    string_long(root, key, name).filter(|s| !s.is_empty())
}

pub fn subkeys(root: HKEY, key: &str) -> Vec<String> {
    let mut out = vec![];
    unsafe {
        let mut h: HKEY = null_mut();
        if RegOpenKeyExW(root, wide(key).as_ptr(), 0, KEY_READ, &mut h) != ERROR_SUCCESS {
            return out;
        }
        for i in 0.. {
            let mut buf = [0u16; 512];
            let mut n = buf.len() as u32;
            let rc = RegEnumKeyExW(h, i, buf.as_mut_ptr(), &mut n, null(), null_mut(), null_mut(), null_mut());
            if rc != ERROR_SUCCESS {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..n as usize]));
        }
        RegCloseKey(h);
    }
    out
}

/// Every value of a key as `(name, printable value)`. Binary blobs are shown as their size.
pub fn values(root: HKEY, key: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    unsafe {
        let mut h: HKEY = null_mut();
        if RegOpenKeyExW(root, wide(key).as_ptr(), 0, KEY_READ, &mut h) != ERROR_SUCCESS {
            return out;
        }
        for i in 0.. {
            let mut name = [0u16; 512];
            let mut n = name.len() as u32;
            let mut ty = 0u32;
            let mut data = [0u8; 4096];
            let mut len = data.len() as u32;
            let rc = RegEnumValueW(h, i, name.as_mut_ptr(), &mut n, null(), &mut ty, data.as_mut_ptr(), &mut len);
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc != ERROR_SUCCESS {
                if rc == ERROR_MORE_DATA {
                    out.push((String::from_utf16_lossy(&name[..n as usize]), "<too long>".into()));
                    continue;
                }
                break;
            }
            let key = String::from_utf16_lossy(&name[..n as usize]);
            let value = match ty {
                REG_SZ | REG_EXPAND_SZ => {
                    let units: Vec<u16> = data[..len as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                    String::from_utf16_lossy(&units).trim_end_matches('\0').to_string()
                }
                REG_DWORD if len >= 4 => u32::from_le_bytes([data[0], data[1], data[2], data[3]]).to_string(),
                REG_QWORD if len >= 8 => u64::from_le_bytes([data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7]]).to_string(),
                _ => format!("<{} bytes>", len),
            };
            out.push((key, value));
        }
        RegCloseKey(h);
    }
    out
}

/// Opens an `ms-settings:` URI, a control panel applet or an executable.
/// The return value is ShellExecute's own code when Windows refused (<= 32).
pub fn open(target: Target) -> Result<(), isize> {
    let (file, args) = match target {
        Target::Uri(uri) => (wide(uri), Vec::new()),
        Target::File(file) => (wide(file), Vec::new()),
        Target::Control(name) => (wide("control.exe"), wide(&format!("/name {name}"))),
    };
    let op = wide("open");
    let r = unsafe { ShellExecuteW(null_mut(), op.as_ptr(), file.as_ptr(), if args.is_empty() { null() } else { args.as_ptr() }, null(), SW_SHOWNORMAL) };
    if r as isize > 32 {
        Ok(())
    } else {
        Err(r as isize)
    }
}

/// Starts the command line a program registered for its own uninstaller, exactly as Windows' Apps list does.
pub fn run_command_line(cmd: &str) -> Result<(), u32> {
    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let mut line = wide(cmd);
        if CreateProcessW(null(), line.as_mut_ptr(), null(), null(), 0, 0, null(), null(), &si, &mut pi) == 0 {
            return Err(GetLastError());
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        Ok(())
    }
}

/// A yes/no question for a destructive action.
pub fn confirm(hwnd: HWND, text: &str, title: &str) -> bool {
    unsafe {
        let t = wide(text);
        let c = wide(title);
        MessageBoxW(hwnd, t.as_ptr(), c.as_ptr(), MB_YESNO | MB_ICONWARNING) == IDYES
    }
}

/// `IsUserAnAdmin` is the same check the classic control panel uses. Cached: it cannot change for this process.
pub fn is_admin() -> bool {
    static ADMIN: OnceLock<bool> = OnceLock::new();
    *ADMIN.get_or_init(|| unsafe { IsUserAnAdmin() != 0 })
}

pub fn error_text(code: u32) -> String {
    if code == 0 {
        return t!("неизвестно", "unknown", ja: "不明", pt: "desconhecido", zh: "未知").into();
    }
    std::io::Error::from_raw_os_error(code as i32).to_string()
}

/// `1024`-based sizes, as Explorer shows them. The decimal separator follows the
/// language: a comma in Spanish, Russian and Portuguese.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes} {}", UNITS[i])
    } else {
        let number = format!("{v:.1}");
        let number = if dot!() { number } else { number.replace('.', ",") };
        format!("{number} {}", UNITS[i])
    }
}

/// `EstimatedSize` is stored in KB.
pub fn format_kb(kb: u64) -> String {
    format_size(kb.saturating_mul(1024))
}

/// `YYYYMMDD` -> a date in the order the language uses; anything else is returned unchanged.
/// English, Japanese and Chinese keep the sortable ISO order; Spanish and Portuguese use
/// day/month/year and Russian day.month.year.
pub fn format_date(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() == 8 && b.iter().all(|c| c.is_ascii_digit()) {
        let (y, m, d) = (&s[..4], &s[4..6], &s[6..8]);
        match winrust_common::lang!() {
            winrust_common::Lang::Es | winrust_common::Lang::Pt => format!("{d}/{m}/{y}"),
            winrust_common::Lang::Ru => format!("{d}.{m}.{y}"),
            _ => format!("{y}-{m}-{d}"),
        }
    } else {
        s.to_string()
    }
}

/// `0x00112233-4455-6677-8899-aabbccddeeff` for the power-plan GUIDs.
pub fn guid_text(g: &windows_sys::core::GUID) -> String {
    let d = &g.data4;
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// `3d 4h 12m`, used for the system uptime, with the unit letters of the language.
pub fn duration_text(seconds: u64) -> String {
    let d = seconds / 86_400;
    let h = seconds % 86_400 / 3600;
    let m = seconds % 3600 / 60;
    let (du, hu, mu) = match winrust_common::lang!() {
        winrust_common::Lang::Ru => ("д", "ч", "м"),
        winrust_common::Lang::Ja => ("日", "時間", "分"),
        winrust_common::Lang::Zh => ("天", "小时", "分"),
        winrust_common::Lang::Es | winrust_common::Lang::Pt => ("d", "h", "min"),
        winrust_common::Lang::En => ("d", "h", "m"),
    };
    if d > 0 {
        format!("{d}{du} {h}{hu} {m}{mu}")
    } else if h > 0 {
        format!("{h}{hu} {m}{mu}")
    } else {
        format!("{m}{mu}")
    }
}

pub fn loword(v: usize) -> u16 {
    v as u32 as u16
}

pub fn hiword(v: usize) -> u16 {
    (v as u32 >> 16) as u16
}
