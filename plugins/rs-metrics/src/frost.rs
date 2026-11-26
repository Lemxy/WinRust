//! The frosted pill background: the desktop wallpaper behind the widget, downscaled and box-blurred once, sampled for
//! each pill. `%LOCALAPPDATA%\rs-shell\wallpaper.bmp` is rs-shell's own copy of the wallpaper (11 MB, 24 bpp), so it is
//! decoded once, reduced to ~480 px wide and blurred by hand; sampling a pill then costs one read per pixel.
//!
//! It is a still picture: when a Wallpaper Engine video is running, the background really moves and the frost cannot
//! follow it (a real capture every frame would cost far more than this widget is allowed to spend).
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    path::PathBuf,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::SystemTime,
};

use windows_sys::Win32::System::SystemInformation::GetTickCount64;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

struct Wall {
    w: usize,
    h: usize,
    /// Blurred, downscaled RGB (u32 0x00RRGGBB).
    px: Vec<u32>,
    stamp: Option<(u64, u64)>,
}

static WALL: OnceLock<Mutex<Option<Wall>>> = OnceLock::new();
/// Bumped on every reload so the windows know their cached backgrounds are stale.
static GEN: AtomicU64 = AtomicU64::new(1);

fn wall() -> &'static Mutex<Option<Wall>> {
    WALL.get_or_init(|| Mutex::new(None))
}

fn path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|b| PathBuf::from(b).join("rs-shell").join("wallpaper.bmp"))
}

fn stamp(p: &PathBuf) -> Option<(u64, u64)> {
    let m = std::fs::metadata(p).ok()?;
    let t = m.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64);
    Some((t, m.len()))
}

pub fn generation() -> u64 {
    GEN.load(Relaxed)
}

/// Loads (or reloads) the wallpaper when its timestamp changed. Call once every few seconds from the widget's timer.
pub fn ensure() {
    static AT: AtomicU64 = AtomicU64::new(0);
    let t = unsafe { GetTickCount64() };
    if t.saturating_sub(AT.load(Relaxed)) < 5000 {
        return;
    }
    AT.store(t, Relaxed);
    let Some(p) = path() else { return };
    let now = stamp(&p);
    if now.is_none() {
        return;
    }
    {
        let w = wall().lock().unwrap_or_else(|e| e.into_inner());
        if w.as_ref().is_some_and(|w| w.stamp == now) {
            return;
        }
    }
    if let Some(w) = decode(&p, now) {
        let mut slot = wall().lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some(w);
        GEN.fetch_add(1, Relaxed);
    }
}

/// A 24/32-bpp BMP read by hand (no GDI+), reduced to <= 480 px wide and blurred twice.
fn decode(p: &PathBuf, stamp: Option<(u64, u64)>) -> Option<Wall> {
    let b = std::fs::read(p).ok()?;
    if b.len() < 54 || &b[0..2] != b"BM" {
        return None;
    }
    let off = u32::from_le_bytes(b[10..14].try_into().ok()?) as usize;
    let hdr = u32::from_le_bytes(b[14..18].try_into().ok()?) as usize;
    if hdr < 40 {
        return None;
    }
    let w = i32::from_le_bytes(b[18..22].try_into().ok()?);
    let h = i32::from_le_bytes(b[22..26].try_into().ok()?);
    let bpp = u16::from_le_bytes(b[28..30].try_into().ok()?);
    let comp = u32::from_le_bytes(b[30..34].try_into().ok()?);
    if w <= 0 || h == 0 || !(bpp == 24 || bpp == 32) || comp != 0 {
        return None;
    }
    let (sw, sh) = (w as usize, h.unsigned_abs() as usize);
    let stride = (sw * (bpp as usize / 8) + 3) & !3;
    if off + stride * sh > b.len() {
        return None;
    }
    let step = (sw / 480).max(1);
    let (dw, dh) = (sw.div_ceil(step), sh.div_ceil(step));
    let mut px = vec![0u32; dw * dh];
    let bottom_up = h > 0;
    for y in 0..dh {
        let sy = (y * step).min(sh - 1);
        let row = off + (if bottom_up { sh - 1 - sy } else { sy }) * stride;
        for x in 0..dw {
            let sx = (x * step).min(sw - 1);
            let i = row + sx * (bpp as usize / 8);
            px[y * dw + x] = (b[i + 2] as u32) << 16 | (b[i + 1] as u32) << 8 | b[i] as u32;
        }
    }
    // two box-blur passes (radius 2): a smooth, cheap frost
    for _ in 0..2 {
        px = blur(&px, dw, dh, 2);
    }
    Some(Wall { w: dw, h: dh, px, stamp })
}

fn blur(src: &[u32], w: usize, h: usize, r: isize) -> Vec<u32> {
    let mut tmp = vec![0u32; w * h];
    let mut out = vec![0u32; w * h];
    let ch = |c: u32, s: u32| (c >> s) & 255;
    for y in 0..h {
        let row = y * w;
        let (mut sr, mut sg, mut sb, mut n) = (0u32, 0u32, 0u32, 0u32);
        for x in -r..=r {
            let xx = x.clamp(0, w as isize - 1) as usize;
            let c = src[row + xx];
            sr += ch(c, 16);
            sg += ch(c, 8);
            sb += ch(c, 0);
            n += 1;
        }
        for x in 0..w {
            tmp[row + x] = (sr / n) << 16 | (sg / n) << 8 | sb / n;
            let add = src[row + ((x as isize + r).clamp(0, w as isize - 1) as usize)];
            let sub = src[row + ((x as isize - r).clamp(0, w as isize - 1) as usize)];
            sr = sr + ch(add, 16) - ch(sub, 16);
            sg = sg + ch(add, 8) - ch(sub, 8);
            sb = sb + ch(add, 0) - ch(sub, 0);
        }
    }
    for x in 0..w {
        let (mut sr, mut sg, mut sb, mut n) = (0u32, 0u32, 0u32, 0u32);
        for y in -r..=r {
            let yy = y.clamp(0, h as isize - 1) as usize;
            let c = tmp[yy * w + x];
            sr += ch(c, 16);
            sg += ch(c, 8);
            sb += ch(c, 0);
            n += 1;
        }
        for y in 0..h {
            out[y * w + x] = (sr / n) << 16 | (sg / n) << 8 | sb / n;
            let add = tmp[((y as isize + r).clamp(0, h as isize - 1) as usize) * w + x];
            let sub = tmp[((y as isize - r).clamp(0, h as isize - 1) as usize) * w + x];
            sr = sr + ch(add, 16) - ch(sub, 16);
            sg = sg + ch(add, 8) - ch(sub, 8);
            sb = sb + ch(add, 0) - ch(sub, 0);
        }
    }
    out
}

/// The blurred wallpaper colour under a screen point (None while it is not loaded).
pub fn sample(x: i32, y: i32) -> Option<(u8, u8, u8)> {
    let w = wall().lock().ok()?;
    let w = w.as_ref()?;
    // rs-shell's BMP is the primary screen's wallpaper, stretched to that monitor: map to it and clamp anywhere else
    // (a second monitor with its own picture gets the nearest edge colour, not the wrong half of the primary one)
    let (pw, ph) = unsafe { (GetSystemMetrics(SM_CXSCREEN).max(1), GetSystemMetrics(SM_CYSCREEN).max(1)) };
    let bx = (x.max(0) as i64 * w.w as i64 / pw as i64).clamp(0, w.w as i64 - 1) as usize;
    let by = (y.max(0) as i64 * w.h as i64 / ph as i64).clamp(0, w.h as i64 - 1) as usize;
    let c = w.px[by * w.w + bx];
    Some(((c >> 16) as u8, (c >> 8) as u8, c as u8))
}
