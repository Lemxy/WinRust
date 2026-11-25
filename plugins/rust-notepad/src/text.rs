//! What the bytes on disk mean: the BOM, the encoding and the end-of-line style.
//!
//! A Windows `EDIT` control breaks lines only on CRLF, so the text always goes into the editor with CRLF.
//! `encode` puts the file's own line endings back, and a BOM is written only when the file had a UTF-8
//! one: UTF-16 files are saved as UTF-8, which is what the ISO plan asks the text editor for.
//! No crate is used for this: the BOM is four byte patterns and the ANSI fallback is one Win32 call.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enc {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// Not valid UTF-8 and no BOM: decoded with the system code page (`MultiByteToWideChar(CP_ACP)`).
    Ansi,
}

impl Enc {
    /// The name shown in the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Enc::Utf8 => "UTF-8",
            Enc::Utf8Bom => "UTF-8 BOM",
            Enc::Utf16Le => "UTF-16 LE",
            Enc::Utf16Be => "UTF-16 BE",
            Enc::Ansi => "ANSI",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eol {
    Crlf,
    Lf,
    Cr,
}

impl Eol {
    pub fn name(self) -> &'static str {
        match self {
            Eol::Crlf => "CRLF",
            Eol::Lf => "LF",
            Eol::Cr => "CR",
        }
    }
}

pub struct Loaded {
    /// The text with CRLF line endings, ready for the EDIT control.
    pub text: String,
    pub enc: Enc,
    pub eol: Eol,
}

pub fn decode(bytes: &[u8]) -> Loaded {
    let (raw, enc) = if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        (String::from_utf8_lossy(rest).into_owned(), Enc::Utf8Bom)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        (utf16(rest, true), Enc::Utf16Le)
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        (utf16(rest, false), Enc::Utf16Be)
    } else {
        match std::str::from_utf8(bytes) {
            Ok(s) => (s.to_string(), Enc::Utf8),
            Err(_) => (ansi(bytes), Enc::Ansi),
        }
    };
    let eol = detect_eol(&raw);
    Loaded { text: normalize(&raw), enc, eol }
}

/// The file's own text with the editor's CRLF and the BOM the caller asked for.
pub fn encode(text: &str, eol: Eol, bom: bool) -> Vec<u8> {
    let body = match eol {
        Eol::Crlf => text.to_string(),
        Eol::Lf => text.replace("\r\n", "\n"),
        Eol::Cr => text.replace("\r\n", "\r"),
    };
    let mut out = Vec::with_capacity(body.len() + 3);
    if bom {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(body.as_bytes());
    out
}

/// The most frequent line ending of `s`; CRLF for a file with none.
pub fn detect_eol(s: &str) -> Eol {
    let (mut crlf, mut lf, mut cr) = (0usize, 0usize, 0usize);
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' if b.get(i + 1) == Some(&b'\n') => {
                crlf += 1;
                i += 1;
            }
            b'\r' => cr += 1,
            b'\n' => lf += 1,
            _ => {}
        }
        i += 1;
    }
    if crlf == 0 && lf == 0 && cr == 0 {
        Eol::Crlf
    } else if crlf >= lf && crlf >= cr {
        Eol::Crlf
    } else if lf >= cr {
        Eol::Lf
    } else {
        Eol::Cr
    }
}

/// Every line break as CRLF: the EDIT control and the caret arithmetic expect it.
pub fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\r\n");
            }
            '\n' => out.push_str("\r\n"),
            _ => out.push(c),
        }
    }
    out
}

fn utf16(bytes: &[u8], le: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(windows)]
fn ansi(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{MultiByteToWideChar, CP_ACP};
    if bytes.is_empty() {
        return String::new();
    }
    let n = unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, std::ptr::null_mut(), 0) };
    if n <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut w = vec![0u16; n as usize];
    unsafe { MultiByteToWideChar(CP_ACP, 0, bytes.as_ptr(), bytes.len() as i32, w.as_mut_ptr(), n) };
    String::from_utf16_lossy(&w)
}

#[cfg(not(windows))]
fn ansi(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The UTF-16 length of `s`: the unit `EM_SETSEL` and the caret count in.
pub fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_with_and_without_bom() {
        let plain = decode(b"hola\r\nmundo\r\n");
        assert_eq!((plain.enc, plain.eol), (Enc::Utf8, Eol::Crlf));
        assert_eq!(plain.text, "hola\r\nmundo\r\n");

        let bom = decode(b"\xEF\xBB\xBFhola\nmundo\n");
        assert_eq!((bom.enc, bom.eol), (Enc::Utf8Bom, Eol::Lf));
        assert_eq!(bom.text, "hola\r\nmundo\r\n");
    }

    #[test]
    fn utf16_both_byte_orders() {
        let mut bytes = vec![0xFF, 0xFE];
        for u in "aé\r\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let le = decode(&bytes);
        assert_eq!((le.enc, le.text.as_str()), (Enc::Utf16Le, "aé\r\n"));

        let mut bytes = vec![0xFE, 0xFF];
        for u in "aé\n".encode_utf16() {
            bytes.extend_from_slice(&u.to_be_bytes());
        }
        let be = decode(&bytes);
        assert_eq!((be.enc, be.eol, be.text.as_str()), (Enc::Utf16Be, Eol::Lf, "aé\r\n"));
    }

    #[test]
    fn eol_detection_follows_the_majority() {
        assert_eq!(detect_eol("a\r\nb\r\nc\n"), Eol::Crlf);
        assert_eq!(detect_eol("a\nb\nc\r\n"), Eol::Lf);
        assert_eq!(detect_eol("a\rb\r"), Eol::Cr);
        assert_eq!(detect_eol(""), Eol::Crlf);
    }

    #[test]
    fn encode_keeps_the_line_ending_and_the_bom() {
        assert_eq!(encode("a\r\nb", Eol::Lf, false), b"a\nb");
        assert_eq!(encode("a\r\nb", Eol::Cr, false), b"a\rb");
        assert_eq!(encode("a\r\nb", Eol::Crlf, true), b"\xEF\xBB\xBFa\r\nb");
        assert_eq!(encode("", Eol::Lf, false), b"");
    }

    #[test]
    fn lone_cr_and_lf_become_crlf() {
        assert_eq!(normalize("a\rb\nc\r\nd"), "a\r\nb\r\nc\r\nd");
        assert_eq!(utf16_len("aé😀"), 4); // 1 + 1 + 2
    }
}
