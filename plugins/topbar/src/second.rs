//! topbar: a bar on every other monitor (#124), like Windows 11's: Start, the same pins and windows, the clock. The tray and
//! the system group stay on the primary one.
//! shortcut: always horizontal (a side bar or the dock is a plain strip here), the primary monitor's scale, no tooltips,
//! previews or app menus; a button with several windows steps through them.
use super::*;
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, HMONITOR};

thread_local! {
    static BARS: RefCell<Vec<(HWND, [i32; 4])>> = const { RefCell::new(Vec::new()) }; // window and its monitor (l, t, r, b)
    static HOT: Cell<(HWND, i32)> = const { Cell::new((null_mut(), NONE)) };
}

fn off_marker() -> PathBuf {
    appdata_file("topbar-primary-only.txt")
}

pub(crate) fn enabled() -> bool {
    !off_marker().exists()
}

pub(crate) fn set_enabled(on: bool) {
    let _ = if on { std::fs::remove_file(off_marker()) } else { std::fs::write(off_marker(), "1") };
}

fn bars() -> Vec<(HWND, [i32; 4])> {
    BARS.with_borrow(|b| b.clone())
}

fn rect([left, top, right, bottom]: [i32; 4]) -> RECT {
    RECT { left, top, right, bottom }
}

/// Every monitor but the primary one.
unsafe fn others() -> Vec<[i32; 4]> {
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> i32 {
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(m, &mut mi) != 0 && mi.dwFlags & 1 == 0 {
            // MONITORINFOF_PRIMARY
            let r = mi.rcMonitor;
            (*(lp as *mut Vec<[i32; 4]>)).push([r.left, r.top, r.right, r.bottom]);
        }
        1
    }
    let mut v: Vec<[i32; 4]> = Vec::new();
    EnumDisplayMonitors(null_mut(), null(), Some(each), &mut v as *mut _ as LPARAM);
    v
}

fn height() -> i32 {
    if vert() { sc(BAR_H) } else { bar_h() }
}

/// The bar's rectangle on a monitor.
fn bar_rect(m: [i32; 4], h: i32, top: bool) -> [i32; 4] {
    if top { [m[0], m[1], m[2], m[1] + h] } else { [m[0], m[3] - h, m[2], m[3]] }
}

/// What stays for maximized windows when the bar takes `strip` pixels.
fn work(m: [i32; 4], strip: i32, top: bool) -> [i32; 4] {
    if top { [m[0], m[1] + strip, m[2], m[3]] } else { [m[0], m[1], m[2], m[3] - strip] }
}

/// Creates, moves or removes the bars to match the monitors and settings. Called whenever the main bar is placed.
pub(crate) unsafe fn sync() {
    let want = if enabled() { others() } else { Vec::new() };
    if bars().iter().map(|b| b.1).ne(want.iter().copied()) {
        release();
        let hi = GetModuleHandleW(null());
        let class = wide("topbar_second");
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), hInstance: hi, hCursor: LoadCursorW(null_mut(), IDC_ARROW), lpszClassName: class.as_ptr(), ..zeroed() });
        let made = want.iter().map(|&m| (CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED, class.as_ptr(), null(), WS_POPUP, m[0], m[1], 1, 1, null_mut(), null_mut(), hi, null()), m)).collect();
        BARS.set(made);
    }
    for (w, m) in bars() {
        apply_opacity(w);
        let b = bar_rect(m, height(), at_top());
        SetWindowPos(w, HWND_TOPMOST, b[0], b[1], b[2] - b[0], b[3] - b[1], SWP_NOACTIVATE);
        set_work_area(rect(work(m, if i18n::style::autohide() { 0 } else { height() }, at_top())));
        InvalidateRect(w, null(), 0);
    }
    update(BAR.load(Relaxed) as HWND);
}

/// Hidden under a fullscreen window of its monitor, and with auto-hide while the pointer is away.
pub(crate) unsafe fn update(main: HWND) {
    let list = bars();
    if list.is_empty() {
        return;
    }
    let covered = fg_covers(main).map(|m| [m.left, m.top, m.right, m.bottom]);
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    for (w, m) in list {
        let shown = IsWindowVisible(w) != 0;
        let b = bar_rect(m, height(), at_top());
        let edge = if at_top() { pt.y <= m[1] } else { pt.y >= m[3] - 1 };
        let over = pt.x >= b[0] && pt.x < b[2] && (edge || (shown && pt.y >= b[1] && pt.y < b[3]));
        let show = covered != Some(m) && (!i18n::style::autohide() || over || fly::any_open());
        if show != shown {
            if show {
                SetWindowPos(w, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            } else {
                ShowWindow(w, SW_HIDE);
            }
        }
    }
}

pub(crate) unsafe fn invalidate() {
    for (w, _) in bars() {
        InvalidateRect(w, null(), 0);
    }
}

/// Removes the bars and gives their strips back.
pub(crate) unsafe fn release() {
    for (w, _) in BARS.take() {
        DestroyWindow(w);
    }
    restore_work_areas();
}

/// Also for `--restore`: a killed bar's strips are still reserved.
pub(crate) unsafe fn restore_work_areas() {
    for m in others() {
        set_work_area(rect(m));
    }
}

/// (left of the first button, button width) for `n` buttons on a bar `w` wide with `side` kept free at both ends.
fn row(w: i32, n: i32, gap: i32, max: i32, side: i32, left: bool) -> (i32, i32) {
    let n = n.max(1);
    let room = w - 2 * side;
    let bw = ((room + gap) / n - gap).clamp(max / 3, max);
    let used = n * (bw + gap) - gap;
    (if left { gap } else { ((w - used) / 2).max(gap) }, bw)
}

fn clock_w() -> i32 {
    sc(96)
}

unsafe fn geometry(width: i32) -> (i32, i32, i32) {
    let n = BTNS.with_borrow(|b| b.len()) as i32;
    let (x0, bw) = row(width, n, sc(4), sc(44), clock_w(), i18n::style::align_left());
    (x0, bw, n)
}

/// CLOCK, a button index (0 = Start) or NONE.
unsafe fn hit(w: HWND, x: i32) -> i32 {
    let mut r: RECT = zeroed();
    GetClientRect(w, &mut r);
    if x >= r.right - clock_w() {
        return CLOCK;
    }
    let (x0, bw, n) = geometry(r.right);
    let (d, step) = (x - x0, bw + sc(4));
    if d < 0 || d % step >= bw || d / step >= n { NONE } else { d / step }
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let wdc = BeginPaint(hwnd, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(hwnd, &mut cr);
    let (dc, bmp) = (CreateCompatibleDC(wdc), CreateCompatibleBitmap(wdc, cr.right, cr.bottom));
    let old = SelectObject(dc, bmp);
    SetBkMode(dc, TRANSPARENT as i32);
    let [f_txt, f_small, _, f_bold] = bar_fonts();
    let of = SelectObject(dc, f_txt);
    fill(dc, cr, pal().bg, 0);
    let edge = if at_top() { cr.bottom - 1 } else { 0 };
    fill(dc, RECT { left: 0, top: edge, right: cr.right, bottom: edge + 1 }, pal().line, 0);

    let (top, bot) = (sc(4), cr.bottom - sc(4));
    let mid = (top + bot) / 2;
    let hot = if HOT.get().0 == hwnd { HOT.get().1 } else { NONE };
    let fg = GetForegroundWindow();
    let (x0, bw, _) = geometry(cr.right);
    BTNS.with_borrow(|btns| {
        for (i, b) in btns.iter().enumerate() {
            let x = x0 + i as i32 * (bw + sc(4));
            let r = RECT { left: x, top, right: x + bw, bottom: bot };
            let act = b.wins.contains(&fg);
            let att = !act && FLASH.with_borrow(|f| b.wins.iter().any(|w| f.contains(w)));
            if att {
                fill(dc, r, mix(pal().bg, pal().accent, 0.4), sc(8));
            } else if act {
                fill(dc, r, mix(pal().active, pal().accent, 0.12), sc(8));
            } else if hot == i as i32 {
                fill(dc, r, pal().hover, sc(8));
            }
            if i == 0 {
                let (t, g, cx) = (sc(8), sc(1).max(1), (r.left + r.right) / 2);
                for (dx, dy) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
                    let (px, py) = (cx + if dx < 0 { -t - g } else { g }, mid + if dy < 0 { -t - g } else { g });
                    fill(dc, RECT { left: px, top: py, right: px + t, bottom: py + t }, pal().accent, sc(3));
                }
                continue;
            }
            let ic = match b.pin {
                Some(p) => PINS.with_borrow(|pins| pins.get(p).map_or(null_mut(), |p| p.icon)),
                None => b.icon,
            };
            let ic = if ic.is_null() { b.wins.first().map_or(ic, |&w| icon_of(w)) } else { ic };
            let isz = icon_px().min(bw - sc(6));
            if !ic.is_null() {
                DrawIconEx(dc, x + (bw - isz) / 2, mid - isz / 2 - sc(2), ic, isz, isz, 0, null_mut(), DI_NORMAL);
            }
            if !b.wins.is_empty() {
                let pw = if act { sc(18) } else { sc(6) };
                fill(dc, RECT { left: x + (bw - pw) / 2, top: bot - sc(4), right: x + (bw + pw) / 2, bottom: bot - sc(1) }, if act || att { pal().accent } else { pal().dim }, sc(3));
            }
        }
    });

    let cb = RECT { left: cr.right - clock_w(), top, right: cr.right - sc(6), bottom: bot };
    if hot == CLOCK {
        fill(dc, cb, pal().hover, sc(8));
    }
    let (tm, dt) = clock_texts();
    SelectObject(dc, f_bold);
    text(dc, &tm, RECT { bottom: mid + sc(1), ..cb }, pal().text, DT_CENTER);
    SelectObject(dc, f_small);
    text(dc, &dt, RECT { top: mid - sc(1), ..cb }, mix(pal().dim, pal().text, 0.35), DT_CENTER);

    BitBlt(wdc, 0, 0, cr.right, cr.bottom, dc, 0, 0, SRCCOPY);
    SelectObject(dc, of);
    SelectObject(dc, old);
    DeleteObject(bmp);
    DeleteDC(dc);
    EndPaint(hwnd, &ps);
}

/// Start and the calendar open next to this bar instead of the primary one.
unsafe fn panel(hwnd: HWND, kind: fly::Kind) {
    fly::toggle_click(kind);
    let mut b: RECT = zeroed();
    GetWindowRect(hwnd, &mut b);
    fly::move_to_bar(kind, b, at_top());
}

unsafe fn press_button(hwnd: HWND, i: i32, msg: u32) {
    match i {
        CLOCK if msg == WM_LBUTTONDOWN => panel(hwnd, fly::Kind::Cal),
        0 if msg == WM_LBUTTONDOWN => panel(hwnd, fly::Kind::Start),
        1.. => {
            let wins = BTNS.with_borrow(|b| b.get(i as usize).map(|b| b.wins.clone())).unwrap_or_default();
            let at = wins.iter().position(|&w| w == GetForegroundWindow());
            match msg {
                // several windows: the next one each click (the list menu belongs to the primary bar)
                WM_LBUTTONDOWN if wins.len() > 1 => {
                    let w = wins[at.map_or(0, |a| (a + 1) % wins.len())];
                    if IsIconic(w) != 0 {
                        ShowWindow(w, SW_RESTORE);
                    }
                    SetForegroundWindow(w);
                }
                WM_LBUTTONDOWN => click(i, true, false),
                WM_MBUTTONDOWN => click(i, false, true),
                _ => {}
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp & 0xffff) as i16 as i32;
    match msg {
        WM_PAINT => paint(hwnd),
        WM_ERASEBKGND => return 1,
        WM_MOUSEMOVE => {
            let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut tme);
            let now = (hwnd, hit(hwnd, x));
            if HOT.replace(now) != now {
                InvalidateRect(hwnd, null(), 0);
            }
        }
        WM_MOUSELEAVE => {
            HOT.set((null_mut(), NONE));
            InvalidateRect(hwnd, null(), 0);
        }
        WM_LBUTTONDOWN | WM_MBUTTONDOWN => {
            press_button(hwnd, hit(hwnd, x), msg);
            InvalidateRect(hwnd, null(), 0);
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_at_the_monitors_edge() {
        let m = [1920, -200, 3840, 880]; // right of the primary, higher up
        assert_eq!(bar_rect(m, 40, false), [1920, 840, 3840, 880]);
        assert_eq!(bar_rect(m, 40, true), [1920, -200, 3840, -160]);
        assert_eq!(work(m, 40, false), [1920, -200, 3840, 840]);
        assert_eq!(work(m, 0, true), m); // auto-hide reserves nothing
    }

    #[test]
    fn buttons_centered_and_shrinking() {
        assert_eq!(row(1000, 3, 4, 44, 96, false), (430, 44));
        assert_eq!(row(1000, 3, 4, 44, 96, true), (4, 44));
        let (x0, bw) = row(1000, 40, 4, 44, 96, false);
        assert!(bw < 44 && x0 >= 96 && x0 + 40 * (bw + 4) <= 1000 - 96 + 4);
    }
}
