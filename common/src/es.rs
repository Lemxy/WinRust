//! Spanish texts.
//!
//! The other languages keep their translation inside the `t!` / `tf!` call (`ja: "..."`). Spanish is different on
//! purpose: it is a lookup table keyed by the English text (`common/es.tsv`, one `english<TAB>español` line per text),
//! so a text that is not translated yet falls back to English instead of breaking the build. Adding a translation is one
//! line in that file and a rebuild.
use std::{
    collections::HashMap,
    fmt::Display,
    sync::OnceLock,
};

/// Undoes the escapes the texts carry in the source (`es.tsv` keeps them as they are written in the Rust literals).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('0') => out.push('\0'),
            Some('u') => {
                // \u{1F600}
                let mut hex = String::new();
                if let Some('{') = it.next() {
                    for c in it.by_ref() {
                        if c == '}' {
                            break;
                        }
                        hex.push(c);
                    }
                }
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => out.push_str("\\u{"),
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn table() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut m = HashMap::new();
        for line in include_str!("../es.tsv").lines() {
            if let Some((en, es)) = line.split_once('\t') {
                if !en.is_empty() {
                    m.insert(unescape(en), unescape(es));
                }
            }
        }
        m
    })
}

/// The Spanish text of an English one, or the English one itself when nobody translated it yet.
pub fn tr<'a>(en: &'a str) -> &'a str {
    table().get(en).map_or(en, String::as_str)
}

struct Spec {
    idx: Option<usize>,
    align: Option<char>,
    width: Option<usize>,
    prec: Option<usize>,
    ty: Option<char>,
}

fn parse_spec(s: &str) -> Spec {
    let (arg, fmt) = match s.find(':') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    };
    let idx = if arg.is_empty() { None } else { arg.parse().ok() };
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    let mut align = None;
    if chars.len() >= 2 && matches!(chars[1], '<' | '>' | '^') {
        align = Some(chars[1]);
        i = 2;
    } else if !chars.is_empty() && matches!(chars[0], '<' | '>' | '^') {
        align = Some(chars[0]);
        i = 1;
    }
    let start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    let width = if i > start { chars[start..i].iter().collect::<String>().parse().ok() } else { None };
    let mut prec = None;
    if i < chars.len() && chars[i] == '.' {
        i += 1;
        let from = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i > from {
            prec = chars[from..i].iter().collect::<String>().parse().ok();
        }
    }
    let ty = chars.get(i).copied();
    Spec { idx, align, width, prec, ty }
}

fn apply(a: &dyn Display, sp: &Spec) -> String {
    let mut s = match sp.prec {
        Some(p) => format!("{:.p$}", a, p = p),
        None => format!("{}", a),
    };
    if let Some(w) = sp.width {
        let n = s.chars().count();
        if n < w {
            let pad = w - n;
            match sp.align.unwrap_or('>') {
                '<' => s.push_str(&" ".repeat(pad)),
                '^' => {
                    let l = pad / 2;
                    s = format!("{}{}{}", " ".repeat(l), s, " ".repeat(pad - l));
                }
                _ => s = format!("{}{}", " ".repeat(pad), s),
            }
        }
    }
    s
}

/// `tf!` in Spanish: the Spanish template (or the English one) with `{}`, `{0}`, `{:.1}`, `{0:<18}` … filled in.
pub fn fill(tpl: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(tpl.len() + 16);
    let mut next = 0usize;
    let mut it = tpl.chars().peekable();
    while let Some(c) = it.next() {
        if c != '{' {
            out.push(c);
            continue;
        }
        if it.peek() == Some(&'{') {
            it.next();
            out.push('{');
            continue;
        }
        let mut spec = String::new();
        let mut closed = false;
        for c in it.by_ref() {
            if c == '}' {
                closed = true;
                break;
            }
            spec.push(c);
        }
        if !closed {
            out.push('{');
            out.push_str(&spec);
            continue;
        }
        let sp = parse_spec(&spec);
        let i = sp.idx.unwrap_or(next);
        if sp.idx.is_none() {
            next += 1;
        }
        match args.get(i) {
            Some(a) => out.push_str(&apply(*a, &sp)),
            None => {
                out.push('{');
                out.push_str(&spec);
                out.push('}');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_and_indexed() {
        assert_eq!(fill("{} y {}", &[&1, &"dos"]), "1 y dos");
        assert_eq!(fill("{0} {0}", &[&"a"]), "a a");
    }

    #[test]
    fn precision_and_width() {
        assert_eq!(fill("{:.1} ms", &[&3.14159]), "3.1 ms");
        assert_eq!(fill("[{0:<5}]", &[&"ab"]), "[ab   ]");
        assert_eq!(fill("[{:>4}]", &[&"ab"]), "[  ab]");
    }

    #[test]
    fn literals_survive() {
        assert_eq!(fill("{{}}", &[]), "{}");
        assert_eq!(fill("100 %", &[]), "100 %");
    }

    #[test]
    fn spanish_is_wired() {
        assert_eq!(tr("Settings"), "Ajustes");
        assert_eq!(tr("esto no existe en la tabla"), "esto no existe en la tabla");
    }
}
