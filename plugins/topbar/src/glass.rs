//! topbar: a see-through bar (style::bar_opacity below 100) and the blur behind it (style::bar_blur). Windows composes a
//! window with an accent policy by its own pixels' alpha, so the bar is painted into a 32-bit DIB and, at the end, every
//! pixel the background left alone gets the bar's opacity; whatever was drawn over it (text, icons, pills) stays solid.
//! Fading the whole layered window instead faded the icons and the clock along with the background.
//! GDI writes no alpha at all, hence comparing against a copy of the background instead of trusting the alpha byte.
use super::*;
use std::{cell::RefCell, ffi::c_void, sync::atomic::AtomicU8};

/// The bar is see-through: its background, not its contents, gets the opacity (and the blur, when that is on).
pub(crate) fn see_through() -> bool {
    i18n::style::bar_opacity() < 100
}

/// What Windows draws behind the bar: 0 nothing (solid), 2 nothing but our own alpha, 3 its blur.
pub(crate) fn accent() -> u32 {
    match (see_through(), i18n::style::bar_blur()) {
        (false, _) => 0,
        (true, false) => 2,
        (true, true) => 3,
    }
}

/// A top-down 32-bit DIB of `w` x `h`: the bitmap and its pixels; None if Windows would not make one.
pub(crate) unsafe fn dib(dc: HDC, w: i32, h: i32) -> Option<(HBITMAP, *mut u32)> {
    let mut bi: BITMAPINFO = zeroed();
    bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h; // top-down: row 0 first
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut c_void = null_mut();
    let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
    (!bmp.is_null() && !bits.is_null()).then_some((bmp, bits.cast()))
}

thread_local! {
    // the background the last frames started from, and bg_layer's key for it: copied again only when that changes
    static BACKDROP: RefCell<(u64, Vec<u32>)> = const { RefCell::new((0, Vec::new())) };
}

/// Called right after the background is copied in: keeps a copy of it to tell it apart from what comes on top.
/// The background is cached by bg_layer, so the copy is too; an animation frame costs nothing here.
pub(crate) unsafe fn remember_backdrop(px: *const u32, n: usize, key: u64) {
    BACKDROP.with_borrow_mut(|(k, bg)| {
        if *k != key || bg.len() != n {
            GdiFlush();
            *bg = std::slice::from_raw_parts(px, n).to_vec();
            *k = key;
        }
    });
}

/// Background pixels (still equal to the remembered backdrop) become `alpha`, premultiplied as DWM wants them; everything
/// else opaque.
pub(crate) unsafe fn finish(px: *mut u32, n: usize, alpha: u8) {
    GdiFlush();
    BACKDROP.with_borrow(|(_, bg)| {
        if bg.len() == n {
            blend(std::slice::from_raw_parts_mut(px, n), bg, alpha);
        }
    });
}

fn blend(px: &mut [u32], bg: &[u32], alpha: u8) {
    let a = alpha as u32;
    for (p, &b) in px.iter_mut().zip(bg) {
        *p = if (*p ^ b) & 0x00FF_FFFF == 0 {
            let ch = |shift: u32| ((*p >> shift & 255) * a / 255) << shift;
            a << 24 | ch(16) | ch(8) | ch(0)
        } else {
            *p | 0xFF00_0000
        };
    }
}

#[repr(C)]
struct AccentPolicy {
    state: u32, // 0 off, 2 transparent gradient (with flags 2 and colour 0: our alpha only), 3 blur behind (Windows 10 and 11)
    flags: u32,
    color: u32,
    anim: u32,
}

#[repr(C)]
struct CompositionData {
    attr: u32, // 19 = WCA_ACCENT_POLICY
    data: *mut c_void,
    size: usize,
}

/// The accent behind `hwnd` (see `accent`). user32's SetWindowCompositionAttribute has no header, so it is looked up;
/// a Windows without it simply gets no see-through bar. Only called when the state changes.
pub(crate) unsafe fn set_accent(hwnd: HWND, state: u32) {
    static LAST: AtomicU8 = AtomicU8::new(u8::MAX); // the first call always goes through
    if LAST.swap(state as u8, Relaxed) == state as u8 {
        return;
    }
    let Some(f) = GetProcAddress(GetModuleHandleW(wide("user32.dll").as_ptr()), c"SetWindowCompositionAttribute".as_ptr().cast()) else { return };
    let set: unsafe extern "system" fn(HWND, *mut CompositionData) -> i32 = std::mem::transmute(f);
    let mut policy = AccentPolicy { state, flags: if state == 2 { 2 } else { 0 }, color: 0, anim: 0 };
    let mut d = CompositionData { attr: 19, data: (&mut policy as *mut AccentPolicy).cast(), size: size_of::<AccentPolicy>() };
    set(hwnd, &mut d);
}

#[cfg(test)]
mod tests {
    #[test]
    fn background_goes_see_through_and_the_rest_stays_solid() {
        let bg = [0x0020_2020, 0x0020_2020, 0x0010_2030];
        let mut px = [0x0020_2020, 0x00FF_FFFF, 0x0010_2030]; // the middle one was drawn over
        super::blend(&mut px, &bg, 128);
        assert_eq!(px[0], 0x8010_1010); // premultiplied: 0x20 * 128 / 255 = 0x10
        assert_eq!(px[1], 0xFFFF_FFFF);
        assert_eq!(px[2], 0x8008_1018);
    }
}
