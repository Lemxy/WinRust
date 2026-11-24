//! topbar: a see-through bar (style::bar_opacity below 100) and the blur behind it (style::bar_blur). Windows composes a
//! window with an accent policy by its own pixels' alpha, so the bar is painted into a 32-bit DIB and, at the end, every
//! pixel the background left alone gets the bar's opacity; whatever was drawn over it (text, icons, pills) stays solid.
//! Fading the whole layered window instead faded the icons and the clock along with the background.
//! GDI writes no alpha at all, hence comparing against a copy of the background instead of trusting the alpha byte.
//! Highlights (main.rs `pill`) are the exception: they fade in, so they are recorded with their strength as they are drawn and
//! get an alpha in between. Made solid as soon as they differed from the background, they popped up at full strength over a
//! clear bar instead of fading in.
use super::*;
use std::{cell::RefCell, ffi::c_void, sync::atomic::AtomicU8};
use windows_sys::Win32::Graphics::GdiPlus::{GdipCreateBitmapFromScan0, GdipDisposeImage, GdipGetImageGraphicsContext, GpBitmap, GpImage};

pub(crate) const ARGB32: i32 = 0x0026_200A; // PixelFormat32bppARGB: straight (not premultiplied) alpha

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
    // this frame's highlights, alone on a clear sheet the size of the bar, and the GDI+ canvas over it (see `record`)
    static TINTS: RefCell<Tints> = const { RefCell::new(Tints { dc: null_mut(), px: Vec::new(), bmp: null_mut(), g: null_mut() }) };
}

struct Tints {
    dc: HDC, // the bar being painted; nothing else is recorded
    px: Vec<u32>,
    bmp: *mut GpBitmap,
    g: *mut GpGraphics,
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

/// Called after `remember_backdrop`: until `finish`, what `pill` draws on `dc` (`w` x `h`) is also recorded.
pub(crate) unsafe fn record(dc: HDC, w: i32, h: i32) {
    TINTS.with_borrow_mut(|t| {
        drop_canvas(t);
        t.px.clear();
        t.px.resize((w.max(0) * h.max(0)) as usize, 0);
        let (mut bmp, mut g) = (null_mut(), null_mut());
        // GDI+ draws straight into px: the sheet lives in the Vec, the bitmap only wraps it
        if t.px.is_empty() || GdipCreateBitmapFromScan0(w, h, w * 4, ARGB32, t.px.as_mut_ptr().cast(), &mut bmp) != 0 {
            return;
        }
        if GdipGetImageGraphicsContext(bmp as *mut GpImage, &mut g) != 0 {
            GdipDisposeImage(bmp as *mut GpImage);
            return;
        }
        smooth(g);
        (t.dc, t.bmp, t.g) = (dc, bmp, g);
    });
}

/// The canvas recording highlights while `dc` is the see-through bar being painted, None anywhere else.
pub(crate) fn recorder(dc: HDC) -> Option<*mut GpGraphics> {
    TINTS.with_borrow(|t| (!t.g.is_null() && t.dc == dc).then_some(t.g))
}

/// Lets go of the canvas; what it drew stays in px. Returns whether there was one.
unsafe fn drop_canvas(t: &mut Tints) -> bool {
    if t.g.is_null() {
        return false;
    }
    GdipDeleteGraphics(t.g); // flushes
    GdipDisposeImage(t.bmp as *mut GpImage);
    (t.dc, t.bmp, t.g) = (null_mut(), null_mut(), null_mut());
    true
}

/// Background pixels (still equal to the remembered backdrop) become `alpha`, premultiplied as DWM wants them; recorded
/// highlights, as far as they show, something in between; everything else opaque.
pub(crate) unsafe fn finish(px: *mut u32, n: usize, alpha: u8) {
    GdiFlush();
    TINTS.with_borrow_mut(|t| {
        let tints: &[u32] = if drop_canvas(t) && t.px.len() == n { &t.px } else { &[] };
        BACKDROP.with_borrow(|(_, bg)| {
            if bg.len() == n {
                blend(std::slice::from_raw_parts_mut(px, n), bg, tints, alpha);
            }
        });
    });
}

/// `tints` (straight ARGB, may be empty) holds the highlights alone: where one shows, the pixel was drawn as the background
/// under it, and it gets the highlight over the see-through background instead.
fn blend(px: &mut [u32], bg: &[u32], tints: &[u32], alpha: u8) {
    let a = alpha as u32;
    let ch = |v: u32, shift: u32| v >> shift & 255;
    for (i, (p, &b)) in px.iter_mut().zip(bg).enumerate() {
        let t = tints.get(i).copied().unwrap_or(0);
        let m = t >> 24; // how much of the pixel the highlight covers
        // only the highlight on the background here: nothing drawn over it (GDI+ rounds a step or two)
        let bare = m > 0 && [0, 8, 16].into_iter().all(|s| ch(*p, s).abs_diff((ch(b, s) * (255 - m) + ch(t, s) * m + 127) / 255) <= 3);
        *p = if bare {
            // the highlight over what the background shows: alpha and colour both from the two, premultiplied
            let c = |s: u32| (ch(t, s) * m * 255 + ch(b, s) * a * (255 - m)) / (255 * 255) << s;
            (m + a * (255 - m) / 255) << 24 | c(16) | c(8) | c(0)
        } else if m == 0 && (*p ^ b) & 0x00FF_FFFF == 0 {
            a << 24 | (ch(b, 16) * a / 255) << 16 | (ch(b, 8) * a / 255) << 8 | ch(b, 0) * a / 255
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
    use super::*;

    #[test]
    fn a_highlight_drawn_by_the_bar_fades_in() {
        // the whole road a highlight takes: GDI+ into the DIB and the record, then finish
        unsafe {
            let (mut tok, mut inp): (usize, GdiplusStartupInput) = (0, zeroed());
            inp.GdiplusVersion = 1;
            GdiplusStartup(&mut tok, &inp, null_mut());
            let (w, h) = (32, 16);
            let dc = CreateCompatibleDC(null_mut());
            let (bmp, px) = dib(dc, w, h).unwrap();
            let old = SelectObject(dc, bmp);
            let n = (w * h) as usize;
            std::slice::from_raw_parts_mut(px, n).fill(0x0020_2020); // as GDI paints: no alpha
            remember_backdrop(px, n, 7);
            record(dc, w, h);
            pill(dc, RECT { left: 4, top: 2, right: 28, bottom: 14 }, 0x0020_2020, &[(0x00FF_FFFF, 0.25)], 8);
            finish(px, n, 0);
            let p = std::slice::from_raw_parts(px, n);
            let mid = p[(h / 2 * w + w / 2) as usize];
            assert!((60..=68).contains(&(mid >> 24)), "{mid:08x}"); // a quarter strength: see-through, not solid
            assert_eq!(p[0], 0); // the clear bar around it stays clear
            assert!(recorder(dc).is_none()); // nothing recorded after the frame
            SelectObject(dc, old);
            DeleteObject(bmp);
            DeleteDC(dc);
        }
    }

    #[test]
    fn background_goes_see_through_and_the_rest_stays_solid() {
        let bg = [0x0020_2020, 0x0020_2020, 0x0010_2030];
        let mut px = [0x0020_2020, 0x00FF_FFFF, 0x0010_2030]; // the middle one was drawn over
        super::blend(&mut px, &bg, &[], 128);
        assert_eq!(px[0], 0x8010_1010); // premultiplied: 0x20 * 128 / 255 = 0x10
        assert_eq!(px[1], 0xFFFF_FFFF);
        assert_eq!(px[2], 0x8008_1018);
    }

    #[test]
    fn a_highlight_fades_in_over_a_clear_bar() {
        // a white highlight at a quarter strength over a dark background, the bar fully clear
        let (bg, t) = ([0x0020_2020u32; 3], 0x40FF_FFFF);
        let drawn = 0x0058_5858; // what GDI+ made of it: 0x20 * 191/255 + 0xFF * 64/255
        let mut px = [drawn, drawn, 0x00FF_FFFF]; // the last one has an icon over it
        super::blend(&mut px, &bg, &[t, 0, t], 0);
        assert_eq!(px[0], 0x4040_4040); // a quarter white, premultiplied: not solid
        assert_eq!(px[1], 0xFF58_5858); // no highlight recorded there: drawn over, solid
        assert_eq!(px[2], 0xFFFF_FFFF);
        // at full strength the highlight is as solid as before, and at none it is the background
        let mut px = [0x00FF_FFFF, 0x0020_2020];
        super::blend(&mut px, &bg[..2], &[0xFFFF_FFFF, 0], 128);
        assert_eq!(px, [0xFFFF_FFFF, 0x8010_1010]);
    }
}
