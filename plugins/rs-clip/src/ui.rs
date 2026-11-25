//! Small GDI helpers and the window geometry both popups share: DPI, fonts, fills, text, the search field,
//! and putting an entry into the foreground window with Ctrl+V.
use std::{
    ptr::null_mut,
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::{FILETIME, HWND, POINT, RECT, SYSTEMTIME},
    Graphics::Gdi::*,
    Storage::FileSystem::FileTimeToLocalFileTime,
    System::Time::FileTimeToSystemTime,
    UI::{
        HiDpi::GetDpiForMonitor,
        Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL},
        WindowsAndMessaging::{GetClientRect, GetCursorPos, GetForegroundWindow, SetForegroundWindow},
    },
};

use winrust_common::{
    frost,
    look,
    style::{mix, pal},
};

pub const TITLE: &str = "rs-clip";
pub const POPUP_W: i32 = 420;
pub const POPUP_H: i32 = 520;
pub const PAD: i32 = 12;
pub const SEARCH_H: i32 = 38;
pub const FOOTER_H: i32 = 40;
pub const ROW_H: i32 = 52;
pub const EMOJI_FACE: &str = "Segoe UI Emoji";

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

pub unsafe fn dpi(hwnd: HWND) -> i32 {
    let d = windows_sys::Win32::UI::HiDpi::GetDpiForWindow(hwnd) as i32;
    if d <= 0 {
        96
    } else {
        d
    }
}

pub unsafe fn sc(hwnd: HWND, v: i32) -> i32 {
    (v as f32 * dpi(hwnd) as f32 / 96.0).round() as i32
}

pub unsafe fn font(hwnd: HWND, px: i32, weight: i32, face: &str) -> HFONT {
    let f = wide(face);
    CreateFontW(-sc(hwnd, px), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, f.as_ptr())
}

pub unsafe fn fill(dc: HDC, r: RECT, color: u32) {
    let b = CreateSolidBrush(color);
    FillRect(dc, &r, b);
    DeleteObject(b as HGDIOBJ);
}

pub unsafe fn rounded(dc: HDC, r: RECT, color: u32, rad: i32) {
    let b = CreateSolidBrush(color);
    let old_b = SelectObject(dc, b as HGDIOBJ);
    let old_p = SelectObject(dc, GetStockObject(NULL_PEN) as HGDIOBJ);
    if rad <= 0 {
        Rectangle(dc, r.left, r.top, r.right, r.bottom);
    } else {
        RoundRect(dc, r.left, r.top, r.right, r.bottom, rad, rad);
    }
    SelectObject(dc, old_b);
    SelectObject(dc, old_p);
    DeleteObject(b as HGDIOBJ);
}

pub unsafe fn text(dc: HDC, s: &str, r: RECT, f: HFONT, color: u32, flags: u32) {
    let old = SelectObject(dc, f as HGDIOBJ);
    SetTextColor(dc, color);
    SetBkMode(dc, TRANSPARENT as i32);
    let mut rr = r;
    let w: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(dc, w.as_ptr(), w.len() as i32, &mut rr, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | flags);
    SelectObject(dc, old);
}

pub unsafe fn text_w(dc: HDC, s: &str, f: HFONT) -> i32 {
    let old = SelectObject(dc, f as HGDIOBJ);
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut size: windows_sys::Win32::Foundation::SIZE = std::mem::zeroed();
    GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
    SelectObject(dc, old);
    size.cx
}

pub unsafe fn line(dc: HDC, x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
    let p = CreatePen(PS_SOLID, 1, color);
    let old = SelectObject(dc, p as HGDIOBJ);
    MoveToEx(dc, x1, y1, null_mut());
    LineTo(dc, x2, y2);
    SelectObject(dc, old);
    DeleteObject(p as HGDIOBJ);
}

/// `14:32` in local time for a `unix ms` timestamp.
pub fn hhmm(at: u64) -> String {
    // FILETIME counts 100 ns ticks since 1601-01-01; unix ms are 10 000 ticks since 1970.
    let ticks = at.saturating_mul(10_000).saturating_add(116_444_736_000_000_000);
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut local, mut st): (FILETIME, SYSTEMTIME) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    unsafe {
        if FileTimeToLocalFileTime(&ft, &mut local) == 0 || FileTimeToSystemTime(&local, &mut st) == 0 {
            return String::new();
        }
    }
    format!("{:02}:{:02}", st.wHour, st.wMinute)
}

/// Sanitises a single display line: tabs/newlines and control characters become spaces.
pub fn one_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(400));
    for (n, c) in s.chars().enumerate() {
        if n >= 400 {
            out.push('…');
            break;
        }
        out.push(if c.is_control() { ' ' } else { c });
    }
    out
}

pub struct Metrics {
    pub title: RECT,
    pub search: RECT,
    pub body: RECT,
    pub footer: RECT,
}

pub unsafe fn metrics(hwnd: HWND) -> Metrics {
    let mut cr: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut cr);
    let (w, h) = (cr.right, cr.bottom);
    let pad = sc(hwnd, PAD);
    Metrics {
        title: RECT { left: pad, top: sc(hwnd, 8), right: w - pad, bottom: sc(hwnd, 38) },
        search: RECT { left: pad, top: sc(hwnd, 44), right: w - pad, bottom: sc(hwnd, 44 + SEARCH_H) },
        body: RECT { left: pad, top: sc(hwnd, 90), right: w - pad, bottom: h - sc(hwnd, FOOTER_H) },
        footer: RECT { left: pad, top: h - sc(hwnd, FOOTER_H), right: w - pad, bottom: h - sc(hwnd, 4) },
    }
}

pub unsafe fn title(dc: HDC, m: &Metrics, s: &str, f: HFONT) {
    text(dc, s, m.title, f, pal().text, 0);
}

/// The search box: magnifier, query or placeholder, caret.
pub unsafe fn search(dc: HDC, hwnd: HWND, r: RECT, query: &str, placeholder: &str, f: HFONT, icon: HFONT, caret_on: bool) {
    let p = pal();
    rounded(dc, r, mix(p.bg, p.text, 0.07), sc(hwnd, 10));

    let pad = sc(hwnd, 12);
    let icon_w = sc(hwnd, 20);
    let left = r.left + pad + icon_w + sc(hwnd, 8);
    text(dc, "\u{E721}", RECT { left: r.left + pad, top: r.top, right: r.left + pad + icon_w, bottom: r.bottom }, icon, p.dim, 0);
    let text_r = RECT { left, top: r.top, right: r.right - sc(hwnd, 10), bottom: r.bottom };
    if query.is_empty() {
        text(dc, placeholder, text_r, f, p.dim, DT_END_ELLIPSIS);
    } else {
        text(dc, query, text_r, f, p.text, DT_END_ELLIPSIS);
    }
    if caret_on {
        let tw = text_w(dc, query, f);
        let cx = (left + tw + sc(hwnd, 1)).min(r.right - sc(hwnd, 8));
        line(dc, cx, r.top + sc(hwnd, 10), cx, r.bottom - sc(hwnd, 10), p.accent);
    }
}

pub unsafe fn footer_sep(dc: HDC, m: &Metrics, hwnd: HWND) {
    let p = pal();
    line(dc, m.body.left, m.footer.top - sc(hwnd, 2), m.body.right, m.footer.top - sc(hwnd, 2), mix(p.bg, p.text, 0.10));
}

pub unsafe fn scrollbar(dc: HDC, hwnd: HWND, body: RECT, total: usize, visible: usize, scroll: usize) {
    if total <= visible || visible == 0 {
        return;
    }
    let p = pal();
    let w = sc(hwnd, 3);
    let x = body.right + sc(hwnd, 6);
    let track = RECT { left: x, top: body.top + sc(hwnd, 4), right: x + w, bottom: body.bottom - sc(hwnd, 4) };
    rounded(dc, track, mix(p.bg, p.text, 0.08), w);
    let th = (track.bottom - track.top) as usize;
    let thumb = (th * visible / total).max(sc(hwnd, 20) as usize);
    let max_scroll = total - visible;
    let y = track.top as usize + (th - thumb) * scroll.min(max_scroll) / max_scroll.max(1);
    rounded(dc, RECT { left: x, top: y as i32, right: x + w, bottom: y as i32 + thumb as i32 }, mix(p.bg, p.text, 0.26), w);
}

/// Fits `hb` inside `r` (letterboxed) with a half-tone downscale.
pub unsafe fn bitmap_fit(dc: HDC, hb: HBITMAP, r: RECT) {
    let mut bm: BITMAP = std::mem::zeroed();
    if GetObjectW(hb, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut _) == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
        return;
    }
    let (rw, rh) = (r.right - r.left, r.bottom - r.top);
    let scale = (rw as f32 / bm.bmWidth as f32).min(rh as f32 / bm.bmHeight as f32);
    let (dw, dh) = ((bm.bmWidth as f32 * scale) as i32, (bm.bmHeight as f32 * scale) as i32);
    let (x, y) = (r.left + (rw - dw) / 2, r.top + (rh - dh) / 2);
    let mem = CreateCompatibleDC(dc);
    let old = SelectObject(mem, hb as HGDIOBJ);
    let old_mode = SetStretchBltMode(dc, HALFTONE);
    StretchBlt(dc, x, y, dw, dh, mem, 0, 0, bm.bmWidth, bm.bmHeight, SRCCOPY);
    SetStretchBltMode(dc, old_mode);
    SelectObject(mem, old);
    DeleteDC(mem);
}

/// Physical popup rectangle on the monitor under the pointer, and that monitor's DPI.
pub unsafe fn geometry(logical_w: i32, logical_h: i32) -> (i32, i32, i32, i32) {
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    let mon = winrust_common::monitors::at(pt.x, pt.y);
    let mut dx = 96u32;
    let mut dy = 96u32;
    if GetDpiForMonitor(mon.handle, 0 /* MDT_EFFECTIVE_DPI */, &mut dx, &mut dy) != 0 {
        dx = 96;
    }
    let scale = (dx.max(72) as f32 / 96.0).clamp(0.75, 4.0);
    let w = (logical_w as f32 * scale).round() as i32;
    let h = (logical_h as f32 * scale).round() as i32;
    let (ww, wh) = (mon.work.right - mon.work.left, mon.work.bottom - mon.work.top);
    let w = w.min(ww.max(1));
    let h = h.min(wh.max(1));
    let x = mon.work.left + ((ww - w) / 2).max(0);
    let y = mon.work.top + ((wh - h) / 2).max(0);
    (x, y, w, h)
}

/// The blurred screen behind a popup that is still invisible.
pub unsafe fn frost_bits(w: i32, h: i32, x: i32, y: i32) -> Vec<u32> {
    frost::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + h }, frost::radius_for(w, h))
}

/// The pane itself: frost (or the solid background), the rounded edge and the optional border.
pub unsafe fn surface(dc: HDC, px: &[u32], r: RECT) {
    let p = pal();
    let want = ((r.right - r.left).max(0) * (r.bottom - r.top).max(0)) as usize;
    let drew = px.len() == want && want > 0 && frost::paint_capture(dc, px, r, p.bg, (255 * look::menu_alpha().max(60) as u32 / 100) as u8);
    if !drew {
        fill(dc, r, p.bg);
    }
    let edge = CreateSolidBrush(mix(p.bg, p.text, 0.12));
    FrameRect(dc, &r, edge);
    DeleteObject(edge as HGDIOBJ);
}

fn key(vk: u16, up: bool) -> INPUT {
    let mut i = INPUT::default();
    i.r#type = INPUT_KEYBOARD;
    i.Anonymous.ki = KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 };
    i
}

/// Brings `target` back and sends Ctrl+V. The clipboard must already hold the entry.
pub fn paste_into(target: HWND) {
    if target.is_null() {
        log("paste: no foreground window to paste into (the item is on the clipboard)");
        return;
    }
    unsafe {
        SetForegroundWindow(target);
    }
    // The popup was the foreground window until a moment ago; give Windows a few tries to hand it back.
    let mut back = false;
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(40));
        if unsafe { GetForegroundWindow() } == target {
            back = true;
            break;
        }
        unsafe {
            SetForegroundWindow(target);
        }
    }
    if !back {
        log("paste: the target window did not come back to the foreground (the item is on the clipboard)");
        return;
    }
    std::thread::sleep(Duration::from_millis(40));
    if unsafe { GetForegroundWindow() } != target {
        log("paste: the target window left the foreground (the item is on the clipboard)");
        return;
    }
    let keys = [key(VK_CONTROL, false), key(b'V' as u16, false), key(b'V' as u16, true), key(VK_CONTROL, true)];
    let n = unsafe { SendInput(keys.len() as u32, keys.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    if n != keys.len() as u32 {
        log("paste: SendInput was refused (an elevated window?)");
    }
}

pub fn log(msg: &str) {
    crate::log(msg);
}
