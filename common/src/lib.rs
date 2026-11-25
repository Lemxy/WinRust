//! Code shared by the WinRust programs: the language switch (`t!`, `tf!`, `lang!`, `en!`), the NTFS MFT reader, Win+D.
//! Use it with `#[macro_use] extern crate winrust_common;` at the crate root.
//!
//! English by default; Russian, Japanese, Portuguese (Brazil), Simplified Chinese or Spanish when
//! %ProgramData%\WinRust\lang.txt contains "ru", "ja", "pt", "zh" or "es" (one list, `Lang::ALL`: the installer, the bar's
//! settings window and rust-settings all write that file through `lang::set`). Spanish lives in `es.tsv` (see `es`), so a
//! text that is not translated yet shows English.
//! The file is re-read a couple of times a second (`lang`), so a switch reaches the programs that are already running.
use std::path::PathBuf;

pub mod appid;
pub mod desktop;
pub mod es;
pub mod fg;
pub mod frost;
pub mod icon;
pub mod journal;
pub mod lang;
pub mod look;
pub mod launch;
pub mod mem;
pub mod mft;
pub mod notify;
pub mod monitors;
pub mod place;
pub mod proc;
pub mod reg;
pub mod search;
pub mod settings;
pub mod starticons;
pub mod style;

pub use lang::{get, lang, set, set_lang, Lang};

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

/// `t!("русский", "english", ja: "日本語", pt: "português", zh: "中文")`: a text in the current language.
/// Without `ja:`/`pt:`/`zh:` those languages show the English text.
#[macro_export]
macro_rules! t {
    ($ru:expr, $en:expr, ja: $ja:expr, pt: $pt:expr, zh: $zh:expr $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => $ru,
            $crate::Lang::En => $en,
            $crate::Lang::Ja => $ja,
            $crate::Lang::Pt => $pt,
            $crate::Lang::Zh => $zh,
            $crate::Lang::Es => $crate::es::tr($en),
        }
    };
    ($ru:expr, $en:expr, ja: $ja:expr, pt: $pt:expr $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => $ru,
            $crate::Lang::En => $en,
            $crate::Lang::Ja => $ja,
            $crate::Lang::Pt => $pt,
            $crate::Lang::Zh => $en,
            $crate::Lang::Es => $crate::es::tr($en),
        }
    };
    ($ru:expr, $en:expr, zh: $zh:expr $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => $ru,
            $crate::Lang::Zh => $zh,
            $crate::Lang::Es => $crate::es::tr($en),
            _ => $en,
        }
    };
    ($ru:expr, $en:expr) => {
        match $crate::lang!() {
            $crate::Lang::Ru => $ru,
            $crate::Lang::Es => $crate::es::tr($en),
            _ => $en,
        }
    };
}

/// `tf!("русский {}", "english {}", ja: "日本語 {}", pt: "português {}", zh: "中文 {}", args...)`: format! in the current
/// language (all templates are checked at compile time). Without `ja:`/`pt:`/`zh:` those languages use the English one.
#[macro_export]
macro_rules! tf {
    ($ru:literal, $en:literal, ja: $ja:literal, pt: $pt:literal, zh: $zh:literal $(, $a:expr)* $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => format!($ru $(, $a)*),
            $crate::Lang::En => format!($en $(, $a)*),
            $crate::Lang::Ja => format!($ja $(, $a)*),
            $crate::Lang::Pt => format!($pt $(, $a)*),
            $crate::Lang::Zh => format!($zh $(, $a)*),
            $crate::Lang::Es => $crate::es::fill($crate::es::tr($en), &[$(&$a as &dyn ::core::fmt::Display),*]),
        }
    };
    ($ru:literal, $en:literal, ja: $ja:literal, pt: $pt:literal $(, $a:expr)* $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => format!($ru $(, $a)*),
            $crate::Lang::En => format!($en $(, $a)*),
            $crate::Lang::Ja => format!($ja $(, $a)*),
            $crate::Lang::Pt => format!($pt $(, $a)*),
            $crate::Lang::Zh => format!($en $(, $a)*),
            $crate::Lang::Es => $crate::es::fill($crate::es::tr($en), &[$(&$a as &dyn ::core::fmt::Display),*]),
        }
    };
    ($ru:literal, $en:literal, zh: $zh:literal $(, $a:expr)* $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => format!($ru $(, $a)*),
            $crate::Lang::Zh => format!($zh $(, $a)*),
            $crate::Lang::Es => $crate::es::fill($crate::es::tr($en), &[$(&$a as &dyn ::core::fmt::Display),*]),
            _ => format!($en $(, $a)*),
        }
    };
    ($ru:literal, $en:literal $(, $a:expr)* $(,)?) => {
        match $crate::lang!() {
            $crate::Lang::Ru => format!($ru $(, $a)*),
            $crate::Lang::Es => $crate::es::fill($crate::es::tr($en), &[$(&$a as &dyn ::core::fmt::Display),*]),
            _ => format!($en $(, $a)*),
        }
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
    }

    #[test]
    fn lang_codes() {
        use super::Lang;
        assert_eq!(Lang::parse(" pt-BR "), Lang::Pt);
        assert_eq!(Lang::parse("ZH-cn"), Lang::Zh);
        assert_eq!(Lang::parse("zh_Hans"), Lang::Zh);
        assert_eq!(Lang::Zh.next(), Lang::En);
        assert_eq!(Lang::parse("JA"), Lang::Ja);
        assert_eq!(Lang::parse("ru"), Lang::Ru);
        assert_eq!(Lang::parse("x"), Lang::En);
        assert_eq!(Lang::Pt.next(), Lang::Zh);
    }
}
