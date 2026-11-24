//! The region overlay: one window over the whole virtual desktop, the desktop shown dimmed and the dragged rectangle
//! bright, with live pixel size, thirds guides and handles. A plain click picks the window under the pointer (or that
//! monitor); Esc / right click cancels. The same overlay feeds screenshots and region recordings.
use std::{
    cell::RefCell,
    mem::zeroed,
    ptr::{null, null_mut},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::SetThreadDpiAwarenessContext,
        Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN},
        WindowsAndMessaging::*,
    },
};

use winrust_common::style::{pal, rgb};

use crate::{cap, img::Image, ui};

pub struct Pick {
    /// Screen coordinates of the source.
    pub rect: RECT,
    pub image: Option<Image>,
    pub window: Option<HWND>,
}

struct St {
    orig: HBITMAP,
    dim: HBITMAP,
    w: i32,
    h: i32,
    vx: i32,
    vy: i32,
    from: Option<(i32, i32)>,
    to: (i32, i32),
    pick: Option<(RECT, Option<HWND>)>,
}

thread_local! {
    static ST: RefCell<Option<St>> = const { RefCell::new(None) };
}

fn client_rect(a: (i32, i32), b: (i32, i32)) -> RECT {
    RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0), bottom: a.1.max(b.1) }
}

fn selection() -> Option<RECT> {
    ST.with_borrow(|s| s.as_ref().and_then(|s| s.from.map(|f| client_rect(f, s.to))))
}

/// Shows the picker and waits for a choice. `with_cursor` bakes the pointer into the result; `want_image` also copies
/// the pixels (recording only needs the rectangle and, for a clicked window, its handle).
pub fn select(with_cursor: bool, want_image: bool) -> Option<Pick> {
    unsafe { run(with_cursor, want_image) }
}

unsafe fn run(with_cursor: bool, want_image: bool) -> Option<Pick> {
    SetThreadDpiAwarenessContext(-4isize as _); // physical pixels on every screen
    let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
    let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
    let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
    let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    if w <= 0 || h <= 0 {
        return None;
    }
    let scr = GetDC(null_mut());
    let (mem, dd, one) = (CreateCompatibleDC(scr), CreateCompatibleDC(scr), CreateCompatibleDC(scr));
    let (orig, dim, black) = (CreateCompatibleBitmap(scr, w, h), CreateCompatibleBitmap(scr, w, h), CreateCompatibleBitmap(scr, 1, 1));
    let (o1, o2, o3) = (SelectObject(mem, orig as HGDIOBJ), SelectObject(dd, dim as HGDIOBJ), SelectObject(one, black as HGDIOBJ));
    BitBlt(mem, 0, 0, w, h, scr, vx, vy, SRCCOPY | CAPTUREBLT);
    if with_cursor {
        // draw the pointer into the frozen copy; the live crosshair sits on top in the overlay only
        draw_cursor(mem, POINT { x: vx, y: vy });
    }
    BitBlt(dd, 0, 0, w, h, mem, 0, 0, SRCCOPY);
    PatBlt(one, 0, 0, 1, 1, BLACKNESS);
    AlphaBlend(dd, 0, 0, w, h, one, 0, 0, 1, 1, BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 125, AlphaFormat: 0 });
    SelectObject(mem, o1);
    SelectObject(dd, o2);
    SelectObject(one, o3);
    for d in [mem, dd, one] {
        DeleteDC(d);
    }
    DeleteObject(black as HGDIOBJ);
    ReleaseDC(null_mut(), scr);
    ST.set(Some(St { orig, dim, w, h, vx, vy, from: None, to: (0, 0), pick: None }));

    let cls = ui::wide("rs-shot-overlay");
    let hi = GetModuleHandleW(null());
    let mut wc: WNDCLASSW = zeroed();
    wc.lpfnWndProc = Some(proc_);
    wc.hInstance = hi;
    wc.hCursor = LoadCursorW(null_mut(), IDC_CROSS);
    wc.lpszClassName = cls.as_ptr();
    RegisterClassW(&wc);
    let hw = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
        cls.as_ptr(),
        ui::wide("rs-shot").as_ptr(),
        WS_POPUP | WS_VISIBLE,
        vx,
        vy,
        w,
        h,
        null_mut(),
        null_mut(),
        hi,
        null(),
    );
    if hw.is_null() {
        cleanup();
        return None;
    }
    winrust_common::fg::force(hw);
    let mut m: MSG = zeroed();
    while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    let (rect, hwnd) = ST.with_borrow(|s| s.as_ref().and_then(|s| s.pick)).unwrap_or((RECT { left: 0, top: 0, right: 0, bottom: 0 }, None));
    let image = if !want_image || rect.right - rect.left < 2 || rect.bottom - rect.top < 2 {
        None
    } else if let Some(h) = hwnd {
        cap::capture_window(h, with_cursor)
    } else {
        let piece = {
            let (orig, vx, vy) = ST.with_borrow(|s| s.as_ref().map(|s| (s.orig, s.vx, s.vy)).unwrap_or((null_mut(), 0, 0)));
            let scr = GetDC(null_mut());
            let src = CreateCompatibleDC(scr);
            let dst = CreateCompatibleDC(scr);
            let bmp = CreateCompatibleBitmap(scr, rect.right - rect.left, rect.bottom - rect.top);
            let so = SelectObject(src, orig as HGDIOBJ);
            let do_ = SelectObject(dst, bmp as HGDIOBJ);
            BitBlt(dst, 0, 0, rect.right - rect.left, rect.bottom - rect.top, src, rect.left - vx, rect.top - vy, SRCCOPY);
            SelectObject(src, so);
            SelectObject(dst, do_);
            DeleteDC(src);
            DeleteDC(dst);
            ReleaseDC(null_mut(), scr);
            bmp
        };
        let im = Image::from_hbitmap(piece);
        DeleteObject(piece as HGDIOBJ);
        im
    };
    cleanup();
    (rect.right - rect.left >= 2 && rect.bottom - rect.top >= 2).then_some(Pick { rect, image, window: hwnd })
}

unsafe fn cleanup() {
    if let Some(s) = ST.take() {
        DeleteObject(s.orig as HGDIOBJ);
        DeleteObject(s.dim as HGDIOBJ);
    }
}

unsafe fn draw_cursor(mem: HDC, origin: POINT) {
    let mut ci: CURSORINFO = zeroed();
    ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
    if GetCursorInfo(&mut ci) != 0 && ci.flags & CURSOR_SHOWING != 0 && !ci.hCursor.is_null() {
        DrawIconEx(mem, ci.ptScreenPos.x - origin.x, ci.ptScreenPos.y - origin.y, ci.hCursor, 0, 0, 0, null_mut(), DI_NORMAL);
    }
}

unsafe fn paint(hw: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hw, &mut ps);
    let p = ps.rcPaint;
    let (dim, orig, w, h) = ST.with_borrow(|s| {
        s.as_ref().map(|s| (s.dim, s.orig, s.w, s.h)).unwrap_or((null_mut(), null_mut(), 0, 0))
    });
    let mem = CreateCompatibleDC(hdc);
    let old = SelectObject(mem, dim as HGDIOBJ);
    BitBlt(hdc, p.left, p.top, p.right - p.left, p.bottom - p.top, mem, p.left, p.top, SRCCOPY);
    let acc = pal().accent;
    // A hovered window (no drag yet): light outline so a click has a visible target.
    if let Some(r) = hover_window_rect(hw) {
        pt_rect(hdc, &r, acc, 2);
    }
    let sel = selection();
    if let Some(r) = sel {
        let (l, t, rr, b) = (r.left.max(p.left), r.top.max(p.top), r.right.min(p.right), r.bottom.min(p.bottom));
        SelectObject(mem, orig as HGDIOBJ);
        if rr > l && b > t {
            BitBlt(hdc, l, t, rr - l, b - t, mem, l, t, SRCCOPY);
        }
        // thirds guides inside the selection
        let g = CreateSolidBrush(acc);
        let gw = (r.right - r.left).max(1);
        let gh = (r.bottom - r.top).max(1);
        for i in 1..3 {
            let x = r.left + gw * i / 3;
            let y = r.top + gh * i / 3;
            if !(x < p.left || x >= p.right) {
                FillRect(hdc, &RECT { left: x, top: r.top, right: x + 1, bottom: r.bottom }, g);
            }
            if !(y < p.top || y >= p.bottom) {
                FillRect(hdc, &RECT { left: r.left, top: y, right: r.right, bottom: y + 1 }, g);
            }
        }
        DeleteObject(g as HGDIOBJ);
        pt_rect(hdc, &r, acc, 2);
        handles(hdc, r, acc);
        badge(hdc, r, w, h);
    } else {
        let hint = t!(
            "Протяните мышью, чтобы выбрать область · щёлкните окно · Esc — отмена",
            "Drag to select a region · click a window · Esc cancels",
            ja: "ドラッグして範囲を選択 · ウィンドウをクリック · Esc でキャンセル",
            pt: "Arraste para selecionar uma região · clique em uma janela · Esc cancela",
            zh: "拖动选择区域 · 单击窗口 · Esc 取消"
        );
        let hint2 = t!(
            "Запись: Win+Alt+R",
            "Record: Win+Alt+R",
            ja: "録画: Win+Alt+R",
            pt: "Gravar: Win+Alt+R",
            zh: "录制：Win+Alt+R"
        );
        pill(hdc, 0, 24, w, tformat(&format!("{hint}\n{hint2}"), hdc), acc);
    }
    SelectObject(mem, old);
    DeleteDC(mem);
    EndPaint(hw, &ps);
}

fn tformat(s: &str, _hdc: HDC) -> String {
    s.to_string()
}

/// Width of the window that would be picked, in client coordinates (for the hover outline).
unsafe fn hover_window_rect(hw: HWND) -> Option<RECT> {
    if selection().is_some() {
        return None;
    }
    let mut p = POINT { x: 0, y: 0 };
    GetCursorPos(&mut p);
    let hit = cap::window_at(p.x, p.y)?;
    let (vx, vy) = ST.with_borrow(|s| s.as_ref().map(|s| (s.vx, s.vy)).unwrap_or((0, 0)));
    let _ = hw;
    let r = cap::window_rect(hit);
    Some(RECT { left: r.left - vx, top: r.top - vy, right: r.right - vx, bottom: r.bottom - vy })
}

unsafe fn pt_rect(dc: HDC, r: &RECT, color: u32, thick: i32) {
    let b = CreateSolidBrush(color);
    for i in 0..thick {
        let rr = RECT { left: r.left + i, top: r.top + i, right: r.right - i, bottom: r.bottom - i };
        FrameRect(dc, &rr, b);
    }
    DeleteObject(b as HGDIOBJ);
}

unsafe fn handles(dc: HDC, r: RECT, color: u32) {
    let s = 5;
    let pts = [
        (r.left, r.top),
        (r.right, r.top),
        (r.left, r.bottom),
        (r.right, r.bottom),
        ((r.left + r.right) / 2, r.top),
        ((r.left + r.right) / 2, r.bottom),
        (r.left, (r.top + r.bottom) / 2),
        (r.right, (r.top + r.bottom) / 2),
    ];
    let w = CreateSolidBrush(rgb(255, 255, 255));
    let d = CreateSolidBrush(color);
    for (x, y) in pts {
        let rr = RECT { left: x - s, top: y - s, right: x + s + 1, bottom: y + s + 1 };
        FillRect(dc, &rr, w);
        FrameRect(dc, &rr, d);
    }
    DeleteObject(w as HGDIOBJ);
    DeleteObject(d as HGDIOBJ);
}

/// "1280 × 720" near the selection, above it when there is room, below otherwise.
unsafe fn badge(dc: HDC, r: RECT, w: i32, h: i32) {
    let f = ui::make_font(15, 600, winrust_common::style::ui_face());
    let text = format!("{} × {}", r.right - r.left, r.bottom - r.top);
    let tw = ui::text_w(dc, &text, f) + 18;
    let th = 26;
    let x = (r.left + (r.right - r.left - tw) / 2).clamp(4, (w - tw - 4).max(4));
    let y = if r.top - th - 8 >= 0 { r.top - th - 8 } else { r.bottom + 8 };
    let box_ = RECT { left: x, top: y, right: x + tw, bottom: y + th };
    ui::rounded(dc, box_, rgb(20, 20, 22), 8);
    pt_rect(dc, &box_, pal().accent, 1);
    let tr = RECT { left: x, top: y, right: x + tw, bottom: y + th };
    ui::text(dc, &text, tr, f, pal().text, DT_CENTER as u32);
    DeleteObject(f as HGDIOBJ);
    let _ = h;
}

/// A dark pill with a centred two-line hint.
unsafe fn pill(dc: HDC, x0: i32, y: i32, w: i32, text: String, acc: u32) {
    let f = ui::make_font(15, 600, winrust_common::style::ui_face());
    let f2 = ui::make_font(13, 400, winrust_common::style::ui_face());
    let mut lines = text.split('\n');
    let a = lines.next().unwrap_or("");
    let b = lines.next().unwrap_or("");
    let tw = ui::text_w(dc, a, f).max(ui::text_w(dc, b, f2)) + 28;
    let x = (w - tw) / 2 + x0;
    let r = RECT { left: x, top: y, right: x + tw, bottom: y + 54 };
    ui::rounded(dc, r, rgb(18, 18, 20), 10);
    pt_rect(dc, &r, acc, 1);
    ui::text(dc, a, RECT { left: x, top: y + 6, right: x + tw, bottom: y + 30 }, f, pal().text, DT_CENTER as u32);
    ui::text(dc, b, RECT { left: x, top: y + 29, right: x + tw, bottom: y + 50 }, f2, pal().dim, DT_CENTER as u32);
    DeleteObject(f as HGDIOBJ);
    DeleteObject(f2 as HGDIOBJ);
}

/// Finishes picking: a dragged rectangle, else the window under the pointer, else that monitor.
unsafe fn finish(hw: HWND) {
    let (vx, vy) = ST.with_borrow(|s| s.as_ref().map(|s| (s.vx, s.vy)).unwrap_or((0, 0)));
    if let Some(r) = selection() {
        if r.right - r.left >= 3 && r.bottom - r.top >= 3 {
            let screen = RECT { left: r.left + vx, top: r.top + vy, right: r.right + vx, bottom: r.bottom + vy };
            ST.with_borrow_mut(|s| {
                if let Some(s) = s {
                    s.pick = Some((screen, None));
                }
            });
        }
        DestroyWindow(hw);
        return;
    }
    let mut p = POINT { x: 0, y: 0 };
    GetCursorPos(&mut p);
    if let Some(hit) = cap::window_at(p.x, p.y) {
        let r = cap::window_rect(hit);
        ST.with_borrow_mut(|s| {
            if let Some(s) = s {
                s.pick = Some((r, Some(hit)));
            }
        });
    } else {
        let m = winrust_common::monitors::at(p.x, p.y);
        ST.with_borrow_mut(|s| {
            if let Some(s) = s {
                s.pick = Some((m.rc, None));
            }
        });
    }
    DestroyWindow(hw);
}

unsafe fn cancel(hw: HWND) {
    DestroyWindow(hw);
}

unsafe extern "system" fn proc_(hw: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp & 0xffff) as i16 as i32;
    let y = ((lp >> 16) & 0xffff) as i16 as i32;
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hw);
            0
        }
        WM_MOUSEMOVE => {
            let before = selection();
            ST.with_borrow_mut(|s| {
                if let Some(s) = s {
                    if s.from.is_some() {
                        s.to = (x, y);
                    }
                }
            });
            if selection().is_some() {
                let _ = before;
                InvalidateRect(hw, null(), 0);
            } else {
                InvalidateRect(hw, null(), 0); // the hover window outline follows the pointer
            }
            0
        }
        WM_LBUTTONDOWN => {
            ST.with_borrow_mut(|s| {
                if let Some(s) = s {
                    s.from = Some((x, y));
                    s.to = (x, y);
                }
            });
            InvalidateRect(hw, null(), 0);
            0
        }
        WM_LBUTTONUP => {
            finish(hw);
            0
        }
        WM_RBUTTONUP => {
            cancel(hw);
            0
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => {
            cancel(hw);
            0
        }
        WM_KEYDOWN if wp == VK_RETURN as usize => {
            finish(hw);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hw, msg, wp, lp),
    }
}
