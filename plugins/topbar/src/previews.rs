//! Live DWM window previews. Hover never activates the application; selecting
//! a card restores that exact window. Thumbnail handles live only with the popup.
use super::*;
use windows_sys::Win32::Graphics::Dwm::*;

struct Preview {
    hwnd: HWND,
    button: i32,
    wins: Vec<HWND>,
    thumbs: Vec<isize>,
    page: usize,
    cols: usize,
    cell_w: i32,
    hot: i32,
    away: u64,
}
thread_local! {
    static STATE: RefCell<Option<Preview>> = const { RefCell::new(None) };
}

pub fn is_open() -> bool { STATE.with_borrow(|s| s.is_some()) }
pub fn button() -> i32 { STATE.with_borrow(|s| s.as_ref().map_or(NONE, |s| s.button)) }

pub unsafe fn close() {
    let old = STATE.with_borrow_mut(Option::take);
    if let Some(s) = old {
        for h in s.thumbs { if h != 0 { DwmUnregisterThumbnail(h); } }
        DestroyWindow(s.hwnd);
    }
}

fn page_columns(count: usize, screen: i32, preferred: i32, padding: i32) -> usize {
    count.min(((screen - padding * 2) / preferred.max(1)).max(1) as usize).max(1)
}

pub(super) fn fit(bounds: RECT, width: i32, height: i32) -> RECT {
    let bw = (bounds.right - bounds.left).max(1);
    let bh = (bounds.bottom - bounds.top).max(1);
    let ratio = (bw as f64 / width.max(1) as f64).min(bh as f64 / height.max(1) as f64);
    let w = (width.max(1) as f64 * ratio).round() as i32;
    let h = (height.max(1) as f64 * ratio).round() as i32;
    let left = bounds.left + (bw - w) / 2;
    let top = bounds.top + (bh - h) / 2;
    RECT { left, top, right: left + w, bottom: top + h }
}

fn card_rect(s: &Preview, i: usize) -> RECT {
    let x = sc(8) + i as i32 * s.cell_w;
    RECT { left: x, top: sc(8), right: x + s.cell_w - sc(6), bottom: sc(186) }
}
fn image_rect(r: RECT) -> RECT {
    RECT { left: r.left + sc(6), top: r.top + sc(36), right: r.right - sc(6), bottom: r.bottom - sc(7) }
}
fn contains(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

pub unsafe fn show(button: i32) -> bool {
    if button < 1 { return false; }
    let wins = BTNS.with_borrow(|b| b.get(button as usize).map(|b| b.wins.iter().copied().filter(|&w| IsWindow(w) != 0).collect::<Vec<_>>()).unwrap_or_default());
    if wins.is_empty() { return false; }
    if self::button() == button { return true; }
    close();
    hide_tip(BAR.load(Relaxed) as HWND);
    let class = wide("WinRustWindowPreviews");
    let hi = GetModuleHandleW(null());
    RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), hInstance: hi, hCursor: LoadCursorW(null_mut(), IDC_ARROW), lpszClassName: class.as_ptr(), ..zeroed() });
    let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
        class.as_ptr(), wide("Window previews").as_ptr(), WS_POPUP | WS_CLIPCHILDREN,
        0, 0, 1, 1, BAR.load(Relaxed) as HWND, null_mut(), hi, null());
    if hwnd.is_null() { return false; }
    STATE.with_borrow_mut(|s| *s = Some(Preview { hwnd, button, wins, thumbs: vec![], page: 0, cols: 1, cell_w: sc(240), hot: -1, away: 0 }));
    rebuild();
    SetTimer(hwnd, 1, 100, None);
    true
}

unsafe fn rebuild() {
    STATE.with_borrow_mut(|state| {
        let Some(s) = state else { return; };
        for h in s.thumbs.drain(..) { if h != 0 { DwmUnregisterThumbnail(h); } }
        let b = monitors::bounds();
        let sw = b.width();
        s.cols = page_columns(s.wins.len(), sw, sc(240), sc(12));
        s.cell_w = sc(240).min((sw - sc(24)) / s.cols as i32).max(1);
        s.page = s.page.min((s.wins.len() - 1) / s.cols);
        s.hot = -1;
        let visible = (s.wins.len() - s.page * s.cols).min(s.cols);
        let width = visible as i32 * s.cell_w + sc(10);
        let height = sc(if s.wins.len() > s.cols { 222 } else { 194 });
        let center = tip_cx(&bar_layout(BAR.load(Relaxed) as HWND), s.button);
        let x = if vert() { side_x(width) } else { (center - width / 2).clamp(b.left + sc(4), (b.right - width - sc(4)).max(b.left + sc(4))) };
        let y = if vert() { (center - height / 2).clamp(b.top + sc(4), (b.bottom - height - sc(4)).max(b.top + sc(4))) } else { pop_y(height) };
        SetWindowPos(s.hwnd, HWND_TOPMOST, x, y, width, height, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        for (i, &source) in s.wins.iter().skip(s.page * s.cols).take(s.cols).enumerate() {
            let mut handle = 0;
            if DwmRegisterThumbnail(s.hwnd, source, &mut handle) >= 0 {
                let mut size: SIZE = zeroed();
                DwmQueryThumbnailSourceSize(handle, &mut size);
                let r = fit(image_rect(card_rect(s, i)), size.cx, size.cy);
                let props = DWM_THUMBNAIL_PROPERTIES {
                    dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
                    rcDestination: r, opacity: 255, fVisible: 1, fSourceClientAreaOnly: 0, ..zeroed()
                };
                if DwmUpdateThumbnailProperties(handle, &props) < 0 {
                    DwmUnregisterThumbnail(handle);
                    handle = 0;
                }
            }
            s.thumbs.push(handle);
        }
        InvalidateRect(s.hwnd, null(), 0);
    });
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let dc = BeginPaint(hwnd, &mut ps);
    let mut r: RECT = zeroed();
    GetClientRect(hwnd, &mut r);
    fill(dc, r, pal().bg, 0);
    let font = CreateFontW(-sc(12), 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, wide(ui_face()).as_ptr());
    let old = SelectObject(dc, font);
    SetBkMode(dc, TRANSPARENT as i32);
    STATE.with_borrow(|state| {
        let Some(s) = state else { return; };
        for (i, &w) in s.wins.iter().skip(s.page * s.cols).take(s.cols).enumerate() {
            let cr = card_rect(s, i);
            fill(dc, cr, if s.hot == i as i32 { pal().hover } else { pal().bg }, sc(7));
            let mut title = [0u16; 512];
            let n = GetWindowTextW(w, title.as_mut_ptr(), 512).max(0) as usize;
            text(dc, &String::from_utf16_lossy(&title[..n]), RECT { left: cr.left + sc(9), top: cr.top + sc(7), right: cr.right - sc(32), bottom: cr.top + sc(31) }, pal().text, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX);
            text(dc, "×", RECT { left: cr.right - sc(30), top: cr.top + sc(4), right: cr.right - sc(3), bottom: cr.top + sc(31) }, pal().text, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            // DWM owns the live image. Unavailable/protected windows still have
            // a selectable card and title; no blocking PrintWindow capture.
            if s.thumbs.get(i).copied().unwrap_or(0) == 0 {
                text(dc, t!("Предпросмотр недоступен", "Preview unavailable"), image_rect(cr), pal().dim, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            }
        }
        if s.wins.len() > s.cols {
            let pages = s.wins.len().div_ceil(s.cols);
            text(dc, &format!("‹  {} / {}  ›   {}", s.page + 1, pages, t!("Колесо мыши — другие окна", "Scroll for more windows")), RECT { left: 0, top: sc(190), right: r.right, bottom: r.bottom }, pal().dim, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }
    });
    SelectObject(dc, old);
    DeleteObject(font);
    EndPaint(hwnd, &ps);
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_ERASEBKGND => 1,
        WM_PAINT => { paint(hwnd); 0 }
        WM_TIMER => {
            let mut pt: POINT = zeroed(); GetCursorPos(&mut pt);
            let mut rect: RECT = zeroed(); GetWindowRect(hwnd, &mut rect);
            let mut barpt = pt; let bar = BAR.load(Relaxed) as HWND;
            ScreenToClient(bar, &mut barpt);
            let group = hit(bar, barpt.x, barpt.y);
            let mut br: RECT = zeroed(); GetWindowRect(bar, &mut br);
            let now = now_ms();
            let (dismiss, changed) = STATE.with_borrow_mut(|state| {
                let Some(s) = state else { return (false, false); };
                let over = contains(rect, pt.x, pt.y) || (contains(br, pt.x, pt.y) && group == s.button);
                if over { s.away = 0; } else if s.away == 0 { s.away = now; }
                let current = BTNS.with_borrow(|b| b.get(s.button as usize).map(|b| b.wins.clone()).unwrap_or_default());
                let mut fresh = current.into_iter().filter(|&w| IsWindow(w) != 0).collect::<Vec<_>>();
                let same = fresh.len() == s.wins.len() && fresh.iter().all(|w| s.wins.contains(w));
                let dismiss = fresh.is_empty() || !fresh.iter().any(|w| s.wins.contains(w)) || fly::any_open() || (s.away != 0 && now.saturating_sub(s.away) > 400);
                if !same && !dismiss {
                    // Keep card order stable while the system's z-order changes.
                    s.wins.retain(|w| fresh.contains(w));
                    fresh.retain(|w| !s.wins.contains(w));
                    s.wins.extend(fresh);
                }
                (dismiss, !same)
            });
            if dismiss { close(); } else if changed { rebuild(); }
            0
        }
        WM_MOUSEMOVE => {
            let x = (lp & 0xffff) as i16 as i32;
            let y = (lp >> 16) as i16 as i32;
            STATE.with_borrow_mut(|state| { if let Some(s) = state {
                let hot = (0..s.thumbs.len()).find(|&i| contains(card_rect(s, i), x, y)).map_or(-1, |i| i as i32);
                if s.hot != hot { s.hot = hot; InvalidateRect(hwnd, null(), 0); }
            }});
            0
        }
        WM_MOUSEWHEEL => {
            STATE.with_borrow_mut(|state| { if let Some(s) = state {
                let last = (s.wins.len() - 1) / s.cols;
                s.page = if ((wp >> 16) as i16) < 0 { (s.page + 1).min(last) } else { s.page.saturating_sub(1) };
            }});
            rebuild(); 0
        }
        WM_LBUTTONUP => {
            let x = (lp & 0xffff) as i16 as i32;
            let y = (lp >> 16) as i16 as i32;
            let selected = STATE.with_borrow(|state| state.as_ref().and_then(|s| {
                (0..s.thumbs.len()).find(|&i| contains(card_rect(s, i), x, y)).map(|i| (s.wins[s.page * s.cols + i], x >= card_rect(s, i).right - sc(32) && y < sc(43)))
            }));
            if let Some((w, closing)) = selected {
                if IsWindow(w) != 0 {
                    if closing { close_window(w); } else {
                        close();
                        if IsIconic(w) != 0 { ShowWindow(w, SW_RESTORE); }
                        force_fg(w);
                    }
                }
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_popup_preserves_focus_handles_closed_windows_and_releases_dwm() {
        let _guard = NATIVE_TEST.lock().unwrap();
        unsafe {
            struct TestWindows(Vec<HWND>);
            impl Drop for TestWindows {
                fn drop(&mut self) { unsafe {
                    close();
                    for &w in &self.0 { if IsWindow(w) != 0 { DestroyWindow(w); } }
                    BAR.store(0, Relaxed);
                    BTNS.set(vec![]);
                }}
            }
            let class = wide("WinRustPreviewTestSource");
            let hi = GetModuleHandleW(null());
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), hInstance: hi, lpszClassName: class.as_ptr(), ..zeroed() });
            let mut owned = TestWindows(vec![]);
            for title in ["Test bar", "First test window", "Second test window"] {
                let w = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), wide(title).as_ptr(), WS_OVERLAPPEDWINDOW,
                    50, 50, 640, 480, null_mut(), null_mut(), hi, null());
                assert!(!w.is_null());
                owned.0.push(w);
            }
            let bar = owned.0[0];
            BAR.store(bar as isize, Relaxed);
            BTNS.set(vec![Btn { pin: None, wins: vec![], icon: null_mut() }, Btn { pin: None, wins: owned.0[1..].to_vec(), icon: null_mut() }]);
            let foreground = GetForegroundWindow();
            SWWND.set(bar);
            SW.set((owned.0[1..].to_vec(), 1));
            update_switcher_preview();
            assert_ne!(SW_THUMB.get(), 0, "Alt+Tab shows a live selected-window thumbnail");
            alt_done(true);
            assert_eq!(SW_THUMB.get(), 0, "cancelling Alt+Tab releases its thumbnail");
            SWWND.set(null_mut());
            assert!(show(1));
            assert_eq!(GetForegroundWindow(), foreground, "hover must not take keyboard focus");
            let (popup, handles) = STATE.with_borrow(|s| { let s = s.as_ref().unwrap(); (s.hwnd, s.thumbs.clone()) });
            assert_ne!(IsWindowVisible(popup), 0);
            assert_eq!(handles.len(), 2);
            for &h in &handles {
                assert_ne!(h, 0, "DWM must provide a live thumbnail");
                let mut size: SIZE = zeroed();
                assert!(DwmQueryThumbnailSourceSize(h, &mut size) >= 0);
                assert!(size.cx > 0 && size.cy > 0);
            }
            ShowWindow(owned.0[1], SW_SHOWMINNOACTIVE);
            ShowWindow(owned.0[2], SW_SHOWMINNOACTIVE);
            // Click the second thumbnail, away from its close button.
            SendMessageW(popup, WM_LBUTTONUP, 0, ((sc(100) as isize) << 16) | sc(360) as isize);
            assert!(!is_open());
            assert_eq!(IsIconic(owned.0[2]), 0, "selection restores the chosen window");
            assert_ne!(IsIconic(owned.0[1]), 0, "selection leaves the other window minimized");
            if !foreground.is_null() { SetForegroundWindow(foreground); }
            assert!(show(1));
            let popup = STATE.with_borrow(|s| s.as_ref().unwrap().hwnd);
            // A source can disappear while the popup is open; refreshing removes
            // it without leaving stale cards or invalid indexing on the last page.
            DestroyWindow(owned.0[2]);
            SendMessageW(popup, WM_TIMER, 1, 0);
            if is_open() { // cursor outside may legitimately dismiss the popup
                STATE.with_borrow(|s| assert_eq!(s.as_ref().unwrap().wins.len(), 1));
            }
            close();
            assert_eq!(IsWindow(popup), 0);
            for h in handles {
                let mut size: SIZE = zeroed();
                assert!(DwmQueryThumbnailSourceSize(h, &mut size) < 0, "closed popups must release thumbnail handles");
            }
        }
    }
    #[test]
    fn pages_fit_small_and_large_displays() {
        assert_eq!(page_columns(2, 1920, 240, 12), 2);
        assert_eq!(page_columns(20, 1920, 240, 12), 7);
        assert_eq!(page_columns(20, 320, 240, 12), 1);
        assert_eq!(page_columns(1, 0, 0, 12), 1);
    }
    #[test]
    fn thumbnails_keep_aspect_and_stay_inside_card() {
        let b = RECT { left: 10, top: 20, right: 230, bottom: 160 };
        for (w, h) in [(1920, 1080), (800, 1600), (0, 0), (3840, 1080)] {
            let r = fit(b, w, h);
            assert!(r.left >= b.left && r.top >= b.top && r.right <= b.right && r.bottom <= b.bottom);
            assert!(r.right > r.left && r.bottom > r.top);
            if w > 0 && h > 0 { assert!(((r.right-r.left) as f64 / (r.bottom-r.top) as f64 - w as f64 / h as f64).abs() < 0.1); }
        }
    }
}
