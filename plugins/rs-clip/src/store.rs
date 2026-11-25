//! The clipboard history on disk.
//!
//! `%APPDATA%\WinRust\clipboard\index.tsv` is the list (newest first); every row names its own data file:
//!   `<id>\t<text|image|files>\t<unix ms>\t<hash>\t<label>\t<data file>\t<thumbnail|->`
//! Text lives in `<id>.txt` (UTF-8), files in `<id>.files` (one path per line) and images in `<id>.bmp` plus
//! `<id>.thumb.bmp`. Nothing is encrypted: a clipboard is not a secret store. Windows' own "do not put this on the
//! clipboard history" formats are honoured (see `sensitive`), and password managers' known markers are skipped, but this
//! is best effort by design, not a guarantee.
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    time::{SystemTime, UNIX_EPOCH},
};

use windows_sys::Win32::{
    Foundation::{GlobalFree, HGLOBAL, HWND},
    Graphics::Gdi::DeleteObject,
    System::{
        DataExchange::{
            CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
            RegisterClipboardFormatW, SetClipboardData,
        },
        Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE},
        Ole::{CF_BITMAP, CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT},
    },
    UI::{
        Shell::{DragQueryFileW, DROPFILES, HDROP},
        WindowsAndMessaging::{LoadImageW, IMAGE_BITMAP, LR_LOADFROMFILE},
    },
};

use crate::{img, log, ui::wide};

/// Formats a password manager (or Windows' cloud-clipboard switch) uses to say "keep this one out".
const HIDDEN_FORMATS: [&str; 2] = ["ExcludeClipboardContentFromMonitorProcessing", "Clipboard Viewer Ignore"];
const HISTORY_SWITCHES: [&str; 2] = ["CanIncludeInClipboardHistory", "CanUploadToCloudClipboard"];

/// Hash of the bytes this process has just put on the clipboard: the next WM_CLIPBOARDUPDATE is ours, not a copy.
/// 0 = nothing is expected.
pub static LAST_WRITE: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn unix_ms() -> u64 {
    now_ms()
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn hash_text(s: &str) -> u64 {
    let units: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    hash_bytes(&units)
}

fn hash_files(paths: &[String]) -> u64 {
    hash_bytes(paths.join("\n").as_bytes())
}

/// The folder every file of the history lives in.
pub fn dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join(r"WinRust\clipboard"))
        .unwrap_or_else(|| std::env::temp_dir().join("rs-clip"))
}

fn index_file() -> PathBuf {
    dir().join("index.tsv")
}

/// `%ProgramData%\WinRust\look.txt`: `clip-history=100`. One line, re-read on every change; junk means 100.
pub fn history_limit() -> usize {
    let text = std::env::var_os("ProgramData")
        .map(|p| PathBuf::from(p).join(r"WinRust\look.txt"))
        .and_then(|p| fs::read_to_string(p).ok());
    for line in text.unwrap_or_default().lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "clip-history" {
                return v.trim().parse::<usize>().map_or(100, |n| n.clamp(1, 100));
            }
        }
    }
    100
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Image,
    Files,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Image => "image",
            Kind::Files => "files",
        }
    }
    fn parse(s: &str) -> Option<Kind> {
        match s {
            "text" => Some(Kind::Text),
            "image" => Some(Kind::Image),
            "files" => Some(Kind::Files),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    pub at: u64,
    pub hash: u64,
    pub label: String,
    pub data: PathBuf,
    pub thumb: Option<PathBuf>,
}

impl Entry {
    fn thumb_file(&self) -> String {
        self.thumb.as_ref().and_then(|p| p.file_name()).map_or_else(|| "-".into(), |n| n.to_string_lossy().into_owned())
    }
    fn data_file(&self) -> String {
        self.data.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())
    }
}

pub struct Store {
    entries: Vec<Entry>,
    limit: usize,
}

impl Store {
    pub fn load() -> Store {
        let limit = history_limit();
        let mut entries: Vec<Entry> = Vec::new();
        let text = fs::read_to_string(index_file()).unwrap_or_default();
        for line in text.lines() {
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                continue;
            }
            let (Ok(at), Ok(hash), Some(kind)) = (f[2].parse::<u64>(), u64::from_str_radix(f[3], 16), Kind::parse(f[1])) else {
                continue;
            };
            let data = dir().join(f[5]);
            if !data.is_file() {
                continue;
            }
            let thumb = (f[6] != "-").then(|| dir().join(f[6])).filter(|p| p.is_file());
            entries.push(Entry { id: f[0].to_string(), kind, at, hash, label: f[4].to_string(), data, thumb });
        }
        entries.sort_by(|a, b| b.at.cmp(&a.at));
        let mut st = Store { entries, limit };
        st.prune();
        st.clean_orphans();
        st.save();
        st
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.entries.clone()
    }

    /// Re-reads `clip-history` (the popup calls this so a lowered limit applies without waiting for a copy).
    pub fn refresh_limit(&mut self) {
        let limit = history_limit();
        if limit != self.limit {
            self.limit = limit;
            self.prune();
            self.save();
        }
    }

    fn top_hash(&self) -> u64 {
        self.entries.first().map_or(0, |e| e.hash)
    }

    /// Reads the clipboard once and adds it to the history. Returns true when an entry was written.
    /// The clipboard is only held while the data is copied out; images are converted after CloseClipboard.
    pub fn capture(&mut self, hwnd: HWND) -> bool {
        if !open(hwnd) {
            log("clipboard busy: could not open it");
            return false;
        }
        let grabbed = if sensitive() {
            LAST_WRITE.store(0, Relaxed); // a new copy superseded any paste of ours that was still expected
            log("skipped: clipboard content marked private");
            None
        } else {
            grab()
        };
        unsafe { CloseClipboard() };
        let Some(g) = grabbed else { return false };

        // Our own paste: the watcher must not turn the entry into a new history row.
        let own = match &g {
            Grabbed::Text(s) => hash_text(s),
            Grabbed::Files(v) => hash_files(v),
            Grabbed::Dib(d) => hash_bytes(d),
        };
        let expected = LAST_WRITE.load(Relaxed);
        if expected != 0 {
            LAST_WRITE.store(0, Relaxed);
            if expected == own {
                return false;
            }
        }

        self.limit = history_limit();
        let at = unix_ms();
        let id = format!("{at}-{:04}", next_seq());
        let entry = match g {
            Grabbed::Text(s) => {
                if s.trim().is_empty() || s.len() > 8 * 1024 * 1024 {
                    return false;
                }
                let hash = hash_text(&s);
                if hash == self.top_hash() {
                    return false; // the same text copied again: not a new history entry
                }
                let data = dir().join(format!("{id}.txt"));
                if fs::create_dir_all(dir()).and_then(|_| fs::write(&data, &s)).is_err() {
                    return false;
                }
                Entry { id, kind: Kind::Text, at, hash, label: preview(&s, Kind::Text), data, thumb: None }
            }
            Grabbed::Files(paths) => {
                if paths.is_empty() {
                    return false;
                }
                let hash = hash_files(&paths);
                if hash == self.top_hash() {
                    return false;
                }
                let data = dir().join(format!("{id}.files"));
                if fs::create_dir_all(dir()).and_then(|_| fs::write(&data, paths.join("\n"))).is_err() {
                    return false;
                }
                Entry { id, kind: Kind::Files, at, hash, label: preview(&paths.join("\n"), Kind::Files), data, thumb: None }
            }
            Grabbed::Dib(dib) => {
                let hash = hash_bytes(&dib);
                if hash == self.top_hash() {
                    return false;
                }
                let data = dir().join(format!("{id}.bmp"));
                let thumb = dir().join(format!("{id}.thumb.bmp"));
                if fs::create_dir_all(dir()).is_err() {
                    return false;
                }
                match img::dib_to_images(&dib, &data, &thumb) {
                    Some((w, h)) => Entry { id, kind: Kind::Image, at, hash, label: format!("{w} × {h}"), data, thumb: Some(thumb) },
                    None => {
                        log("image: not a DIB GDI can read");
                        let _ = fs::remove_file(&data);
                        let _ = fs::remove_file(&thumb);
                        return false;
                    }
                }
            }
        };
        self.entries.insert(0, entry);
        self.prune();
        self.save();
        true
    }

    /// Drops entries older than the limit, files and all.
    fn prune(&mut self) {
        while self.entries.len() > self.limit {
            let e = self.entries.pop().expect("checked length");
            remove_files(&e);
        }
    }

    /// Removes data files that no index row points at (a crash between writing the image and the index).
    fn clean_orphans(&mut self) {
        let mut keep: HashSet<String> = HashSet::new();
        for e in &self.entries {
            keep.insert(e.data_file());
            if let Some(t) = &e.thumb {
                if let Some(n) = t.file_name() {
                    keep.insert(n.to_string_lossy().into_owned());
                }
            }
        }
        keep.insert("index.tsv".into());
        keep.insert("index.tsv.tmp".into());
        for e in fs::read_dir(dir()).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if !keep.contains(&n) && (n.ends_with(".bmp") || n.ends_with(".txt") || n.ends_with(".files")) {
                let _ = fs::remove_file(e.path());
            }
        }
    }

    /// Writes the index atomically: a half-written index would hide every entry.
    pub fn save(&self) {
        let d = dir();
        if let Err(e) = fs::create_dir_all(&d) {
            log(&format!("history folder: {e}"));
            return;
        }
        let mut text = String::from("# rs-clip index 1\n");
        for e in &self.entries {
            text.push_str(&format!(
                "{}\t{}\t{}\t{:016x}\t{}\t{}\t{}\n",
                e.id,
                e.kind.as_str(),
                e.at,
                e.hash,
                escape(&e.label),
                e.data_file(),
                e.thumb_file()
            ));
        }
        let tmp = d.join("index.tsv.tmp");
        if let Err(e) = fs::write(&tmp, text) {
            log(&format!("index: {e}"));
            return;
        }
        if fs::rename(&tmp, d.join("index.tsv")).is_err() {
            let _ = fs::remove_file(tmp);
            log("index: could not replace index.tsv");
        }
    }

    pub fn remove(&mut self, id: &str) {
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            let e = self.entries.remove(i);
            remove_files(&e);
            self.save();
        }
    }

    pub fn clear(&mut self) {
        for e in self.entries.drain(..) {
            remove_files(&e);
        }
        self.save();
    }

    pub fn touch(&mut self, id: &str) {
        if let Some(i) = self.entries.iter().position(|e| e.id == id) {
            let mut e = self.entries.remove(i);
            e.at = unix_ms();
            self.entries.insert(0, e);
            self.save();
        }
    }

    /// A copy of one row, for code that must not hold the store lock while it talks to the clipboard.
    pub fn get(&self, id: &str) -> Option<Entry> {
        self.entries.iter().find(|e| e.id == id).cloned()
    }
}

/// Puts an entry back on the clipboard in its own format (CF_UNICODETEXT / CF_DIB / CF_HDROP).
/// Call it **without** the store lock: `CloseClipboard` can deliver WM_CLIPBOARDUPDATE, whose handler wants that lock.
pub fn copy_entry(e: &Entry) -> bool {
    match e.kind {
        Kind::Text => fs::read_to_string(&e.data).ok().is_some_and(|s| put_text(&s)),
        Kind::Image => img::bmp_to_dib(&e.data).is_some_and(|dib| put_dib(&dib, &e.data)),
        Kind::Files => fs::read_to_string(&e.data)
            .ok()
            .map(|s| s.lines().map(str::to_string).filter(|p| !p.is_empty()).collect::<Vec<_>>())
            .is_some_and(|v| put_files(&v)),
    }
}

fn next_seq() -> u32 {
    static N: AtomicU64 = AtomicU64::new(0);
    (N.fetch_add(1, Relaxed) & 0xffff) as u32
}

fn remove_files(e: &Entry) {
    let _ = fs::remove_file(&e.data);
    if let Some(t) = &e.thumb {
        let _ = fs::remove_file(t);
    }
}

fn escape(s: &str) -> String {
    s.replace(['\t', '\r', '\n'], " ")
}

fn preview(s: &str, kind: Kind) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    match kind {
        Kind::Files => {
            let paths: Vec<&str> = s.lines().filter(|l| !l.is_empty()).collect();
            let names: Vec<String> = paths
                .iter()
                .take(3)
                .map(|p| Path::new(p).file_name().map_or_else(|| p.to_string(), |n| n.to_string_lossy().into_owned()))
                .collect();
            let mut out = names.join(", ");
            if paths.len() > 3 {
                out.push_str(&format!("  +{}", paths.len() - 3));
            }
            if out.is_empty() {
                "?".into()
            } else {
                out
            }
        }
        _ => {
            let mut out: String = line.chars().take(160).collect();
            if line.chars().count() > 160 {
                out.push('…');
            }
            if out.is_empty() {
                "…".into()
            } else {
                out
            }
        }
    }
}

/// The full text of an entry, for the search box (capped: searching gigabytes of copied text is not useful).
pub fn entry_text(e: &Entry) -> String {
    match e.kind {
        Kind::Text => {
            let bytes = fs::read(&e.data).unwrap_or_default();
            let cut = bytes.len().min(256 * 1024);
            String::from_utf8_lossy(&bytes[..cut]).into_owned()
        }
        Kind::Files => fs::read_to_string(&e.data).unwrap_or_default(),
        Kind::Image => format!("image {}", e.label),
    }
}

enum Grabbed {
    Text(String),
    Files(Vec<String>),
    Dib(Vec<u8>),
}

fn open(hwnd: HWND) -> bool {
    // The program that just copied may still hold the clipboard for a few milliseconds.
    for _ in 0..8 {
        if unsafe { OpenClipboard(hwnd) } != 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(12));
    }
    false
}

fn sensitive() -> bool {
    unsafe {
        for name in HIDDEN_FORMATS {
            let f = RegisterClipboardFormatW(wide(name).as_ptr());
            if f != 0 && !GetClipboardData(f).is_null() {
                return true;
            }
        }
        for name in HISTORY_SWITCHES {
            let f = RegisterClipboardFormatW(wide(name).as_ptr());
            let h = if f == 0 { std::ptr::null_mut() } else { GetClipboardData(f) };
            if !h.is_null() {
                let p = GlobalLock(h);
                let off = !p.is_null() && *(p as *const u32) == 0;
                if !p.is_null() {
                    GlobalUnlock(h);
                }
                if off {
                    return true;
                }
            }
        }
    }
    false
}

fn grab() -> Option<Grabbed> {
    unsafe {
        if IsClipboardFormatAvailable(CF_HDROP as u32) != 0 {
            if let Some(v) = files() {
                return Some(Grabbed::Files(v));
            }
        }
        // CF_DIB is the compatible copy; DIBV5 is only a fallback (some apps write both).
        for f in [CF_DIB as u32, CF_DIBV5 as u32] {
            if IsClipboardFormatAvailable(f) != 0 {
                if let Some(v) = dib(f) {
                    return Some(Grabbed::Dib(v));
                }
            }
        }
        if IsClipboardFormatAvailable(CF_BITMAP as u32) != 0 {
            let h = GetClipboardData(CF_BITMAP as u32);
            if !h.is_null() {
                if let Some(v) = img::hbitmap_to_dib(h as windows_sys::Win32::Graphics::Gdi::HBITMAP) {
                    return Some(Grabbed::Dib(v));
                }
            }
        }
        if IsClipboardFormatAvailable(CF_UNICODETEXT as u32) != 0 {
            if let Some(v) = text() {
                return Some(Grabbed::Text(v));
            }
        }
    }
    None
}

unsafe fn dib(format: u32) -> Option<Vec<u8>> {
    let h = GetClipboardData(format);
    if h.is_null() {
        return None;
    }
    let len = GlobalSize(h);
    if len < 40 {
        return None;
    }
    let p = GlobalLock(h);
    if p.is_null() {
        return None;
    }
    let v = std::slice::from_raw_parts(p as *const u8, len).to_vec();
    GlobalUnlock(h);
    let n = img::dib_len(&v)?;
    let mut v = v;
    v.truncate(n);
    Some(v)
}

unsafe fn text() -> Option<String> {
    let h = GetClipboardData(CF_UNICODETEXT as u32);
    if h.is_null() {
        return None;
    }
    let p = GlobalLock(h);
    if p.is_null() {
        return None;
    }
    let units = (GlobalSize(h) / 2).max(1).min(4 * 1024 * 1024);
    let mut n = 0;
    while n < units && *(p as *const u16).add(n) != 0 {
        n += 1;
    }
    let s = String::from_utf16_lossy(std::slice::from_raw_parts(p as *const u16, n));
    GlobalUnlock(h);
    Some(s)
}

unsafe fn files() -> Option<Vec<String>> {
    let h = GetClipboardData(CF_HDROP as u32) as HDROP;
    if h.is_null() {
        return None;
    }
    let count = DragQueryFileW(h, u32::MAX, std::ptr::null_mut(), 0);
    let mut v = Vec::new();
    for i in 0..count.min(100) {
        let len = DragQueryFileW(h, i, std::ptr::null_mut(), 0);
        if len == 0 {
            continue;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let got = DragQueryFileW(h, i, buf.as_mut_ptr(), buf.len() as u32);
        if got > 0 {
            buf.truncate(got as usize);
            v.push(String::from_utf16_lossy(&buf));
        }
    }
    (!v.is_empty()).then_some(v)
}

fn alloc(len: usize) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, len);
        (!h.is_null()).then_some(h)
    }
}

/// CF_UNICODETEXT, NUL-terminated.
pub fn put_text(s: &str) -> bool {
    let mut units: Vec<u16> = s.encode_utf16().collect();
    units.push(0);
    let bytes = units.len() * 2;
    LAST_WRITE.store(hash_text(s), Relaxed);
    unsafe {
        if !open(std::ptr::null_mut()) {
            LAST_WRITE.store(0, Relaxed);
            return false;
        }
        EmptyClipboard();
        let ok = match alloc(bytes) {
            Some(h) => {
                let p = GlobalLock(h);
                if p.is_null() {
                    GlobalFree(h);
                    false
                } else {
                    std::ptr::copy_nonoverlapping(units.as_ptr().cast::<u8>(), p as *mut u8, bytes);
                    GlobalUnlock(h);
                    let r = SetClipboardData(CF_UNICODETEXT as u32, h);
                    if r.is_null() {
                        GlobalFree(h);
                        false
                    } else {
                        true
                    }
                }
            }
            None => false,
        };
        CloseClipboard();
        if !ok {
            LAST_WRITE.store(0, Relaxed);
        }
        ok
    }
}

/// CF_DIB (and CF_BITMAP when the BMP can be loaded), for pasting an image entry.
pub fn put_dib(dib: &[u8], bmp: &Path) -> bool {
    LAST_WRITE.store(hash_bytes(dib), Relaxed);
    unsafe {
        if !open(std::ptr::null_mut()) {
            LAST_WRITE.store(0, Relaxed);
            return false;
        }
        EmptyClipboard();
        let mut ok = false;
        if let Some(h) = (dib.len() >= 40).then(|| alloc(dib.len())).flatten() {
            let p = GlobalLock(h);
            if p.is_null() {
                GlobalFree(h);
            } else {
                std::ptr::copy_nonoverlapping(dib.as_ptr(), p as *mut u8, dib.len());
                GlobalUnlock(h);
                let r = SetClipboardData(CF_DIB as u32, h);
                if r.is_null() {
                    GlobalFree(h);
                } else {
                    ok = true;
                }
            }
        }
        // An HBITMAP as well: a few old programs ask for CF_BITMAP only. The clipboard owns it once set.
        let hb = LoadImageW(std::ptr::null_mut(), wide(&bmp.to_string_lossy()).as_ptr(), IMAGE_BITMAP, 0, 0, LR_LOADFROMFILE);
        if !hb.is_null() && SetClipboardData(CF_BITMAP as u32, hb).is_null() {
            DeleteObject(hb);
        }
        CloseClipboard();
        if !ok {
            LAST_WRITE.store(0, Relaxed);
        }
        ok
    }
}

/// CF_HDROP: DROPFILES + a double-NUL-terminated UTF-16 path list.
pub fn put_files(paths: &[String]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let mut list: Vec<u16> = Vec::new();
    for p in paths {
        list.extend(p.encode_utf16());
        list.push(0);
    }
    list.push(0);
    let header = std::mem::size_of::<DROPFILES>();
    let bytes = header + list.len() * 2;
    LAST_WRITE.store(hash_files(paths), Relaxed);
    unsafe {
        if !open(std::ptr::null_mut()) {
            LAST_WRITE.store(0, Relaxed);
            return false;
        }
        EmptyClipboard();
        let ok = match alloc(bytes) {
            Some(h) => {
                let p = GlobalLock(h);
                if p.is_null() {
                    GlobalFree(h);
                    false
                } else {
                    let df = DROPFILES { pFiles: header as u32, pt: windows_sys::Win32::Foundation::POINT { x: 0, y: 0 }, fNC: 0, fWide: 1 };
                    std::ptr::write(p as *mut DROPFILES, df);
                    std::ptr::copy_nonoverlapping(list.as_ptr(), (p as *mut u8).add(header).cast::<u16>(), list.len());
                    GlobalUnlock(h);
                    let r = SetClipboardData(CF_HDROP as u32, h);
                    if r.is_null() {
                        GlobalFree(h);
                        false
                    } else {
                        true
                    }
                }
            }
            None => false,
        };
        CloseClipboard();
        if !ok {
            LAST_WRITE.store(0, Relaxed);
        }
        ok
    }
}
