//! Real blur behind the bar.
//!
//! The bar paints its own background with GDI, and GDI cannot leave per-pixel-transparent holes, so Windows' own
//! acrylic (SetWindowCompositionAttribute) can never show through it. What this module does instead is the honest
//! version of "a blurred bar": capture the screen under the bar, blur it, and paint that as the bar's background with a
//! translucent tint on top. The capture is refreshed a few times per second, so a window or a video moving behind the
//! bar shows up blurred.
use std::{
    ffi::c_void,
    sync::Mutex,
};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::{
        AC_SRC_OVER, AlphaBlend, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, BitBlt, CreateCompatibleDC,
        CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, ReleaseDC, SRCCOPY, SelectObject,
        StretchBlt, StretchDIBits,
    },
    System::SystemInformation::GetTickCount64,
    UI::WindowsAndMessaging::{GetSystemMetrics, GetWindowRect},
};

/// The setting lives in `%ProgramData%\WinRust\look.txt` (`bar-blur`), shared with the settings page.
pub fn on() -> bool {
    winrust_common::look::bar_blur()
}

pub fn set(on: bool) {
    winrust_common::look::bar_blur_set(on);
}

struct Cache {
    at: u64,
    bar: isize, // the bar window it was captured for: one bar per monitor, and each monitor shows other pixels
    w: i32,     // the size it is painted at
    h: i32,
    x: i32,
    y: i32,
    sw: i32, // the size it was captured and blurred at (half of w/h: 4x fewer pixels and 4x less memory)
    sh: i32,
    px: Vec<u32>,
}

/// One capture per bar: a single slot meant that painting monitor 1 threw away monitor 2's capture, so with two bars
/// every paint recaptured and reblurred (the 3 % of one core the bar used to show while idle).
static CACHE: Mutex<Vec<Cache>> = Mutex::new(Vec::new());

/// How long a capture is reused: faster than the bar repaints anyway (the clock ticks once a second).
const TTL_MS: u64 = 150;

unsafe fn capture(bar: HWND, w: i32, h: i32) -> Option<(i32, i32, i32, i32, Vec<u32>)> {
    let mut r: RECT = std::mem::zeroed();
    if GetWindowRect(bar, &mut r) == 0 || w <= 0 || h <= 0 {
        return None;
    }
    // A window can always capture itself, so the strip is taken from just outside the bar: above a bottom bar, below a
    // top one, beside a side one. The background continues vertically/horizontally there, so blurring that strip and
    // painting it under the bar gives the same colours without ever seeing our own pixels.
    // The reference is the bar's own monitor (one bar per monitor, each at a different screen position).
    let mon = winrust_common::monitors::of(bar).rc;
    let (vx, vy) = (GetSystemMetrics(76), GetSystemMetrics(77)); // SM_X/YVIRTUALSCREEN
    let (vw, vh) = (GetSystemMetrics(78), GetSystemMetrics(79)); // SM_CX/CYVIRTUALSCREEN
    let (mut sx, mut sy) = (r.left, r.top);
    if r.bottom >= mon.bottom {
        sy = r.top - h;
    } else if r.top <= mon.top {
        sy = r.bottom;
    } else if r.right >= mon.right {
        sx = r.left - w;
    } else if r.left <= mon.left {
        sx = r.right;
    } else {
        sy = r.top - h;
    }
    sx = sx.clamp(vx, (vx + vw - w).max(vx));
    sy = sy.clamp(vy, (vy + vh - h).max(vy));
    // Half resolution: a blur throws the detail away anyway, and this quarters both the pixels to process and the memory
    // the capture keeps. StretchBlt does the downscale (and the paint does the upscale).
    let (sw, sh) = ((w / 2).max(1), (h / 2).max(1));
    let sdc = GetDC(std::ptr::null_mut());
    if sdc.is_null() {
        return None;
    }
    let mdc = CreateCompatibleDC(sdc);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = sw;
    bi.bmiHeader.biHeight = -sh; // top-down
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    let mut out = None;
    if !dib.is_null() && !bits.is_null() {
        let old = SelectObject(mdc, dib);
        StretchBlt(mdc, 0, 0, sw, sh, sdc, sx, sy, w, h, SRCCOPY);
        SelectObject(mdc, old);
        let px = std::slice::from_raw_parts(bits as *const u32, (sw * sh) as usize).to_vec();
        out = Some((sx, sy, sw, sh, px));
        DeleteObject(dib);
    }
    DeleteDC(mdc);
    ReleaseDC(std::ptr::null_mut(), sdc);
    out
}


/// Sliding-window box blur, horizontally then vertically, three times: a cheap Gaussian.
fn blur(px: &mut [u32], w: usize, h: usize, r: usize) {
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

/// A 1x1 solid-colour source for `AlphaBlend`, so the tint carries the bar's opacity.
unsafe fn tint(dc: windows_sys::Win32::Graphics::Gdi::HDC, r: RECT, color: u32, alpha: u8) {
    let mdc = CreateCompatibleDC(dc);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = 1;
    bi.bmiHeader.biHeight = -1;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if !dib.is_null() && !bits.is_null() {
        // DIB memory is B, G, R, A
        let px = bits as *mut u32;
        *px = (color & 255) << 16 | (color & 0xFF00) | (color >> 16 & 255);
        let old = SelectObject(mdc, dib);
        let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: 0 };
        AlphaBlend(dc, r.left, r.top, r.right - r.left, r.bottom - r.top, mdc, 0, 0, 1, 1, bf);
        SelectObject(mdc, old);
        DeleteObject(dib);
    }
    DeleteDC(mdc);
}

/// Paints the bar's background in `dc`: the blurred screen behind it, then `color` at `opacity` % on top.
/// Returns false when the capture failed, so the caller can fall back to the plain background.
pub unsafe fn paint(bar: HWND, dc: windows_sys::Win32::Graphics::Gdi::HDC, w: i32, h: i32, color: u32, opacity: u32) -> bool {
    let now = GetTickCount64();
    let mut guard = match CACHE.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let key = bar as isize;
    let (sw, sh) = ((w / 2).max(1), (h / 2).max(1));
    let i = guard.iter().position(|c| c.bar == key && c.w == w && c.h == h);
    let stale = i.is_none_or(|i| now.saturating_sub(guard[i].at) > TTL_MS);
    let mut fresh = false;
    if stale {
        if let Some((x, y, cw, ch, mut px)) = capture(bar, w, h) {
            let radius = (winrust_common::look::bar_radius().max(1) as f32 * cw as f32 / w.max(1) as f32).round() as usize;
            blur(&mut px, cw as usize, ch as usize, radius.max(1));
            let c = Cache { at: now, bar: key, w, h, x, y, sw: cw, sh: ch, px };
            match i {
                Some(i) => guard[i] = c,
                None => guard.push(c),
            }
            if guard.len() > 4 {
                let oldest = guard.iter().enumerate().min_by_key(|(_, c)| c.at).map_or(0, |(i, _)| i);
                guard.remove(oldest);
            }
            fresh = true;
        }
    }
    let Some(c) = guard.iter().find(|c| c.bar == key && c.w == w && c.h == h) else {
        return false;
    };
    if !fresh && c.sw != sw && c.sh != sh {
        return false;
    }
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = c.sw;
    bi.bmiHeader.biHeight = -c.sh;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    // the half-resolution capture is stretched back to the bar's size
    StretchDIBits(dc, 0, 0, w, h, 0, 0, c.sw, c.sh, c.px.as_ptr() as *const c_void, &bi, DIB_RGB_COLORS, SRCCOPY);
    // The tint: how much of the bar's own colour sits over the blur. The slider still means "how solid the bar looks",
    // but it is taken at 80 % so the blur is actually visible at the default 85 %.
    let a = (255 * opacity.min(100) / 100 * 4 / 5) as u8;
    tint(dc, RECT { left: 0, top: 0, right: w, bottom: h }, color, a);
    true
}

/// The screen behind `rect`, blurred. The flyouts are fully transparent when this runs, so this is what is behind them.
pub unsafe fn capture_rect(rect: RECT, radius: i32) -> Vec<u32> {
    let (w, h) = ((rect.right - rect.left).max(1), (rect.bottom - rect.top).max(1));
    let sdc = GetDC(std::ptr::null_mut());
    if sdc.is_null() {
        return Vec::new();
    }
    let mdc = CreateCompatibleDC(sdc);
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut c_void = std::ptr::null_mut();
    let dib = CreateDIBSection(mdc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    let mut px = Vec::new();
    if !dib.is_null() && !bits.is_null() {
        let old = SelectObject(mdc, dib);
        BitBlt(mdc, 0, 0, w, h, sdc, rect.left, rect.top, SRCCOPY);
        SelectObject(mdc, old);
        px = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize).to_vec();
        blur(&mut px, w as usize, h as usize, radius.max(1) as usize);
        DeleteObject(dib);
    }
    DeleteDC(mdc);
    ReleaseDC(std::ptr::null_mut(), sdc);
    px
}

/// Draws a capture at `r` and the bar's colour over it at `alpha` (0..255): the same frost as the bar.
pub unsafe fn paint_capture(dc: windows_sys::Win32::Graphics::Gdi::HDC, px: &[u32], r: RECT, color: u32, alpha: u8) {
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if px.is_empty() || w <= 0 || h <= 0 {
        return;
    }
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    StretchDIBits(dc, r.left, r.top, w, h, 0, 0, w, h, px.as_ptr() as *const c_void, &bi, DIB_RGB_COLORS, SRCCOPY);
    tint(dc, r, color, alpha);
}