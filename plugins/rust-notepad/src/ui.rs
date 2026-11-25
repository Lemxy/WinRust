//! Drawing helpers: the WinRust palette of the current style, fonts for every role, rounded shapes and
//! text. Everything paints into the memory DC that `main::paint` blits in one go.

use std::ptr::null_mut;
use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::*};

pub use winrust_common::style::{card, icon_face, mix, pal, ui_face};

pub const F_BODY: usize = 0;
pub const F_TITLE: usize = 1;
pub const F_SMALL: usize = 2;
pub const F_ICON: usize = 3;
pub const F_ICON_S: usize = 4;
pub const F_MONO: usize = 5;
pub const FONT_COUNT: usize = 6;

/// Colours of the current WinRust style, derived once per paint.
pub struct Theme {
    pub bg: u32,
    pub side: u32,
    pub card: u32,
    pub sep: u32,
    pub hover: u32,
    pub accent: u32,
    pub text: u32,
    pub dim: u32,
    pub warn: u32,
    pub bad: u32,
}

pub fn theme() -> Theme {
    let p = pal();
    let card = card();
    Theme {
        bg: p.bg,
        side: mix(p.bg, p.text, 0.035),
        card,
        sep: p.line,
        hover: mix(card, p.text, 0.05),
        accent: p.accent,
        text: p.text,
        dim: p.dim,
        warn: winrust_common::style::WARN,
        bad: winrust_common::style::BAD,
    }
}

/// A font for every role, scaled for `dpi`.
pub fn create_fonts(dpi: i32) -> [isize; FONT_COUNT] {
    let face = ui_face();
    let icons = icon_face();
    let f = |h: f32, w: i32, name: &str| unsafe {
        let px = -((h * dpi as f32 / 96.0).round() as i32);
        CreateFontW(px, 0, 0, 0, w, 0, 0, 0, 1, 0, 0, 5, 0, crate::wide(name).as_ptr()) as isize
    };
    [f(13.5, 400, face), f(14.5, 600, face), f(11.5, 400, face), f(15.0, 400, icons), f(12.5, 400, icons), f(14.5, 400, mono_face())]
}

pub unsafe fn delete_fonts(fonts: &[isize; FONT_COUNT]) {
    for &f in fonts {
        if f != 0 {
            unsafe { DeleteObject(f as _) };
        }
    }
}

/// Fills `r`; `radius` is the corner radius in device pixels (0 = square corners).
/// Rounded shapes go through GDI+ for smooth edges; GDI+ is started in `main`.
pub unsafe fn shape(dc: HDC, r: RECT, color: u32, radius: i32) {
    use windows_sys::Win32::Graphics::GdiPlus::*;
    unsafe {
        if r.right <= r.left || r.bottom <= r.top {
            return;
        }
        if radius <= 0 {
            let b = CreateSolidBrush(color);
            FillRect(dc, &r, b);
            DeleteObject(b as _);
            return;
        }
        let mut g = null_mut();
        if GdipCreateFromHDC(dc, &mut g) != 0 || g.is_null() {
            let b = CreateSolidBrush(color);
            FillRect(dc, &r, b);
            DeleteObject(b as _);
            return;
        }
        GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
        let (x, y, w, h) = (r.left as f32, r.top as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32);
        let d = (radius as f32 * 2.0).min(w).min(h);
        let mut path = null_mut();
        if GdipCreatePath(FillModeAlternate, &mut path) != 0 {
            GdipDeleteGraphics(g);
            let b = CreateSolidBrush(color);
            FillRect(dc, &r, b);
            DeleteObject(b as _);
            return;
        }
        GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
        GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
        GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
        GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
        GdipClosePathFigure(path);
        // COLORREF -> ARGB
        let argb = 0xFF00_0000 | (color & 255) << 16 | (color >> 8 & 255) << 8 | (color >> 16 & 255);
        let mut br = null_mut();
        if GdipCreateSolidFill(argb, &mut br) == 0 {
            GdipFillPath(g, br as *mut GpBrush, path);
            GdipDeleteBrush(br as *mut GpBrush);
        }
        GdipDeletePath(path);
        GdipDeleteGraphics(g);
    }
}

pub unsafe fn hline(dc: HDC, x0: i32, x1: i32, y: i32, color: u32) {
    unsafe { shape(dc, RECT { left: x0, top: y, right: x1, bottom: y + 1 }, color, 0) };
}

pub unsafe fn text(dc: HDC, font: isize, s: &str, r: RECT, color: u32, flags: u32) {
    unsafe {
        SelectObject(dc, font as _);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        let mut rr = r;
        DrawTextW(dc, crate::wide(s).as_ptr(), -1, &mut rr, flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    }
}

pub fn inset(r: RECT, n: i32) -> RECT {
    RECT { left: r.left + n, top: r.top + n, right: r.right - n, bottom: r.bottom - n }
}

pub fn contains(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// The editor's face: a monospace font whatever the UI style is (Cascadia Mono where Windows staged
/// it, else the Consolas every installation has).
fn mono_face() -> &'static str {
    static F: std::sync::OnceLock<&str> = std::sync::OnceLock::new();
    F.get_or_init(|| {
        let file = |n: &str| std::env::var_os("WINDIR").is_some_and(|w| std::path::PathBuf::from(w).join("Fonts").join(n).is_file());
        if file("CascadiaMono.ttf") {
            "Cascadia Mono"
        } else if file("consola.ttf") {
            "Consolas"
        } else {
            "Courier New"
        }
    })
}
