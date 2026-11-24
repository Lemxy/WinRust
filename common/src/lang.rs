//! The UI language of every WinRust program, in one place.
//!
//! The choice lives in `%ProgramData%\WinRust\lang.txt` (`en`, `ru`, `ja`, `pt`, `zh` or `es`) and every program reads
//! that same file: the installer's `--lang`, `rust-settings`, the bar's settings window and the bar's language button all
//! write it through [`set`]. English is the default when the file is missing or says nothing known.
//!
//! The file is looked at again a couple of times a second, so switching the language in one program reaches the others
//! that are already running on their next text lookup (`t!`), not only at their next start.
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU8, AtomicU64, Ordering::Relaxed},
};

/// The UI languages, in the order the switch goes round them. This is the only list: menus, settings windows and the
/// installer all walk it instead of keeping their own copy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Ru,
    Ja,
    Pt,
    Zh,
    Es,
}

impl Lang {
    pub const ALL: [Lang; 6] = [Lang::En, Lang::Ru, Lang::Ja, Lang::Pt, Lang::Zh, Lang::Es];

    /// The file/code of this language (`en`, `ru`, `ja`, `pt`, `zh`, `es`).
    pub const fn code(self) -> &'static str {
        ["en", "ru", "ja", "pt", "zh", "es"][self as usize]
    }

    /// The name of the language in itself (what a language picker shows: "Русский", "日本語"...).
    pub const fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ru => "Русский",
            Lang::Ja => "日本語",
            Lang::Pt => "Português",
            Lang::Zh => "简体中文",
            Lang::Es => "Español",
        }
    }

    /// The language after this one: the switch button goes round them.
    pub fn next(self) -> Lang {
        Lang::ALL[(self as usize + 1) % Lang::ALL.len()]
    }

    /// The language of a code or a lang.txt line (`en`, `ru`, `ja`, `pt`, `zh`, `es`; `zh-CN` and `zh_CN` work too).
    pub fn parse(s: &str) -> Lang {
        let s = s.trim().to_ascii_lowercase();
        match s.get(..2) {
            Some("ru") => Lang::Ru,
            Some("ja") => Lang::Ja,
            Some("pt") => Lang::Pt,
            Some("zh") => Lang::Zh,
            Some("es") => Lang::Es,
            _ => Lang::En,
        }
    }

    fn from_u8(v: u8) -> Option<Lang> {
        Lang::ALL.get(v as usize).copied()
    }
}

static LANG: AtomicU8 = AtomicU8::new(u8::MAX); // MAX = not read yet, else a `Lang`
static CHECKED: AtomicU64 = AtomicU64::new(0); // when the file was last looked at (ms, 0 = never)

fn file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join("WinRust").join("lang.txt"))
}

fn now() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

fn current() -> Lang {
    Lang::from_u8(LANG.load(Relaxed)).unwrap_or(Lang::En)
}

/// The current language at run time. In code that has unit tests use `lang!()` instead: it is always Russian under `cargo test`.
///
/// Cheap on the hot path (two atomic loads); the shared file is read again at most twice a second, so a switch made in
/// another WinRust program shows up here without a restart. A missing or unreadable file keeps the language already in use.
pub fn lang() -> Lang {
    let cached = LANG.load(Relaxed);
    let t = now();
    if cached != u8::MAX && t.saturating_sub(CHECKED.load(Relaxed)) < 500 {
        return Lang::from_u8(cached).unwrap_or(Lang::En);
    }
    CHECKED.store(t, Relaxed);
    // An empty or half-written file (another program is switching the language right now) is "no opinion": keep the
    // language already in use instead of falling back to English for a second.
    let read = file().and_then(|f| std::fs::read_to_string(f).ok());
    let l = match read.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => Lang::parse(s),
        _ if cached == u8::MAX => Lang::En,
        _ => current(),
    };
    LANG.store(l as u8, Relaxed);
    l
}

/// The current language. Same as [`lang`], for code that reads better as `lang::get()`.
pub fn get() -> Lang {
    lang()
}

/// Switches this process and writes the choice for the other programs. True when the file could be written: false means
/// only this program changed (an unelevated user without rights on %ProgramData%), so the caller can say so instead of
/// promising a switch that will not reach the others.
pub fn set(l: Lang) -> bool {
    LANG.store(l as u8, Relaxed);
    CHECKED.store(now(), Relaxed);
    match file() {
        Some(f) => {
            if let Some(d) = f.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            std::fs::write(&f, l.code()).is_ok()
        }
        None => false,
    }
}

/// The old name of [`set`], kept for the programs that already call it.
pub fn set_lang(l: Lang) -> bool {
    set(l)
}

#[cfg(test)]
mod tests {
    use super::Lang;

    #[test]
    fn lang_codes() {
        assert_eq!(Lang::parse(" pt-BR "), Lang::Pt);
        assert_eq!(Lang::parse("ZH-cn"), Lang::Zh);
        assert_eq!(Lang::parse("zh_Hans"), Lang::Zh);
        assert_eq!(Lang::parse("JA"), Lang::Ja);
        assert_eq!(Lang::parse("ru"), Lang::Ru);
        assert_eq!(Lang::parse("x"), Lang::En);
        assert_eq!(Lang::Pt.next(), Lang::Zh);
        assert_eq!(Lang::Zh.next(), Lang::En);
        assert_eq!(Lang::Es.next(), Lang::En);
        assert_eq!(Lang::ALL.iter().map(|l| l.code()).collect::<Vec<_>>(), ["en", "ru", "ja", "pt", "zh", "es"]);
        assert_eq!(Lang::ALL.iter().map(|l| l.next()).collect::<Vec<_>>(), Lang::ALL.iter().cycle().skip(1).take(6).copied().collect::<Vec<_>>());
    }
}
