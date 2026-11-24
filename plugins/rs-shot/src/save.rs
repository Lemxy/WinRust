//! File and clipboard output. PNG/JPEG go through the GDI+ encoders that ship with Windows (the same ones topbar's
//! screenshot already uses), so there is no image crate and no extra dependency. The clipboard gets a plain 24-bit
//! CF_DIB, which every Windows application understands.
use std::{
    ffi::c_void,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

use windows_sys::{
    Win32::{
        Foundation::{HANDLE, GlobalFree},
        Graphics::{
            Gdi::{
                BITMAP, BitBlt, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
            },
            GdiPlus::{GdipCreateBitmapFromHBITMAP, GdipDisposeImage, GdipSaveImageToFile, GpBitmap, GpImage, GdiplusStartup, GdiplusStartupInput},
        },
        System::{
            DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            SystemInformation::GetLocalTime,
        },
    },
    core::GUID,
};

use crate::img::{Image, make_dib};
use crate::ui::wide;

static GP: AtomicUsize = AtomicUsize::new(0);

/// Once per process; called from main before any capture or recording.
pub unsafe fn gdiplus_init() {
    if GP.load(Relaxed) != 0 {
        return;
    }
    let mut token = 0usize;
    let inp = GdiplusStartupInput { GdiplusVersion: 1, DebugEventCallback: 0, SuppressBackgroundThread: 0, SuppressExternalCodecs: 0 };
    if GdiplusStartup(&mut token, &inp, null_mut()) == 0 {
        GP.store(token, Relaxed);
    }
}

pub fn stamp() -> String {
    let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}-{:02}-{:02}_{:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

pub fn pictures_dir() -> PathBuf {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Pictures)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Pictures"))
        .join("Screenshots")
}

pub fn recordings_dir() -> PathBuf {
    known_folder(&windows_sys::Win32::UI::Shell::FOLDERID_Videos)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Videos"))
        .join("Screen Records")
}

/// The real user folder (redirected Pictures/Videos included), via SHGetKnownFolderPath.
fn known_folder(id: &GUID) -> Option<PathBuf> {
    use std::ptr::null_mut;
    use windows_sys::Win32::{System::Com::CoTaskMemFree, UI::Shell::SHGetKnownFolderPath};
    unsafe {
        let mut p: *mut u16 = null_mut();
        if SHGetKnownFolderPath(id, 0, null_mut(), &mut p) != 0 || p.is_null() {
            return None;
        }
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        CoTaskMemFree(p as *const core::ffi::c_void);
        (!s.is_empty()).then(|| PathBuf::from(s))
    }
}

/// The base name of a screenshot file, in the UI language (`Screenshot_...`, `Снимок_...`, …). The folder keeps its
/// Windows name in every language: two installations switching language must not scatter the pictures.
pub fn screenshot_base() -> String {
    t!("Снимок", "Screenshot", ja: "スクリーンショット", pt: "Captura de tela", zh: "截图").to_string()
}

/// The base name of a recording file, in the UI language.
pub fn recording_base() -> String {
    t!("Запись", "Recording", ja: "録画", pt: "Gravação", zh: "录制").to_string()
}

/// `Screenshot_2026-11-24_15-30-00.png` in Pictures\Screenshots, with a `-2`, `-3` suffix if two land in one second.
pub fn auto_path() -> PathBuf {
    let dir = pictures_dir();
    let _ = std::fs::create_dir_all(&dir);
    let base = format!("{}_{}", screenshot_base(), stamp());
    let mut p = dir.join(format!("{base}.png"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{base}-{n}.png"));
        n += 1;
    }
    p
}

pub fn default_video_path() -> PathBuf {
    let dir = recordings_dir();
    let _ = std::fs::create_dir_all(&dir);
    let base = stamp();
    let mut p = dir.join(format!("{}_{base}.mp4", recording_base()));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{}_{base}-{n}.mp4", recording_base()));
        n += 1;
    }
    p
}

pub fn frames_dir() -> PathBuf {
    let mut p = std::env::temp_dir().join(format!("rs-shot-rec-{}", stamp()));
    let mut n = 2;
    while p.exists() {
        p = std::env::temp_dir().join(format!("rs-shot-rec-{}-{n}", stamp()));
        n += 1;
    }
    p
}

fn png() -> GUID {
    GUID { data1: 0x557c_f406, data2: 0x1a04, data3: 0x11d3, data4: [0x9a, 0x73, 0x00, 0x00, 0xf8, 0x1e, 0xf3, 0x2e] }
}
fn jpeg() -> GUID {
    GUID { data1: 0x557c_f401, data2: 0x1a04, data3: 0x11d3, data4: [0x9a, 0x73, 0x00, 0x00, 0xf8, 0x1e, 0xf3, 0x2e] }
}
fn bmp() -> GUID {
    GUID { data1: 0x557c_f400, data2: 0x1a04, data3: 0x11d3, data4: [0x9a, 0x73, 0x00, 0x00, 0xf8, 0x1e, 0xf3, 0x2e] }
}

fn encoder_for(path: &Path) -> GUID {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => jpeg(),
        "bmp" => bmp(),
        _ => png(),
    }
}

/// Encodes a 24-bit DIB. The bitmap may be deleted when this returns.
unsafe fn encode_dib(hbm: HBITMAP, path: &Path) -> bool {
    if hbm.is_null() {
        return false;
    }
    let mut gp: *mut GpBitmap = null_mut();
    if GdipCreateBitmapFromHBITMAP(hbm, null_mut(), &mut gp) != 0 || gp.is_null() {
        return false;
    }
    let w = wide(&path.to_string_lossy());
    let ok = GdipSaveImageToFile(gp as *mut GpImage, w.as_ptr(), &encoder_for(path), null()) == 0;
    GdipDisposeImage(gp as *mut GpImage);
    ok
}

/// Copies any HBITMAP into a 24-bit DIB so GDI+ never sees the screen's zero alpha.
unsafe fn as_dib24(hbm: HBITMAP) -> Option<HBITMAP> {
    let mut bm: BITMAP = std::mem::zeroed();
    if GetObjectW(hbm as HGDIOBJ, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut c_void) == 0 {
        return None;
    }
    let dc = GetDC(null_mut());
    let (dst, _) = make_dib(dc, bm.bmWidth, bm.bmHeight, 24)?;
    let sdc = CreateCompatibleDC(dc);
    let ddc = CreateCompatibleDC(dc);
    let so = SelectObject(sdc, hbm as HGDIOBJ);
    let do_ = SelectObject(ddc, dst as HGDIOBJ);
    BitBlt(ddc, 0, 0, bm.bmWidth, bm.bmHeight, sdc, 0, 0, SRCCOPY);
    SelectObject(sdc, so);
    SelectObject(ddc, do_);
    DeleteDC(sdc);
    DeleteDC(ddc);
    ReleaseDC(null_mut(), dc);
    Some(dst)
}

pub unsafe fn encode_hbitmap(hbm: HBITMAP, path: &Path) -> bool {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Some(dib) = as_dib24(hbm) else { return false };
    let ok = encode_dib(dib, path);
    DeleteObject(dib as HGDIOBJ);
    ok
}

pub unsafe fn save(img: &Image, path: &Path) -> bool {
    if !img.valid() {
        return false;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Some(dib) = img.to_dib24() else { return false };
    let ok = encode_dib(dib, path);
    DeleteObject(dib as HGDIOBJ);
    ok
}

pub unsafe fn encode_jpeg(hbm: HBITMAP, path: &Path) -> bool {
    encode_hbitmap(hbm, path)
}

/// CF_DIB: BITMAPINFOHEADER + bottom-up 24-bit rows, the widest-compatibility clipboard image format.
pub unsafe fn copy_to_clipboard(img: &Image) -> bool {
    if !img.valid() {
        return false;
    }
    let (w, h) = (img.w, img.h);
    let stride = ((w * 3 + 3) / 4 * 4) as usize;
    let size = 40 + stride * h as usize;
    let mem = GlobalAlloc(GMEM_MOVEABLE, size);
    if mem.is_null() {
        return false;
    }
    let p = GlobalLock(mem) as *mut u8;
    if p.is_null() {
        return false;
    }
    // BITMAPINFOHEADER
    let put = |o: usize, v: u32| std::ptr::write_unaligned(p.add(o) as *mut u32, v);
    put(0, 40);
    put(4, w as u32);
    put(8, h as u32);
    std::ptr::write_unaligned(p.add(12) as *mut u16, 1);
    std::ptr::write_unaligned(p.add(14) as *mut u16, 24);
    put(16, 0); // BI_RGB
    put(20, (stride * h as usize) as u32);
    put(24, 2835);
    put(28, 2835);
    let mut o = 40;
    for y in (0..h).rev() {
        let row = (y * w * 4) as usize;
        let mut x = 0i32;
        while x < w {
            let i = row + (x * 4) as usize;
            p.add(o).write(img.px[i]);
            p.add(o + 1).write(img.px[i + 1]);
            p.add(o + 2).write(img.px[i + 2]);
            o += 3;
            x += 1;
        }
        o += stride - (w * 3) as usize;
    }
    GlobalUnlock(mem);
    if OpenClipboard(null_mut()) == 0 {
        GlobalFree(mem);
        return false;
    }
    EmptyClipboard();
    let ok = !SetClipboardData(8, mem as HANDLE).is_null(); // CF_DIB; the system owns the block from here
    if !ok {
        GlobalFree(mem);
    }
    CloseClipboard();
    ok
}

