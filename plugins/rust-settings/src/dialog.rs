//! A Windows 11 style dark dialog (WinUI's ContentDialog look): title, text, and a bar of equal-width
//! buttons at the bottom, the default one in the accent colour. Replaces the classic grey MessageBox.
//!
//! Modal like MessageBox: `ask` returns the index of the button pressed, or None for Esc / the close key.

use std::cell::RefCell;

use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::flyout::{rect, utf16};
use crate::quick::rgb;

const W: f32 = 448.0;
const PAD: f32 = 24.0;
const TITLE_H: f32 = 28.0;
const BTN_H: f32 = 32.0;
const BAR_H: f32 = BTN_H + 2.0 * PAD;
const GAP: f32 = 8.0;
const EDIT_H: f32 = 32.0; // the text box of ask_text (WinUI TextBox: 32 tall, 4 rounded, accent underline)

#[derive(Default)]
struct St {
    title: String,
    body: String,
    buttons: Vec<String>,
    default: usize,
    focus: usize,
    keys: bool, // a key was used: show the focus ring (Windows shows it only for the keyboard)
    hover: Option<usize>,
    down: Option<usize>,
    done: Option<Option<usize>>,
    edit: isize,  // ask_text's text box (a child EDIT), 0 for none
    edit_y: f32,  // its top (DIPs)
    rt: Option<ID2D1DCRenderTarget>,
    dw: Option<IDWriteFactory>,
}

thread_local!(static ST: RefCell<Option<St>> = const { RefCell::new(None) });

/// The buttons' rectangles (DIPs) in a dialog `h` tall: they share the bar's width equally.
pub fn button_rects(n: usize, h: f32) -> Vec<D2D_RECT_F> {
    let n = n.max(1) as f32;
    let bw = (W - 2.0 * PAD - GAP * (n - 1.0)) / n;
    (0..n as usize).map(|i| {
        let x = PAD + i as f32 * (bw + GAP);
        rect(x, h - PAD - BTN_H, x + bw, h - PAD)
    }).collect()
}

fn height(body_h: f32, edit: bool) -> f32 {
    PAD + TITLE_H + 12.0 + body_h + if edit { 12.0 + EDIT_H } else { 0.0 } + PAD + BAR_H
}

unsafe fn body_layout(dw: &IDWriteFactory, body: &str) -> windows::core::Result<IDWriteTextLayout> {
    let f = dw.CreateTextFormat(w!("Segoe UI Variable Text"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 14.0, w!("en-us"))?;
    f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
    dw.CreateTextLayout(&utf16(body), &f, W - 2.0 * PAD, 2000.0)
}

unsafe fn draw(st: &St, h: f32) -> windows::core::Result<()> {
    let rt = st.rt.clone().unwrap();
    let dw = st.dw.clone().unwrap();
    let b = |c: u32| rt.CreateSolidColorBrush(&rgb(c), None);
    rt.Clear(Some(&rgb(0x2B2B2B)));
    // the button bar is a shade darker, with a hairline above it
    rt.FillRectangle(&rect(0.0, h - BAR_H, W, h), &b(0x202020)?);
    rt.FillRectangle(&rect(0.0, h - BAR_H, W, h - BAR_H + 1.0), &b(0x1D1D1D)?);
    let title = dw.CreateTextFormat(w!("Segoe UI Variable Display"), None, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 20.0, w!("en-us"))?;
    title.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
    rt.DrawText(&utf16(&st.title), &title, &rect(PAD, PAD - 2.0, W - PAD, PAD + TITLE_H), &b(0xFFFFFF)?, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    let body = body_layout(&dw, &st.body)?;
    rt.DrawTextLayout(windows_numerics::Vector2 { X: PAD, Y: PAD + TITLE_H + 12.0 }, &body, &b(0xFFFFFF)?, D2D1_DRAW_TEXT_OPTIONS_NONE);
    if st.edit != 0 {
        // the text box frame: the EDIT itself sits inside it
        let r = rect(PAD, st.edit_y, W - PAD, st.edit_y + EDIT_H);
        rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: r, radiusX: 4.0, radiusY: 4.0 }, &b(0x1F1F1F)?);
        rt.DrawRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), radiusX: 4.0, radiusY: 4.0 }, &b(0x3A3A3A)?, 1.0, None);
        rt.FillRectangle(&rect(r.left + 1.0, r.bottom - 2.0, r.right - 1.0, r.bottom), &b(0x4CC2FF)?);
    }
    let label = dw.CreateTextFormat(w!("Segoe UI Variable Text"), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 14.0, w!("en-us"))?;
    label.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
    label.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    for (i, r) in button_rects(st.buttons.len(), h).into_iter().enumerate() {
        let accent = i == st.default;
        let (fill, text) = match (accent, st.down == Some(i), st.hover == Some(i)) {
            (true, true, _) => (0x42A1D2, 0x1A3A4A),
            (true, _, true) => (0x47B1E8, 0x000000),
            (true, _, _) => (0x4CC2FF, 0x000000),
            (false, true, _) => (0x272727, 0xCFCFCF),
            (false, _, true) => (0x323232, 0xFFFFFF),
            _ => (0x373737, 0xFFFFFF),
        };
        let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: 4.0, radiusY: 4.0 };
        rt.FillRoundedRectangle(&rr, &b(fill)?);
        if !accent {
            let edge = D2D1_ROUNDED_RECT { rect: rect(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), radiusX: 4.0, radiusY: 4.0 };
            rt.DrawRoundedRectangle(&edge, &b(0x3F3F3F)?, 1.0, None);
        }
        if st.keys && st.focus == i {
            let ring = D2D1_ROUNDED_RECT { rect: rect(r.left - 3.0, r.top - 3.0, r.right + 3.0, r.bottom + 3.0), radiusX: 7.0, radiusY: 7.0 };
            rt.DrawRoundedRectangle(&ring, &b(0xFFFFFF)?, 2.0, None);
        }
        rt.DrawText(&utf16(&st.buttons[i]), &label, &r, &b(text)?, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
    }
    Ok(())
}

fn with<R>(f: impl FnOnce(&mut St) -> R) -> Option<R> {
    ST.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.as_mut().map(f)))
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let s = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
    let at = |lp: LPARAM| ((lp.0 & 0xFFFF) as i16 as f32 / s, ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s);
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let h = rc.bottom as f32 / s;
    let btn_at = |x: f32, y: f32| with(|st| button_rects(st.buttons.len(), h).iter().position(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)).flatten();
    let repaint = || { let _ = InvalidateRect(Some(hwnd), None, false); };
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            with(|st| {
                if st.rt.is_none() {
                    if let Ok(f) = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) {
                        let props = D2D1_RENDER_TARGET_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE }, r#type: crate::flyout::rt_type(), ..Default::default() };
                        st.rt = f.CreateDCRenderTarget(&props).ok();
                        if let Some(rt) = &st.rt { crate::flyout::crisp_text(rt); }
                    }
                }
                if let Some(rt) = st.rt.clone() {
                    if rt.BindDC(hdc, &rc).is_ok() {
                        rt.SetDpi(96.0 * s, 96.0 * s);
                        rt.BeginDraw();
                        let _ = draw(st, h);
                        if rt.EndDraw(None, None).is_err() { st.rt = None; }
                    }
                }
            });
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let (x, y) = at(lp);
            let k = btn_at(x, y);
            if with(|st| std::mem::replace(&mut st.hover, k) != k).unwrap_or(false) {
                let mut tme = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
                repaint();
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => { with(|st| st.hover = None); repaint(); LRESULT(0) }
        WM_LBUTTONDOWN => {
            let (x, y) = at(lp);
            let k = btn_at(x, y);
            with(|st| { st.down = k; if let Some(k) = k { st.focus = k; } });
            let _ = SetCapture(hwnd);
            repaint();
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            let (x, y) = at(lp);
            let k = btn_at(x, y);
            with(|st| { if k.is_some() && k == st.down { st.done = Some(k); } st.down = None; });
            repaint();
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wp.0 as u16);
            with(|st| {
                let n = st.buttons.len().max(1);
                let back = GetKeyState(VK_SHIFT.0 as i32) < 0;
                match vk {
                    VK_ESCAPE => st.done = Some(None),
                    VK_RETURN | VK_SPACE => st.done = Some(Some(st.focus)),
                    VK_TAB => { st.focus = if back { (st.focus + n - 1) % n } else { (st.focus + 1) % n }; st.keys = true; }
                    VK_LEFT => { st.focus = st.focus.saturating_sub(1); st.keys = true; }
                    VK_RIGHT => { st.focus = (st.focus + 1).min(n - 1); st.keys = true; }
                    _ => {}
                }
            });
            repaint();
            LRESULT(0)
        }
        WM_CLOSE => { with(|st| st.done = Some(None)); LRESULT(0) }
        WM_CTLCOLOREDIT => {
            static BRUSH: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
            let dc = HDC(wp.0 as *mut _);
            SetTextColor(dc, COLORREF(0xFFFFFF));
            SetBkColor(dc, COLORREF(0x1F1F1F));
            if BRUSH.load(std::sync::atomic::Ordering::Relaxed) == 0 {
                BRUSH.store(CreateSolidBrush(COLORREF(0x1F1F1F)).0 as isize, std::sync::atomic::Ordering::Relaxed);
            }
            LRESULT(BRUSH.load(std::sync::atomic::Ordering::Relaxed))
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Show the dialog and wait. `default` is the accent button that Enter presses at first.
pub unsafe fn ask(owner: Option<HWND>, title: &str, body: &str, buttons: &[&str], default: usize) -> Option<usize> {
    run(owner, title, body, buttons, default, None).0
}

/// The same with a text box under the text, holding `initial` (all selected): the typed text when the
/// first button (or Enter) is pressed, None for Cancel / Esc.
pub unsafe fn ask_text(owner: Option<HWND>, title: &str, body: &str, initial: &str, ok: &str) -> Option<String> {
    match run(owner, title, body, &[ok, "Cancel"], 0, Some(initial)) {
        (Some(0), Some(s)) => Some(s),
        _ => None,
    }
}

unsafe fn run(owner: Option<HWND>, title: &str, body: &str, buttons: &[&str], default: usize, text: Option<&str>) -> (Option<usize>, Option<String>) {
    let Ok(dw) = DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) else { return (None, None) };
    let mut m = DWRITE_TEXT_METRICS::default();
    if let Ok(l) = body_layout(&dw, body) { let _ = l.GetMetrics(&mut m); }
    let body_h = m.height.ceil();
    let prev = ST.with(|s| s.borrow_mut().replace(St {
        title: title.into(), body: body.into(), buttons: buttons.iter().map(|b| b.to_string()).collect(),
        default, focus: default, dw: Some(dw), edit_y: PAD + TITLE_H + 12.0 + body_h + 12.0, ..Default::default()
    }));
    let inst = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
    let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst.into(), lpszClassName: w!("shellbits_dialog"), hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
    RegisterClassW(&wc);
    // centred over the owner, else over the main screen's work area; sized for that screen's scale
    let mut area = RECT::default();
    let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut area as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    if let Some(o) = owner.filter(|o| IsWindowVisible(*o).as_bool() && !IsIconic(*o).as_bool()) {
        let _ = GetWindowRect(o, &mut area);
    }
    let sc = crate::flyout::primary_scale();
    let (pw, ph) = ((W * sc).round() as i32, (height(body_h, text.is_some()) * sc).round() as i32);
    let (x, y) = ((area.left + area.right - pw) / 2, (area.top + area.bottom - ph) / 2);
    let ex = if owner.is_none() { WS_EX_TOPMOST | WS_EX_DLGMODALFRAME } else { WS_EX_DLGMODALFRAME };
    let Ok(hwnd) = CreateWindowExW(ex, w!("shellbits_dialog"), &windows::core::HSTRING::from(title), WS_POPUP, x, y, pw, ph, owner, None, Some(inst.into()), None) else {
        ST.with(|s| *s.borrow_mut() = prev);
        return (None, None);
    };
    if let Some(initial) = text {
        let ey = PAD + TITLE_H + 12.0 + body_h + 12.0;
        let e = CreateWindowExW(WINDOW_EX_STYLE(0), w!("EDIT"), &windows::core::HSTRING::from(initial), WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            ((PAD + 11.0) * sc) as i32, ((ey + 7.0) * sc) as i32, ((W - 2.0 * PAD - 22.0) * sc) as i32, (18.0 * sc) as i32, Some(hwnd), None, Some(inst.into()), None).unwrap_or_default();
        let font = CreateFontW(-(14.0 * sc) as i32, 0, 0, 0, 400, 0, 0, 0, DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY, 0, w!("Segoe UI Variable Text"));
        SendMessageW(e, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        SendMessageW(e, 0x00B1 /* EM_SETSEL */, Some(WPARAM(0)), Some(LPARAM(-1)));
        with(|st| st.edit = e.0 as isize);
    }
    let dark: i32 = 1;
    let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
    let round = DWMWCP_ROUND;
    let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
    let edge: u32 = 0x3B3B3B;
    let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &edge as *const _ as *const _, 4);
    if let Some(o) = owner { let _ = EnableWindow(o, false); }
    let _ = ShowWindow(hwnd, SW_SHOW);
    crate::flyout::force_foreground(hwnd);
    let edit = HWND(with(|st| st.edit).unwrap_or(0) as *mut _);
    let _ = SetFocus(Some(if edit.is_invalid() { hwnd } else { edit }));
    let mut msg = MSG::default();
    let result = loop {
        if let Some(Some(r)) = with(|st| st.done) {
            break r;
        }
        if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
            PostQuitMessage(msg.wParam.0 as i32); // pass the quit on to the outer loop
            break None;
        }
        // in the text box, Enter presses the default button and Esc cancels, as in a WinUI dialog
        if !edit.is_invalid() && msg.hwnd == edit && msg.message == WM_KEYDOWN && (msg.wParam.0 == 0x0D || msg.wParam.0 == 0x1B) {
            let enter = msg.wParam.0 == 0x0D;
            with(|st| st.done = Some(if enter { Some(st.default) } else { None }));
            continue;
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
    };
    let typed = (!edit.is_invalid()).then(|| {
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(edit, &mut buf) as usize;
        String::from_utf16_lossy(&buf[..n])
    });
    if let Some(o) = owner { let _ = EnableWindow(o, true); let _ = SetForegroundWindow(o); }
    let _ = DestroyWindow(hwnd);
    ST.with(|s| *s.borrow_mut() = prev);
    (result, typed)
}

/// `--snapshot-dialog out.bmp [keys]`: the dialog drawn off-screen at 100% (keys: k = focus ring on Cancel).
pub unsafe fn snapshot(out: &str, keys: &str) -> windows::core::Result<()> {
    let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
    let body = "This app and its related info will be uninstalled.

Visual Studio Code";
    let mut m = DWRITE_TEXT_METRICS::default();
    body_layout(&dw, body)?.GetMetrics(&mut m)?;
    let h = height(m.height.ceil(), false);
    let (w, hh) = (W as i32, h.round() as i32);
    let f: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
    let props = D2D1_RENDER_TARGET_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE }, r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE, ..Default::default() };
    let rt = f.CreateDCRenderTarget(&props)?;
    crate::flyout::crisp_text(&rt);
    let st = St { title: "Uninstall".into(), body: body.into(), buttons: vec!["Uninstall".into(), "Cancel".into()], default: 1, focus: 1, keys: keys.contains('k'), rt: Some(rt.clone()), dw: Some(dw), ..Default::default() };
    let dc = CreateCompatibleDC(None);
    let bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -hh, biPlanes: 1, biBitCount: 32, ..Default::default() }, ..Default::default() };
    let mut bits = std::ptr::null_mut();
    let dib = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0)?;
    let old = SelectObject(dc, dib.into());
    rt.BindDC(dc, &RECT { left: 0, top: 0, right: w, bottom: hh })?;
    rt.BeginDraw();
    draw(&st, h)?;
    rt.EndDraw(None, None)?;
    let px = std::slice::from_raw_parts(bits as *const u8, (w * hh * 4) as usize);
    crate::photos::write_bmp(out, w, hh, px);
    SelectObject(dc, old);
    let _ = DeleteDC(dc);
    let _ = DeleteObject(dib.into());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn buttons_share_the_bar() {
        let r = button_rects(2, 200.0);
        assert_eq!(r.len(), 2);
        assert!((r[0].left - PAD).abs() < 0.01 && (r[1].right - (W - PAD)).abs() < 0.01);
        assert!(((r[1].left - r[0].right) - GAP).abs() < 0.01);
        assert!((r[0].bottom - (200.0 - PAD)).abs() < 0.01 && (r[0].bottom - r[0].top - BTN_H).abs() < 0.01);
        assert_eq!(button_rects(1, 200.0)[0].right, W - PAD);
    }
}
