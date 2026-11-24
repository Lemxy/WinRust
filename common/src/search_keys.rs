//! Which shortcuts open rust-search's card: %ProgramData%\WinRust\search-keys.txt, written by the installer's gear.
//! Both are on unless the file says `alt_space=0` / `win_s=0`. rs-launcher reads it at start and on `style::msg()`.
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SearchKeys {
    /// Alt+Space: quick to reach, but it is also every window's system menu (Move, Size, Close).
    pub alt_space: bool,
    /// Win+S (and Win+Q): where Windows' own search used to be.
    pub win_s: bool,
}

impl Default for SearchKeys {
    fn default() -> Self {
        SearchKeys { alt_space: true, win_s: true }
    }
}

fn file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join("WinRust").join("search-keys.txt"))
}

impl SearchKeys {
    pub fn read() -> SearchKeys {
        file().and_then(|f| std::fs::read_to_string(f).ok()).map_or_else(SearchKeys::default, |s| SearchKeys::parse(&s))
    }

    pub fn write(&self) -> Result<(), String> {
        let f = file().ok_or("no %ProgramData%")?;
        let _ = std::fs::create_dir_all(f.parent().unwrap());
        std::fs::write(&f, format!("alt_space={}\nwin_s={}\n", self.alt_space as u8, self.win_s as u8)).map_err(|e| format!("{}: {e}", f.display()))
    }

    fn parse(s: &str) -> SearchKeys {
        let mut k = SearchKeys::default();
        for (name, v) in s.lines().filter_map(|l| l.split_once('=')) {
            let on = v.trim() != "0";
            match name.trim() {
                "alt_space" => k.alt_space = on,
                "win_s" => k.win_s = on,
                _ => {}
            }
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::SearchKeys;

    #[test]
    fn both_on_unless_switched_off() {
        assert_eq!(SearchKeys::parse(""), SearchKeys { alt_space: true, win_s: true });
        assert_eq!(SearchKeys::parse("alt_space=0\r\nwin_s=1\r\n"), SearchKeys { alt_space: false, win_s: true });
        assert_eq!(SearchKeys::parse(" win_s = 0 \nnonsense\nother=0"), SearchKeys { alt_space: true, win_s: false });
    }
}
