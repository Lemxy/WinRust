//! Icons at the size they are drawn at, from the sharpest source the shell has.
//!
//! Windows keeps every icon in several sizes (16, 32, 48 and, since Vista, 256 px) and `DrawIconEx` scales with a plain
//! nearest/stretch filter: a 32 px icon drawn at 24 px comes out jagged, and a 16 px one drawn at 24 px is mush. So
//! `best` asks the shell for the largest asset of a path (the 256 px image list first), and `draw` resamples
//! anything bigger than the target here — area average on a 32-bit DIB, alpha included — instead of letting GDI stretch
//! it. An icon smaller than the target is never enlarged: it is drawn at its own size, centred.
//!
//! The shell image lists are `IImageList` COM objects and windows-sys ships no COM interfaces, so this module walks the
//! vtable by hand (only `Release` and `GetIcon`; the layout of `IImageList` is fixed by shobjidl_core.h). COM must be
//! initialized on the calling thread — the UI thread of a program that draws icons always is.
//!
//! Every `HICON` handed out is cached and shared: never destroy one. Copies made here are cached too; `clear_cache`
//! forgets them (for a theme or DPI change) without destroying the handles, since a button may still be holding one.
use std::{
    collections::HashMap,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::null_mut,
    sync::{LazyLock, Mutex},
};
use windows_sys::{
    core::{GUID, HRESULT},
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
};

/// `ILD_TRANSPARENT` (commctrl.h): draw the icon with its mask, not over a filled background. `common` does not enable
/// `Win32_UI_Controls`, and this is the only constant taken from it.
const ILD_TRANSPARENT: u32 = 1;

/// `IID_IImageList` (shobjidl_core.h): what `SHGetImageList` is asked for.
const IID_IIMAGELIST: GUID = GUID::from_u128(0x46EB5926_582E_4017_9FDF_E8998DAA0950);
/// `IImageList` vtable slots: IUnknown's three, then Add, ReplaceIcon, SetOverlayImage, Replace, AddMasked, Draw,
/// Remove, GetIcon (shobjidl_core.h, in that order).
const SLOT_RELEASE: usize = 2;
const SLOT_GET_ICON: usize = 10;
/// A cache is a convenience, not a budget: when one grows past this (long sessions, many animation sizes) it is emptied
/// and filled again on demand.
const CACHE_MAX: usize = 2048;

type ReleaseFn = unsafe extern "system" fn(*mut std::ffi::c_void) -> u32;
type GetIconFn = unsafe extern "system" fn(*mut std::ffi::c_void, i32, u32, *mut HICON) -> HRESULT;

/// The best icon of a path at `px` pixels, or null if the shell has none (the caller keeps its own fallback).
///
/// Tries, in order, the 256 px shell image list (`SHIL_JUMBO`), the 48 px one (`SHIL_EXTRALARGE`), `ExtractIconExW` and
/// finally the icon the shell gives a file of that type. Anything bigger than `px` is resampled down to it, so the
/// result is usually exactly `px` and `draw` can put it on the DC without scaling.
pub unsafe fn best(path: &Path, px: i32) -> HICON {
    unsafe {
        if px <= 0 || path.as_os_str().is_empty() {
            return null_mut();
        }
        let key = (path.to_path_buf(), px);
        if let Some(&h) = best_cache().get(&key) {
            return h as HICON;
        }
        let mut ic = from_image_list(path, SHIL_JUMBO);
        if ic.is_null() {
            ic = from_image_list(path, SHIL_EXTRALARGE);
        }
        if ic.is_null() {
            ic = extract(path);
        }
        if ic.is_null() {
            return null_mut();
        }
        // Never enlarge: an icon that only exists at 16 px stays 16 px and the caller centres it.
        let (sw, sh) = icon_size(ic);
        let ic = if sw > px && sh > px {
            let small = shrink(ic, px);
            if small.is_null() {
                ic
            } else {
                DestroyIcon(ic); // ours (GetIcon / ExtractIconEx copy): the temporary goes, the cached one stays
                small
            }
        } else {
            ic
        };
        let mut m = best_cache();
        if m.len() >= CACHE_MAX {
            m.clear();
        }
        m.insert(key, ic as isize);
        ic
    }
}

/// Draws `ic` at (x, y) in a `px` x `px` box, sharing `best`'s quality rule: exact size straight through, bigger
/// resampled by area average, smaller drawn as it is (centred in the box).
pub unsafe fn draw(dc: HDC, ic: HICON, x: i32, y: i32, px: i32) {
    unsafe {
        if dc.is_null() || ic.is_null() || px <= 0 {
            return;
        }
        let (sw, sh) = icon_size(ic);
        if sw <= 0 || (sw == px && sh == px) {
            DrawIconEx(dc, x, y, ic, px, px, 0, null_mut(), DI_NORMAL);
            return;
        }
        let fitted = fit(ic, px);
        if fitted == ic && sw <= px && sh <= px {
            // already small enough: its own size, centred, rather than a blurred enlargement
            DrawIconEx(dc, x + (px - sw) / 2, y + (px - sh) / 2, ic, sw, sh, 0, null_mut(), DI_NORMAL);
        } else {
            // the copy (or, if GDI could not make one, the icon itself stretched) fills the box
            DrawIconEx(dc, x, y, fitted, px, px, 0, null_mut(), DI_NORMAL);
        }
    }
}

/// The icon a window shows for itself, at `px` pixels: `WM_GETICON` (big, then small2), else the class icon. Null if it
/// has none. The handles are the window's own, borrowed, so they are never destroyed — a bigger one is copied down to
/// `px` (and cached) the same way `draw` does it.
pub unsafe fn for_window(h: HWND, px: i32) -> HICON {
    unsafe {
        if h.is_null() || px <= 0 {
            return null_mut();
        }
        let mut ic: HICON = null_mut();
        for which in [ICON_BIG, ICON_SMALL2] {
            let mut r = 0usize;
            SendMessageTimeoutW(h, WM_GETICON, which as usize, 0, SMTO_ABORTIFHUNG, 40, &mut r);
            if r != 0 {
                ic = r as HICON;
                break;
            }
        }
        if ic.is_null() {
            let c = GetClassLongPtrW(h, GCLP_HICON);
            let c = if c != 0 { c } else { GetClassLongPtrW(h, GCLP_HICONSM) };
            ic = c as HICON;
        }
        if ic.is_null() {
            return null_mut();
        }
        fit(ic, px)
    }
}

/// Forgets every cached icon (theme and DPI changes hand out different art). The handles stay alive: a button may still
/// be pointing at one.
pub fn clear_cache() {
    if let Ok(mut m) = BEST.lock() {
        m.clear();
    }
    if let Ok(mut m) = FIT.lock() {
        m.clear();
    }
}

// ---------- caches ----------

static BEST: LazyLock<Mutex<HashMap<(PathBuf, i32), isize>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Icons copied down to a size, by (source handle, px): the same window icon is asked for on every frame of an animation.
static FIT: LazyLock<Mutex<HashMap<(isize, i32), isize>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn best_cache() -> std::sync::MutexGuard<'static, HashMap<(PathBuf, i32), isize>> {
    BEST.lock().unwrap_or_else(|e| e.into_inner())
}

/// `ic` as an icon no bigger than `px` (itself when it already is). Shared, cached: do not destroy the result.
unsafe fn fit(ic: HICON, px: i32) -> HICON {
    unsafe {
        let (sw, sh) = icon_size(ic);
        if sw <= 0 || px <= 0 || (sw <= px && sh <= px) {
            return ic;
        }
        let key = (ic as isize, px);
        let mut m = FIT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&h) = m.get(&key) {
            return h as HICON;
        }
        let small = shrink(ic, px);
        if m.len() >= CACHE_MAX {
            m.clear();
        }
        // a failed copy is remembered the same way (as the icon itself): the same one is asked for on every frame
        let out = if small.is_null() { ic } else { small };
        m.insert(key, out as isize);
        out
    }
}

// ---------- sources ----------

/// The path as a NUL-terminated UTF-16 string.
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// The shell's own rendering of `path` at one of its image-list sizes (`SHIL_JUMBO`, `SHIL_EXTRALARGE`): the icon
/// Explorer would show, arrow overlay on shortcuts included. A copy we own, null if the shell has nothing there.
unsafe fn from_image_list(path: &Path, shil: u32) -> HICON {
    unsafe {
        let w = wide(path);
        let mut sfi: SHFILEINFOW = zeroed();
        // Only the index: the index is the same in every system image list, whichever size is asked for here.
        if SHGetFileInfoW(w.as_ptr(), 0, &mut sfi, size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX) == 0 {
            return null_mut();
        }
        let mut list: *mut std::ffi::c_void = null_mut();
        if SHGetImageList(shil as i32, &IID_IIMAGELIST, &mut list) < 0 || list.is_null() {
            return null_mut();
        }
        let get_icon: GetIconFn = vfunc(list, SLOT_GET_ICON);
        let release: ReleaseFn = vfunc(list, SLOT_RELEASE);
        let mut ic: HICON = null_mut();
        let hr = get_icon(list, sfi.iIcon, ILD_TRANSPARENT, &mut ic);
        release(list);
        if hr < 0 { null_mut() } else { ic }
    }
}

/// `ExtractIconExW` (the exe's large icon), else the icon the shell gives a file of that type. A copy we own.
unsafe fn extract(path: &Path) -> HICON {
    unsafe {
        let w = wide(path);
        let mut ic: HICON = null_mut();
        if ExtractIconExW(w.as_ptr(), 0, &mut ic, null_mut(), 1) > 0 && !ic.is_null() {
            return ic;
        }
        let mut sfi: SHFILEINFOW = zeroed();
        if SHGetFileInfoW(w.as_ptr(), 0, &mut sfi, size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON) != 0 {
            return sfi.hIcon;
        }
        null_mut()
    }
}

/// `slot` of a COM object's vtable. windows-sys has no interfaces, so the function pointer is read by hand.
unsafe fn vfunc<T: Copy>(obj: *mut std::ffi::c_void, slot: usize) -> T {
    unsafe {
        let vtbl = *(obj as *mut *const *const std::ffi::c_void);
        std::mem::transmute_copy::<*const std::ffi::c_void, T>(&*vtbl.add(slot))
    }
}

// ---------- resampling ----------

/// The pixel size of an icon: its colour bitmap when it has one, else its mask. (0, 0) if GDI cannot say.
unsafe fn icon_size(ic: HICON) -> (i32, i32) {
    unsafe {
        if ic.is_null() {
            return (0, 0);
        }
        let mut ii: ICONINFO = zeroed();
        if GetIconInfo(ic, &mut ii) == 0 {
            return (0, 0);
        }
        let (mut w, mut h) = bmp_size(ii.hbmColor);
        if w <= 0 {
            (w, h) = bmp_size(ii.hbmMask);
        }
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        if w < 0 {
            w = -w;
        }
        if h < 0 {
            h = -h;
        }
        (w, h)
    }
}

/// The size of a bitmap, (0, 0) when it is null or not a bitmap.
unsafe fn bmp_size(hbm: HBITMAP) -> (i32, i32) {
    unsafe {
        if hbm.is_null() {
            return (0, 0);
        }
        let mut bm: BITMAP = zeroed();
        if GetObjectW(hbm, size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut _) == 0 {
            return (0, 0);
        }
        (bm.bmWidth, bm.bmHeight)
    }
}

/// A copy of `ic` at `px` x `px`, resampled by area average. Null if GDI cannot make the copy; the caller then keeps the
/// icon it has and lets `DrawIconEx` do what it did before.
///
/// Not `StretchBlt`: HALFTONE averages the colours but takes the alpha channel of the nearest source pixel, which leaves
/// hard, fringed edges where the icon fades out. The icon is lifted to a 32-bit DIB, averaged here (colour weighted by
/// alpha, so a transparent pixel cannot darken its neighbours) and handed back as an icon with a matching AND mask.
unsafe fn shrink(ic: HICON, px: i32) -> HICON {
    unsafe {
        let mut ii: ICONINFO = zeroed();
        if ic.is_null() || px <= 0 || GetIconInfo(ic, &mut ii) == 0 {
            return null_mut();
        }
        let (mut sw, mut sh) = bmp_size(ii.hbmColor);
        if sw <= 0 {
            (sw, sh) = bmp_size(ii.hbmMask);
        }
        let out = if sw > 0 && sh > 0 { resample(&ii, sw, sh, px) } else { null_mut() };
        if !ii.hbmColor.is_null() {
            DeleteObject(ii.hbmColor);
        }
        if !ii.hbmMask.is_null() {
            DeleteObject(ii.hbmMask);
        }
        out
    }
}

/// The icon at `px` from its two bitmaps: colour + alpha (or the mask when the colour bitmap has no alpha) averaged
/// down, mask rebuilt from what is left transparent.
unsafe fn resample(ii: &ICONINFO, sw: i32, sh: i32, px: i32) -> HICON {
    unsafe {
        let Some(mut src) = bgra(ii.hbmColor, sw, sh) else { return null_mut() };
        // Graded per-pixel alpha (anything between 0 and 255) already carries the shape. A colour bitmap whose alpha
        // is flat says nothing about it: all zero is what an icon with no alpha channel comes back from `GetDIBits`,
        // and all 255 is what some exporters store over a classic mask. In both cases the shape lives in the AND mask
        // (bit set = transparent) and dropping it would turn the icon into an opaque square.
        let no_alpha = src.iter().all(|p| p >> 24 == 0);
        let flat_opaque = src.iter().all(|p| p >> 24 == 255);
        if no_alpha || flat_opaque {
            match bgra(ii.hbmMask, sw, sh) {
                Some(mask) => {
                    for (p, m) in src.iter_mut().zip(&mask) {
                        if m & 0x00FF_FFFF != 0 {
                            *p = 0; // transparent: no colour left to average into the neighbours
                        } else if no_alpha {
                            *p |= 0xFF00_0000; // opaque in the mask, but the colour bitmap carried no alpha
                        }
                    }
                }
                // neither alpha nor mask: the icon is a solid square
                None if no_alpha => src.iter_mut().for_each(|p| *p |= 0xFF00_0000),
                None => {}
            }
        }
        // `downscale` divides the colour by the alpha sum, so the output keeps the representation it came in with:
        // an alpha-premultiplied bitmap stays premultiplied and a straight one stays straight. `CreateIconIndirect`
        // is fed the same way the source HICON was stored, so no extra (un)premultiply is needed here.
        let dst = downscale(&src, sw, sh, px);
        icon_from_bgra(&dst, px)
    }
}

/// The bitmap as 32-bit BGRA pixels, bottom row first: that is the order `GetDIBits` fills for every kind of bitmap
/// (a DIB section or a device bitmap) when the header asks for a positive height, so no orientation is left to chance.
/// A 32-bit DIB keeps its alpha channel through the copy.
unsafe fn bgra(hbm: HBITMAP, w: i32, h: i32) -> Option<Vec<u32>> {
    unsafe {
        if hbm.is_null() || w <= 0 || h <= 0 {
            return None;
        }
        let mut bi: BITMAPINFO = zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = h; // bottom-up
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut px = vec![0u32; (w * h) as usize];
        let dc = CreateCompatibleDC(null_mut());
        if dc.is_null() {
            return None;
        }
        let n = GetDIBits(dc, hbm, 0, h as u32, px.as_mut_ptr() as *mut std::ffi::c_void, &mut bi, DIB_RGB_COLORS);
        DeleteDC(dc);
        (n == h).then_some(px)
    }
}

/// Area-average downscale: every target pixel is the average of the source rectangle it covers, each source pixel
/// weighted by its alpha (a transparent pixel adds nothing, so edges keep their colour instead of greying out).
/// `src` is bottom row first, the order `bgra` leaves it in.
fn downscale(src: &[u32], sw: i32, sh: i32, px: i32) -> Vec<u32> {
    let (sw, sh, px) = (sw as usize, sh as usize, px.max(1) as usize);
    let mut dst = vec![0u32; px * px];
    let (fx, fy) = (sw as f64 / px as f64, sh as f64 / px as f64);
    for dy in 0..px {
        let (y0, y1) = (dy as f64 * fy, (dy + 1) as f64 * fy);
        let iy0 = y0.floor() as usize;
        let iy1 = ((y1.ceil() as usize).max(iy0 + 1)).min(sh);
        for dx in 0..px {
            let (x0, x1) = (dx as f64 * fx, (dx + 1) as f64 * fx);
            let ix0 = x0.floor() as usize;
            let ix1 = ((x1.ceil() as usize).max(ix0 + 1)).min(sw);
            let (mut wsum, mut asum, mut r, mut g, mut b) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for y in iy0..iy1 {
                let wy = (y1.min((y + 1) as f64) - y0.max(y as f64)).max(0.0);
                if wy <= 0.0 {
                    continue;
                }
                for x in ix0..ix1 {
                    let wx = (x1.min((x + 1) as f64) - x0.max(x as f64)).max(0.0);
                    if wx <= 0.0 {
                        continue;
                    }
                    let w = wy * wx;
                    let p = src[(sh - 1 - y) * sw + x]; // source rows are bottom-up
                    let a = (p >> 24) as f64;
                    wsum += w;
                    asum += w * a;
                    b += w * a * (p & 0xFF) as f64;
                    g += w * a * ((p >> 8) & 0xFF) as f64;
                    r += w * a * ((p >> 16) & 0xFF) as f64;
                }
            }
            if wsum <= 0.0 {
                continue;
            }
            let a = asum / wsum;
            let (r, g, b) = if asum > 0.0 { (r / asum, g / asum, b / asum) } else { (0.0, 0.0, 0.0) };
            // 0xAARRGGBB: blue in bits 0..8, red in bits 16..24 - the same word `bgra` hands out (a 32-bpp DIB
            // stores its bytes as B, G, R, A) and `icon_from_bgra` writes back. Packing by position instead of by
            // name swaps red and blue, which turns every warm icon (Firefox, AMD, the Explorer folder) into its
            // complement while a dark grey one (Steam) still looks plausible.
            dst[dy * px + dx] = (a.round() as u32) << 24 | (r.round() as u32) << 16 | (g.round() as u32) << 8 | b.round() as u32;
        }
    }
    dst
}

/// A `BITMAPINFO` with room for the two-entry colour table a 1-bpp DIB section needs (`BITMAPINFO` itself declares a
/// single `RGBQUAD`, while `CreateDIBSection` reads one entry per palette index - two at 1 bpp).
#[repr(C)]
struct MonoBmi {
    header: BITMAPINFOHEADER,
    colors: [RGBQUAD; 2],
}

/// An icon of `side` x `side` from BGRA pixels (top row first): a 32-bit DIB section plus the AND mask that marks the
/// transparent pixels, which is what `CreateIconIndirect` expects of an icon with alpha. Both sections are bottom-up,
/// the way a device bitmap holds its rows, so the icon cannot come out upside down.
unsafe fn icon_from_bgra(px: &[u32], side: i32) -> HICON {
    unsafe {
        if side <= 0 || px.len() < (side * side) as usize {
            return null_mut();
        }
        let n = side as usize;
        let mut bi: BITMAPINFO = zeroed();
        bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = side;
        bi.bmiHeader.biHeight = side; // bottom-up
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut std::ffi::c_void = null_mut();
        let color = CreateDIBSection(null_mut(), &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if color.is_null() || bits.is_null() {
            if !color.is_null() {
                DeleteObject(color);
            }
            return null_mut();
        }
        for y in 0..n {
            let dst = (bits as *mut u8).add(y * n * 4);
            let src = px.as_ptr().add((n - 1 - y) * n) as *const u8; // memory row 0 is the bottom of the picture
            std::ptr::copy_nonoverlapping(src, dst, n * 4);
        }
        // The AND mask: 1 bit per pixel, rows padded to a DWORD and stored bottom-up like the colour section so the
        // two stay aligned. 1 = transparent (the destination is kept), 0 = paint the colour. A 1-bpp DIB section
        // needs its two-entry colour table present: `BITMAPINFO` only declares one entry, so passing it as is makes
        // `CreateDIBSection` read a palette word off the end of the struct - and a mask that maps index 1 to black
        // reads as inverted. `MonoBmi` carries the table, so 1 is always white ("keep the destination").
        let stride = ((side + 31) / 32) as usize * 4;
        let mut mi: MonoBmi = zeroed();
        mi.header.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        mi.header.biWidth = side;
        mi.header.biHeight = side; // bottom-up
        mi.header.biPlanes = 1;
        mi.header.biBitCount = 1;
        mi.header.biCompression = BI_RGB;
        mi.colors[0] = RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 };
        mi.colors[1] = RGBQUAD { rgbBlue: 255, rgbGreen: 255, rgbRed: 255, rgbReserved: 0 };
        let mut mbits: *mut std::ffi::c_void = null_mut();
        let mask = CreateDIBSection(null_mut(), &mi as *const MonoBmi as *const BITMAPINFO, DIB_RGB_COLORS, &mut mbits, null_mut(), 0);
        if mask.is_null() || mbits.is_null() {
            if !mask.is_null() {
                DeleteObject(mask);
            }
            DeleteObject(color);
            return null_mut();
        }
        {
            let m = std::slice::from_raw_parts_mut(mbits as *mut u8, stride * n);
            for (y, row) in m.chunks_mut(stride).enumerate() {
                let src = (n - 1 - y) * n; // memory row y is image row n-1-y
                for x in 0..n {
                    if (px[src + x] >> 24) == 0 {
                        row[x / 8] |= 1 << (7 - (x % 8));
                    }
                }
            }
        }
        let mut ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let ic = CreateIconIndirect(&mut ii);
        DeleteObject(mask);
        DeleteObject(color);
        ic
    }
}

#[cfg(test)]
mod tests {
    use super::downscale;

    /// `downscale` hands out 0xAARRGGBB words: an opaque red pixel must stay in bits 16..24. Packing blue there
    /// instead (the bug this guards against) swaps red and blue in every resampled icon.
    #[test]
    fn downscale_keeps_the_channel_order() {
        // Two source rows of one pixel each, bottom row first (the order `bgra` hands out): blue below red.
        let src = [0xFF00_00FFu32, 0xFFFF_0000u32]; // rows: blue, then red
        let dst = downscale(&src, 1, 2, 1);
        // the single target pixel averages both: alpha 255, red 128 in bits 16..24, blue 128 in bits 0..8
        assert_eq!(dst[0], 0xFF80_0080);
    }

    /// A fully transparent source pixel contributes no colour, so a lone transparent area stays transparent and
    /// black instead of picking up whatever colour the bitmap stored underneath the mask.
    #[test]
    fn downscale_leaves_transparent_pixels_empty() {
        let dst = downscale(&[0x00FF_FFFFu32], 1, 1, 1);
        assert_eq!(dst[0], 0);
    }
}
