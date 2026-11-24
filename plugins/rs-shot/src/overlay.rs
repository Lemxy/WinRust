//! The selection layer: one overlay over the whole virtual desktop (all monitors, physical pixels), the desktop dimmed,
//! the dragged rectangle bright, plus a floating bottom bar (Photo/Video, Region/Full/Window/Monitor, delay, action).
//! Every frame is composed into a persistent back buffer and copied to the window once; mouse movement invalidates only
//! the previous and the new selection/bar/hover rectangles, never the whole desktop.
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

use winrust_common::{
    monitors::{self, Mon},
    style::{pal, rgb},
};

use crate::{cap, img::Image, ui};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Region,
    Full,
    Monitor,
    Window,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub video: bool,
    pub kind: Kind,
    pub delay: u32,
    pub cursor: bool,
    pub want_image: bool,
}

pub struct Pick {
    /// Screen coordinates of the source.
    pub rect: RECT,
    pub image: Option<Image>,
    pub window: Option<HWND>,
    pub video: bool,
    pub kind: Kind,
    pub delay: u32,
}

struct PickData {
    rect: RECT,
    window: Option<HWND>,
    video: bool,
    kind: Kind,
    delay: u32,
}

struct St {
    orig: HBITMAP,
    dim: HBITMAP,
    back: HBITMAP,
    bdc: HDC,
    w: i32,
    h: i32,
    vx: i32,
    vy: i32,
    from: Option<(i32, i32)>,
    to: (i32, i32),
    pick: Option<PickData>,
    video: bool,
    kind: Kind,
    delay: u32,
    mon: Mon,
    fresh: bool,
    hover: Option<usize>,
    last_hover: Option<RECT>,
    target: Option<HWND>,
}

thread_local! {
    static ST: RefCell<Option<St>> = const { RefCell::new(None) };
}

const DELAYS: [u32; 4] = [0, 2, 5, 10];

fn client_rect(a: (i32, i32), b: (i32, i32)) -> RECT {
    RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0), bottom: a.1.max(b.1) }
}

fn selection() -> Option<RECT> {
    ST.with_borrow(|s| s.as_ref().and_then(|s| s.from.map(|f| client_rect(f, s.to))))
}

fn union(a: RECT, b: RECT) -> RECT {
    RECT { left: a.left.min(b.left), top: a.top.min(b.top), right: a.right.max(b.right), bottom: a.bottom.max(b.bottom) }
}

fn expanded(mut r: RECT, m: i32) -> RECT {
    r.left -= m;
    r.top -= m;
    r.right += m;
    r.bottom += m;
    r
}

fn hits(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn inval(hw: HWND, r: RECT) {
    unsafe { InvalidateRect(hw, &r, 0) };
}

/// Shows the picker. `video` preselects Vídeo, `kind` the capture type, `delay` the 0/2/5/10 starting point.
pub fn choose(opt: Options) -> Option<Pick> {
    unsafe { run(opt) }
}

unsafe fn run(opt: Options) -> Option<Pick> {
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
    if opt.cursor {
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

    let back = CreateCompatibleBitmap(scr, w, h);
    let bdc = CreateCompatibleDC(scr);
    SelectObject(bdc, back as HGDIOBJ);
    ovlog(&format!("back={back:?} bdc={bdc:?}"));
    ReleaseDC(null_mut(), scr);
    let cur = cap::cursor_pos();
    ST.set(Some(St {
        orig,
        dim,
        back,
        bdc,
        w,
        h,
        vx,
        vy,
        from: None,
        to: (0, 0),
        pick: None,
        video: opt.video,
        kind: opt.kind,
        delay: opt.delay,
        mon: monitors::at(cur.x, cur.y),
        fresh: true,
        hover: None,
        last_hover: None,
        target: None,
    }));

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
    // The WS_VISIBLE style plus the virtual-screen rect is enough; a later SetWindowPos was observed to leave the
    // popup unpainted on some desktops, so the original creation geometry is authoritative.
    ovlog(&format!("created hw={hw:?} rect={vx},{vy},{w},{h}"));
    winrust_common::fg::force(hw);
    ovlog("foreground forced");
    let mut m: MSG = zeroed();
    while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    let pd = ST.with_borrow(|s| s.as_ref().and_then(|s| s.pick.as_ref().map(|p| PickData {
        rect: p.rect,
        window: p.window,
        video: p.video,
        kind: p.kind,
        delay: p.delay,
    })));
    let image = match pd.as_ref() {
        Some(p) if opt.want_image && !p.video && p.kind == Kind::Region && p.window.is_none() => crop_from_orig(p.rect),
        _ => None,
    };
    cleanup();
    pd.map(|p| Pick { rect: p.rect, image, window: p.window, video: p.video, kind: p.kind, delay: p.delay })
}

unsafe fn crop_from_orig(rect: RECT) -> Option<Image> {
    let (orig, vx, vy) = ST.with_borrow(|s| s.as_ref().map(|s| (s.orig, s.vx, s.vy)).unwrap_or((null_mut(), 0, 0)));
    if orig.is_null() {
        return None;
    }
    let (rw, rh) = (rect.right - rect.left, rect.bottom - rect.top);
    if rw < 2 || rh < 2 {
        return None;
    }
    let scr = GetDC(null_mut());
    let src = CreateCompatibleDC(scr);
    let dst = CreateCompatibleDC(scr);
    let bmp = CreateCompatibleBitmap(scr, rw, rh);
    let so = SelectObject(src, orig as HGDIOBJ);
    let do_ = SelectObject(dst, bmp as HGDIOBJ);
    BitBlt(dst, 0, 0, rw, rh, src, rect.left - vx, rect.top - vy, SRCCOPY);
    SelectObject(src, so);
    SelectObject(dst, do_);
    DeleteDC(src);
    DeleteDC(dst);
    ReleaseDC(null_mut(), scr);
    let im = Image::from_hbitmap(bmp);
    DeleteObject(bmp as HGDIOBJ);
    im
}

unsafe fn cleanup() {
    if let Some(s) = ST.take() {
        DeleteObject(s.orig as HGDIOBJ);
        DeleteObject(s.dim as HGDIOBJ);
        DeleteObject(s.back as HGDIOBJ);
        DeleteDC(s.bdc);
    }
}

unsafe fn draw_cursor(mem: HDC, origin: POINT) {
    let mut ci: CURSORINFO = zeroed();
    ci.cbSize = std::mem::size_of::<CURSORINFO>() as u32;
    if GetCursorInfo(&mut ci) != 0 && ci.flags & CURSOR_SHOWING != 0 && !ci.hCursor.is_null() {
        DrawIconEx(mem, ci.ptScreenPos.x - origin.x, ci.ptScreenPos.y - origin.y, ci.hCursor, 0, 0, 0, null_mut(), DI_NORMAL);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    Photo,
    Video,
    Kind(Kind),
    Delay,
    Go,
}

fn scale(hw: HWND) -> i32 {
    unsafe { ui::sc(hw, 1).max(1) }
}

/// The bar, on the cursor monitor, centred at the bottom; it jumps above the selection when that reaches the bottom.
unsafe fn bar_rect(hw: HWND) -> RECT {
    let s = scale(hw);
    let mon = ST.with_borrow(|st| st.as_ref().map(|s| s.mon.work).unwrap_or(RECT { left: 0, top: 0, right: 1, bottom: 1 }));
    let mx = (mon.right - mon.left).max(1);
    let bw = (760 * s).min(mx - 24);
    let bh = 64 * s;
    let x = mon.left + (mx - bw) / 2;
    let mut y = mon.bottom - bh - 16 * s;
    if let Some(sel) = selection() {
        let bar = RECT { left: x, top: y, right: x + bw, bottom: y + bh };
        if sel.left < bar.right && sel.right > bar.left && sel.top < bar.bottom && sel.bottom > bar.top {
            y = sel.top - bh - 8 * s;
            if y < mon.top + 8 * s {
                y = (sel.bottom + 8 * s).min(mon.bottom - bh - 8 * s);
            }
        }
    }
    RECT { left: x, top: y.max(mon.top + 4 * s), right: x + bw, bottom: (y.max(mon.top + 4 * s) + bh).min(mon.bottom) }
}

fn buttons(hw: HWND, bar: RECT) -> Vec<(RECT, Act)> {
    fn add(v: &mut Vec<(RECT, Act)>, x: &mut i32, top: i32, h: i32, gap: i32, w: i32, act: Act) {
        v.push((RECT { left: *x, top, right: *x + w, bottom: top + h }, act));
        *x += w + gap;
    }
    let s = scale(hw);
    let pad = 8 * s;
    let gap = 6 * s;
    let mut x = bar.left + pad;
    let top = bar.top + 8 * s;
    let h = bar.bottom - top - 8 * s;
    let mut v = Vec::new();
    let mode_w = 68 * s;
    add(&mut v, &mut x, top, h, gap, mode_w, Act::Photo);
    add(&mut v, &mut x, top, h, gap, mode_w, Act::Video);
    x += 6 * s;
    for k in [Kind::Region, Kind::Full, Kind::Monitor, Kind::Window] {
        add(&mut v, &mut x, top, h, gap, 78 * s, Act::Kind(k));
    }
    x += 6 * s;
    add(&mut v, &mut x, top, h, gap, 58 * s, Act::Delay);
    let action = (bar.right - pad - x).max(96 * s);
    add(&mut v, &mut x, top, h, 0, action, Act::Go);
    v
}

unsafe fn paint(hw: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hw, &mut ps);
    ovlog(&format!("paint rc={},{},{},{}", ps.rcPaint.left, ps.rcPaint.top, ps.rcPaint.right, ps.rcPaint.bottom));
    let fresh = ST.with_borrow_mut(|s| {
        if let Some(s) = s {
            if s.fresh {
                s.fresh = false;
                true
            } else {
                false
            }
        } else {
            false
        }
    });
    let mut c: RECT = zeroed();
    GetClientRect(hw, &mut c);
    let p = if fresh { c } else { ps.rcPaint };
    let (dim, orig, bdc, w, h) = ST.with_borrow(|s| {
        s.as_ref().map(|s| (s.dim, s.orig, s.bdc, s.w, s.h)).unwrap_or((null_mut(), null_mut(), null_mut(), 0, 0))
    });
    if p.right <= p.left || p.bottom <= p.top || bdc.is_null() {
        ovlog(&format!("paint early p={},{},{},{} bdc={bdc:?}", p.left, p.top, p.right, p.bottom));
        EndPaint(hw, &ps);
        return;
    }
    ovlog("paint have back");
    let clip = CreateRectRgn(p.left, p.top, p.right, p.bottom);
    SelectClipRgn(bdc, clip);
    // frozen desktop, dimmed
    let tmp = CreateCompatibleDC(bdc);
    let old = SelectObject(tmp, dim as HGDIOBJ);
    BitBlt(bdc, p.left, p.top, p.right - p.left, p.bottom - p.top, tmp, p.left, p.top, SRCCOPY);
    ovlog("paint dim");
    let sel = selection();
    if let Some(r) = sel {
        SelectObject(tmp, orig as HGDIOBJ);
        let (l, t, rr, b) = (r.left.max(p.left), r.top.max(p.top), r.right.min(p.right), r.bottom.min(p.bottom));
        if rr > l && b > t {
            BitBlt(bdc, l, t, rr - l, b - t, tmp, l, t, SRCCOPY);
        }
    }
    SelectObject(tmp, old);
    DeleteDC(tmp);

    let acc = pal().accent;
    let (kind, hover) = ST.with_borrow(|s| s.as_ref().map(|s| (s.kind, s.hover)).unwrap_or((Kind::Region, None)));
    // window/region hover outline
    if kind == Kind::Region || kind == Kind::Window {
        if sel.is_none() {
            if let Some(r) = hover_window_rect() {
                pt_rect(bdc, &r, acc, 2);
            }
        }
    }
    if let Some(r) = sel {
        let g = CreateSolidBrush(acc);
        for i in 1..3 {
            let x = r.left + (r.right - r.left).max(1) * i / 3;
            let y = r.top + (r.bottom - r.top).max(1) * i / 3;
            FillRect(bdc, &RECT { left: x, top: r.top, right: x + 1, bottom: r.bottom }, g);
            FillRect(bdc, &RECT { left: r.left, top: y, right: r.right, bottom: y + 1 }, g);
        }
        DeleteObject(g as HGDIOBJ);
        pt_rect(bdc, &r, acc, 2);
        handles(bdc, r, acc);
        badge(bdc, r, w, h);
    } else {
        hint(bdc, hw);
    }

    draw_bar(bdc, hw, hover);
    ovlog("paint bar");
    SelectClipRgn(bdc, null_mut());
    DeleteObject(clip as HGDIOBJ);
    let ok = BitBlt(hdc, p.left, p.top, p.right - p.left, p.bottom - p.top, bdc, p.left, p.top, SRCCOPY);
    ovlog(&format!("blit={ok} px={:x}", GetPixel(hdc, 10, 10)));
    EndPaint(hw, &ps);
}

unsafe fn hint(dc: HDC, hw: HWND) {
    let mon = ST.with_borrow(|s| s.as_ref().map(|s| s.mon.work).unwrap_or(RECT { left: 0, top: 0, right: 1, bottom: 1 }));
    let s = scale(hw);
    let a = t!(
        "Протяните мышью, чтобы выбрать область · Esc — отмена",
        "Drag to select a region · Esc cancels",
        ja: "ドラッグして範囲を選択 · Esc でキャンセル",
        pt: "Arraste para selecionar uma região · Esc cancela",
        zh: "拖动选择区域 · Esc 取消"
    );
    let b = t!(
        "Win+Alt+R — запись · кнопка внизу — снимок",
        "Win+Alt+R records · the bottom button captures",
        ja: "Win+Alt+R で録画 · 下のボタンでキャプチャ",
        pt: "Win+Alt+R grava · o botão inferior captura",
        zh: "Win+Alt+R 录制 · 底部按钮截图"
    );
    let f = ui::make_font(15 * s, 600, winrust_common::style::ui_face());
    let f2 = ui::make_font(12 * s, 400, winrust_common::style::ui_face());
    let tw = (ui::text_w(dc, a, f).max(ui::text_w(dc, b, f2)) + 30 * s).min((mon.right - mon.left) - 24 * s).max(220 * s);
    let x = mon.left + ((mon.right - mon.left) - tw) / 2;
    let y = mon.top + 24 * s;
    let r = RECT { left: x, top: y, right: x + tw, bottom: y + 54 * s };
    ui::rounded(dc, r, rgb(18, 18, 20), 10 * s);
    pt_rect(dc, &r, pal().accent, 1);
    ui::text(dc, a, RECT { left: x, top: y + 6 * s, right: x + tw, bottom: y + 30 * s }, f, pal().text, DT_CENTER as u32);
    ui::text(dc, b, RECT { left: x, top: y + 29 * s, right: x + tw, bottom: y + 50 * s }, f2, pal().dim, DT_CENTER as u32);
    DeleteObject(f as HGDIOBJ);
    DeleteObject(f2 as HGDIOBJ);
}

unsafe fn draw_bar(dc: HDC, hw: HWND, hover: Option<usize>) {
    let bar = bar_rect(hw);
    let s = scale(hw);
    // shadow-ish dark plate with a hairline and the project radius
    ui::rounded(dc, bar, rgb(22, 22, 24), 12 * s);
    ui::outline(dc, bar, pal().line, 1);
    let (video, kind, delay, has_sel) = ST.with_borrow(|st| {
        st.as_ref().map(|s| (s.video, s.kind, s.delay, s.from.is_some())).unwrap_or((false, Kind::Region, 0, false))
    });
    let f = ui::make_font(11 * s, 600, winrust_common::style::ui_face());
    let fg = ui::make_font(18 * s, 400, winrust_common::style::icon_face());
    let labels = [
        (if video { "\u{E714}" } else { "\u{E722}" }, t!("Фото", "Photo", ja: "写真", pt: "Foto", zh: "照片").to_string(), Act::Photo),
        ("\u{E714}", t!("Видео", "Video", ja: "動画", pt: "Vídeo", zh: "视频").to_string(), Act::Video),
        ("\u{E7A8}", t!("Регион", "Region", ja: "範囲", pt: "Região", zh: "区域").to_string(), Act::Kind(Kind::Region)),
        ("\u{E740}", t!("Экран", "Full", ja: "全画面", pt: "Tela", zh: "全屏").to_string(), Act::Kind(Kind::Full)),
        ("\u{E7F4}", t!("Монитор", "Monitor", ja: "モニター", pt: "Monitor", zh: "显示器").to_string(), Act::Kind(Kind::Monitor)),
        ("\u{E737}", t!("Окно", "Window", ja: "ウィンドウ", pt: "Janela", zh: "窗口").to_string(), Act::Kind(Kind::Window)),
    ];
    for (i, (r, act)) in buttons(hw, bar).into_iter().enumerate() {
        if let Act::Kind(k) = act {
            let is_active = kind == k;
            let bg = if is_active { pal().active } else if hover == Some(i) { pal().hover } else { bar_color() };
            ui::rounded(dc, r, bg, 8 * s);
            if is_active {
                ui::outline(dc, r, pal().accent, 1);
            }
            if let Some((g, lab, _)) = labels.get(i) {
                let col = if is_active { pal().accent } else { pal().text };
                ui::text(dc, g, RECT { left: r.left, top: r.top + 4 * s, right: r.right, bottom: r.top + 28 * s }, fg, col, DT_CENTER as u32);
                ui::text(dc, lab, RECT { left: r.left, top: r.top + 28 * s, right: r.right, bottom: r.bottom - 2 * s }, f, if is_active { pal().accent } else { pal().dim }, DT_CENTER as u32);
            }
            continue;
        }
        if matches!(act, Act::Go) {
            let enabled = kind != Kind::Region || has_sel;
            let col = if enabled { pal().accent } else { pal().line };
            ui::rounded(dc, r, if enabled { col } else { pal().active }, 8 * s);
            let label = if video {
                t!("Записать", "Record", ja: "録画", pt: "Gravar", zh: "录制")
            } else {
                t!("Снять", "Capture", ja: "撮影", pt: "Capturar", zh: "截图")
            };
            ui::text(dc, label, r, f, if enabled { rgb(255, 255, 255) } else { pal().dim }, DT_CENTER as u32);
            continue;
        }
        if matches!(act, Act::Delay) {
            let text = format!("{delay} s");
            ui::rounded(dc, r, if hover == Some(i) { pal().hover } else { bar_color() }, 8 * s);
            ui::text(dc, &text, r, f, pal().text, DT_CENTER as u32);
            continue;
        }
        let (g, lab) = labels.get(i).map(|(g, l, _)| (*g, l.as_str())).unwrap_or(("", ""));
        let is_active = match act {
            Act::Photo => !video,
            Act::Video => video,
            _ => false,
        };
        let bg = if is_active { pal().active } else if hover == Some(i) { pal().hover } else { bar_color() };
        ui::rounded(dc, r, bg, 8 * s);
        if is_active {
            ui::outline(dc, r, pal().accent, 1);
        }
        let col = if is_active { pal().accent } else { pal().text };
        ui::text(dc, g, RECT { left: r.left, top: r.top + 4 * s, right: r.right, bottom: r.top + 28 * s }, fg, col, DT_CENTER as u32);
        ui::text(dc, lab, RECT { left: r.left, top: r.top + 28 * s, right: r.right, bottom: r.bottom - 2 * s }, f, col, DT_CENTER as u32);
    }
    DeleteObject(f as HGDIOBJ);
    DeleteObject(fg as HGDIOBJ);
}

fn bar_color() -> u32 {
    rgb(28, 28, 31)
}

fn ovlog(s: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("rs-shot-overlay.log")) {
        let _ = writeln!(f, "{s}");
    }
}

/// Width of the window that would be picked, in client coordinates (for the hover outline).
unsafe fn hover_window_rect() -> Option<RECT> {
    if selection().is_some() {
        return None;
    }
    let hit = ST.with_borrow(|s| s.as_ref().and_then(|s| s.target))?;
    let (vx, vy) = ST.with_borrow(|s| s.as_ref().map(|s| (s.vx, s.vy)).unwrap_or((0, 0)));
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
    ui::text(dc, &text, box_, f, pal().text, DT_CENTER as u32);
    DeleteObject(f as HGDIOBJ);
    let _ = h;
}

/// Acts from the bar: mode/kind/delay change the live state, Go turns it into a Pick.
unsafe fn act(hw: HWND, a: Act) {
    let mut refresh_bar = true;
    let mut full = false;
    match a {
        Act::Photo => ST.with_borrow_mut(|s| if let Some(s) = s { s.video = false }),
        Act::Video => ST.with_borrow_mut(|s| if let Some(s) = s { s.video = true }),
        Act::Kind(k) => {
            ST.with_borrow_mut(|s| {
                if let Some(s) = s {
                    s.kind = k;
                    if k != Kind::Region {
                        s.from = None;
                    }
                }
            });
            full = true;
            refresh_bar = true;
        }
        Act::Delay => ST.with_borrow_mut(|s| {
            if let Some(s) = s {
                let i = DELAYS.iter().position(|d| *d == s.delay).unwrap_or(0);
                s.delay = DELAYS[(i + 1) % DELAYS.len()];
            }
        }),
        Act::Go => {
            go(hw);
            return;
        }
    }
    if full {
        InvalidateRect(hw, null(), 0);
    } else if refresh_bar {
        inval(hw, bar_rect(hw));
    }
}

unsafe fn go(hw: HWND) {
    let mut p = POINT { x: 0, y: 0 };
    GetCursorPos(&mut p);
    let (kind, video, delay, sel, vx, vy) = ST.with_borrow(|s| {
        s.as_ref().map(|s| (s.kind, s.video, s.delay, s.from.map(|f| client_rect(f, s.to)), s.vx, s.vy)).unwrap_or((Kind::Region, false, 0, None, 0, 0))
    });
    let (rect, window) = match kind {
        Kind::Region => {
            let Some(r) = sel else { return };
            (RECT { left: r.left + vx, top: r.top + vy, right: r.right + vx, bottom: r.bottom + vy }, None)
        }
        Kind::Full => (cap::full_rect(), None),
        Kind::Monitor => (monitors::at(p.x, p.y).rc, None),
        Kind::Window => {
            let target = ST.with_borrow(|s| s.as_ref().and_then(|s| s.target));
            let h = target.or_else(|| {
                let f = cap::foreground();
                (!f.is_null()).then_some(f)
            });
            let Some(h) = h else { return };
            (cap::window_rect(h), Some(h))
        }
    };
    ST.with_borrow_mut(|s| {
        if let Some(s) = s {
            s.pick = Some(PickData { rect, window, video, kind, delay });
        }
    });
    DestroyWindow(hw);
}

unsafe fn cancel(hw: HWND) {
    DestroyWindow(hw);
}

fn rect_eq(a: RECT, b: RECT) -> bool {
    a.left == b.left && a.top == b.top && a.right == b.right && a.bottom == b.bottom
}

fn same_opt_rect(a: Option<RECT>, b: Option<RECT>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => rect_eq(a, b),
        (None, None) => true,
        _ => false,
    }
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
            let old_sel = selection();
            let old_bar = bar_rect(hw);
            let mut m = POINT { x: 0, y: 0 };
            GetCursorPos(&mut m);
            let new_mon = monitors::at(m.x, m.y);
            ST.with_borrow_mut(|s| {
                if let Some(s) = s {
                    if s.mon.handle != new_mon.handle {
                        s.mon = new_mon;
                    }
                    if s.from.is_some() {
                        s.to = (x, y);
                    }
                }
            });
            let new_sel = selection();
            let new_bar = bar_rect(hw);
            let hover = buttons(hw, new_bar).iter().position(|(r, _)| hits(*r, x, y));
            ST.with_borrow_mut(|s| {
                if let Some(s) = s {
                    s.hover = hover;
                }
            });
            // Keep the window under the last cursor position that was not over the bar, so Capture/Record on a
            // Window source does not pick whatever sits behind the button.
            if !hits(new_bar, x, y) && new_sel.is_none() {
                let t = cap::window_at(m.x, m.y);
                ST.with_borrow_mut(|s| {
                    if let Some(s) = s {
                        s.target = t;
                    }
                });
            }
            if let (Some(a), Some(b)) = (old_sel, new_sel) {
                inval(hw, expanded(union(a, b), 110));
            } else if !same_opt_rect(old_sel, new_sel) {
                InvalidateRect(hw, null(), 0);
            } else {
                // hover window outline, only the changed rects
                let hr = hover_window_rect();
                let old_hover = ST.with_borrow(|s| s.as_ref().and_then(|s| s.last_hover));
                if !same_opt_rect(old_hover, hr) {
                    if let Some(r) = old_hover {
                        inval(hw, expanded(r, 3));
                    }
                    if let Some(r) = hr {
                        inval(hw, expanded(r, 3));
                    }
                    ST.with_borrow_mut(|s| {
                        if let Some(s) = s {
                            s.last_hover = hr;
                        }
                    });
                }
            }
            let bar_changed = !rect_eq(old_bar, new_bar);
            if bar_changed {
                inval(hw, union(old_bar, new_bar));
            } else {
                inval(hw, new_bar);
            }
            0
        }
        WM_LBUTTONDOWN => {
            // The bar takes precedence over starting a selection.
            let bar = bar_rect(hw);
            if hits(bar, x, y) {
                if let Some((_, a)) = buttons(hw, bar).into_iter().find(|(r, _)| hits(*r, x, y)) {
                    act(hw, a);
                }
                return 0;
            }
            let kind = ST.with_borrow(|s| s.as_ref().map(|s| s.kind).unwrap_or(Kind::Region));
            if kind == Kind::Region {
                ST.with_borrow_mut(|s| {
                    if let Some(s) = s {
                        s.from = Some((x, y));
                        s.to = (x, y);
                    }
                });
                InvalidateRect(hw, null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            // Selection stays on screen; the Capture/Record button finishes it.
            if let Some(sel) = selection() {
                if sel.right - sel.left < 3 && sel.bottom - sel.top < 3 {
                    let mut p = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut p);
                    if let Some(hit) = cap::window_at(p.x, p.y) {
                        let (vx, vy) = ST.with_borrow(|s| s.as_ref().map(|s| (s.vx, s.vy)).unwrap_or((0, 0)));
                        let r = cap::window_rect(hit);
                        let cr = RECT { left: r.left - vx, top: r.top - vy, right: r.right - vx, bottom: r.bottom - vy };
                        ST.with_borrow_mut(|s| {
                            if let Some(s) = s {
                                s.from = Some((cr.left, cr.top));
                                s.to = (cr.right, cr.bottom);
                            }
                        });
                    }
                }
                InvalidateRect(hw, null(), 0);
            }
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
            let bar = bar_rect(hw);
            let kind = ST.with_borrow(|s| s.as_ref().map(|s| s.kind).unwrap_or(Kind::Region));
            if kind != Kind::Region || selection().is_some() {
                go(hw);
            } else {
                let _ = bar;
            }
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hw, msg, wp, lp),
    }
}
