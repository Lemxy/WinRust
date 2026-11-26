//! The right-click menu: a second layered window drawn with the same painter as the card.
//!
//! Native Win32 menus do not render reliably over a `WS_EX_NOACTIVATE` layered owner (the popup exists but stays blank),
//! and this way the menu matches the widget exactly. One row per module (click toggles it, hovering opens its style
//! panel), then the actions. The window is one surface with room for the style panel, so no submenu ever moves the card.
#![allow(unsafe_op_in_unsafe_fn)]
use std::cell::RefCell;

use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::{
        BeginPaint, EndPaint, GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        SystemInformation::GetTickCount64,
    },
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, VK_ESCAPE},
        WindowsAndMessaging::*,
    },
};

use crate::{
    canvas::{Align, Canvas, Fonts, wide},
    config::{self, Config, Style},
};
use winrust_common::style::pal;

pub const ID_TOGGLE: u32 = 100;
pub const ID_STYLE: u32 = 200;
pub const ID_RELOAD: u32 = 900;
pub const ID_OPEN: u32 = 901;
pub const ID_SETTINGS: u32 = 902;
pub const ID_COLUMNS: u32 = 903;
pub const ID_RESET: u32 = 904;
pub const ID_CLOSE: u32 = 905;
pub const ID_DETACH: u32 = 906;
pub const ID_ATTACH: u32 = 907;

const CLASS: &str = "WinRustMetricsMenu";

#[derive(Clone)]
struct Item {
    id: u32,
    label: String,
    check: Option<bool>,
    /// A module row: its style panel opens on hover.
    module: Option<usize>,
    sep: bool,
}

fn style_label(s: Style) -> &'static str {
    match s {
        Style::Bar => t!("Полоса", "Bar", ja: "バー", pt: "Barra", zh: "条"),
        Style::Ring => t!("Кольцо", "Ring", ja: "リング", pt: "Anel", zh: "圆环"),
        Style::Text => t!("Текст", "Text", ja: "テキスト", pt: "Texto", zh: "文字"),
        Style::Spark => t!("График", "Sparkline", ja: "スパーク", pt: "Minigráfico", zh: "迷你图"),
        Style::Pill => t!("Пилюля", "Pill", ja: "ピル", pt: "Pílula", zh: "胶囊"),
    }
}

fn items(c: &Config, context: Option<(usize, bool)>) -> Vec<Item> {
    let mut v = Vec::with_capacity(20);
    // the module the click came from: separate/acoplar right here, before the rest
    if let Some((m, detached)) = context {
        let label = if detached {
            format!("{} {}", t!("Присоединить", "Attach", ja: "戻す", pt: "Fixar", zh: "收回"), c.label[m])
        } else {
            format!("{} {}", t!("Отделить", "Detach", ja: "分離", pt: "Separar", zh: "分离"), c.label[m])
        };
        v.push(Item { id: if detached { ID_ATTACH } else { ID_DETACH }, label, check: None, module: None, sep: false });
        v.push(Item { id: 0, label: String::new(), check: None, module: None, sep: true });
    }
    for i in 0..config::MODS.len() {
        v.push(Item { id: ID_TOGGLE + i as u32, label: c.label[i].clone(), check: Some(c.on[i]), module: Some(i), sep: false });
    }
    v.push(Item { id: 0, label: String::new(), check: None, module: None, sep: true });
    for (id, label) in [
        (ID_RELOAD, t!("Перечитать настройки", "Reload config", ja: "設定を再読み込み", pt: "Recarregar configuração", zh: "重新加载配置")),
        (ID_OPEN, t!("Открыть настройки", "Open config", ja: "設定を開く", pt: "Abrir configuração", zh: "打开配置")),
        (ID_SETTINGS, t!("Настройки WinRust", "WinRust Settings", ja: "WinRust 設定", pt: "Configurações do WinRust", zh: "WinRust 设置")),
    ] {
        v.push(Item { id, label: label.to_string(), check: None, module: None, sep: false });
    }
    v.push(Item { id: ID_COLUMNS, label: t!("Две колонки", "Two columns", ja: "2 列", pt: "Duas colunas", zh: "两列").to_string(), check: Some(c.cols == 2), module: None, sep: false });
    v.push(Item { id: ID_RESET, label: t!("Сбросить позицию", "Reset position", ja: "位置をリセット", pt: "Redefinir posição", zh: "重置位置").to_string(), check: None, module: None, sep: false });
    v.push(Item { id: ID_CLOSE, label: t!("Закрыть", "Close", ja: "閉じる", pt: "Fechar", zh: "关闭").to_string(), check: None, module: None, sep: false });
    v
}

struct Menu {
    items: Vec<Item>,
    styles: [Style; 8],
    /// Top of every item, window coordinates.
    tops: Vec<i32>,
    hover: i32,
    sub_hover: i32,
    /// The module whose style panel is open.
    sub: Option<usize>,
    chosen: u32,
    done: bool,
    hwnd: HWND,
    pos: (i32, i32),
    fonts: Fonts,
    cv: Canvas,
    scale: f32,
    pad: i32,
    row_h: i32,
    sep_h: i32,
    main_w: i32,
    main_h: i32,
    sub_w: i32,
    sub_h: i32,
    sub_x: i32,
    sub_y: i32,
    opened: u64,
}

thread_local! {
    /// Address of the `Menu` behind the open window (cleared before `show` returns).
    static OPEN: RefCell<Option<usize>> = const { RefCell::new(None) };
}

fn inside(r: (i32, i32, i32, i32), x: i32, y: i32) -> bool {
    x >= r.0 && y >= r.1 && x < r.0 + r.2 && y < r.1 + r.3
}

fn card(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32, scale: f32) {
    let p = pal();
    let r = (12.0 * scale).round();
    // a soft shadow, then the body with its hairline rim
    for (i, a) in [14, 18, 22].into_iter().enumerate() {
        let e = (3 - i) as f32 * 2.0 * scale;
        cv.round_rect(x as f32 - e, y as f32 - e + 2.0, w as f32 + 2.0 * e, h as f32 + 2.0 * e, r + e, 0, a);
    }
    cv.round_rect(x as f32, y as f32, w as f32, h as f32, r, p.line, 250);
    cv.round_rect(x as f32 + 1.0, y as f32 + 1.0, w as f32 - 2.0, h as f32 - 2.0, (r - 1.0).max(0.0), p.bg, 250);
}

/// (top, height) of item `i` in window coordinates.
fn span(m: &Menu, i: usize) -> (i32, i32) {
    (m.tops[i], if m.items[i].sep { m.sep_h } else { m.row_h })
}

fn row_at(m: &Menu, y: i32) -> i32 {
    for i in 0..m.items.len() {
        let (top, h) = span(m, i);
        if y >= top && y < top + h {
            return i as i32;
        }
    }
    -1
}

/// (module, style index) under a point in the style panel.
fn sub_row(m: &Menu, x: i32, y: i32) -> Option<(usize, usize)> {
    let module = m.sub?;
    if !inside((m.sub_x, m.sub_y, m.sub_w, m.sub_h), x, y) {
        return None;
    }
    let k = (y - m.sub_y - m.pad).div_euclid(m.row_h);
    (0..5).contains(&k).then_some((module, k as usize))
}

fn repaint(m: &mut Menu) {
    let p = pal();
    m.cv.clear();
    card(&mut m.cv, 0, 0, m.main_w, m.main_h, m.scale);
    for i in 0..m.items.len() {
        let (top, h) = span(m, i);
        let it = &m.items[i];
        if it.sep {
            m.cv.round_rect((m.pad + 4) as f32, (top + h / 2) as f32, (m.main_w - m.pad * 2 - 8) as f32, 1.0, 0.5, p.line, 160);
            continue;
        }
        if m.hover == i as i32 {
            let r = (h - 8) as f32 / 2.0;
            m.cv.round_rect((m.pad / 2) as f32, (top + 4) as f32, (m.main_w - m.pad) as f32, (h - 8) as f32, r, p.text, 22);
        }
        let dot = (m.scale * 3.0) as i32;
        let cyd = top + h / 2;
        if let Some(on) = it.check {
            let c = if on { p.accent } else { p.dim };
            m.cv.round_rect((m.pad + 5) as f32 - dot as f32, (cyd - dot) as f32, (dot * 2) as f32, (dot * 2) as f32, dot as f32, c, if on { 255 } else { 110 });
        }
        let tx = m.pad + (18.0 * m.scale) as i32;
        m.cv.text((tx, top, m.main_w - tx - m.pad, h), &it.label, m.fonts.label, p.text, 240, Align::Left);
        if it.module.is_some() {
            let ax = m.main_w - m.pad - (16.0 * m.scale) as i32;
            m.cv.text((ax, top, (14.0 * m.scale) as i32, h), "›", m.fonts.mini, p.dim, 200, Align::Right);
        }
    }
    if let Some(module) = m.sub {
        card(&mut m.cv, m.sub_x, m.sub_y, m.sub_w, m.sub_h, m.scale);
        for (k, st) in Style::all().into_iter().enumerate() {
            let (top, h) = (m.sub_y + m.pad + k as i32 * m.row_h, m.row_h);
            if m.sub_hover == k as i32 {
                let r = (h - 8) as f32 / 2.0;
                m.cv.round_rect((m.sub_x + m.pad / 2) as f32, (top + 4) as f32, (m.sub_w - m.pad) as f32, (h - 8) as f32, r, p.text, 22);
            }
            let dot = (m.scale * 3.0) as i32;
            let cyd = top + h / 2;
            let selected = m.styles[module] == st;
            let c = if selected { p.accent } else { p.dim };
            m.cv.round_rect((m.sub_x + m.pad + 5) as f32 - dot as f32, (cyd - dot) as f32, (dot * 2) as f32, (dot * 2) as f32, dot as f32, c, if selected { 255 } else { 90 });
            let tx = m.sub_x + m.pad + (18.0 * m.scale) as i32;
            m.cv.text((tx, top, m.sub_x + m.sub_w - tx - m.pad, h), style_label(st), m.fonts.label, p.text, 240, Align::Left);
        }
    }
    m.cv.present(m.hwnd, m.pos);
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(addr) = OPEN.with(|o| *o.borrow()) else { return DefWindowProcW(hwnd, msg, wp, lp) };
    let m = &mut *(addr as *mut Menu);
    let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            BeginPaint(hwnd, &mut ps);
            EndPaint(hwnd, &ps);
            0
        }
        WM_MOUSEMOVE => {
            if m.sub.is_some() && inside((m.sub_x, m.sub_y, m.sub_w, m.sub_h), x, y) {
                let k = (y - m.sub_y - m.pad).div_euclid(m.row_h);
                if (0..5).contains(&k) {
                    if m.sub_hover != k {
                        m.sub_hover = k;
                        repaint(m);
                    }
                    return 0;
                }
                return 0; // padding of the panel: keep it open
            }
            let i = row_at(m, y);
            if i >= 0 && m.items[i as usize].module.is_some() {
                if let Some(module) = m.items[i as usize].module {
                    if m.sub != Some(module) {
                        let (top, _) = span(m, i as usize);
                        m.sub = Some(module);
                        m.sub_hover = -1;
                        m.sub_y = (top + m.row_h / 2 - m.sub_h / 2).clamp(m.pad, (m.main_h - m.sub_h - m.pad).max(m.pad));
                        m.hover = i;
                        repaint(m);
                    }
                    return 0;
                }
            }
            if m.hover != i || (i < 0 && m.sub.is_some()) {
                m.hover = i;
                m.sub_hover = -1;
                if i < 0 {
                    m.sub = None;
                }
                repaint(m);
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some((module, k)) = sub_row(m, x, y) {
                m.chosen = ID_STYLE + module as u32 * 8 + k as u32;
                m.done = true;
                return 0;
            }
            if inside((0, 0, m.main_w, m.main_h), x, y) {
                let i = row_at(m, y);
                if i >= 0 && !m.items[i as usize].sep {
                    m.chosen = m.items[i as usize].id;
                    m.done = true;
                }
                return 0;
            }
            m.done = true; // a click anywhere else dismisses
            0
        }
        WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
            m.done = true;
            0
        }
        WM_CAPTURECHANGED => {
            // another window took the capture: the menu is no longer modal
            if GetTickCount64().saturating_sub(m.opened) > 150 {
                m.done = true;
            }
            0
        }
        WM_TIMER => {
            if GetAsyncKeyState(VK_ESCAPE as i32) < 0 {
                m.done = true;
            }
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn as_style(cfg: &Config) -> [Style; 8] {
    cfg.style
}

/// Opens the menu at screen `(x, y)` and returns the chosen id (0 when nothing was picked).
pub unsafe fn show(x: i32, y: i32, cfg: &Config, scale: f32, context: Option<usize>) -> u32 {
    let items = items(cfg, context.map(|m| (m, cfg.detached[m])));
    let fonts = Fonts::new(winrust_common::style::ui_face(), scale);
    let pad = (8.0 * scale).round() as i32;
    let row_h = (30.0 * scale).round() as i32;
    let sep_h = (9.0 * scale).round() as i32;

    // measure on a throwaway surface: the real one needs the width first
    let probe = match Canvas::new(1, 1) {
        Some(c) => c,
        None => return 0,
    };
    let label_w = items.iter().map(|i| probe.measure(fonts.label, &i.label)).max().unwrap_or(80);
    let style_w = Style::all().iter().map(|s| probe.measure(fonts.label, style_label(*s))).max().unwrap_or(60);
    drop(probe);
    let main_w = (label_w + (58.0 * scale) as i32).clamp((150.0 * scale) as i32, (300.0 * scale) as i32);
    let sub_w = (style_w + (38.0 * scale) as i32).clamp((96.0 * scale) as i32, (180.0 * scale) as i32);
    let sub_x = main_w + (6.0 * scale) as i32;
    let mut main_h = pad;
    let mut tops = Vec::with_capacity(items.len());
    for it in &items {
        tops.push(main_h);
        main_h += if it.sep { sep_h } else { row_h };
    }
    main_h += pad;
    let sub_h = pad * 2 + row_h * 5;
    let (w, h) = (main_w + sub_x + sub_w, main_h);

    // keep the whole menu on the monitor under the pointer
    let (mut px, mut py) = (x, y);
    unsafe {
        let mon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if !mon.is_null() && GetMonitorInfoW(mon, &mut mi) != 0 {
            let (l, t, r, b) = (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom);
            px = x.min(r - w).max(l);
            py = y.min(b - h).max(t);
        }
    }
    let cv = match Canvas::new(w, h) {
        Some(c) => c,
        None => return 0,
    };
    let menu = Box::into_raw(Box::new(Menu {
        items,
        styles: as_style(cfg),
        tops,
        hover: -1,
        sub_hover: -1,
        sub: None,
        chosen: 0,
        done: false,
        hwnd: std::ptr::null_mut(),
        pos: (px, py),
        fonts,
        cv,
        scale,
        pad,
        row_h,
        sep_h,
        main_w,
        main_h,
        sub_w,
        sub_h,
        sub_x,
        sub_y: pad,
        opened: unsafe { GetTickCount64() },
    }));
    unsafe {
        let inst = GetModuleHandleW(std::ptr::null_mut());
        let class = wide(CLASS);
        let wc = WNDCLASSW { style: 0, lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc); // a second registration fails harmlessly
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class.as_ptr(),
            wide("rs-metrics menu").as_ptr(),
            WS_POPUP,
            px,
            py,
            w,
            h,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            inst,
            std::ptr::null_mut(),
        );
        if hwnd.is_null() {
            drop(Box::from_raw(menu));
            return 0;
        }
        (*menu).hwnd = hwnd;
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, menu as isize);
        OPEN.with(|o| *o.borrow_mut() = Some(menu as usize));
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetCapture(hwnd);
        SetTimer(hwnd, 1, 60, None);
        repaint(&mut *menu);
        let mut msg: MSG = std::mem::zeroed();
        while !(*menu).done {
            let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if r <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let chosen = (*menu).chosen;
        KillTimer(hwnd, 1);
        if GetCapture() == hwnd {
            ReleaseCapture();
        }
        OPEN.with(|o| *o.borrow_mut() = None);
        DestroyWindow(hwnd);
        drop(Box::from_raw(menu));
        chosen
    }
}
