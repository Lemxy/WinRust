//! The context menu: a rounded popup with real blur behind it, icons, keyboard hints, ticks and one submenu level —
//! the shape Windows 11 uses, drawn by hand because the Win32 menu (TrackPopupMenu) cannot be blurred nor carry icons.
use std::{mem::{size_of, zeroed}, ptr};
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount64},
    UI::{
        Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_USEFILEATTRIBUTES},
        WindowsAndMessaging::*,
    },
    UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_UP},
};

use crate::ui::sc;

/// Work area of the monitor a popup is about to open on. `SM_CXSCREEN` is only the primary: a menu opened on a second
/// monitor has to be clamped against that monitor, or it jumps to the primary's edge.
unsafe fn screen_of(x: i32, y: i32) -> RECT {
    let mon = MonitorFromPoint(windows_sys::Win32::Foundation::POINT { x, y }, MONITOR_DEFAULTTONEAREST);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    if !mon.is_null() && GetMonitorInfoW(mon, &mut mi) != 0 && mi.rcWork.right > mi.rcWork.left {
        return mi.rcWork;
    }
    RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) }
}

#[derive(Clone)]
pub enum Icon {
    None,
    /// A Segoe Fluent Icons glyph.
    Glyph(&'static str),
    /// The shell icon of `name` with these file attributes: a folder, a `.txt`, a drive.
    Shell(&'static str, u32),
}

#[derive(Clone)]
pub struct Item {
    pub id: u32,
    pub label: String,
    pub icon: Icon,
    pub hint: &'static str,
    pub sep: bool,
    pub danger: bool,
    pub check: Option<bool>,
    pub sub: Vec<Item>,
    pub disabled: bool, // shown dimmed, never highlighted or chosen
}

impl Item {
    pub fn new(id: u32, label: impl Into<String>) -> Self {
        Item { id, label: label.into(), icon: Icon::None, hint: "", sep: false, danger: false, check: None, sub: Vec::new(), disabled: false }
    }
    pub fn dim(mut self, on: bool) -> Self {
        self.disabled = on;
        self
    }
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = icon;
        self
    }
    pub fn hint(mut self, hint: &'static str) -> Self {
        self.hint = hint;
        self
    }
    pub fn sep(mut self) -> Self {
        self.sep = true;
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
    pub fn check(mut self, on: bool) -> Self {
        self.check = Some(on);
        self
    }
    pub fn sub(mut self, items: Vec<Item>) -> Self {
        self.sub = items;
        self
    }
}

/// Posted by the keyboard hook when Escape is pressed while the menu is open.
const WM_MENU_ESC: u32 = WM_APP + 1;

const ROW: i32 = 34;
const SEP: i32 = 9;
const PAD: i32 = 6;
const SIDE: i32 = 12;
const ICON: i32 = 16;
const GAP: i32 = 12;
const MIN_W: i32 = 230;
const MAX_W: i32 = 380;

struct Menu {
    items: Vec<Item>,
    ys: Vec<(i32, i32)>,
    icons: Vec<HICON>,
    w: i32,
    h: i32,
    hover: i32,
    sub_open: i32,
    sub_hover: i32,
    sub_items: Vec<Item>,
    sub_ys: Vec<(i32, i32)>,
    sub_w: i32,
    sub_h: i32,
    sub_x: i32,
    sub_y: i32,
    main: HWND,
    sub: HWND,
    chosen: u32,
    done: bool,
    opened: u64,
    px: Vec<u32>,
    sub_px: Vec<u32>,
    font: HFONT,
    font_small: HFONT,
    font_glyph: HFONT,
}

thread_local! {
    static MENU_HWND: std::cell::Cell<HWND> = const { std::cell::Cell::new(ptr::null_mut()) };
}

/// Escape and the arrows while the menu is open: a low-level hook sees them without the foreground (and without
/// missing a quick tap, which polling did).
unsafe extern "system" fn kb_hook(code: i32, wp: usize, lp: isize) -> isize {
    if code >= 0 && (wp as u32) == WM_KEYDOWN {
        let k = (*(lp as *const KBDLLHOOKSTRUCT)).vkCode;
        let h = MENU_HWND.with(|c| c.get());
        if !h.is_null() {
            if k == VK_ESCAPE as u32 {
                PostMessageW(h, WM_MENU_ESC, 0, 0);
                return 1;
            }
            for vk in [VK_UP, VK_DOWN, VK_RETURN, VK_LEFT, VK_RIGHT] {
                if k == vk as u32 {
                    PostMessageW(h, WM_KEYDOWN, vk as usize, 0);
                    return 1;
                }
            }
        }
    }
    CallNextHookEx(ptr::null_mut(), code, wp, lp)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn mw(hint: &str, label_w: i32) -> i32 {
    // the row: padding + icon + gap + label + gap + hint + padding (+ the chevron when there is a submenu)
    let hint_w = if hint.is_empty() { 0 } else { 62 };
    (SIDE + ICON + GAP + label_w + if hint_w > 0 { GAP + hint_w } else { 0 } + SIDE).clamp(sc(MIN_W), sc(MAX_W))
}

unsafe fn shell_icon(name: &str, attrs: u32) -> HICON {
    let mut fi: SHFILEINFOW = zeroed();
    let ok = SHGetFileInfoW(wide(name).as_ptr(), attrs, &mut fi, std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES);
    if ok == 0 {
        return ptr::null_mut();
    }
    fi.hIcon
}

unsafe fn icons_of(items: &[Item]) -> Vec<HICON> {
    items
        .iter()
        .map(|it| match it.icon {
            Icon::Shell(name, attrs) => shell_icon(name, attrs),
            _ => ptr::null_mut(),
        })
        .collect()
}

fn layout(items: &[Item], dc: HDC, font: HFONT) -> (Vec<(i32, i32)>, i32) {
    unsafe {
        let old = SelectObject(dc, font);
        let mut label_w = 0;
        for it in items {
            let mut r: RECT = zeroed();
            let t = wide(&it.label);
            DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
            label_w = label_w.max(r.right - r.left);
        }
        SelectObject(dc, old);
        let has_sub = items.iter().any(|i| !i.sub.is_empty());
        let w = mw("", label_w + if has_sub { sc(18) } else { 0 });
        let mut y = sc(PAD);
        let ys = items
            .iter()
            .map(|it| {
                if it.sep {
                    y += sc(SEP);
                }
                y += sc(ROW);
                (y - sc(ROW), y)
            })
            .collect();
        (ys, y + sc(PAD))
    }
}

unsafe fn tint(dc: HDC, w: i32, h: i32, color: u32, alpha: u8) {
    let mdc = CreateCompatibleDC(dc);
    let mut bi: BITMAPINFO = zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = 1;
    bi.bmiHeader.biHeight = -1;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut std::ffi::c_void = ptr::null_mut();
    let dib = CreateDIBSection(mdc, &bi, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
    if !dib.is_null() && !bits.is_null() {
        *(bits as *mut u32) = (color & 255) << 16 | (color & 0xFF00) | (color >> 16 & 255);
        let old = SelectObject(mdc, dib);
        let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: 0 };
        AlphaBlend(dc, 0, 0, w, h, mdc, 0, 0, 1, 1, bf);
        SelectObject(mdc, old);
        DeleteObject(dib);
    }
    DeleteDC(mdc);
}

/// What is behind `r` on screen, blurred: the menu is not visible yet when this runs.
unsafe fn capture(r: RECT, radius: i32) -> Vec<u32> {
    let (w, h) = ((r.right - r.left).max(1), (r.bottom - r.top).max(1));
    let sdc = GetDC(ptr::null_mut());
    let mdc = CreateCompatibleDC(sdc);
    let mut bi: BITMAPINFO = zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut std::ffi::c_void = ptr::null_mut();
    let dib = CreateDIBSection(mdc, &bi, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
    let mut px = Vec::new();
    if !dib.is_null() && !bits.is_null() {
        let old = SelectObject(mdc, dib);
        BitBlt(mdc, 0, 0, w, h, sdc, r.left, r.top, SRCCOPY);
        SelectObject(mdc, old);
        px = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize).to_vec();
        blur(&mut px, w as usize, h as usize, radius.max(1) as usize);
        DeleteObject(dib);
    }
    DeleteDC(mdc);
    ReleaseDC(ptr::null_mut(), sdc);
    px
}

/// Three sliding-window box passes: cheap, smooth, good enough behind a 70 % tint.
fn blur(px: &mut [u32], w: usize, h: usize, r: usize) {
    if w == 0 || h == 0 {
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

unsafe fn new_font(face: &str, px: i32, weight: i32) -> HFONT {
    let f = wide(face);
    CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, f.as_ptr())
}

unsafe fn paint(h: HWND, m: &Menu) {
    let mut ps: PAINTSTRUCT = zeroed();
    let dc = BeginPaint(h, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(h, &mut cr);
    let (w, hh) = (cr.right, cr.bottom);
    let (items, ys, icons, px, is_sub) = if h == m.sub {
        (&m.sub_items, &m.sub_ys, &m.icons[m.items.len()..], &m.sub_px, true)
    } else {
        (&m.items, &m.ys, &m.icons[..], &m.px, false)
    };
    let p = winrust_common::style::pal();
    // the blurred screen behind, then the bar's colour over it: the same recipe as the bar
    if !px.is_empty() {
        let mut bi: BITMAPINFO = zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -hh;
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        StretchDIBits(dc, 0, 0, w, hh, 0, 0, w, hh, px.as_ptr() as *const _, &bi, DIB_RGB_COLORS, SRCCOPY);
        tint(dc, w, hh, p.bg, (255 * winrust_common::look::menu_alpha() as u32 / 100) as u8);
    } else {
        // no blur: a plain solid menu in the theme's colour, like a native one
        let b = CreateSolidBrush(p.bg);
        FillRect(dc, &cr, b);
        DeleteObject(b);
    }
    SetBkMode(dc, TRANSPARENT as i32);
    let old = SelectObject(dc, m.font);
    let hover = if is_sub { m.sub_hover } else { m.hover };
    let glyph_of = |it: &Item, i: usize, icons: &[HICON]| -> (bool, HICON, &'static str) {
        let _ = i;
        match it.icon {
            Icon::Glyph(g) => (true, ptr::null_mut(), g),
            Icon::Shell(_, _) => (false, icons.get(i).copied().unwrap_or(ptr::null_mut()), ""),
            Icon::None => (false, ptr::null_mut(), ""),
        }
    };
    for (i, it) in items.iter().enumerate() {
        let (top, bot) = ys[i];
        if it.sep {
            let y = top - sc(SEP) / 2;
            let line = CreateSolidBrush(winrust_common::style::mix(p.bg, p.text, 0.10));
            let r = RECT { left: sc(SIDE), top: y, right: w - sc(SIDE), bottom: y + 1 };
            FillRect(dc, &r, line);
            DeleteObject(line);
        }
        if i as i32 == hover && !it.disabled {
            let r = RECT { left: sc(4), top: top + 1, right: w - sc(4), bottom: bot - 1 };
            let b = CreateSolidBrush(p.hover);
            let (ob, op) = (SelectObject(dc, b), SelectObject(dc, GetStockObject(NULL_PEN)));
            RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, sc(8), sc(8));
            SelectObject(dc, ob);
            SelectObject(dc, op);
            DeleteObject(b);
        }
        // disabled items are painted dimmed, icon and label alike
        let fg = if it.disabled { winrust_common::style::mix(p.text, p.bg, 0.55) } else if it.danger { sc_rgb(232, 92, 92) } else { p.text };
        // the icon slot: a tick when the item is a toggle that is on
        let (ix, iy) = (sc(SIDE), top + (bot - top - sc(ICON)) / 2);
        if it.check == Some(true) {
            SelectObject(dc, m.font_glyph);
            SetTextColor(dc, p.accent);
            let mut r = RECT { left: ix, top, right: ix + sc(ICON), bottom: bot };
            let t = wide("\u{E73E}");
            DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        } else {
            let (is_glyph, hic, g) = glyph_of(it, i, icons);
            if is_glyph {
                SelectObject(dc, m.font_glyph);
                SetTextColor(dc, fg);
                let mut r = RECT { left: ix, top, right: ix + sc(ICON), bottom: bot };
                let t = wide(g);
                DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            } else if !hic.is_null() {
                DrawIconEx(dc, ix, iy, hic, sc(ICON), sc(ICON), 0, ptr::null_mut(), DI_NORMAL);
            }
        }
        // the label and the keyboard hint
        SelectObject(dc, m.font);
        SetTextColor(dc, fg);
        let mut r = RECT { left: sc(SIDE) + sc(ICON) + sc(GAP), top: top + 1, right: w - sc(SIDE) - if it.hint.is_empty() { 0 } else { sc(64) }, bottom: bot };
        let t = wide(&it.label);
        DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        if !it.hint.is_empty() {
            SelectObject(dc, m.font_small);
            SetTextColor(dc, p.dim);
            let mut r = RECT { left: w - sc(SIDE) - sc(64), top: top + 1, right: w - sc(SIDE), bottom: bot };
            let t = wide(it.hint);
            DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        }
        // the submenu chevron
        if !it.sub.is_empty() {
            SelectObject(dc, m.font_glyph);
            SetTextColor(dc, p.dim);
            let mut r = RECT { left: w - sc(SIDE) - sc(4), top, right: w - sc(4), bottom: bot };
            let t = wide("\u{E76C}");
            DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        }
    }
    SelectObject(dc, old);
    EndPaint(h, &ps);
}

fn sc_rgb(r: u32, g: u32, b: u32) -> u32 {
    r | g << 8 | b << 16
}

unsafe fn hit(ys: &[(i32, i32)], y: i32) -> i32 {
    for (i, &(top, bot)) in ys.iter().enumerate() {
        if y >= top && y < bot {
            return i as i32;
        }
    }
    -1
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let m = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Menu;
    if m.is_null() {
        return DefWindowProcW(h, msg, wp, lp);
    }
    let m = &mut *m;
    let is_sub = h == m.sub;
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(h, m);
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let mut cr: RECT = zeroed();
            GetClientRect(h, &mut cr);
            // we hold the capture, so moves from anywhere in the screen arrive here: only the ones inside count
            let inside = x >= 0 && x < cr.right && y >= 0 && y < cr.bottom;
            if is_sub {
                let i = if inside { hit(&m.sub_ys, y) } else { -1 };
                if i != m.sub_hover {
                    m.sub_hover = i;
                    InvalidateRect(h, ptr::null(), 0);
                }
            } else if inside {
                let i = hit(&m.ys, y);
                let i = if i >= 0 && m.items[i as usize].disabled { -1 } else { i };
                if i != m.hover {
                    m.hover = i;
                    let open = i >= 0 && !m.items[i as usize].sub.is_empty();
                    if open {
                        open_sub(m, i as usize);
                    } else if m.sub_open >= 0 && i != m.sub_open {
                        close_sub(m);
                    }
                    InvalidateRect(h, ptr::null(), 0);
                }
            } else if !(m.sub_open >= 0 && x >= cr.right) {
                // left the menu, and not on the way to its submenu: drop the highlight
                if m.hover != -1 || m.sub_open >= 0 {
                    m.hover = -1;
                    close_sub(m);
                    InvalidateRect(h, ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let _ = x;
            let i = if is_sub { hit(&m.sub_ys, y) } else { hit(&m.ys, y) };
            if i >= 0 {
                let items = if is_sub { &m.sub_items } else { &m.items };
                let it = &items[i as usize];
                if it.sub.is_empty() && it.id != 0 && !it.disabled {
                    m.chosen = it.id;
                    m.done = true;
                }
            } else if !is_sub {
                m.done = true; // a click on the padding closes it
            }
            0
        }
        WM_KEYDOWN => {
            let k = wp as u16;
            match k {
                VK_ESCAPE => {
                    if m.sub_open >= 0 {
                        close_sub(m);
                        InvalidateRect(h, ptr::null(), 0);
                    } else {
                        m.done = true;
                    }
                }
                VK_UP | VK_DOWN => {
                    let items = if is_sub { &m.sub_items } else { &m.items };
                    let ys = if is_sub { &m.sub_ys } else { &m.ys };
                    let cur = if is_sub { m.sub_hover } else { m.hover };
                    let step = if k == VK_DOWN { 1 } else { -1 };
                    let mut i = cur;
                    for _ in 0..items.len() + 1 {
                        i += step;
                        if i < 0 {
                            i = items.len() as i32 - 1;
                        }
                        if i >= items.len() as i32 {
                            i = 0;
                        }
                        let _ = ys;
                        if items[i as usize].id != 0 {
                            break;
                        }
                    }
                    if is_sub {
                        m.sub_hover = i;
                    } else {
                        m.hover = i;
                        if i >= 0 && !m.items[i as usize].sub.is_empty() {
                            open_sub(m, i as usize);
                        } else {
                            close_sub(m);
                        }
                    }
                    InvalidateRect(h, ptr::null(), 0);
                }
                VK_RETURN | VK_SPACE => {
                    let items = if is_sub { &m.sub_items } else { &m.items };
                    let cur = if is_sub { m.sub_hover } else { m.hover };
                    if cur >= 0 {
                        let it = &items[cur as usize];
                        if it.id != 0 && !it.disabled {
                            m.chosen = it.id;
                            m.done = true;
                        }
                    }
                }
                VK_RIGHT if !is_sub && m.hover >= 0 && !m.items[m.hover as usize].sub.is_empty() => {
                    let i = m.hover as usize;
                    open_sub(m, i);
                    InvalidateRect(h, ptr::null(), 0);
                }
                VK_LEFT if is_sub => {
                    close_sub(m);
                }
                _ => {}
            }
            0
        }
        WM_MENU_ESC => {
            // the keyboard hook saw Escape (a quick tap included: the timer missed those)
            if m.sub_open >= 0 {
                close_sub(m);
                InvalidateRect(h, ptr::null(), 0);
            } else {
                m.done = true;
            }
            0
        }        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            // a click outside the menu closes it (we hold the capture, so every click comes here)
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let mut cr: RECT = zeroed();
            GetClientRect(h, &mut cr);
            // a click of the very gesture that opened the menu must not close it
            if GetTickCount64().saturating_sub(m.opened) > 200 && (x < 0 || y < 0 || x >= cr.right || y >= cr.bottom) {
                m.done = true;
            }
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

impl Menu {
    fn sub_icons(&self) -> &[HICON] {
        &self.icons[self.items.len()..]
    }
}

/// Opens the submenu of `m.items[i]` at its right (or its left when it does not fit).
unsafe fn open_sub(m: &mut Menu, i: usize) {
    if m.sub_open == i as i32 {
        return;
    }
    close_sub(m);
    let it = &m.items[i];
    let sub = it.sub.clone();
    let (top, bot) = m.ys[i];
    let _ = bot;
    let mut wr: RECT = zeroed();
    GetWindowRect(m.main, &mut wr);
    let (dc, old) = (GetDC(m.main), 0 as HDC);
    let font = SelectObject(dc, m.font);
    let (ys, h) = layout(&sub, dc, m.font);
    let (sub_w, _) = {
        let mut label_w = 0;
        for s in &sub {
            let mut r: RECT = zeroed();
            let t = wide(&s.label);
            DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
            label_w = label_w.max(r.right - r.left);
        }
        (mw("", label_w + sc(18)), 0)
    };
    SelectObject(dc, font);
    let _ = old;
    ReleaseDC(m.main, dc);
    let sa = screen_of(wr.right, wr.top);
    let sw = sub_w.max(sc(MIN_W)).min(sc(MAX_W));
    let mut x = wr.right - sc(2);
    if x + sw > sa.right {
        x = wr.left - sw + sc(2);
    }
    let mut y = wr.top + top - sc(PAD);
    if y + h > sa.bottom {
        y = sa.bottom - h;
    }
    if y < sa.top {
        y = sa.top;
    }
    if x < sa.left {
        x = sa.left;
    }
    let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, wide("WinRustMenu").as_ptr(), ptr::null(), WS_POPUP, x, y, sw, h, m.main, ptr::null_mut(), GetModuleHandleW(ptr::null()), ptr::null());
    if hwnd.is_null() {
        return;
    }
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, m as *mut Menu as isize);
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, sw + 1, h + 1, sc(16), sc(16)), 1);
    let rect = RECT { left: x, top: y, right: x + sw, bottom: y + h };
    let radius = winrust_common::look::menu_radius();
    m.sub_px = if winrust_common::look::menu_blur() { capture(rect, sc(radius)) } else { Vec::new() };
    m.sub = hwnd;
    m.sub_open = i as i32;
    m.sub_items = sub;
    { let sub = &m.sub_items; m.icons.extend(icons_of(sub)); }
    m.sub_ys = ys;
    m.sub_w = sw;
    m.sub_h = h;
    m.sub_x = x;
    m.sub_y = y;
    m.sub_hover = -1;
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    InvalidateRect(hwnd, ptr::null(), 0);
}

unsafe fn close_sub(m: &mut Menu) {
    if !m.sub.is_null() {
        DestroyWindow(m.sub);
        m.sub = ptr::null_mut();
    }
    m.sub_open = -1;
    m.sub_hover = -1;
    m.sub_items.clear();
    for h in m.icons.drain(m.items.len()..) {
        if !h.is_null() {
            DestroyIcon(h);
        }
    }
    m.sub_px.clear();
}

unsafe fn ensure_class() {
    let name = wide("WinRustMenu");
    let mut wc: WNDCLASSW = zeroed();
    if GetClassInfoW(GetModuleHandleW(ptr::null()), name.as_ptr(), &mut wc) != 0 {
        return;
    }
    let wc = WNDCLASSW { style: CS_HREDRAW | CS_VREDRAW, lpfnWndProc: Some(proc), hInstance: GetModuleHandleW(ptr::null()), hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW), lpszClassName: name.as_ptr(), ..zeroed() };
    RegisterClassW(&wc);
}

/// Shows the menu at screen `(x, y)` and returns the id of the chosen item (0 when it was dismissed).
pub unsafe fn show(x: i32, y: i32, items: Vec<Item>) -> u32 {
    if items.is_empty() {
        return 0;
    }
    ensure_class();
    let font = new_font(winrust_common::style::ui_face(), sc(13), 400);
    let font_small = new_font(winrust_common::style::ui_face(), sc(11), 400);
    let font_glyph = new_font(winrust_common::style::icon_face(), sc(15), 400);
    let measure = GetDC(ptr::null_mut());
    let (ys, h) = layout(&items, measure, font);
    ReleaseDC(ptr::null_mut(), measure);
    let sa = screen_of(x, y);
    let has_sub = items.iter().any(|i| !i.sub.is_empty());
    let mut label_w = 0;
    {
        let dc = GetDC(ptr::null_mut());
        let old = SelectObject(dc, font);
        for it in &items {
            let mut r: RECT = zeroed();
            let t = wide(&it.label);
            DrawTextW(dc, t.as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
            label_w = label_w.max(r.right - r.left);
        }
        SelectObject(dc, old);
        ReleaseDC(ptr::null_mut(), dc);
    }
    let w = mw("", label_w + if has_sub { sc(18) } else { 0 });
    let mut px_x = x;
    let mut px_y = y;
    if px_x + w > sa.right {
        px_x = sa.right - w;
    }
    if px_y + h > sa.bottom {
        px_y = sa.bottom - h;
    }
    if px_x < sa.left {
        px_x = sa.left;
    }
    if px_y < sa.top {
        px_y = sa.top;
    }
    let mut menu = Box::new(Menu {
        items,
        ys,
        icons: Vec::new(),
        w,
        h,
        hover: -1,
        sub_open: -1,
        sub_hover: -1,
        sub_items: Vec::new(),
        sub_ys: Vec::new(),
        sub_w: 0,
        sub_h: 0,
        sub_x: 0,
        sub_y: 0,
        main: ptr::null_mut(),
        sub: ptr::null_mut(),
        chosen: 0,
        done: false,
        opened: GetTickCount64(),
        px: Vec::new(),
        sub_px: Vec::new(),
        font,
        font_small,
        font_glyph,
    });
    menu.icons = icons_of(&menu.items);
    let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, wide("WinRustMenu").as_ptr(), ptr::null(), WS_POPUP, px_x, px_y, w, h, ptr::null_mut(), ptr::null_mut(), GetModuleHandleW(ptr::null()), ptr::null());
    if hwnd.is_null() {
        return 0;
    }
    menu.main = hwnd;
    let boxed = Box::into_raw(menu);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, boxed as isize);
    // the window exists but is not visible yet: what the screen shows there is what is behind the menu
    let radius = winrust_common::look::menu_radius();
    (*boxed).px = if winrust_common::look::menu_blur() { capture(RECT { left: px_x, top: px_y, right: px_x + w, bottom: px_y + h }, sc(radius)) } else { Vec::new() };
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, sc(16), sc(16)), 1);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetCapture(hwnd); // clicks outside come to us even without the foreground
    MENU_HWND.set(hwnd);
    let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_hook), GetModuleHandleW(ptr::null()), 0);
    InvalidateRect(hwnd, ptr::null(), 0);

    let state = &mut *boxed;
    let mut msg: MSG = zeroed();
    loop {
        let r = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
        if r <= 0 {
            break;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if state.done {
            break;
        }
    }
    if !hook.is_null() {
        UnhookWindowsHookEx(hook);
    }
    MENU_HWND.set(ptr::null_mut());
    if GetCapture() == hwnd {
        ReleaseCapture();
    }
    if !state.sub.is_null() {
        DestroyWindow(state.sub);
    }
    DestroyWindow(hwnd);
    let chosen = state.chosen;
    let m = Box::from_raw(boxed);
    for h_ in m.icons {
        if !h_.is_null() {
            DestroyIcon(h_);
        }
    }
    for f in [m.font, m.font_small, m.font_glyph] {
        DeleteObject(f);
    }
    chosen
}
