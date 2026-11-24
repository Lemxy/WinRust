//! Drawing helpers shared by every page: the theme, fonts, rounded shapes and text.
//! Everything paints into the memory DC that `main::paint` blits in one go.

use std::ptr::null_mut;
use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::*};

use crate::model::{BtnKind, Tone};

pub use winrust_common::style::{card, mix, pal, rgb, ui_face, icon_face};

pub const F_BODY: usize = 0;
pub const F_TITLE: usize = 1;
pub const F_SECTION: usize = 2;
pub const F_SMALL: usize = 3;
pub const F_NAV: usize = 4;
pub const F_ICON: usize = 5;
pub const F_ICON_S: usize = 6;
pub const F_CARD: usize = 7;
pub const F_LINK: usize = 8;
pub const F_MONO: usize = 9;
pub const FONT_COUNT: usize = 10;

/// Colours of the current WinRust style, derived once per paint.
pub struct Theme {
    pub bg: u32,
    pub side: u32,
    pub card: u32,
    pub sep: u32,
    pub hover: u32,
    pub active: u32,
    pub accent: u32,
    pub text: u32,
    pub dim: u32,
    pub off: u32,
    pub good: u32,
    pub warn: u32,
    pub bad: u32,
    pub square: bool,
    pub light: bool,
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
        active: mix(card, p.accent, 0.14),
        accent: p.accent,
        text: p.text,
        dim: p.dim,
        off: mix(p.bg, p.text, 0.20),
        good: winrust_common::style::GOOD,
        warn: winrust_common::style::WARN,
        bad: winrust_common::style::BAD,
        square: winrust_common::style::style().square(),
        light: winrust_common::style::style().light(),
    }
}

pub fn tone_color(tone: Tone, t: &Theme) -> u32 {
    match tone {
        Tone::Neutral => t.text,
        Tone::Accent => t.accent,
        Tone::Good => t.good,
        Tone::Warn => t.warn,
        Tone::Bad => t.bad,
        Tone::Dim => t.dim,
    }
}

/// A font for every role, scaled for `dpi`.
pub fn create_fonts(dpi: i32) -> [isize; FONT_COUNT] {
    let face = ui_face();
    let icons = icon_face();
    let mono = if winrust_common::style::style().mono() { "Consolas" } else { face };
    let f = |h: f32, w: i32, name: &str| unsafe {
        let px = -((h * dpi as f32 / 96.0).round() as i32);
        CreateFontW(px, 0, 0, 0, w, 0, 0, 0, 1, 0, 0, 5, 0, crate::sys::wide(name).as_ptr()) as isize
    };
    [
        f(13.5, 400, face),
        f(20.0, 600, face),
        f(12.5, 600, face),
        f(11.5, 400, face),
        f(13.5, 400, face),
        f(17.0, 400, icons),
        f(13.0, 400, icons),
        f(14.5, 600, face),
        f(12.5, 400, face),
        f(11.5, 400, mono),
    ]
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

pub unsafe fn vline(dc: HDC, x: i32, y0: i32, y1: i32, color: u32) {
    unsafe { shape(dc, RECT { left: x, top: y0, right: x + 1, bottom: y1 }, color, 0) };
}

pub unsafe fn text(dc: HDC, font: isize, s: &str, r: RECT, color: u32, flags: u32) {
    unsafe {
        SelectObject(dc, font as _);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, color);
        let mut rr = r;
        DrawTextW(dc, crate::sys::wide(s).as_ptr(), -1, &mut rr, flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    }
}

pub unsafe fn text_top(dc: HDC, font: isize, s: &str, r: RECT, color: u32) {
    unsafe {
        SelectObject(dc, font as _);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, color);
        let mut rr = r;
        DrawTextW(dc, crate::sys::wide(s).as_ptr(), -1, &mut rr, DT_TOP | DT_LEFT | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    }
}

/// Wraps the text inside `r` (used by the longer home-page links).
pub unsafe fn text_wrap(dc: HDC, font: isize, s: &str, r: RECT, color: u32) {
    unsafe {
        SelectObject(dc, font as _);
        SetBkMode(dc, TRANSPARENT as _);
        SetTextColor(dc, color);
        let mut rr = r;
        DrawTextW(dc, crate::sys::wide(s).as_ptr(), -1, &mut rr, DT_TOP | DT_LEFT | DT_WORDBREAK | DT_NOPREFIX | DT_END_ELLIPSIS);
    }
}

pub unsafe fn text_width(dc: HDC, font: isize, s: &str) -> i32 {
    unsafe {
        SelectObject(dc, font as _);
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        DrawTextW(dc, crate::sys::wide(s).as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
        r.right
    }
}

pub fn inset(r: RECT, n: i32) -> RECT {
    RECT { left: r.left + n, top: r.top + n, right: r.right - n, bottom: r.bottom - n }
}

/// A small pill button, drawn inside `r`.
#[allow(clippy::too_many_arguments)]
pub unsafe fn button(dc: HDC, r: RECT, label: &str, kind: BtnKind, hot: bool, font: isize, t: &Theme) {
    unsafe {
        let fill = match kind {
            BtnKind::Accent => {
                if hot {
                    mix(t.accent, rgb(255, 255, 255), 0.14)
                } else {
                    t.accent
                }
            }
            BtnKind::Danger => {
                if hot {
                    mix(t.card, t.bad, 0.28)
                } else {
                    mix(t.card, t.bad, 0.16)
                }
            }
            BtnKind::Normal => {
                if hot {
                    mix(t.card, t.text, 0.14)
                } else {
                    mix(t.card, t.text, 0.075)
                }
            }
        };
        let ink = match kind {
            BtnKind::Accent => winrust_common::style::on_accent(),
            BtnKind::Danger => t.bad,
            BtnKind::Normal => t.text,
        };
        shape(dc, r, fill, if t.square { 0 } else { ((r.bottom - r.top) / 3).max(4) });
        text(dc, font, label, r, ink, DT_CENTER);
    }
}

/// Draws one line of text centred in `r` with the requested horizontal alignment.
pub fn align(flags: u32) -> u32 {
    flags & (DT_LEFT | DT_CENTER | DT_RIGHT)
}
