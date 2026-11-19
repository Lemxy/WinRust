//! A Windows 11 style pop-up menu: rounded 8 px panel on the dark acrylic material, 1 px edge, items
//! 32 px tall with a 16 px glyph at the left and the label after it, a rounded highlight under the mouse
//! or keyboard, separators, check marks, submenus that open to the side. Win32 popup menus look like
//! Windows 7's, so every menu goes through here - our own lists (`pick`, `pick_up`) and Win32 menus,
//! including the shell's context menus with their items from other programs (`track`).
//!
//! Modal like TrackPopupMenu: returns the chosen id, or None for Esc / a click outside / losing the focus.

use std::cell::RefCell;
use std::ffi::c_void;

use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::flyout::{rect, utf16};
use crate::quick::rgb;

/// One line of a menu. `id` 0 is a separator.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: u32,
    pub glyph: u32, // Segoe Fluent Icons code point, 0 for none
    pub label: String,
    pub hint: String, // dim text at the right (a shortcut or an explanation): the part after a tab
    pub checked: bool,
    pub enabled: bool,
    pub sub: Vec<Item>, // a submenu, opened to the side
    pub hsub: isize,    // ...or a Win32 submenu, read when it opens (the shell fills some only then)
}

impl Item {
    pub fn new(id: u32, glyph: u32, label: &str) -> Self {
        let (l, h) = label.split_once('\t').unwrap_or((label, ""));
        Item { id, glyph, label: l.into(), hint: h.into(), checked: false, enabled: true, sub: Vec::new(), hsub: 0 }
    }
    pub fn sep() -> Self {
        Item { enabled: false, ..Item::new(0, 0, "") }
    }
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
    fn has_sub(&self) -> bool {
        !self.sub.is_empty() || self.hsub != 0
    }
}

const ROW: f32 = 32.0;
thread_local! {
    /// Row height of the menus opened now: Windows' 32, or the Claude app's 28 (its menus are denser).
    static ROW_H: std::cell::Cell<f32> = const { std::cell::Cell::new(ROW) };
}
fn row() -> f32 {
    ROW_H.with(|r| r.get())
}
const SEP: f32 = 9.0;
const PAD: f32 = 4.0;
const T_OPEN: usize = 0x6D65; // the opening slide
const T_SUB: usize = 0x6D66; // resting on a submenu item opens it
/// Windows opens a submenu after the pointer rests on its item this long (SPI_GETMENUSHOWDELAY's default).
const SUB_DELAY: u32 = 400;

/// Each item's top and bottom inside the menu (separators are thinner), and the menu's height.
pub fn layout(items: &[Item]) -> (Vec<(f32, f32)>, f32) {
    let mut y = PAD;
    let mut v = Vec::new();
    for it in items {
        let h = if it.id == 0 { SEP } else { row() };
        v.push((y, y + h));
        y += h;
    }
    (v, y + PAD)
}

/// The next item the arrow keys land on (skipping separators and greyed items), wrapping round.
pub fn step(items: &[Item], now: Option<usize>, down: bool) -> Option<usize> {
    let ok: Vec<usize> = items.iter().enumerate().filter(|(_, it)| it.id != 0 && it.enabled).map(|(i, _)| i).collect();
    if ok.is_empty() {
        return None;
    }
    Some(match now.and_then(|n| ok.iter().position(|&i| i == n)) {
        None => if down { ok[0] } else { ok[ok.len() - 1] },
        Some(k) => if down { ok[(k + 1) % ok.len()] } else { ok[(k + ok.len() - 1) % ok.len()] },
    })
}

/// A Win32 menu label as shown: "&Open" -> "Open" ("&&" is a real ampersand); "Copy\tCtrl+C" keeps its tab.
pub fn plain(label: &str) -> String {
    label.replace("&&", "\u{1}").replace('&', "").replace('\u{1}', "&")
}

/// Separators only between items: none at the ends, never two in a row.
pub fn tidy(items: Vec<Item>) -> Vec<Item> {
    let mut v: Vec<Item> = Vec::new();
    for it in items {
        if it.id == 0 && v.last().is_none_or(|l| l.id == 0) {
            continue;
        }
        v.push(it);
    }
    while v.last().is_some_and(|l| l.id == 0) {
        v.pop();
    }
    v
}

#[derive(Default)]
struct St {
    hwnd: isize,
    items: Vec<Item>,
    hover: Option<usize>,
    done: Option<Option<u32>>,
    scroll: f32, // a list taller than the screen scrolls (wheel, or the keys keep the highlight in view)
    view: f32,   // the menu's visible height (DIPs)
    open: Option<usize>, // a submenu asked for (click, Right, Enter, or resting on it)
    child: bool, // a submenu is open: the mouse is its, and losing the capture to it is not closing
    is_child: bool,
    rt: Option<ID2D1DCRenderTarget>,
    dw: Option<IDWriteFactory>,
}

thread_local! {
    // the menus open now, outermost first (a submenu sits after its parent)
    static MENUS: RefCell<Vec<St>> = const { RefCell::new(Vec::new()) };
    // called with a Win32 submenu just before it is read (the shell fills "Send to", "New", ... then)
    static INIT: std::cell::Cell<Option<*const dyn Fn(HMENU)>> = const { std::cell::Cell::new(None) };
}

/// Run `f` on the state of the menu window `hwnd`, if it is one of ours and nothing else holds it.
fn with<R>(hwnd: HWND, f: impl FnOnce(&mut St) -> R) -> Option<R> {
    MENUS.with(|m| m.try_borrow_mut().ok().and_then(|mut m| m.iter_mut().find(|s| s.hwnd == hwnd.0 as isize).map(f)))
}

unsafe fn draw(st: &mut St, w: f32) -> windows::core::Result<()> {
    let rt = st.rt.clone().unwrap();
    let dw = st.dw.clone().unwrap();
    rt.Clear(Some(&crate::flyout::base(rgb(0x2C2C2C))));
    let b = |c: u32| rt.CreateSolidColorBrush(&rgb(c), None);
    let fmt = |family: windows::core::PCWSTR, size: f32, align: DWRITE_TEXT_ALIGNMENT| -> windows::core::Result<IDWriteTextFormat> {
        let f = dw.CreateTextFormat(family, None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))?;
        f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        f.SetTextAlignment(align)?;
        f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(f)
    };
    let text = fmt(w!("Segoe UI Variable Text"), 14.0, DWRITE_TEXT_ALIGNMENT_LEADING)?;
    let icons = fmt(w!("Segoe Fluent Icons"), 16.0, DWRITE_TEXT_ALIGNMENT_CENTER)?;
    let icons12 = fmt(w!("Segoe Fluent Icons"), 12.0, DWRITE_TEXT_ALIGNMENT_CENTER)?;
    let hint = fmt(w!("Segoe UI Variable Text"), 12.0, DWRITE_TEXT_ALIGNMENT_TRAILING)?;
    let (rows, _) = layout(&st.items);
    let rows: Vec<(f32, f32)> = rows.into_iter().map(|(a, b)| (a - st.scroll, b - st.scroll)).collect();
    let any_glyph = st.items.iter().any(|i| i.glyph != 0 || i.checked);
    for (k, (it, (y0, y1))) in st.items.iter().zip(rows).enumerate() {
        if it.id == 0 {
            rt.FillRectangle(&rect(0.0, (y0 + y1) / 2.0, w, (y0 + y1) / 2.0 + 1.0), &b(0x3B3B3B)?);
            continue;
        }
        if (st.hover == Some(k) || (st.child && st.open == Some(k))) && it.enabled {
            rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rect(PAD, y0, w - PAD, y1), radiusX: 4.0, radiusY: 4.0 }, &b(0x383838)?);
        }
        let fg = b(if it.enabled { 0xFFFFFF } else { 0x7A7A7A })?;
        if it.checked {
            rt.DrawText(&utf16("\u{E73E}"), &icons12, &rect(12.0, y0, 32.0, y1), &fg, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        } else if it.glyph != 0 {
            let g = char::from_u32(it.glyph).map(|c| c.to_string()).unwrap_or_default();
            rt.DrawText(&utf16(&g), &icons, &rect(12.0, y0, 32.0, y1), &fg, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
        let x = if any_glyph { 44.0 } else { 16.0 };
        // "label\u{1}badge": a small grey pill after the label (the Claude app's "Default", "Recommended")
        let (label, badge) = it.label.split_once('\u{1}').unwrap_or((&it.label, ""));
        rt.DrawText(&utf16(label), &text, &rect(x, y0, w - 16.0, y1), &fg, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        if !badge.is_empty() {
            if let (Ok(lw), Ok(bl)) = (dw.CreateTextLayout(&utf16(label), &text, 1000.0, 100.0), dw.CreateTextLayout(&utf16(badge), &hint, 1000.0, 100.0)) {
                let (mut m1, mut m2) = (DWRITE_TEXT_METRICS::default(), DWRITE_TEXT_METRICS::default());
                let _ = lw.GetMetrics(&mut m1);
                let _ = bl.GetMetrics(&mut m2);
                let bx = x + m1.width + 8.0;
                let cy = (y0 + y1) / 2.0;
                let pill = D2D1_ROUNDED_RECT { rect: rect(bx, cy - 8.0, bx + m2.width + 12.0, cy + 8.0), radiusX: 4.0, radiusY: 4.0 };
                rt.FillRoundedRectangle(&pill, &b(0x3A3A3A)?);
                rt.DrawTextLayout(windows_numerics::Vector2 { X: bx + 6.0, Y: cy - m2.height / 2.0 }, &bl, &b(0xBDBDBD)?, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
        if it.has_sub() {
            rt.DrawText(&utf16("\u{E76C}"), &icons12, &rect(w - 32.0, y0, w - 12.0, y1), &fg, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        } else if !it.hint.is_empty() {
            rt.DrawText(&utf16(&it.hint), &hint, &rect(x, y0, w - 16.0, y1), &b(0x9F9F9F)?, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }
    Ok(())
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if with(hwnd, |_| ()).is_none() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let s = GetDpiForWindow(hwnd) as f32 / 96.0;
    let at = |lp: LPARAM| ((lp.0 & 0xFFFF) as i16 as f32 / s, ((lp.0 >> 16) & 0xFFFF) as i16 as f32 / s);
    let row_at = |y: f32| with(hwnd, |st| { let (rows, _) = layout(&st.items); let y = y + st.scroll; rows.iter().position(|(a, b)| y >= *a && y < *b).filter(|&k| st.items[k].id != 0 && st.items[k].enabled) }).flatten();
    // a click or Enter on an item: a submenu opens, anything else is the choice
    let choose = |k: usize| with(hwnd, |st| if st.items[k].has_sub() { st.open = Some(k) } else { st.done = Some(Some(st.items[k].id)) });
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            let params = BP_PAINTPARAMS { cbSize: size_of::<BP_PAINTPARAMS>() as u32, ..Default::default() };
            let mut mem = HDC::default();
            let pb = BeginBufferedPaint(hdc, &rc, BPBF_TOPDOWNDIB, Some(&params), &mut mem);
            if !mem.is_invalid() {
                with(hwnd, |st| {
                    if st.rt.is_none() {
                        if let Ok(f) = D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) {
                            let props = D2D1_RENDER_TARGET_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, r#type: crate::flyout::rt_type(), ..Default::default() };
                            st.rt = f.CreateDCRenderTarget(&props).ok();
                            if let Some(rt) = &st.rt { crate::flyout::crisp_text(rt); }
                            st.dw = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok();
                        }
                    }
                    if let Some(rt) = st.rt.clone() {
                        if rt.BindDC(mem, &rc).is_ok() {
                            rt.SetDpi(96.0 * s, 96.0 * s);
                            rt.BeginDraw();
                            let _ = draw(st, rc.right as f32 / s);
                            if rt.EndDraw(None, None).is_err() { st.rt = None; }
                        }
                    }
                });
                let _ = EndBufferedPaint(pb, true);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let (_, y) = at(lp);
            let k = row_at(y);
            let (changed, sub) = with(hwnd, |st| (std::mem::replace(&mut st.hover, k) != k, k.is_some_and(|k| st.items[k].has_sub()))).unwrap_or((false, false));
            if changed {
                let _ = KillTimer(Some(hwnd), T_SUB);
                if sub {
                    SetTimer(Some(hwnd), T_SUB, SUB_DELAY, None);
                }
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_SUB => {
            let _ = KillTimer(Some(hwnd), T_SUB);
            with(hwnd, |st| if let Some(k) = st.hover.filter(|&k| st.items[k].has_sub()) { st.open = Some(k) });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let d = ((wp.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
            with(hwnd, |st| { let (_, h) = layout(&st.items); let max = (h - st.view).max(0.0); st.scroll = (st.scroll - d * row() * 3.0).clamp(0.0, max); });
            let _ = InvalidateRect(Some(hwnd), None, false);
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let (_, y) = at(lp);
            if let Some(k) = row_at(y) {
                choose(k);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            match VIRTUAL_KEY(wp.0 as u16) {
                VK_ESCAPE => { with(hwnd, |st| st.done = Some(None)); }
                VK_LEFT => { with(hwnd, |st| if st.is_child { st.done = Some(None) }); } // back to the parent
                VK_RIGHT => { with(hwnd, |st| if let Some(k) = st.hover.filter(|&k| st.items[k].has_sub()) { st.open = Some(k) }); }
                VK_DOWN | VK_UP => {
                    with(hwnd, |st| {
                        let n = step(&st.items, st.hover, wp.0 as u16 == VK_DOWN.0);
                        st.hover = n;
                        // keep it in view
                        if let Some(k) = n {
                            let (rows, _) = layout(&st.items);
                            let (a, b) = rows[k];
                            if a - PAD < st.scroll { st.scroll = (a - PAD).max(0.0); }
                            if b + PAD > st.scroll + st.view { st.scroll = b + PAD - st.view; }
                        }
                    });
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                VK_RETURN | VK_SPACE => {
                    if let Some(k) = with(hwnd, |st| st.hover).flatten() {
                        choose(k);
                    }
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let (x, y) = at(lp);
            let mut rc = RECT::default();
            let _ = GetClientRect(hwnd, &mut rc);
            if x < 0.0 || y < 0.0 || x * s >= rc.right as f32 || y * s >= rc.bottom as f32 {
                with(hwnd, |st| st.done = Some(None));
            }
            LRESULT(0)
        }
        WM_CAPTURECHANGED if HWND(lp.0 as *mut c_void) != hwnd => {
            with(hwnd, |st| if st.done.is_none() && !st.child { st.done = Some(None) });
            LRESULT(0)
        }
        WM_ACTIVATE if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            with(hwnd, |st| if st.done.is_none() && !st.child { st.done = Some(None) });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Show the menu with its top-left at screen point (x, y) (moved to stay on the screen; above the point
/// when there is no room below) and wait for a choice. `owner` gets the focus back afterwards.
pub unsafe fn pick(owner: HWND, items: Vec<Item>, x: i32, y: i32) -> Option<u32> {
    match show(owner, items, x, y, false, None) {
        Out::Pick(id) => Some(id),
        _ => None,
    }
}

/// The same, with the menu's bottom edge at `y` (it opens upward: Start's footer, the taskbar).
pub unsafe fn pick_up(owner: HWND, items: Vec<Item>, x: i32, y: i32) -> Option<u32> {
    match show(owner, items, x, y, true, None) {
        Out::Pick(id) => Some(id),
        _ => None,
    }
}

/// A Win32 menu's items as ours. Submenus stay Win32 handles until they open.
pub unsafe fn from_hmenu(m: HMENU) -> Vec<Item> {
    let mut v = Vec::new();
    for i in 0..GetMenuItemCount(Some(m)).max(0) {
        let mut buf = [0u16; 260];
        let mut mi = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_ID | MIIM_STATE | MIIM_STRING | MIIM_SUBMENU,
            dwTypeData: windows::core::PWSTR(buf.as_mut_ptr()),
            cch: 259,
            ..Default::default()
        };
        if GetMenuItemInfoW(m, i as u32, true, &mut mi).is_err() {
            continue;
        }
        if mi.fType.0 & MFT_SEPARATOR.0 != 0 {
            v.push(Item::sep());
            continue;
        }
        let label = String::from_utf16_lossy(&buf[..(mi.cch as usize).min(259)]);
        if label.trim().is_empty() {
            continue; // drawn by its program (owner-drawn), with no text to show
        }
        let mut it = Item::new(mi.wID, 0, &plain(&label));
        it.enabled = mi.fState.0 & (MFS_DISABLED.0 | MFS_GRAYED.0) == 0;
        it.checked = mi.fState.0 & MFS_CHECKED.0 != 0;
        if !mi.hSubMenu.is_invalid() {
            it.hsub = mi.hSubMenu.0 as isize;
            it.id = 0xFFFF_0000 | i as u32; // selectable (0 is a separator; Win32 gives the handle here); never returned
        }
        v.push(it);
    }
    tidy(v)
}

/// TrackPopupMenu, drawn as Windows 11 draws its menus: the menu `m` at screen point (x, y), returning
/// the chosen item's id (0 for none). `init` is called with each submenu just before it opens - for the
/// shell's context menus, IContextMenu2::HandleMenuMsg(WM_INITMENUPOPUP), which fills "Send to", "New"...
pub unsafe fn track(m: HMENU, owner: HWND, x: i32, y: i32, init: Option<&dyn Fn(HMENU)>) -> u32 {
    let items = from_hmenu(m);
    if items.is_empty() {
        return 0;
    }
    // ponytail: the callback lives only for this call; stored as a raw pointer for the submenus to reach
    let prev = INIT.with(|c| c.replace(init.map(|f| std::mem::transmute::<&dyn Fn(HMENU), &'static dyn Fn(HMENU)>(f) as *const dyn Fn(HMENU))));
    let r = pick(owner, items, x, y).filter(|id| id & 0xFFFF_0000 != 0xFFFF_0000).unwrap_or(0);
    INIT.with(|c| c.set(prev));
    r
}

/// For a shell context menu: fill a submenu the way Explorer does just before it opens ("Send to", "New").
pub unsafe fn shell_sub(cm: &windows::Win32::UI::Shell::IContextMenu, sub: HMENU) {
    use windows::core::Interface;
    if let Ok(c2) = cm.cast::<windows::Win32::UI::Shell::IContextMenu2>() {
        let _ = c2.HandleMenuMsg(WM_INITMENUPOPUP, WPARAM(sub.0 as usize), LPARAM(0));
    }
}

enum Out {
    Pick(u32),
    Cancel,
    Outside(MSG), // a press outside this menu: closes it; the menu it came from may want the press
}

unsafe fn show(owner: HWND, items: Vec<Item>, x: i32, y: i32, up: bool, parent: Option<(HWND, i32)>) -> Out {
    if parent.is_none() {
        // the Claude window's menus are the app's: 28 px rows
        let mut cls = [0u16; 32];
        let n = GetClassNameW(owner, &mut cls) as usize;
        ROW_H.with(|r| r.set(if String::from_utf16_lossy(&cls[..n]) == "shellbits_claude" { 28.0 } else { ROW }));
    }
    let inst: HINSTANCE = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).map(|m| HINSTANCE(m.0)).unwrap_or_default();
    static REG: std::sync::Once = std::sync::Once::new();
    REG.call_once(|| {
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: w!("shellbits_menu"), hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), style: CS_DROPSHADOW, ..Default::default() };
        RegisterClassW(&wc);
    });
    // size from the longest label (measured), at the owner's scale
    let s = if owner.is_invalid() { crate::flyout::primary_scale() } else { GetDpiForWindow(owner) as f32 / 96.0 };
    let longest = items.iter().map(|i| i.label.chars().count() as f32 * 7.6 + if i.hint.is_empty() { 0.0 } else { i.hint.chars().count() as f32 * 6.4 + 32.0 } + if i.has_sub() { 24.0 } else { 0.0 }).fold(30.0, f32::max);
    let w = ((longest + 44.0 + 40.0).max(180.0).min(720.0) * s).round() as i32;
    let (_, hgt) = layout(&items);
    // keep it on the screen the point is on
    let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(mon, &mut mi);
    let wa = mi.rcWork;
    let room = if up { y - wa.top } else { wa.bottom - y }.max((wa.bottom - wa.top) / 2) - 8;
    let h = ((hgt * s).round() as i32).min(room);
    // a submenu goes to the parent's right, or its left when there is no room (the parent's left edge)
    let px = match parent {
        Some((_, left)) if x + w > wa.right => left - w,
        _ => x.min(wa.right - w),
    }
    .max(wa.left);
    let py = if up || y + h > wa.bottom { if parent.is_some() { wa.bottom - h } else { y - h }.max(wa.top) } else { y };
    // never takes the focus (a panel like Start closes when it loses it): the mouse is captured instead,
    // and the keys pressed meanwhile are routed here, as Win32 menus do
    let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE, w!("shellbits_menu"), w!(""), WS_POPUP, px, py, w, h, Some(owner), None, Some(inst), None) else { return Out::Cancel };
    MENUS.with(|m| m.borrow_mut().push(St { hwnd: hwnd.0 as isize, items, view: h as f32 / s, is_child: parent.is_some(), ..Default::default() }));
    let _ = crate::fluent_window(hwnd); // Windows' acrylic material and rounded corners
    // opens the way Windows 11's menus do: the items slide in from the edge at the button over ~170 ms,
    // easing out; driven by a timer inside the modal loop, so it takes clicks and keys while it runs.
    // (A window region would clip it, but DWM ignores regions on a window with a backdrop.)
    let lift = h as f32 / s * 0.25 * if up { -1.0 } else { 1.0 };
    let reveal = |k: f32| {
        let e = 1.0 - (1.0 - k.clamp(0.0, 1.0)).powi(3);
        with(hwnd, |st| st.scroll = (1.0 - e) * lift);
        let _ = InvalidateRect(Some(hwnd), None, false);
    };
    reveal(0.0);
    let t0 = std::time::Instant::now();
    SetTimer(Some(hwnd), T_OPEN, 8, None);
    let _ = ShowWindow(hwnd, SW_SHOWNA);
    let _ = UpdateWindow(hwnd); // painted before the first frame: no empty grey block
    SetCapture(hwnd);
    // modal loop: a press outside the menu (anywhere else on screen) closes it, like a Win32 menu
    let mut msg = MSG::default();
    let mut out: Option<Out> = None;
    while out.is_none() && with(hwnd, |st| st.done.is_none()).unwrap_or(false) {
        if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
            PostQuitMessage(msg.wParam.0 as i32);
            break;
        }
        if msg.message == WM_TIMER && msg.hwnd == hwnd && msg.wParam.0 == T_OPEN {
            let k = t0.elapsed().as_secs_f32() / 0.17;
            if k >= 1.0 {
                let _ = KillTimer(Some(hwnd), T_OPEN);
                reveal(1.0);
            } else {
                reveal(k);
            }
            continue;
        }
        if matches!(msg.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
            msg.hwnd = hwnd; // the keys belong to the menu while it is open
        }
        if matches!(msg.message, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN) && msg.hwnd != hwnd {
            out = Some(Out::Outside(msg));
            break;
        }
        // a submenu: the pointer moving back onto another item of the parent closes it
        if let (WM_MOUSEMOVE, Some((ph, _))) = (msg.message, parent) {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            if WindowFromPoint(pt) == ph {
                let mut q = pt;
                let _ = ScreenToClient(ph, &mut q);
                let ps = GetDpiForWindow(ph) as f32 / 96.0;
                let other = with(ph, |st| {
                    let (rows, _) = layout(&st.items);
                    let y = q.y as f32 / ps + st.scroll;
                    rows.iter().position(|(a, b)| y >= *a && y < *b) != st.open
                })
                .unwrap_or(false);
                if other {
                    out = Some(Out::Cancel);
                    let _ = PostMessageW(Some(ph), WM_MOUSEMOVE, WPARAM(0), LPARAM(((q.y as i16 as u16 as isize) << 16) | (q.x as i16 as u16 as isize)));
                    break;
                }
            }
        }
        let _ = TranslateMessage(&msg);
        DispatchMessageW(&msg);
        // a submenu asked for: open it beside its item, and wait for it
        let want = with(hwnd, |st| st.open.filter(|_| !st.child).map(|k| (k, st.items[k].clone(), layout(&st.items).0[k].0 - st.scroll))).flatten();
        if let Some((k, it, top)) = want {
            let _ = KillTimer(Some(hwnd), T_SUB);
            let sub = if it.sub.is_empty() {
                let hs = HMENU(it.hsub as *mut c_void);
                if let Some(f) = INIT.with(|c| c.get()) {
                    (*f)(hs);
                }
                from_hmenu(hs)
            } else {
                it.sub.clone()
            };
            if sub.is_empty() {
                with(hwnd, |st| st.open = None);
                continue;
            }
            with(hwnd, |st| st.child = true);
            let _ = InvalidateRect(Some(hwnd), None, false);
            let mut wr = RECT::default();
            let _ = GetWindowRect(hwnd, &mut wr);
            let r = show(hwnd, sub, wr.right - (2.0 * s) as i32, wr.top + ((top - PAD) * s) as i32, false, Some((hwnd, wr.left + (2.0 * s) as i32)));
            with(hwnd, |st| { st.child = false; st.open = None; let _ = k; });
            let _ = InvalidateRect(Some(hwnd), None, false);
            SetCapture(hwnd);
            match r {
                Out::Pick(id) => out = Some(Out::Pick(id)),
                Out::Cancel => {}
                Out::Outside(m) if m.hwnd == hwnd => {
                    // pressed on this menu: it handles the press (and the release that follows)
                    DispatchMessageW(&m);
                }
                Out::Outside(m) => out = Some(Out::Outside(m)),
            }
        }
    }
    let done = MENUS.with(|m| {
        let mut m = m.borrow_mut();
        let i = m.iter().position(|s| s.hwnd == hwnd.0 as isize);
        i.map(|i| m.remove(i)).and_then(|st| st.done)
    });
    if parent.is_none() {
        let _ = ReleaseCapture();
    }
    let _ = DestroyWindow(hwnd);
    match (out, done) {
        (Some(o), _) => o,
        (None, Some(Some(id))) => Out::Pick(id),
        _ => Out::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_and_keys() {
        let items = vec![Item::new(1, 0xE710, "New"), Item::sep(), Item { enabled: false, ..Item::new(2, 0, "Paste") }, Item::new(3, 0, "Refresh")];
        let (rows, h) = layout(&items);
        assert_eq!(rows[0], (4.0, 36.0));
        assert_eq!(rows[1], (36.0, 45.0));
        assert_eq!(h, 4.0 + 32.0 + 9.0 + 32.0 + 32.0 + 4.0);
        assert_eq!(step(&items, None, true), Some(0));
        assert_eq!(step(&items, Some(0), true), Some(3)); // skips the separator and the greyed item
        assert_eq!(step(&items, Some(3), true), Some(0)); // wraps
        assert_eq!(step(&items, Some(0), false), Some(3));
    }
    #[test]
    fn win32_labels_and_separators() {
        assert_eq!(plain("&Open"), "Open");
        assert_eq!(plain("Save && quit"), "Save & quit");
        let v = tidy(vec![Item::sep(), Item::new(1, 0, "A"), Item::sep(), Item::sep(), Item::new(2, 0, "B"), Item::sep()]);
        assert_eq!(v.iter().map(|i| i.id).collect::<Vec<_>>(), vec![1, 0, 2]);
    }
    #[test]
    fn a_win32_menu_reads_with_its_submenu() {
        unsafe {
            let m = CreatePopupMenu().unwrap();
            let sub = CreatePopupMenu().unwrap();
            let _ = AppendMenuW(sub, MF_STRING, 21, w!("&Large icons"));
            let _ = AppendMenuW(m, MF_STRING, 1, w!("&Open\tEnter"));
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(m, MF_POPUP, sub.0 as usize, w!("&View"));
            let _ = AppendMenuW(m, MF_STRING | MF_GRAYED, 2, w!("Paste"));
            let v = from_hmenu(m);
            assert_eq!(v.len(), 4);
            assert_eq!((v[0].label.as_str(), v[0].hint.as_str(), v[0].id), ("Open", "Enter", 1));
            assert_eq!(v[1].id, 0);
            assert!(v[2].has_sub() && v[2].label == "View" && v[2].id & 0xFFFF_0000 == 0xFFFF_0000);
            assert!(!v[3].enabled);
            assert_eq!(from_hmenu(HMENU(v[2].hsub as *mut c_void))[0].label, "Large icons");
            let _ = DestroyMenu(m);
        }
    }
}
