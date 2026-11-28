//! The wallpaper as the BMP the rust-explorer desktop paints (%LOCALAPPDATA%\rs-shell\wallpaper.bmp). rs-shell makes it at
//! sign-in; the desktop makes it again when the wallpaper changes while it runs (#188).
use std::{path::{Path, PathBuf}, ptr, time::SystemTime};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// GDI+ decodes any format it knows and saves a BMP (what the desktop paints without explorer's transcoder).
fn to_bmp(src: &str, dst: &Path) {
    use windows_sys::{core::GUID, Win32::Graphics::GdiPlus::*};
    const BMP: GUID = GUID { data1: 0x557C_F400, data2: 0x1A04, data3: 0x11D3, data4: [0x9A, 0x73, 0x00, 0x00, 0xF8, 0x1E, 0xF3, 0x2E] }; // image/bmp encoder
    unsafe {
        let (mut token, mut input): (usize, GdiplusStartupInput) = (0, std::mem::zeroed());
        input.GdiplusVersion = 1;
        if GdiplusStartup(&mut token, &input, ptr::null_mut()) != 0 {
            return;
        }
        let mut img = ptr::null_mut();
        if GdipLoadImageFromFile(wide(src).as_ptr(), &mut img) == 0 {
            GdipSaveImageToFile(img, wide(&dst.to_string_lossy()).as_ptr(), &BMP, ptr::null());
            GdipDisposeImage(img);
        }
        GdiplusShutdown(token);
    }
}

fn value() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW};
    let mut buf = vec![0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, wide(r"Control Panel\Desktop").as_ptr(), wide("WallPaper").as_ptr(), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len) };
    if r != 0 { return None; }
    buf.truncate((len as usize / 2).saturating_sub(1));
    Some(String::from_utf16_lossy(&buf)).filter(|s| !s.trim().is_empty())
}

/// The picture to show: the WallPaper value, else Explorer's own decoded copy (Spotlight, slideshow and a moved or deleted
/// source still leave it). None = a solid colour.
fn source() -> Option<PathBuf> {
    let transcoded = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Themes\TranscodedWallpaper"));
    value().map(PathBuf::from).into_iter().chain(transcoded).find(|p| p.is_file())
}

fn copy_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|b| PathBuf::from(b).join("rs-shell").join("wallpaper.bmp"))
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Changes when another picture is chosen or the current one is rewritten.
pub fn stamp() -> (Option<PathBuf>, Option<SystemTime>) {
    let src = source();
    let time = src.as_deref().and_then(modified);
    (src, time)
}

/// Makes the BMP and returns the file to show. A failed conversion leaves no stale or missing path behind: the old code
/// then pointed Windows at a file that did not exist, and the desktop went black until the next reboot (#108, #111).
/// `changed`: the wallpaper was just switched, so the copy is stale even if the new picture is an older file.
pub fn bmp(changed: bool) -> Option<PathBuf> {
    let src = source()?;
    let dst = copy_path()?;
    // the system wallpaper may be our own copy (rs-shell sets it): removing it here would delete the source
    if src.to_string_lossy().eq_ignore_ascii_case(&dst.to_string_lossy()) {
        return Some(src);
    }
    std::fs::create_dir_all(dst.parent()?).ok()?;
    if src.extension().is_some_and(|e| e.eq_ignore_ascii_case("bmp")) {
        let _ = std::fs::remove_file(&dst); // the desktop must not keep painting an old conversion
        let _ = std::fs::copy(&src, &dst);
        return Some(src);
    }
    if changed || !dst.is_file() || modified(&dst) < modified(&src) {
        let _ = std::fs::remove_file(&dst);
        to_bmp(&src.to_string_lossy(), &dst);
    }
    dst.is_file().then_some(dst)
}

#[cfg(test)]
mod tests {
    #[test]
    fn converts_a_jpeg_to_bmp() {
        let src = r"C:\Windows\Web\Wallpaper\Windows\img0.jpg";
        if !std::path::Path::new(src).is_file() {
            return;
        }
        let dst = std::env::temp_dir().join("winrust-wallpaper-test.bmp");
        let _ = std::fs::remove_file(&dst);
        super::to_bmp(src, &dst);
        assert_eq!(&std::fs::read(&dst).unwrap()[..2], b"BM");
        let _ = std::fs::remove_file(dst);
    }
}
