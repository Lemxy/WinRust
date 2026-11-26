//! The drives Windows knows, read **once on a worker thread** and cached.
//!
//! Querying a volume is not a cheap memory operation: when a USB stick is unplugged while its letter stays assigned (or
//! a share dies), `GetDriveTypeW` / `GetVolumeInformationW` / `GetDiskFreeSpaceExW` / `SHGetFileInfoW` can block for
//! minutes inside the volume driver. Doing that in `WM_CREATE` (the navigation pane and the "This PC" list did) froze
//! the whole file window, so every read of the drive list goes through this cache now:
//!
//! - `cached()` never touches a device: it returns the last list (empty until the first scan finishes).
//! - `scan()` starts one background scan; each drive is probed on its own thread and the scan waits at most
//!   `DEADLINE` for all of them, so one dead device delays the list a couple of seconds instead of blocking forever.
//! - When the list is in place the owner window gets `msg` and rebuilds from `cached()`.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    path::PathBuf,
    sync::{mpsc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::HWND,
    Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW},
    UI::{Shell::{SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHFILEINFOW, SHGetFileInfoW}, WindowsAndMessaging::PostMessageW},
};

/// One drive as the shell sees it. `label`/`free`/`total`/`icon` are zero for network drives (asking a dead share is
/// exactly what must not be done, even on a worker).
#[derive(Clone)]
pub struct Drive {
    pub letter: char,
    pub root: String,
    pub ty: u32,
    pub label: String,
    /// The volume's file system ("NTFS", "exFAT", ...), empty for a drive whose volume could not be asked.
    pub fs: String,
    pub free: u64,
    pub total: u64,
    pub icon: i32,
}

static CACHE: OnceLock<Mutex<Vec<Drive>>> = OnceLock::new();
static SCANNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long the scan waits for all the drives together. A drive that has not answered by then is left out of the list
/// (its probe thread stays in the kernel until the device comes back; nothing else waits for it).
const DEADLINE: Duration = Duration::from_secs(3);

fn cache() -> &'static Mutex<Vec<Drive>> {
    CACHE.get_or_init(|| Mutex::new(vec![]))
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The last known drives. Never blocks.
pub fn cached() -> Vec<Drive> {
    cache().lock().unwrap().clone()
}

/// True once at least one scan has finished (the list may legitimately be empty: no drives).
pub fn ready() -> bool {
    READY.load(std::sync::atomic::Ordering::Relaxed)
}

/// True if the drive that holds `path` (`C:\...`) is in the cached list. A missing drive cannot be listed, copied or
/// restored; paths without a drive letter (UNC, shell namespaces) are not judged here and return true.
pub fn holds(path: &std::path::Path) -> bool {
    let Some(root) = path.components().next().and_then(|c| match c {
        std::path::Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().into_owned()),
        _ => None,
    }) else {
        return true;
    };
    let root = root.trim_end_matches('\\').to_string();
    cached().iter().any(|d| d.root.trim_end_matches('\\').eq_ignore_ascii_case(&root))
}

/// Probes one drive. Runs on its own thread: any call here can block on a dead device.
fn probe(letter: char) -> Option<Drive> {
    unsafe {
        let root = format!("{letter}:\\");
        let wide = w(&root);
        let ty = GetDriveTypeW(wide.as_ptr());
        if ty <= 1 {
            return None; // DRIVE_NO_ROOT_DIR: the letter is assigned to nothing (a stale USB)
        }
        let (mut label, mut fs, mut free, mut total) = (String::new(), String::new(), 0u64, 0u64);
        if matches!(ty, 2 | 3) {
            let mut buf = [0u16; 128];
            let mut fsbuf = [0u16; 64];
            if GetVolumeInformationW(wide.as_ptr(), buf.as_mut_ptr(), 128, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fsbuf.as_mut_ptr(), 64) != 0 {
                let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
                label = String::from_utf16_lossy(&buf[..n]);
                let n = fsbuf.iter().position(|c| *c == 0).unwrap_or(fsbuf.len());
                fs = String::from_utf16_lossy(&fsbuf[..n]);
            }
            if GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) == 0 {
                total = 0;
            }
        }
        let mut icon = -1i32;
        if ty != 4 {
            // asking the shell for a network root is what stalls on a dead share; the pane has a glyph for those
            let mut fi: SHFILEINFOW = std::mem::zeroed();
            if SHGetFileInfoW(wide.as_ptr(), 0, &mut fi, std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | SHGFI_SMALLICON) != 0 {
                icon = fi.iIcon;
            }
        }
        Some(Drive { letter, root, ty, label, fs, free, total, icon })
    }
}

fn scan_now() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let letters: Vec<char> = (0..26u32).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i as u8) as char).collect();
    let (tx, rx) = mpsc::channel::<(char, Option<Drive>)>();
    let mut pending = letters.len();
    for letter in letters {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((letter, probe(letter))); // fails at once if the scan already gave up on it
        });
    }
    drop(tx);
    let end = Instant::now() + DEADLINE;
    let mut out = vec![];
    while pending > 0 {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left) {
            Ok((_, Some(d))) => out.push(d),
            Ok((_, None)) => {}
            Err(_) => break,
        }
        pending -= 1;
    }
    out.sort_by_key(|d| d.letter);
    out
}

/// Starts one scan (a second call while it runs is a no-op) and posts `msg` to `h` when the cache is updated.
pub fn scan(h: HWND, msg: u32) {
    if SCANNING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let hwnd = h as isize; // HWND is not Send: cross the thread boundary as a plain number
    std::thread::spawn(move || {
        let list = scan_now();
        *cache().lock().unwrap() = list;
        READY.store(true, std::sync::atomic::Ordering::Relaxed);
        SCANNING.store(false, std::sync::atomic::Ordering::Relaxed);
        unsafe { PostMessageW(hwnd as _, msg, 0, 0) };
    });
}

/// The `$Recycle.Bin` folder of every drive that has one, from the cache (never asks a device).
pub fn trash_bins() -> Vec<PathBuf> {
    cached()
        .iter()
        .filter(|d| matches!(d.ty, 2 | 3))
        .map(|d| PathBuf::from(format!("{}$Recycle.Bin", d.root)))
        .collect() // a bin that cannot be read is skipped by the scan itself; checking here would ask the device again
}
