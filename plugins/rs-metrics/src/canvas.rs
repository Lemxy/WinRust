//! A small software painter over a 32-bit top-down DIB, and the `UpdateLayeredWindow` surface the widget is shown with.
//!
//! GDI cannot write per-pixel alpha into a DIB (it leaves the alpha byte alone), so the card, bars and rings are drawn
//! here by hand over premultiplied pixels, and the text is drawn by GDI into a second DIB used as a coverage mask:
//! white-on-black antialiased glyphs give the coverage, which is composited with the real colour and alpha. That is how
//! the widget stays translucent with crisp text without GDI+ or a visible helper window.
#![allow(unsafe_op_in_unsafe_fn)]
use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT, SIZE},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow},
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Blend the source colour (COLORREF) at `alpha` with coverage `c` over a premultiplied BGRA pixel.
#[inline]
fn over(p: &mut u32, color: u32, alpha: u32, c: u32) {
    if c == 0 || alpha == 0 {
        return;
    }
    let sa = alpha * c / 255;
    if sa == 0 {
        return;
    }
    let (sr, sg, sb) = (((color & 255) * sa / 255) as u32, (((color >> 8) & 255) * sa / 255) as u32, (((color >> 16) & 255) * sa / 255) as u32);
    let d = *p;
    let inv = 255 - sa;
    let r = sr + (((d >> 16) & 255) * inv + 127) / 255;
    let g = sg + (((d >> 8) & 255) * inv + 127) / 255;
    let b = sb + ((d & 255) * inv + 127) / 255;
    let a = sa + ((d >> 24) * inv + 127) / 255;
    *p = (a.min(255) << 24) | (r.min(255) << 16) | (g.min(255) << 8) | b.min(255);
}

#[inline]
fn coverage(sd: f32) -> u32 {
    ((0.5 - sd).clamp(0.0, 1.0) * 255.0 + 0.5) as u32
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
    Center,
}

pub struct Canvas {
    pub w: i32,
    pub h: i32,
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    px: *mut u32,
    mask_dc: HDC,
    mask_bmp: HBITMAP,
    mask_old: HGDIOBJ,
    mask: *mut u32,
}

impl Canvas {
    pub fn new(w: i32, h: i32) -> Option<Canvas> {
        let (w, h) = (w.max(1), h.max(1));
        unsafe {
            let screen = GetDC(null_mut());
            let dc = CreateCompatibleDC(screen);
            let mask_dc = CreateCompatibleDC(screen);
            ReleaseDC(null_mut(), screen);
            if dc.is_null() || mask_dc.is_null() {
                if !dc.is_null() {
                    DeleteDC(dc);
                }
                if !mask_dc.is_null() {
                    DeleteDC(mask_dc);
                }
                return None;
            }
            let make = |d: HDC| -> (HBITMAP, HGDIOBJ, *mut u32) {
                let bi = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h, // top-down: row 0 is the top
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB,
                        ..std::mem::zeroed()
                    },
                    ..std::mem::zeroed()
                };
                let mut bits: *mut std::ffi::c_void = null_mut();
                let bmp = CreateDIBSection(d, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
                if bmp.is_null() || bits.is_null() {
                    return (null_mut(), null_mut(), null_mut());
                }
                let old = SelectObject(d, bmp as HGDIOBJ);
                (bmp, old, bits as *mut u32)
            };
            let (bmp, old, px) = make(dc);
            let (mask_bmp, mask_old, mask) = make(mask_dc);
            if bmp.is_null() || mask_bmp.is_null() {
                if !bmp.is_null() {
                    DeleteObject(bmp as HGDIOBJ);
                }
                if !mask_bmp.is_null() {
                    DeleteObject(mask_bmp as HGDIOBJ);
                }
                DeleteDC(dc);
                DeleteDC(mask_dc);
                return None;
            }
            let mut c = Canvas { w, h, dc, bmp, old, px, mask_dc, mask_bmp, mask_old, mask };
            c.clear();
            Some(c)
        }
    }

    pub fn clear(&mut self) {
        unsafe { std::ptr::write_bytes(self.px, 0, self.w as usize * self.h as usize) };
        unsafe { std::ptr::write_bytes(self.mask, 0, self.w as usize * self.h as usize) };
    }

    #[inline]
    fn at(&mut self, x: i32, y: i32) -> Option<&mut u32> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        unsafe { Some(&mut *self.px.add(y as usize * self.w as usize + x as usize)) }
    }

    /// A rounded rectangle, 1 px of antialiasing on its edge.
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32, alpha: u32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (hw, hh) = ((w / 2.0).max(0.0), (h / 2.0).max(0.0));
        let r = radius.min(hw).min(hh).max(0.0);
        let (ix, iy) = ((hw - r).max(0.0), (hh - r).max(0.0));
        let x0 = (x.floor() as i32).max(0);
        let y0 = (y.floor() as i32).max(0);
        let x1 = ((x + w).ceil() as i32).min(self.w);
        let y1 = ((y + h).ceil() as i32).min(self.h);
        for py in y0..y1 {
            for pxi in x0..x1 {
                let dx = (pxi as f32 + 0.5 - cx).abs() - ix;
                let dy = (py as f32 + 0.5 - cy).abs() - iy;
                let sd = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r;
                let c = coverage(sd);
                if c > 0 {
                    if let Some(p) = self.at(pxi, py) {
                        over(p, color, alpha, c);
                    }
                }
            }
        }
    }

    /// A rounded rectangle whose per-pixel colour comes from `color(px, py)` (the frosted fill).
    pub fn round_rect_with<F: FnMut(i32, i32) -> u32>(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, alpha: u32, mut color: F) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let (hw, hh) = ((w / 2.0).max(0.0), (h / 2.0).max(0.0));
        let r = radius.min(hw).min(hh).max(0.0);
        let (ix, iy) = ((hw - r).max(0.0), (hh - r).max(0.0));
        let x0 = (x.floor() as i32).max(0);
        let y0 = (y.floor() as i32).max(0);
        let x1 = ((x + w).ceil() as i32).min(self.w);
        let y1 = ((y + h).ceil() as i32).min(self.h);
        for py in y0..y1 {
            for pxi in x0..x1 {
                let dx = (pxi as f32 + 0.5 - cx).abs() - ix;
                let dy = (py as f32 + 0.5 - cy).abs() - iy;
                let sd = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r;
                let c = coverage(sd);
                if c > 0 {
                    let col = color(pxi, py);
                    if let Some(p) = self.at(pxi, py) {
                        over(p, col, alpha, c);
                    }
                }
            }
        }
    }

    /// A progress ring. `frac` of the circumference is drawn in `color`, the rest in `track`.
    pub fn ring(&mut self, cx: f32, cy: f32, radius: f32, thick: f32, frac: f32, color: u32, track: u32) {
        let frac = frac.clamp(0.0, 1.0);
        let r0 = radius + thick / 2.0 + 1.0;
        let x0 = (cx - r0).floor() as i32;
        let y0 = (cy - r0).floor() as i32;
        let x1 = (cx + r0).ceil() as i32;
        let y1 = (cy + r0).ceil() as i32;
        let half = thick / 2.0;
        for py in y0..y1 {
            for pxi in x0..x1 {
                let dx = pxi as f32 + 0.5 - cx;
                let dy = py as f32 + 0.5 - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let sd = (d - radius).abs() - half;
                let c = coverage(sd);
                if c == 0 {
                    continue;
                }
                let a = (dy.atan2(dx) / std::f32::consts::TAU + 0.25 + 1.0) % 1.0;
                let on = a <= frac;
                if let Some(p) = self.at(pxi, py) {
                    // a faint full circle keeps a 0 % ring readable
                    over(p, track, 26, c);
                    if on {
                        over(p, color, 255, c);
                    }
                }
            }
        }
    }

    /// A tiny history line with a soft fill under it.
    pub fn spark(&mut self, x: f32, y: f32, w: f32, h: f32, vals: &[f32], max: f32, color: u32) {
        if vals.len() < 2 || w < 2.0 || h < 2.0 {
            return;
        }
        let max = max.max(1e-6);
        let n = vals.len();
        let (x0, y0) = (x.round() as i32, y.round() as i32);
        let (wi, hi) = (w.round() as i32, h.round() as i32);
        for i in 0..wi.max(1) {
            let t = i as f32 / (wi.max(2) - 1) as f32;
            let pos = t * (n - 1) as f32;
            let (a, b) = (pos.floor() as usize, (pos.floor() as usize + 1).min(n - 1));
            let f = pos - pos.floor();
            let v = (vals[a] * (1.0 - f) + vals[b] * f).clamp(0.0, max);
            let yy = y + h - v / max * h;
            // keep the line inside the box when the value jumps
            let yy = yy.clamp(y, y + h);
            let pxi = x0 + i;
            // area: 28 alpha, fading in at its top edge
            for py in (yy.round() as i32).max(y0)..(y0 + hi).min(self.h) {
                let c = ((py as f32 + 0.5 - yy) * 255.0).clamp(0.0, 255.0) as u32;
                if c > 0 {
                    if let Some(p) = self.at(pxi, py) {
                        over(p, color, 30, c);
                    }
                }
            }
            // line: 1.7 px, antialiased
            let py0 = (yy.floor() as i32 - 2).max(y0);
            let py1 = (yy.ceil() as i32 + 2).min(y0 + hi);
            for py in py0..py1 {
                let d = (py as f32 + 0.5 - yy).abs();
                let c = ((1.1 - d).clamp(0.0, 1.0) * 255.0) as u32;
                if c > 0 {
                    if let Some(p) = self.at(pxi, py) {
                        over(p, color, 235, c);
                    }
                }
            }
        }
    }

    /// Text through GDI into the mask, composited with `color` at `alpha`. `rect` is (left, top, width, height).
    pub fn text(&mut self, rect: (i32, i32, i32, i32), s: &str, font: HFONT, color: u32, alpha: u32, align: Align) {        if s.is_empty() {
            return;
        }
        let (x, y, w, h) = rect;
        if w <= 1 || h <= 1 {
            return;
        }
        unsafe {
            let old = SelectObject(self.mask_dc, font as HGDIOBJ);
            SetBkMode(self.mask_dc, TRANSPARENT as i32);
            SetTextColor(self.mask_dc, 0x00FF_FFFF);
            let ws: Vec<u16> = s.encode_utf16().collect();
            let (sx, ex) = (x.max(0), (x + w).min(self.w));
            let (sy, ey) = (y.max(0), (y + h).min(self.h));
            if sx >= ex || sy >= ey {
                SelectObject(self.mask_dc, old);
                return;
            }
            // clear the band and let GDI rasterize the glyphs there
            for py in sy..ey {
                std::ptr::write_bytes(self.mask.add(py as usize * self.w as usize + sx as usize), 0, (ex - sx) as usize);
            }
            let mut r = RECT { left: x, top: y, right: x + w, bottom: y + h };
            let mut flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
            flags |= match align {
                Align::Left => DT_LEFT,
                Align::Right => DT_RIGHT,
                Align::Center => DT_CENTER,
            };
            DrawTextW(self.mask_dc, ws.as_ptr(), ws.len() as i32, &mut r, flags);
            SelectObject(self.mask_dc, old);
            // composite what was drawn
            for py in sy..ey {
                for pxi in sx..ex {
                    let m = *self.mask.add(py as usize * self.w as usize + pxi as usize) & 255;
                    if m == 0 {
                        continue;
                    }
                    if let Some(p) = self.at(pxi, py) {
                        over(p, color, alpha, m);
                    }
                }
            }
        }
    }

    /// A copy of the pixels, for the cached card background.
    pub fn snapshot(&self) -> Vec<u32> {
        unsafe { std::slice::from_raw_parts(self.px, self.w as usize * self.h as usize).to_vec() }
    }

    /// Puts a snapshot back (same size), before the rows are drawn over it.
    pub fn restore(&mut self, src: &[u32]) {
        let n = self.w as usize * self.h as usize;
        if src.len() == n {
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), self.px, n) };
        }
    }

    /// The pixel width `s` takes with `font` (the mask DC is otherwise idle).
    pub fn measure(&self, font: HFONT, s: &str) -> i32 {
        unsafe {
            let old = SelectObject(self.mask_dc, font as HGDIOBJ);
            let ws: Vec<u16> = s.encode_utf16().collect();
            let mut sz: SIZE = std::mem::zeroed();
            GetTextExtentPoint32W(self.mask_dc, ws.as_ptr(), ws.len() as i32, &mut sz);
            SelectObject(self.mask_dc, old);
            sz.cx
        }
    }

    /// Hands the pixels to the window. `at` is the top-left corner in screen coordinates.
    pub fn present(&self, hwnd: HWND, at: (i32, i32)) {        unsafe {
            let screen = GetDC(null_mut());
            let dst = POINT { x: at.0, y: at.1 };
            let size = SIZE { cx: self.w, cy: self.h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            UpdateLayeredWindow(hwnd, screen, &dst, &size, self.dc, &src, 0, &blend, ULW_ALPHA);
            ReleaseDC(null_mut(), screen);
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            SelectObject(self.mask_dc, self.mask_old);
            DeleteObject(self.bmp as HGDIOBJ);
            DeleteObject(self.mask_bmp as HGDIOBJ);
            DeleteDC(self.dc);
            DeleteDC(self.mask_dc);
        }
    }
}

/// The fonts the widget paints with, recreated when the style or the DPI changes.
pub struct Fonts {
    pub label: HFONT, // module name
    pub value: HFONT, // the big number
    pub small: HFONT, // units and the secondary line
    pub mini: HFONT,  // the number inside a ring
}

fn font(face: &str, px: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -px.max(6),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_TT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide(face).as_ptr(),
        ) as HFONT
    }
}

impl Fonts {
    pub fn new(face: &str, scale: f32) -> Fonts {
        let px = |v: f32| (v * scale).round() as i32;
        Fonts {
            label: font(face, px(12.0), 400),
            value: font(face, px(19.0), 600),
            small: font(face, px(10.0), 400),
            mini: font(face, px(12.0), 600),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for f in [self.label, self.value, self.small, self.mini] {
            if !f.is_null() {
                unsafe { DeleteObject(f as HGDIOBJ) };
            }
        }
    }
}

