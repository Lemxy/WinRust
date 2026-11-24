//! Code shared by the WinRust programs: the language switch (`t!`, `tf!`, `lang!`, `en!`), the NTFS MFT reader, Win+D.
//! Use it with `#[macro_use] extern crate winrust_common;` at the crate root.
//!
//! The language lives in %ProgramData%\WinRust\lang.txt ("en", "ru", "ja", "pt", "zh" or "es"; the language list in the
//! installer, in rust-settings and in the bar writes it). No file: the language Windows is shown in, if we have it, else
//! English. A text missing in a language is shown in English. Programs re-read the file at most once a second, and
//! `set_lang` broadcasts `style::msg()` so open windows repaint in the new language at once.
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, AtomicU8, Ordering::Relaxed},
};

pub mod caption;
pub mod desktop;
pub mod fg;
pub mod journal;
pub mod launch;
pub mod mem;
pub mod mft;
pub mod proc;
pub mod reg;
pub mod search;
pub mod search_keys;
pub mod settings;
pub mod style;

/// Console programs print the UI language, and Rust writes UTF-8: a console decodes those bytes with its output code page,
/// which is 936 on a Chinese Windows, so every non-ASCII line would come out as noise. Switch this window's output code
/// page to UTF-8. A pipe or a file is left alone: another program reads those bytes as UTF-8 anyway (rtaskmgr reads rs-du).
pub fn utf8_console() {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, SetConsoleOutputCP, STD_OUTPUT_HANDLE};
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode = 0u32;
        if !h.is_null() && h != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE && GetConsoleMode(h, &mut mode) != 0 {
            SetConsoleOutputCP(65001);
        }
    }
}

/// "Show in folder": opens `path`'s folder with the file selected in the installed rust-explorer, else in Windows Explorer.
pub fn reveal(path: &str) {
    use std::{os::windows::process::CommandExt, process::Command};
    let rx = std::env::var_os("ProgramFiles").map(|pf| PathBuf::from(pf).join(r"rust-explorer\rust-explorer.exe")).filter(|p| p.is_file());
    let _ = match rx {
        Some(rx) => Command::new(rx).raw_arg(format!("/select,\"{path}\"")).spawn(),
        None => Command::new("explorer.exe").raw_arg(format!("/select,\"{path}\"")).spawn(),
    };
}

/// The UI languages. Every text has an English version, so English is the fallback for anything missing.
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
    /// Every language, in the order the pickers list them.
    pub const ALL: [Lang; 6] = [Lang::En, Lang::Ru, Lang::Ja, Lang::Pt, Lang::Zh, Lang::Es];

    /// The name of the language in itself (for the switch button).
    pub fn name(self) -> &'static str {
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
        Lang::ALL[(Lang::ALL.iter().position(|&l| l == self).unwrap_or(0) + 1) % Lang::ALL.len()]
    }
    fn code(self) -> &'static str {
        ["en", "ru", "ja", "pt", "zh", "es"][self as usize]
    }
    fn from_u8(v: u8) -> Lang {
        Lang::ALL.into_iter().find(|&l| l as u8 == v).unwrap_or(Lang::En)
    }
    /// The language of a code or a lang.txt line (`en`, `ru`, `ja`, `pt`, `zh`, `es`; `zh-CN`, `zh_CN`, `es-AR` work too).
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
    /// A Windows LANGID (only its primary language counts): the language we have for it, if any.
    fn from_langid(id: u16) -> Option<Lang> {
        match id & 0x3FF {
            0x09 => Some(Lang::En),
            0x19 => Some(Lang::Ru),
            0x11 => Some(Lang::Ja),
            0x16 => Some(Lang::Pt),
            0x04 => Some(Lang::Zh),
            0x0A => Some(Lang::Es),
            _ => None,
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(u8::MAX); // MAX = not read yet, else a `Lang`
static LANG_AT: AtomicU64 = AtomicU64::new(0); // when lang.txt was last read (ms)

fn file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join("WinRust").join("lang.txt"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// What Windows itself is shown in, when we speak it.
fn system_lang() -> Lang {
    let id = unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() };
    Lang::from_langid(id).unwrap_or(Lang::En)
}

/// The Windows build number (19045 = Windows 10 22H2, 22000 and up = Windows 11), read once. 0 if the registry will not say.
pub fn windows_build() -> u32 {
    static BUILD: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *BUILD.get_or_init(|| reg::get_str(r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber").and_then(|s| s.trim().parse().ok()).unwrap_or(0))
}

/// Windows 11 or later: the taskbar, the search window and the tray settings all moved there.
pub fn win11() -> bool {
    windows_build() >= 22000
}

/// Reads lang.txt again now (the `style::msg()` broadcast does this too, through `style::reload`).
pub fn reload_lang() {
    let l = file().and_then(|f| std::fs::read_to_string(f).ok()).map_or_else(system_lang, |s| Lang::parse(&s));
    LANG.store(l as u8, Relaxed);
    LANG_AT.store(now_ms(), Relaxed);
}

/// The current language at run time. In code that has unit tests use `lang!()` instead: it is always Russian under `cargo test`.
/// Re-read at most once a second, so a program that missed the broadcast catches up on its next paint.
pub fn lang() -> Lang {
    if LANG.load(Relaxed) == u8::MAX || now_ms().saturating_sub(LANG_AT.load(Relaxed)) > 1000 {
        reload_lang();
    }
    Lang::from_u8(LANG.load(Relaxed))
}

/// Switches this process, writes the choice for the other programs and tells their open windows to repaint in it.
/// False: the file could not be written (an unelevated installer, a user without rights on %ProgramData%), so the
/// other programs keep their language; the caller can say so instead of promising a switch that does not happen.
pub fn set_lang(l: Lang) -> bool {
    LANG.store(l as u8, Relaxed);
    LANG_AT.store(now_ms(), Relaxed);
    let written = match file() {
        Some(f) => {
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            std::fs::write(&f, l.code()).is_ok()
        }
        None => false,
    };
    if written {
        style::broadcast();
    }
    written
}

/// `lang!()`: the current `Lang`. `cfg!(test)` is checked in the calling crate, so its unit tests keep their Russian expectations.
#[macro_export]
macro_rules! lang {
    () => {
        if cfg!(test) { $crate::Lang::Ru } else { $crate::lang() }
    };
}

/// `en!()`: true for English only.
#[macro_export]
macro_rules! en {
    () => {
        $crate::lang!() == $crate::Lang::En
    };
}

/// `dot!()`: true where the decimal separator is a dot (English, Japanese, Chinese); Russian and Portuguese use a comma.
#[macro_export]
macro_rules! dot {
    () => {
        matches!($crate::lang!(), $crate::Lang::En | $crate::Lang::Ja | $crate::Lang::Zh)
    };
}

/// `t!("русский", "english", ja: "日本語", pt: "português", zh: "中文", es: "español")`: a text in the current language.
/// A language left out shows the English text: `t!(ru, en)` is English for all but Russian, and so on.
#[macro_export]
macro_rules! t {
    ($ru:expr, $en:expr, ja: $ja:expr, pt: $pt:expr, zh: $zh:expr, es: $es:expr $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => $ru,
            $crate::Lang::En => $en,
            $crate::Lang::Ja => $ja,
            $crate::Lang::Pt => $pt,
            $crate::Lang::Zh => $zh,
            $crate::Lang::Es => $es,
        }
    };
    ($ru:expr, $en:expr, ja: $ja:expr, pt: $pt:expr, zh: $zh:expr $(,)?) => {
        $crate::t!($ru, $en, ja: $ja, pt: $pt, zh: $zh, es: $en)
    };
    ($ru:expr, $en:expr, ja: $ja:expr, pt: $pt:expr, es: $es:expr $(,)?) => {
        $crate::t!($ru, $en, ja: $ja, pt: $pt, zh: $en, es: $es)
    };
    ($ru:expr, $en:expr, ja: $ja:expr, pt: $pt:expr $(,)?) => {
        $crate::t!($ru, $en, ja: $ja, pt: $pt, zh: $en, es: $en)
    };
    ($ru:expr, $en:expr, zh: $zh:expr $(,)?) => {
        $crate::t!($ru, $en, ja: $en, pt: $en, zh: $zh, es: $en)
    };
    ($ru:expr, $en:expr) => {
        if $crate::lang!() == $crate::Lang::Ru { $ru } else { $en }
    };
}

/// `tf!("русский {}", "english {}", ja: "日本語 {}", pt: "português {}", zh: "中文 {}", es: "español {}", args...)`: format!
/// in the current language (all templates are checked at compile time). A language left out uses the English template.
#[macro_export]
macro_rules! tf {
    ($ru:literal, $en:literal, ja: $ja:literal, pt: $pt:literal, zh: $zh:literal, es: $es:literal $(, $a:expr)* $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => format!($ru $(, $a)*),
            $crate::Lang::En => format!($en $(, $a)*),
            $crate::Lang::Ja => format!($ja $(, $a)*),
            $crate::Lang::Pt => format!($pt $(, $a)*),
            $crate::Lang::Zh => format!($zh $(, $a)*),
            $crate::Lang::Es => format!($es $(, $a)*),
        }
    };
    ($ru:literal, $en:literal, ja: $ja:literal, pt: $pt:literal, zh: $zh:literal $(, $a:expr)* $(,)?) => {
        $crate::tf!($ru, $en, ja: $ja, pt: $pt, zh: $zh, es: $en $(, $a)*)
    };
    ($ru:literal, $en:literal, ja: $ja:literal, pt: $pt:literal, es: $es:literal $(, $a:expr)* $(,)?) => {
        $crate::tf!($ru, $en, ja: $ja, pt: $pt, zh: $en, es: $es $(, $a)*)
    };
    ($ru:literal, $en:literal, ja: $ja:literal, pt: $pt:literal $(, $a:expr)* $(,)?) => {
        $crate::tf!($ru, $en, ja: $ja, pt: $pt, zh: $en, es: $en $(, $a)*)
    };
    ($ru:literal, $en:literal, zh: $zh:literal $(, $a:expr)* $(,)?) => {
        $crate::tf!($ru, $en, ja: $en, pt: $en, zh: $zh, es: $en $(, $a)*)
    };
    ($ru:literal, $en:literal $(, $a:expr)* $(,)?) => {
        if $crate::lang!() == $crate::Lang::Ru { format!($ru $(, $a)*) } else { format!($en $(, $a)*) }
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn tests_run_in_russian() {
        assert_eq!(t!("да", "yes"), "да");
        assert_eq!(tf!("{} шт", "{} pcs", 3), "3 шт");
        assert_eq!(t!("да", "yes", ja: "はい", pt: "sim", zh: "是"), "да");
        assert_eq!(tf!("{} шт", "{} pcs", ja: "{} 個", pt: "{} un.", zh: "{} 个", 3), "3 шт");
        assert_eq!(t!("да", "yes", ja: "はい", pt: "sim", zh: "是", es: "sí"), "да");
        assert_eq!(t!("да", "yes", ja: "はい", pt: "sim", es: "sí"), "да");
        assert_eq!(tf!("{} шт", "{} pcs", ja: "{} 個", pt: "{} un.", zh: "{} 个", es: "{} ud.", 3), "3 шт");
    }

    #[test]
    fn the_build_number_is_read() {
        // any Windows these programs run on is 10 or later; 0 would mean the registry read broke
        assert!(super::windows_build() >= 10240, "{}", super::windows_build());
        assert_eq!(super::win11(), super::windows_build() >= 22000);
    }

    #[test]
    fn lang_codes() {
        use super::Lang;
        assert_eq!(Lang::parse(" pt-BR "), Lang::Pt);
        assert_eq!(Lang::parse("ZH-cn"), Lang::Zh);
        assert_eq!(Lang::parse("zh_Hans"), Lang::Zh);
        assert_eq!(Lang::Zh.next(), Lang::Es);
        assert_eq!(Lang::parse("JA"), Lang::Ja);
        assert_eq!(Lang::parse("ru"), Lang::Ru);
        assert_eq!(Lang::parse("x"), Lang::En);
        assert_eq!(Lang::Pt.next(), Lang::Zh);
        assert_eq!(Lang::Es.next(), Lang::En);
        assert_eq!(Lang::parse("es-AR"), Lang::Es);
        for l in Lang::ALL {
            assert_eq!(Lang::parse(l.code()), l); // what set_lang writes reads back the same
            assert_eq!(Lang::from_u8(l as u8), l);
        }
        assert_eq!(Lang::from_u8(200), Lang::En);
    }

    #[test]
    fn windows_languages() {
        use super::Lang;
        assert_eq!(Lang::from_langid(0x0C0A), Some(Lang::Es)); // es-ES
        assert_eq!(Lang::from_langid(0x2C0A), Some(Lang::Es)); // es-AR
        assert_eq!(Lang::from_langid(0x0416), Some(Lang::Pt)); // pt-BR
        assert_eq!(Lang::from_langid(0x0804), Some(Lang::Zh)); // zh-CN
        assert_eq!(Lang::from_langid(0x0419), Some(Lang::Ru));
        assert_eq!(Lang::from_langid(0x0411), Some(Lang::Ja));
        assert_eq!(Lang::from_langid(0x0409), Some(Lang::En));
        assert_eq!(Lang::from_langid(0x0407), None); // German: not ours, so English
    }
}
