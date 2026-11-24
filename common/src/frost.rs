//! Frost: a real blur behind a window.
//!
//! Two sources, because a window that is already on screen cannot capture itself:
//! - `capture_rect`: what is on screen in `rect`. Use it for a surface that is still invisible (a popup, a menu, a
//!   window before its first `ShowWindow`) — that is what the bar and the menus do.
//! - `capture_desktop`: the wallpaper under `rect`, painted by Windows itself (`PaintDesktop`). Use it for windows that
//!   are already visible: it is the same recipe as Windows 11's Mica, and it never catches the window itself.
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::*,
};

pub use crate::look::{menu_alpha, menu_blur, menu_radius};

/// Blur radius in pixels for a surface of this size: bigger surfaces need a much wider radius or they look like plain glass.
pub fn radius_for(w: i32, h: i32) -> i32 {
    (menu_radius()).max(h.min(w) / 10).max(16)
}

/// The tint's alpha (0..255) for the panels: the setting, with a readability floor.
pub fn tint_alpha() -> u8 {
    // much lighter than a menu's tint: a window is read by its own cards and text, and at 60 % over a dark wallpaper
    // the frost is indistinguishable from a solid background
    (255 * menu_alpha().max(45).min(55) as u32 / 100) as u8
}

/// Sliding-window box blur, horizontally then vertically, three times: a cheap Gaussian.
pub fn blur(px: &mut [u32], w: usize, h: usize, r: usize) {
    if w == 0 || h == 0 || r == 0 {
        return;
    }
    let n = (2 * r + 1) as u32;
    let (wm, hm) = (w - 1, h - 1);
    let mut line = vec![0u32; w.max(h)];
    for _ in 0..3 {
        for y in 0..h {
            let base = y * w;
            let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
            for i in 0..=r {
                let p = px[base + i.min(wm)];
                sr += p & 255;
                sg += (p >> 8) & 255;
                sb += (p >> 16) & 255;
            }
            let left = px[base];
            sr += (left & 255) * r as u32;
            sg += ((left >> 8) & 255) * r as u32;
            sb += ((left >> 16) & 255) * r as u32;
            for x in 0..w {
                line[x] = (sr / n) | (sg / n) << 8 | (sb / n) << 16;
                let out = px[base + x.saturating_sub(r)];
                let inp = px[base + (x + r + 1).min(wm)];
                sr = sr - (out & 255) + (inp & 255);
                sg = sg - ((out >> 8) & 255) + ((inp >> 8) & 255);
                sb = sb - ((out >> 16) & 255) + ((inp >> 16) & 255);
            }
            px[base..base + w].copy_from_slice(&line[..w]);
        }
        for x in 0..w {
            let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
            for i in 0..=r {
                let p = px[i.min(hm) * w + x];
                sr += p & 255;
                sg += (p >> 8) & 255;
                sb += (p >> 16) & 255;
            }
            let top = px[x];
            sr += (top & 255) * r as u32;
            sg += ((top >> 8) & 255) * r as u32;
            sb += ((top >> 16) & 255) * r as u32;
            for y in 0..h {
                line[y] = (sr / n) | (sg / n) << 8 | (sb / n) << 16;
                let out = px[y.saturating_sub(r) * w + x];
                let inp = px[(y + r + 1).min(hm) * w + x];
                sr = sr - (out & 255) + (inp & 255);
                sg = sg - ((out >> 8) & 255) + ((inp >> 8) & 255);
                sb = sb - ((out >> 16) & 255) + ((inp >> 16) & 255);
            }
            for y in 0..h {
                px[y * w + x] = line[y];
            }
        }
    }
}

unsafe fn dib(w: i32, h: i32) -> (HDC, *mut u32, HBITMAP) {
    let sdc = GetDC(std::ptr::null_mut());
    let mdc = CreateCompatibleDC(sdc);
    ReleaseDC(std::ptr::null_mut(), sdc);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let hb = CreateDIBSection(mdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    SelectObject(mdc, hb);
    (mdc, bits as *mut u32, hb)
}

/// What is on screen inside `rect`, blurred. Call it while the surface is not visible yet.
pub unsafe fn capture_rect(rect: RECT, radius: i32) -> Vec<u32> {
    let (w, h) = ((rect.right - rect.left).max(1), (rect.bottom - rect.top).max(1));
    let (mdc, bits, hb) = dib(w, h);
    if hb.is_null() || bits.is_null() {
        DeleteDC(mdc);
        return Vec::new();
    }
    let sdc = GetDC(std::ptr::null_mut());
    BitBlt(mdc, 0, 0, w, h, sdc, rect.left, rect.top, SRCCOPY);
    ReleaseDC(std::ptr::null_mut(), sdc);
    let mut px = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize).to_vec();
    blur(&mut px, w as usize, h as usize, radius.max(1) as usize);
    DeleteObject(hb);
    DeleteDC(mdc);
    px
}

/// The wallpaper under `rect`, blurred: the Mica recipe. Works for windows that are already on screen.
pub unsafe fn capture_desktop(rect: RECT, radius: i32) -> Vec<u32> {
    if !crate::look::win_blur() {
        return Vec::new(); // the apps fall back to their solid background
    }
    let (w, h) = ((rect.right - rect.left).max(1), (rect.bottom - rect.top).max(1));
    let (mdc, bits, hb) = dib(w, h);
    if hb.is_null() || bits.is_null() {
        DeleteDC(mdc);
        return Vec::new();
    }
    // PaintDesktop draws the wallpaper at desktop coordinates, so shift the origin to the window's corner
    let keep = SaveDC(mdc);
    SetViewportOrgEx(mdc, -rect.left, -rect.top, std::ptr::null_mut());
    if PaintDesktop(mdc) == 0 {
        // no wallpaper (a solid colour): fall back to the screen
        let sdc = GetDC(std::ptr::null_mut());
        BitBlt(mdc, rect.left, rect.top, w, h, sdc, rect.left, rect.top, SRCCOPY);
        ReleaseDC(std::ptr::null_mut(), sdc);
    }
    RestoreDC(mdc, keep);
    let mut px = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize).to_vec();
    blur(&mut px, w as usize, h as usize, radius.max(1) as usize);
    DeleteObject(hb);
    DeleteDC(mdc);
    px
}

/// Paints a capture into `r` and the theme colour over it at `alpha`.
pub unsafe fn paint_capture(dc: HDC, px: &[u32], r: RECT, color: u32, alpha: u8) -> bool {
    // a light theme would wash the frost out: keep the tint dark enough to read as glass
    let lum = ((color & 255) + ((color >> 8) & 255) + ((color >> 16) & 255)) / 3;
    let color = if lum > 96 { 0x202020 } else { color };
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if px.is_empty() || w <= 0 || h <= 0 {
        return false;
    }
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    StretchDIBits(dc, r.left, r.top, w, h, 0, 0, w, h, px.as_ptr() as *const std::ffi::c_void, &bi, DIB_RGB_COLORS, SRCCOPY);
    // the tint: a 1x1 solid source blended with the alpha
    let mdc = CreateCompatibleDC(dc);
    let mut bi2: BITMAPINFO = std::mem::zeroed();
    bi2.bmiHeader = bi.bmiHeader;
    bi2.bmiHeader.biWidth = 1;
    bi2.bmiHeader.biHeight = -1;
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let hb = CreateDIBSection(mdc, &bi2, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if !hb.is_null() && !bits.is_null() {
        *(bits as *mut u32) = (color & 255) << 16 | (color & 0xFF00) | (color >> 16 & 255);
        let old = SelectObject(mdc, hb);
        let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: 0 };
        AlphaBlend(dc, r.left, r.top, w, h, mdc, 0, 0, 1, 1, bf);
        SelectObject(mdc, old);
        DeleteObject(hb);
    }
    DeleteDC(mdc);
    true
}
