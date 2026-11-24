//! Small GDI helpers shared by the overlay, the editor and the recording bar: DPI, fonts, fills and text.
use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::*,
    UI::HiDpi::GetDpiForWindow,
};

use winrust_common::style::pal;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Logical pixels at the window's DPI. 96 dpi -> 1:1.
pub unsafe fn dpi(hwnd: HWND) -> i32 {
    let d = GetDpiForWindow(hwnd) as i32;
    if d <= 0 { 96 } else { d }
}

pub unsafe fn sc(hwnd: HWND, v: i32) -> i32 {
    (v as f32 * dpi(hwnd) as f32 / 96.0).round() as i32
}

pub unsafe fn make_font(height_px: i32, weight: i32, face: &str) -> HFONT {
    CreateFontW(-height_px, 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(face).as_ptr())
}

/// A DC with a solid brush selected, for one fill.
pub unsafe fn fill(dc: HDC, r: RECT, color: u32) {
    let b = CreateSolidBrush(color);
    FillRect(dc, &r, b);
    DeleteObject(b as HGDIOBJ);
}

/// Filled box with rounded corners (plain rectangle when `rad` <= 0).
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

/// Frame of `thick` px inside `r`.
pub unsafe fn outline(dc: HDC, r: RECT, color: u32, thick: i32) {
    let b = CreateSolidBrush(color);
    FrameRect(dc, &r, b);
    if thick > 1 {
        let inner = RECT { left: r.left + thick, top: r.top + thick, right: r.right - thick, bottom: r.bottom - thick };
        FrameRect(dc, &inner, b);
    }
    DeleteObject(b as HGDIOBJ);
}

/// One line of text, vertically centred in `r`.
pub unsafe fn text(dc: HDC, s: &str, r: RECT, font: HFONT, color: u32, flags: u32) {
    let old = SelectObject(dc, font as HGDIOBJ);
    SetTextColor(dc, color);
    SetBkMode(dc, TRANSPARENT as i32);
    let mut rr = r;
    let w: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(dc, w.as_ptr(), w.len() as i32, &mut rr, DT_SINGLELINE as u32 | DT_VCENTER as u32 | flags);
    SelectObject(dc, old);
}

pub unsafe fn text_w(dc: HDC, s: &str, font: HFONT) -> i32 {
    let old = SelectObject(dc, font as HGDIOBJ);
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut size = windows_sys::Win32::Foundation::SIZE { cx: 0, cy: 0 };
    GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
    SelectObject(dc, old);
    size.cx
}

/// Colour of a toolbar button: hover / checked / normal.
pub fn button_bg(hover: bool, active: bool) -> u32 {
    let p = pal();
    if active {
        p.active
    } else if hover {
        p.hover
    } else {
        p.bg
    }
}

/// Dashed 1 px rectangle ("marching ants" without the animation).
pub unsafe fn dashed(dc: HDC, r: RECT, color: u32) {
    let p = CreatePen(PS_SOLID, 1, color);
    let old = SelectObject(dc, p as HGDIOBJ);
    let old_b = SelectObject(dc, GetStockObject(NULL_BRUSH) as HGDIOBJ);
    // GDI has no cheap dash pen in this crate; short segments every 6 px look the same at 1 px.
    let seg = |a: (i32, i32), b: (i32, i32), horizontal: bool| {
        if horizontal {
            let mut x = a.0;
            while x < b.0 {
                MoveToEx(dc, x, a.1, null_mut());
                LineTo(dc, (x + 3).min(b.0), a.1);
                x += 6;
            }
        } else {
            let mut y = a.1;
            while y < b.1 {
                MoveToEx(dc, a.0, y, null_mut());
                LineTo(dc, a.0, (y + 3).min(b.1));
                y += 6;
            }
        }
    };
    seg((r.left, r.top), (r.right, r.top), true);
    seg((r.left, r.bottom - 1), (r.right, r.bottom - 1), true);
    seg((r.left, r.top), (r.left, r.bottom), false);
    seg((r.right - 1, r.top), (r.right - 1, r.bottom), false);
    SelectObject(dc, old_b);
    SelectObject(dc, old);
    DeleteObject(p as HGDIOBJ);
}

