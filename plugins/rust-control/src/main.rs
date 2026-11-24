#![windows_subsystem = "windows"]
// rust-control — a complete Control Panel for WinRust, drawn with the shared WinRust style.
//
// Pages: Home (classic Category view and a flat icon view), System, Programs, Services, Devices,
// Network, User Accounts, Power, Date and Time, Accessibility, Security and Administrative Tools.
// Every page reads real data through documented Win32 APIs; the actions that change something
// (uninstall, service start/stop/start type, power plan) ask first and report in the status bar.
//
// The window has its own chrome (a popup with a thick frame), a 200 px sidebar, a breadcrumb bar,
// a live search box, rounded cards and a status bar. Painting is double buffered.
#[macro_use]
extern crate winrust_common;

mod model;
mod pages;
mod sys;
mod ui;

use std::{
    cell::RefCell,
    mem::size_of,
    ptr::{null, null_mut},
    sync::atomic::{AtomicIsize, Ordering},
};

use model::{Action, Applet, Button, Category, HomeView, NAV, PageId, PageMeta, Row, Tone};
use ui::{F_BODY, F_CARD, F_ICON, F_ICON_S, F_LINK, F_NAV, F_SECTION, F_SMALL, F_TITLE, FONT_COUNT};
use winrust_common::style::{self, mix, rgb};

use windows_sys::Win32::{
    Foundation::*,
    Graphics::{
        Gdi::*,
        GdiPlus::{GdiplusShutdown, GdiplusStartup, GdiplusStartupInput},
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX},
        HiDpi::GetDpiForSystem,
        Input::KeyboardAndMouse::{GetKeyState, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_DOWN, VK_ESCAPE, VK_F5, VK_RETURN, VK_UP},
        WindowsAndMessaging::*,
    },
};

const TITLE_H: i32 = 44;
const TOP_H: i32 = 54;
const SIDE: i32 = 200;
const STATUS_H: i32 = 30;
const PAGE_HEAD: i32 = 66;
const HOME_HEAD: i32 = 58;
const NAV_TOP: i32 = 14;
const NAV_H: i32 = 38;
const ROW_PAD: i32 = 16;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_SEARCH_KEY: u32 = WM_APP + 7;
const SEARCH_ID: usize = 101;
const EM_SETCUEBANNER: u32 = 0x1501;
const EM_SETMARGINS: u32 = 0xD3;
const EM_LIMITTEXT: u32 = 0xC5;
const EN_CHANGE_CODE: u16 = 0x0300;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hot {
    Nav(usize),
    Title(u8),
    Breadcrumb,
    Selector,
    Link(usize),
    Tile(usize),
    Row(usize),
    Btn(usize, usize),
    Menu(usize),
}

struct MenuItem {
    label: String,
    action: Action,
    checked: bool,
}

struct Menu {
    x: i32,
    y: i32,
    w: i32,
    items: Vec<MenuItem>,
}

struct App {
    meta: Vec<PageMeta>,
    page: PageId,
    home_view: HomeView,
    query: String,
    raw: Vec<Row>,
    view: Vec<Row>,
    categories: Vec<Category>,
    applets: Vec<Applet>,
    scroll: i32,
    dpi: i32,
    fonts: [isize; FONT_COUNT],
    font_dpi: i32,
    font_style: u8,
    mono: bool,
    hover: Option<Hot>,
    menu: Option<Menu>,
    status: Option<String>,
    status_at: std::time::Instant,
    edit: isize,
    brush: isize,
    last_load: std::time::Instant,
}

impl Default for App {
    fn default() -> Self {
        App {
            meta: vec![],
            page: PageId::Home,
            home_view: HomeView::Category,
            query: String::new(),
            raw: vec![],
            view: vec![],
            categories: vec![],
            applets: vec![],
            scroll: 0,
            dpi: 96,
            fonts: [0; FONT_COUNT],
            font_dpi: 0,
            font_style: 255,
            mono: false,
            hover: None,
            menu: None,
            status: None,
            status_at: std::time::Instant::now(),
            edit: 0,
            brush: 0,
            last_load: std::time::Instant::now(),
        }
    }
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
}

static SEARCH_PROC: AtomicIsize = AtomicIsize::new(0);

// ───────────────────────────── geometry ─────────────────────────────

fn p(dpi: i32, v: i32) -> i32 {
    v * dpi / 96
}

fn content_rect(cw: i32, ch: i32, dpi: i32) -> RECT {
    RECT { left: p(dpi, SIDE) + p(dpi, 24), top: p(dpi, TITLE_H) + p(dpi, TOP_H), right: cw - p(dpi, 24), bottom: ch - p(dpi, STATUS_H) }
}

fn title_button_rects(cw: i32, dpi: i32) -> [RECT; 3] {
    let w = p(dpi, 46);
    let h = p(dpi, TITLE_H);
    let min = RECT { left: cw - 3 * w, top: 0, right: cw - 2 * w, bottom: h };
    let max = RECT { left: cw - 2 * w, top: 0, right: cw - w, bottom: h };
    let close = RECT { left: cw - w, top: 0, right: cw, bottom: h };
    [min, max, close]
}

fn search_rect(cw: i32, dpi: i32) -> RECT {
    let w = p(dpi, 252);
    let h = p(dpi, 32);
    let right = cw - p(dpi, 22);
    let top = p(dpi, TITLE_H) + (p(dpi, TOP_H) - h) / 2;
    RECT { left: right - w, top, right, bottom: top + h }
}

fn search_edit_rect(cw: i32, dpi: i32) -> RECT {
    let r = search_rect(cw, dpi);
    RECT { left: r.left + p(dpi, 34), top: r.top + p(dpi, 4), right: r.right - p(dpi, 12), bottom: r.bottom - p(dpi, 4) }
}

fn selector_rect(cw: i32, dpi: i32) -> RECT {
    let c = content_rect(cw, 1000, dpi);
    RECT { left: c.right - p(dpi, 198), top: c.top + p(dpi, 8), right: c.right, bottom: c.top + p(dpi, 38) }
}

fn crumb_rect(cw: i32, dpi: i32) -> RECT {
    let c = content_rect(cw, 1000, dpi);
    RECT { left: c.left, top: p(dpi, TITLE_H), right: c.left + p(dpi, 132), bottom: p(dpi, TITLE_H) + p(dpi, TOP_H) }
}

fn contains(r: &RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn nav_index_at(cw: i32, dpi: i32, x: i32, y: i32) -> Option<usize> {
    if x >= p(dpi, SIDE) || y < p(dpi, TITLE_H) {
        return None;
    }
    let top = p(dpi, TITLE_H) + p(dpi, NAV_TOP);
    let h = p(dpi, NAV_H);
    if y < top {
        return None;
    }
    let i = ((y - top) / h) as usize;
    (i < NAV.len() && (y - top) % h < h - p(dpi, 2)).then_some(i)
}

fn page_meta(a: &App) -> Option<&PageMeta> {
    a.meta.get(a.page.index())
}

// ───────────────────────────── data and filtering ─────────────────────────────

fn filter_rows(raw: &[Row], q: &str) -> Vec<Row> {
    if q.is_empty() {
        return raw.to_vec();
    }
    let mut out = vec![];
    let mut i = 0;
    while i < raw.len() {
        if let Row::Section(title) = &raw[i] {
            let start = i + 1;
            let mut end = start;
            while end < raw.len() && !matches!(&raw[end], Row::Section(_)) {
                end += 1;
            }
            let title_match = title.to_lowercase().contains(q);
            let any = title_match || raw[start..end].iter().any(|r| r.haystack().contains(q));
            if any {
                out.push(raw[i].clone());
                if title_match {
                    out.extend_from_slice(&raw[start..end]);
                } else {
                    out.extend(raw[start..end].iter().filter(|r| r.haystack().contains(q)).cloned());
                }
            }
            i = end;
        } else {
            if raw[i].haystack().contains(q) {
                out.push(raw[i].clone());
            }
            i += 1;
        }
    }
    out
}

fn filter_categories(cats: Vec<Category>, q: &str) -> Vec<Category> {
    if q.is_empty() {
        return cats;
    }
    cats.into_iter()
        .filter_map(|mut c| {
            if c.title.to_lowercase().contains(q) {
                return Some(c);
            }
            c.links.retain(|l| l.label.to_lowercase().contains(q));
            (!c.links.is_empty()).then_some(c)
        })
        .collect()
}

fn filter_applets(list: Vec<Applet>, q: &str) -> Vec<Applet> {
    if q.is_empty() {
        return list;
    }
    list.into_iter().filter(|a| a.keywords.to_lowercase().contains(q) || a.title.to_lowercase().contains(q)).collect()
}

fn rebuild_filter(hwnd: HWND) {
    let (page, q, meta) = APP.with_borrow(|a| (a.page, a.query.trim().to_lowercase(), a.meta.clone()));
    if page == PageId::Home {
        let cats = filter_categories(pages::home::categories(), &q);
        let applets = filter_applets(pages::home::applets(&meta), &q);
        APP.with_borrow_mut(|a| {
            a.categories = cats;
            a.applets = applets;
            a.hover = None;
            a.menu = None;
        });
    } else {
        let view = APP.with_borrow(|a| filter_rows(&a.raw, &q));
        APP.with_borrow_mut(|a| {
            a.view = view;
            a.hover = None;
            a.menu = None;
        });
    }
    clamp_scroll(hwnd);
}

fn load_current(hwnd: HWND, force: bool) {
    let (page, empty) = APP.with_borrow(|a| (a.page, a.raw.is_empty()));
    if page == PageId::Home {
        rebuild_filter(hwnd);
        return;
    }
    if !force && !empty {
        return;
    }
    let rows = pages::load(page);
    APP.with_borrow_mut(|a| {
        a.raw = rows;
        a.last_load = std::time::Instant::now();
    });
    rebuild_filter(hwnd);
}

fn goto(hwnd: HWND, page: PageId) {
    {
        let ed = APP.with_borrow(|a| a.edit);
        if ed != 0 {
            let empty = sys::wide("");
            unsafe { SetWindowTextW(ed as HWND, empty.as_ptr()) };
        }
    }
    APP.with_borrow_mut(|a| {
        a.page = page;
        a.query.clear();
        a.scroll = 0;
        a.status = None;
        a.menu = None;
        a.hover = None;
        a.raw.clear();
        a.view.clear();
    });
    if page == PageId::Home {
        rebuild_filter(hwnd);
    } else {
        let rows = pages::load(page);
        APP.with_borrow_mut(|a| {
            a.raw = rows;
            a.last_load = std::time::Instant::now();
        });
        rebuild_filter(hwnd);
    }
    unsafe {
        SetFocus(hwnd);
        InvalidateRect(hwnd, null(), 0);
    }
}

fn set_status(text: impl Into<String>) {
    let text = text.into();
    APP.with_borrow_mut(|a| {
        a.status = Some(text);
        a.status_at = std::time::Instant::now();
    });
}

fn clear_search(hwnd: HWND) {
    let ed = APP.with_borrow(|a| a.edit);
    if ed != 0 {
        let empty = sys::wide("");
        unsafe { SetWindowTextW(ed as HWND, empty.as_ptr()) };
    }
    APP.with_borrow_mut(|a| {
        a.query.clear();
        a.scroll = 0;
    });
    rebuild_filter(hwnd);
    unsafe {
        SetFocus(hwnd);
        InvalidateRect(hwnd, null(), 0);
    }
}

// ───────────────────────────── scrolling and row hit testing ─────────────────────────────

fn page_scroll_max(a: &App, cw: i32, ch: i32) -> i32 {
    let content = content_rect(cw, ch, a.dpi);
    let top = content.top + p(a.dpi, PAGE_HEAD);
    let total: i32 = a.view.iter().map(|r| r.height()).sum();
    let visible = (content.bottom - top) * 96 / a.dpi.max(1);
    (total - visible).max(0)
}

fn scroll_max(a: &App, cw: i32, ch: i32) -> i32 {
    if a.page == PageId::Home {
        let content = content_rect(cw, ch, a.dpi);
        let layout = home_layout(a, content, 0);
        let body_top = content.top + p(a.dpi, HOME_HEAD);
        ((layout.end - (body_top + (content.bottom - body_top))) * 96 / a.dpi.max(1)).max(0)
    } else {
        page_scroll_max(a, cw, ch)
    }
}

fn clamp_scroll(hwnd: HWND) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let max = APP.with_borrow(|a| scroll_max(a, rc.right, rc.bottom));
        APP.with_borrow_mut(|a| a.scroll = a.scroll.clamp(0, max));
    }
}

fn row_at_pos(a: &App, content: RECT, x: i32, y: i32) -> Option<(usize, RECT)> {
    let top = content.top + p(a.dpi, PAGE_HEAD);
    if x < content.left || x >= content.right || y < top || y >= content.bottom {
        return None;
    }
    let mut at = top - p(a.dpi, a.scroll);
    for (i, row) in a.view.iter().enumerate() {
        let h = p(a.dpi, row.height());
        let r = RECT { left: content.left, top: at, right: content.right, bottom: at + h };
        if y >= r.top && y < r.bottom {
            return Some((i, r));
        }
        at += h;
        if at > content.bottom {
            break;
        }
    }
    None
}

fn button_rects(dc: HDC, font: isize, row: RECT, buttons: &[Button], dpi: i32) -> Vec<RECT> {
    let h = p(dpi, 24);
    let gap = p(dpi, 8);
    let mut right = row.right - p(dpi, ROW_PAD);
    let mut out = vec![];
    for b in buttons.iter().rev() {
        let w = unsafe { ui::text_width(dc, font, &b.label) } + p(dpi, 22);
        let r = RECT { left: right - w, top: row.top + (row.bottom - row.top - h) / 2, right, bottom: row.top + (row.bottom - row.top - h) / 2 + h };
        out.push(r);
        right = r.left - gap;
    }
    out.reverse();
    out
}

fn row_buttons(a: &App, row: &Row) -> Vec<Button> {
    match row {
        Row::Entry { buttons, .. } | Row::Table { buttons, .. } => buttons.clone(),
        _ => vec![],
    }
}

fn row_action(row: &Row) -> Option<Action> {
    match row {
        Row::Entry { action, .. } | Row::Table { action, .. } => action.clone(),
        _ => None,
    }
}

fn row_hot(hwnd: HWND, a: &App, i: usize, r: RECT, x: i32, y: i32) -> Option<Hot> {
    let buttons = row_buttons(a, &a.view[i]);
    if !buttons.is_empty() {
        unsafe {
            let dc = GetDC(hwnd);
            let rects = button_rects(dc, a.fonts[F_SMALL], r, &buttons, a.dpi);
            ReleaseDC(hwnd, dc);
            for (j, br) in rects.iter().enumerate() {
                if contains(br, x, y) {
                    return Some(Hot::Btn(i, j));
                }
            }
        }
    }
    row_action(&a.view[i]).map(|_| Hot::Row(i))
}

// ───────────────────────────── menus ─────────────────────────────

fn menu_rect(m: &Menu, dpi: i32) -> RECT {
    RECT { left: m.x, top: m.y, right: m.x + m.w, bottom: m.y + p(dpi, 12) + p(dpi, 34) * m.items.len() as i32 }
}

fn menu_item_rect(m: &Menu, dpi: i32, i: usize) -> RECT {
    let r = menu_rect(m, dpi);
    let top = r.top + p(dpi, 6) + p(dpi, 34) * i as i32;
    RECT { left: r.left + p(dpi, 6), top, right: r.right - p(dpi, 6), bottom: top + p(dpi, 34) }
}

fn menu_item_at(m: &Menu, dpi: i32, x: i32, y: i32) -> Option<(usize, Action)> {
    for (i, item) in m.items.iter().enumerate() {
        if contains(&menu_item_rect(m, dpi, i), x, y) {
            return Some((i, item.action.clone()));
        }
    }
    None
}

fn open_menu(hwnd: HWND, anchor: RECT, items: Vec<MenuItem>) {
    if items.is_empty() {
        return;
    }
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let dpi = APP.with_borrow(|a| a.dpi);
        let (font, widest) = APP.with_borrow(|a| {
            let dc = GetDC(hwnd);
            let w = items.iter().map(|it| ui::text_width(dc, a.fonts[F_BODY], &it.label)).max().unwrap_or(0);
            ReleaseDC(hwnd, dc);
            (a.fonts[F_BODY], w)
        });
        let _ = font;
        let w = widest + p(dpi, 58);
        let h = p(dpi, 12) + p(dpi, 34) * items.len() as i32;
        let min_x = p(dpi, SIDE) + p(dpi, 8);
        let max_x = (rc.right - p(dpi, 8) - w).max(min_x);
        let x = anchor.left.clamp(min_x, max_x);
        let mut y = anchor.bottom + p(dpi, 4);
        if y + h > rc.bottom - p(dpi, STATUS_H + 8) {
            y = anchor.top - h - p(dpi, 4);
        }
        let y = y.max(p(dpi, TITLE_H) + p(dpi, 4));
        let default = items.iter().position(|it| it.checked).or(Some(0));
        APP.with_borrow_mut(|a| {
            a.menu = Some(Menu { x, y, w, items });
            a.hover = default.map(Hot::Menu);
        });
        InvalidateRect(hwnd, null(), 0);
    }
}

fn home_view_menu(hwnd: HWND) {
    let (cw, dpi) = unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        (rc.right, APP.with_borrow(|a| a.dpi))
    };
    let current = APP.with_borrow(|a| a.home_view);
    let items = vec![
        MenuItem { label: t!("Категория", "Category", ja: "カテゴリ", pt: "Categoria", zh: "类别").into(), action: Action::SetHomeView(false), checked: current == HomeView::Category },
        MenuItem { label: t!("Значки", "Icons", ja: "アイコン", pt: "Ícones", zh: "图标").into(), action: Action::SetHomeView(true), checked: current == HomeView::Icons },
    ];
    open_menu(hwnd, selector_rect(cw, dpi), items);
}

fn service_menu(hwnd: HWND, anchor: RECT, name: String, label: String, current: u32) {
    let items = vec![
        MenuItem { label: t!("Автоматически", "Automatic", ja: "自動", pt: "Automático", zh: "自动").into(), action: Action::ServiceStartup { name: name.clone(), label: label.clone(), start: 2 }, checked: current == 2 },
        MenuItem { label: t!("Вручную", "Manual", ja: "手動", pt: "Manual", zh: "手动").into(), action: Action::ServiceStartup { name: name.clone(), label: label.clone(), start: 3 }, checked: current == 3 },
        MenuItem { label: t!("Отключена", "Disabled", ja: "無効", pt: "Desativado", zh: "已禁用").into(), action: Action::ServiceStartup { name, label, start: 4 }, checked: current == 4 },
    ];
    open_menu(hwnd, anchor, items);
}

// ───────────────────────────── painting ─────────────────────────────

unsafe fn paint(hwnd: HWND) {
    unsafe {
        refresh_fonts();
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (cw, ch) = (rc.right, rc.bottom);
        if cw <= 0 || ch <= 0 {
            EndPaint(hwnd, &ps);
            return;
        }
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, cw, ch);
        if mem.is_null() || bmp.is_null() {
            if !bmp.is_null() {
                DeleteObject(bmp as _);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            EndPaint(hwnd, &ps);
            return;
        }
        let old = SelectObject(mem, bmp);
        let t = ui::theme();
        APP.with_borrow(|a| {
            ui::shape(mem, rc, t.bg, 0);
            paint_title_bar(hwnd, mem, a, &t, cw);
            paint_sidebar(mem, a, &t, ch);
            paint_topbar(mem, a, &t, cw);
            if a.page == PageId::Home {
                paint_home(mem, a, &t, cw, ch);
            } else {
                paint_page(mem, a, &t, cw, ch);
            }
            paint_status(mem, a, &t, cw, ch);
            if let Some(m) = &a.menu {
                paint_menu(mem, a, &t, m);
            }
        });
        BitBlt(dc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp as _);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

fn paint_title_bar(hwnd: HWND, dc: HDC, a: &App, t: &ui::Theme, cw: i32) {
    unsafe {
        let d = a.dpi;
        let bar = RECT { left: 0, top: 0, right: cw, bottom: p(d, TITLE_H) };
        ui::shape(dc, bar, t.side, 0);
        ui::hline(dc, 0, cw, p(d, TITLE_H) - 1, t.sep);
        let icon = p(d, 22);
        let ir = RECT { left: p(d, 16), top: (p(d, TITLE_H) - icon) / 2, right: p(d, 16) + icon, bottom: (p(d, TITLE_H) + icon) / 2 };
        ui::shape(dc, ir, t.accent, p(d, 6));
        ui::text(dc, a.fonts[F_ICON_S], "\u{E713}", ir, winrust_common::style::on_accent(), DT_CENTER);
        ui::text(dc, a.fonts[F_CARD], t!("Панель управления", "Control Panel", ja: "コントロール パネル", pt: "Painel de Controle", zh: "控制面板"), RECT { left: p(d, 48), right: cw - p(d, 160), ..bar }, t.text, DT_LEFT);
        let btns = title_button_rects(cw, d);
        let maximized = IsZoomed(hwnd) != 0;
        for (i, r) in btns.iter().enumerate() {
            let hot = a.hover == Some(Hot::Title(i as u8));
            if hot {
                let fill = if i == 2 { mix(t.side, t.bad, 0.62) } else { mix(t.side, t.text, 0.09) };
                ui::shape(dc, *r, fill, 0);
            }
            let glyph = match i {
                0 => "\u{E921}",
                1 if maximized => "\u{E923}",
                1 => "\u{E922}",
                _ => "\u{E8BB}",
            };
            let ink = if hot && i == 2 { rgb(255, 255, 255) } else { t.dim };
            ui::text(dc, a.fonts[F_ICON_S], glyph, *r, ink, DT_CENTER);
        }
    }
}

fn paint_sidebar(dc: HDC, a: &App, t: &ui::Theme, ch: i32) {
    unsafe {
        let d = a.dpi;
        let side = RECT { left: 0, top: p(d, TITLE_H), right: p(d, SIDE), bottom: ch };
        ui::shape(dc, side, t.side, 0);
        ui::vline(dc, p(d, SIDE), p(d, TITLE_H), ch, t.sep);
        let top = p(d, TITLE_H) + p(d, NAV_TOP);
        let h = p(d, NAV_H);
        for (i, id) in NAV.iter().enumerate() {
            let r = RECT { left: p(d, 10), top: top + h * i as i32, right: p(d, SIDE - 10), bottom: top + h * (i as i32 + 1) - p(d, 2) };
            let selected = a.page == *id;
            if selected {
                ui::shape(dc, r, t.active, p(d, 8));
                ui::shape(dc, RECT { left: r.left, top: r.top + p(d, 8), right: r.left + p(d, 3), bottom: r.bottom - p(d, 8) }, t.accent, p(d, 2));
            } else if a.hover == Some(Hot::Nav(i)) {
                ui::shape(dc, r, t.hover, p(d, 8));
            }
            let meta = a.meta.get(i);
            let icon = meta.map_or("\u{E80F}", |m| m.icon);
            let title = meta.map_or_else(|| t!("Главная", "Home", ja: "ホーム", pt: "Início", zh: "主页").to_string(), |m| m.title.clone());
            let ir = RECT { left: r.left + p(d, 12), top: r.top, right: r.left + p(d, 36), bottom: r.bottom };
            ui::text(dc, a.fonts[F_ICON_S], icon, ir, if selected { t.accent } else { mix(t.dim, t.text, 0.35) }, DT_CENTER);
            ui::text(dc, a.fonts[F_NAV], &title, RECT { left: r.left + p(d, 42), ..r }, if selected { t.accent } else { mix(t.dim, t.text, 0.72) }, DT_LEFT);
        }
    }
}

fn paint_topbar(dc: HDC, a: &App, t: &ui::Theme, cw: i32) {
    unsafe {
        let d = a.dpi;
        let c = content_rect(cw, 1000, d);
        let top = p(d, TITLE_H);
        let bottom = top + p(d, TOP_H);
        ui::hline(dc, p(d, SIDE) + 1, cw, bottom - 1, t.sep);
        let y = RECT { left: c.left, top, right: c.right, bottom };
        let crumb = t!("Панель управления", "Control Panel", ja: "コントロール パネル", pt: "Painel de Controle", zh: "控制面板");
        let w = ui::text_width(dc, a.fonts[F_BODY], crumb);
        ui::text(dc, a.fonts[F_BODY], crumb, RECT { right: c.left + w, ..y }, t.accent, DT_LEFT);
        if a.page != PageId::Home {
            let meta = page_meta(a);
            let chev = RECT { left: c.left + w + p(d, 8), right: c.left + w + p(d, 26), ..y };
            ui::text(dc, a.fonts[F_ICON_S], "\u{E76C}", chev, t.dim, DT_CENTER);
            let title = meta.map_or("", |m| m.title.as_str());
            ui::text(dc, a.fonts[F_BODY], title, RECT { left: chev.right + p(d, 4), right: search_rect(cw, d).left - p(d, 20), ..y }, t.text, DT_LEFT);
        }
        let sr = search_rect(cw, d);
        ui::shape(dc, sr, t.card, p(d, 16));
        ui::text(dc, a.fonts[F_ICON_S], "\u{E721}", RECT { left: sr.left + p(d, 10), right: sr.left + p(d, 28), ..sr }, t.dim, DT_CENTER);
    }
}

fn paint_page(dc: HDC, a: &App, t: &ui::Theme, cw: i32, ch: i32) {
    unsafe {
        let d = a.dpi;
        let c = content_rect(cw, ch, d);
        let meta = page_meta(a);
        let title = meta.map_or("", |m| m.title.as_str());
        let description = meta.map_or("", |m| m.description.as_str());
        ui::shape(dc, RECT { left: c.left - p(d, 12), top: c.top + p(d, 9), right: c.left - p(d, 9), bottom: c.top + p(d, 37) }, t.accent, p(d, 2));
        ui::text(dc, a.fonts[F_TITLE], title, RECT { left: c.left, top: c.top + p(d, 4), right: c.right, bottom: c.top + p(d, 40) }, t.text, DT_LEFT);
        ui::text(dc, a.fonts[F_SMALL], description, RECT { left: c.left, top: c.top + p(d, 40), right: c.right, bottom: c.top + p(d, 60) }, t.dim, DT_LEFT);
        paint_rows(dc, a, t, c, cw, ch);
    }
}

fn paint_rows(dc: HDC, a: &App, t: &ui::Theme, content: RECT, cw: i32, ch: i32) {
    unsafe {
        let d = a.dpi;
        let top = content.top + p(d, PAGE_HEAD);
        let clip = RECT { left: content.left, top, right: content.right, bottom: content.bottom };
        let saved = SaveDC(dc);
        IntersectClipRect(dc, clip.left, clip.top, clip.right, clip.bottom);
        let mut y = top - p(d, a.scroll);
        let mut i = 0;
        while i < a.view.len() {
            match &a.view[i] {
                Row::Section(title) => {
                    let h = p(d, 34);
                    if y + h >= clip.top && y <= clip.bottom {
                        ui::text(dc, a.fonts[F_SECTION], &title.to_uppercase(), RECT { left: content.left + p(d, 2), top: y, right: content.right, bottom: y + h }, t.dim, DT_LEFT);
                    }
                    y += h;
                    i += 1;
                }
                Row::Note { text, tone } => {
                    let h = p(d, 40);
                    let r = RECT { left: content.left, top: y, right: content.right, bottom: y + h };
                    if r.bottom >= clip.top && r.top <= clip.bottom {
                        let ink = ui::tone_color(*tone, t);
                        ui::shape(dc, r, mix(t.card, ink, 0.10), p(d, 8));
                        ui::text(dc, a.fonts[F_SMALL], text, RECT { left: r.left + p(d, 14), right: r.right - p(d, 14), ..r }, ink, DT_LEFT);
                    }
                    y += h;
                    i += 1;
                }
                _ => {
                    let start = i;
                    let mut end = i;
                    let mut gh = 0;
                    while end < a.view.len() && !matches!(&a.view[end], Row::Section(_) | Row::Note { .. }) {
                        gh += p(d, a.view[end].height());
                        end += 1;
                    }
                    let group = RECT { left: content.left, top: y, right: content.right, bottom: y + gh };
                    if group.bottom >= clip.top && group.top <= clip.bottom {
                        ui::shape(dc, group, t.card, p(d, 10));
                    }
                    let mut ry = y;
                    for j in start..end {
                        let rh = p(d, a.view[j].height());
                        let r = RECT { left: content.left, top: ry, right: content.right, bottom: ry + rh };
                        if r.bottom >= clip.top && r.top <= clip.bottom {
                            if j > start {
                                ui::hline(dc, r.left + p(d, ROW_PAD), r.right - p(d, ROW_PAD), r.top, t.sep);
                            }
                            if a.hover == Some(Hot::Row(j)) {
                                ui::shape(dc, RECT { left: r.left + p(d, 4), top: r.top + p(d, 2), right: r.right - p(d, 4), bottom: r.bottom - p(d, 2) }, t.hover, p(d, 8));
                            }
                            paint_row(dc, a, t, j, &a.view[j], r);
                        }
                        ry += rh;
                    }
                    y = ry;
                    i = end;
                }
            }
        }
        RestoreDC(dc, saved);
        // A thin scrollbar when the page does not fit.
        let max = scroll_max(a, cw, ch);
        if max > 0 {
            let max_dev = p(d, max);
            let view_h = (clip.bottom - clip.top).max(1);
            let total = max_dev + view_h;
            let track = RECT { left: cw - p(d, 10), top: clip.top + p(d, 2), right: cw - p(d, 7), bottom: clip.bottom - p(d, 2) };
            ui::shape(dc, track, t.off, p(d, 2));
            let th = ((view_h * view_h) / total).max(p(d, 18));
            let travel = (view_h - th).max(0);
            let ty = clip.top + p(d, 2) + p(d, a.scroll) * travel / max_dev.max(1);
            ui::shape(dc, RECT { left: track.left, top: ty, right: track.right, bottom: ty + th }, t.dim, p(d, 2));
        }
    }
}

fn paint_row(dc: HDC, a: &App, t: &ui::Theme, idx: usize, row: &Row, r: RECT) {
    unsafe {
        let d = a.dpi;
        let left = r.left + p(d, ROW_PAD);
        match row {
            Row::Section(_) | Row::Note { .. } => {}
            Row::Info { label, value, tone } => {
                let lw = ui::text_width(dc, a.fonts[F_BODY], label).min((r.right - left - p(d, ROW_PAD)) / 2).max(0);
                ui::text(dc, a.fonts[F_BODY], label, RECT { left, right: left + lw, ..r }, t.text, DT_LEFT);
                ui::text(dc, a.fonts[F_BODY], value, RECT { left: left + lw + p(d, 16), right: r.right - p(d, ROW_PAD), ..r }, ui::tone_color(*tone, t), DT_RIGHT);
            }
            Row::Head { cols, reserve } => paint_table(dc, a, t, idx, r, cols, &[], *reserve, true),
            Row::Table { cols, buttons, reserve, .. } => paint_table(dc, a, t, idx, r, cols, buttons, *reserve, false),
            Row::Entry { title, subtitle, detail, value, tone, action, buttons } => {
                let mut right = r.right - p(d, ROW_PAD);
                if !buttons.is_empty() {
                    let rects = button_rects(dc, a.fonts[F_SMALL], r, buttons, d);
                    for (j, br) in rects.iter().enumerate() {
                        let hot = a.hover == Some(Hot::Btn(idx, j));
                        ui::button(dc, *br, &buttons[j].label, buttons[j].kind, hot, a.fonts[F_SMALL], t);
                    }
                    if let Some(first) = rects.first() {
                        right = first.left - p(d, 12);
                    }
                }
                if action.is_some() && buttons.is_empty() {
                    let chev = RECT { left: right - p(d, 16), right, ..r };
                    ui::text(dc, a.fonts[F_ICON_S], "\u{E76C}", chev, t.dim, DT_CENTER);
                    right -= p(d, 26);
                }
                let ink = ui::tone_color(*tone, t);
                let mut title_right = right;
                let multiline = !detail.is_empty() || !subtitle.is_empty();
                if !value.is_empty() {
                    let vw = ui::text_width(dc, a.fonts[F_SMALL], value);
                    let dot = p(d, 6);
                    let cy = if multiline { r.top + p(d, 16) } else { (r.top + r.bottom) / 2 };
                    if *tone != Tone::Neutral {
                        let dx = right - vw - p(d, 12) - dot;
                        ui::shape(dc, RECT { left: dx, top: cy - dot / 2, right: dx + dot, bottom: cy + dot / 2 }, ink, dot / 2);
                        title_right = dx - p(d, 8);
                    } else {
                        title_right = right - vw - p(d, 12);
                    }
                    let value_r = if multiline {
                        RECT { left: right - vw, top: r.top + p(d, 6), right, bottom: r.top + p(d, 26) }
                    } else {
                        RECT { left: right - vw, right, ..r }
                    };
                    ui::text(dc, a.fonts[F_SMALL], value, value_r, ink, DT_RIGHT);
                }
                let (title_r, sub_r, detail_r) = if !detail.is_empty() {
                    (RECT { left, top: r.top + p(d, 6), right: title_right, bottom: r.top + p(d, 24) }, RECT { left, top: r.top + p(d, 22), right: right, bottom: r.top + p(d, 38) }, RECT { left, top: r.top + p(d, 38), right: right, bottom: r.top + p(d, 54) })
                } else if !subtitle.is_empty() {
                    (RECT { left, top: r.top + p(d, 6), right: title_right, bottom: r.top + p(d, 26) }, RECT { left, top: r.top + p(d, 24), right: right, bottom: r.top + p(d, 42) }, RECT { ..r })
                } else {
                    (RECT { left, right: title_right, ..r }, RECT { ..r }, RECT { ..r })
                };
                ui::text(dc, a.fonts[F_BODY], title, title_r, t.text, DT_LEFT);
                if !subtitle.is_empty() {
                    ui::text(dc, a.fonts[F_SMALL], subtitle, sub_r, t.dim, DT_LEFT);
                }
                if !detail.is_empty() {
                    ui::text(dc, a.fonts[F_SMALL], detail, detail_r, t.dim, DT_LEFT);
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_table(dc: HDC, a: &App, t: &ui::Theme, idx: usize, r: RECT, cols: &[model::Col], buttons: &[Button], reserve: i32, head: bool) {
    unsafe {
        let d = a.dpi;
        let mut right = r.right - p(d, ROW_PAD);
        if !buttons.is_empty() {
            let rects = button_rects(dc, a.fonts[F_SMALL], r, buttons, d);
            for (j, br) in rects.iter().enumerate() {
                let hot = a.hover == Some(Hot::Btn(idx, j));
                ui::button(dc, *br, &buttons[j].label, buttons[j].kind, hot, a.fonts[F_SMALL], t);
            }
            if let Some(first) = rects.first() {
                right = first.left - p(d, 12);
            }
        } else if reserve > 0 {
            right -= p(d, reserve);
        }
        let left = r.left + p(d, ROW_PAD);
        let total: i32 = cols.iter().map(|c| c.weight.max(1) as i32).sum();
        let avail = (right - left).max(1);
        let mut x = left;
        for (ci, c) in cols.iter().enumerate() {
            let w = if ci + 1 == cols.len() { right - x } else { avail * c.weight.max(1) as i32 / total };
            let cr = RECT { left: x, top: r.top, right: x + w - p(d, 10), bottom: r.bottom };
            let font = if head { a.fonts[F_SMALL] } else if ci == 0 { a.fonts[F_BODY] } else { a.fonts[F_SMALL] };
            let ink = if head { t.dim } else { ui::tone_color(c.tone, t) };
            let text = if head { c.text.to_uppercase() } else { c.text.clone() };
            ui::text(dc, font, &text, cr, ink, DT_LEFT);
            x += w;
        }
    }
}

fn paint_home(dc: HDC, a: &App, t: &ui::Theme, cw: i32, ch: i32) {
    unsafe {
        let d = a.dpi;
        let c = content_rect(cw, ch, d);
        ui::shape(dc, RECT { left: c.left - p(d, 12), top: c.top + p(d, 9), right: c.left - p(d, 9), bottom: c.top + p(d, 37) }, t.accent, p(d, 2));
        ui::text(dc, a.fonts[F_TITLE], t!("Настройка параметров компьютера", "Adjust your computer's settings", ja: "コンピューターの設定を調整する", pt: "Ajustar as configurações do computador", zh: "调整计算机的设置"), RECT { left: c.left, top: c.top + p(d, 6), right: c.right - p(d, 210), bottom: c.top + p(d, 42) }, t.text, DT_LEFT);
        // "View by" pill
        let sr = selector_rect(cw, d);
        if a.hover == Some(Hot::Selector) {
            ui::shape(dc, sr, t.hover, p(d, 8));
        }
        ui::text(dc, a.fonts[F_SMALL], t!("Просмотр:", "View by:", ja: "表示:", pt: "Exibir por:", zh: "查看方式："), RECT { left: sr.left + p(d, 10), right: sr.left + p(d, 74), ..sr }, t.dim, DT_LEFT);
        let value = if a.home_view == HomeView::Category {
            t!("Категория", "Category", ja: "カテゴリ", pt: "Categoria", zh: "类别")
        } else {
            t!("Значки", "Icons", ja: "アイコン", pt: "Ícones", zh: "图标")
        };
        let chev = RECT { left: sr.right - p(d, 24), right: sr.right - p(d, 8), ..sr };
        ui::text(dc, a.fonts[F_ICON_S], "\u{E70D}", chev, t.dim, DT_CENTER);
        ui::text(dc, a.fonts[F_BODY], value, RECT { left: sr.left + p(d, 76), right: chev.left - p(d, 4), ..sr }, t.accent, DT_RIGHT);

        let layout = home_layout(a, c, a.scroll);
        let saved = SaveDC(dc);
        IntersectClipRect(dc, c.left, c.top + p(d, HOME_HEAD), c.right, c.bottom);
        if a.home_view == HomeView::Category {
            if a.categories.is_empty() {
                ui::text(dc, a.fonts[F_BODY], t!("Нет категорий, соответствующих запросу.", "No categories match the search.", ja: "検索に一致するカテゴリはありません。", pt: "Nenhuma categoria corresponde à pesquisa.", zh: "没有匹配搜索的类别。"), RECT { left: c.left, top: c.top + p(d, HOME_HEAD), right: c.right, bottom: c.top + p(d, HOME_HEAD + 40) }, t.dim, DT_LEFT);
                RestoreDC(dc, saved);
                return;
            }
            for (r, i) in &layout.cards {
                let cat = &a.categories[*i];
                if r.bottom < c.top || r.top > c.bottom {
                    continue;
                }
                ui::shape(dc, *r, t.card, p(d, 10));
                let tile = p(d, 40);
                let tr = RECT { left: r.left + p(d, 16), top: r.top + p(d, 9), right: r.left + p(d, 16) + tile, bottom: r.top + p(d, 9) + tile };
                ui::shape(dc, tr, cat.tile, p(d, 10));
                ui::text(dc, a.fonts[F_ICON], cat.icon, tr, rgb(255, 255, 255), DT_CENTER);
                let tx = r.left + p(d, 68);
                ui::text(dc, a.fonts[F_CARD], &cat.title, RECT { left: tx, top: r.top + p(d, 8), right: r.right - p(d, 14), bottom: r.top + p(d, 30) }, t.accent, DT_LEFT);
            }
            for (r, action, gi, label) in &layout.links {
                let _ = action;
                let hot = a.hover == Some(Hot::Link(*gi));
                let ink = if hot { t.accent } else { t.dim };
                if r.bottom - r.top > p(d, 20) {
                    ui::text_wrap(dc, a.fonts[F_LINK], label, RECT { left: r.left, right: r.right, top: r.top, bottom: r.bottom }, ink);
                } else {
                    ui::text(dc, a.fonts[F_LINK], label, RECT { left: r.left, right: r.right, top: r.top, bottom: r.bottom }, ink, DT_LEFT);
                }
                if hot && r.bottom - r.top <= p(d, 20) {
                    let w = ui::text_width(dc, a.fonts[F_LINK], label);
                    ui::hline(dc, r.left, r.left + w, r.bottom - p(d, 2), t.accent);
                }
            }
        } else {
            if a.applets.is_empty() {
                ui::text(dc, a.fonts[F_BODY], t!("Нет элементов, соответствующих запросу.", "No applets match the search.", ja: "検索に一致する項目はありません。", pt: "Nenhum item corresponde à pesquisa.", zh: "没有匹配搜索的项。"), RECT { left: c.left, top: c.top + p(d, HOME_HEAD), right: c.right, bottom: c.top + p(d, HOME_HEAD + 40) }, t.dim, DT_LEFT);
                RestoreDC(dc, saved);
                return;
            }
            for (r, i) in &layout.tiles {
                let applet = &a.applets[*i];
                if r.bottom < c.top || r.top > c.bottom {
                    continue;
                }
                let hot = a.hover == Some(Hot::Tile(*i));
                ui::shape(dc, *r, if hot { t.hover } else { t.card }, p(d, 10));
                let tile = p(d, 32);
                let tr = RECT { left: r.left + p(d, 12), top: (r.top + r.bottom - tile) / 2, right: r.left + p(d, 12) + tile, bottom: (r.top + r.bottom + tile) / 2 };
                ui::shape(dc, tr, applet.tile, p(d, 8));
                ui::text(dc, a.fonts[F_ICON_S], applet.icon, tr, rgb(255, 255, 255), DT_CENTER);
                ui::text(dc, a.fonts[F_BODY], &applet.title, RECT { left: tr.right + p(d, 12), right: r.right - p(d, 10), ..*r }, if hot { t.accent } else { t.text }, DT_LEFT);
            }
        }
        RestoreDC(dc, saved);
    }
}

fn paint_menu(dc: HDC, a: &App, t: &ui::Theme, m: &Menu) {
    unsafe {
        let d = a.dpi;
        let r = menu_rect(m, d);
        let shadow = RECT { left: r.left + p(d, 2), top: r.top + p(d, 5), right: r.right + p(d, 2), bottom: r.bottom + p(d, 5) };
        ui::shape(dc, shadow, mix(t.bg, rgb(0, 0, 0), 0.35), p(d, 10));
        ui::shape(dc, r, t.sep, p(d, 10));
        ui::shape(dc, ui::inset(r, 1), t.card, p(d, 9));
        for (i, item) in m.items.iter().enumerate() {
            let ir = menu_item_rect(m, d, i);
            if a.hover == Some(Hot::Menu(i)) {
                ui::shape(dc, ir, t.hover, p(d, 7));
            }
            ui::text(dc, a.fonts[F_BODY], &item.label, RECT { left: ir.left + p(d, 12), right: ir.right - p(d, 28), ..ir }, t.text, DT_LEFT);
            if item.checked {
                ui::text(dc, a.fonts[F_ICON_S], "\u{E73E}", RECT { left: ir.right - p(d, 28), right: ir.right - p(d, 10), ..ir }, t.accent, DT_CENTER);
            }
        }
    }
}

fn paint_status(dc: HDC, a: &App, t: &ui::Theme, cw: i32, ch: i32) {
    unsafe {
        let d = a.dpi;
        let r = RECT { left: 0, top: ch - p(d, STATUS_H), right: cw, bottom: ch };
        ui::shape(dc, r, t.side, 0);
        ui::hline(dc, 0, cw, r.top, t.sep);
        let c = content_rect(cw, ch, d);
        let msg = a.status.as_deref().unwrap_or_else(|| t!("Готово", "Ready", ja: "準備完了", pt: "Pronto", zh: "就绪"));
        let ink = if a.status.is_some() { t.text } else { t.dim };
        ui::text(dc, a.fonts[F_SMALL], msg, RECT { left: c.left, right: c.right - p(d, 200), ..r }, ink, DT_LEFT);
        let admin = sys::is_admin();
        let label = if admin { t!("Администратор", "Administrator", ja: "管理者", pt: "Administrador", zh: "管理员") } else { t!("Обычный пользователь", "Standard user", ja: "標準ユーザー", pt: "Usuário padrão", zh: "标准用户") };
        let lw = ui::text_width(dc, a.fonts[F_SMALL], label);
        let dot = p(d, 6);
        let cy = (r.top + r.bottom) / 2;
        ui::shape(dc, RECT { left: c.right - lw - p(d, 14) - dot, top: cy - dot / 2, right: c.right - lw - p(d, 14), bottom: cy + dot / 2 }, if admin { t.good } else { t.dim }, dot / 2);
        ui::text(dc, a.fonts[F_SMALL], label, RECT { left: c.right - lw, right: c.right, ..r }, if admin { t.text } else { t.dim }, DT_RIGHT);
    }
}

// ───────────────────────────── home layout ─────────────────────────────

struct HomeLayout {
    cards: Vec<(RECT, usize)>,
    links: Vec<(RECT, Action, usize, String)>,
    tiles: Vec<(RECT, usize)>,
    end: i32,
}

fn link_height(label: &str, dpi: i32) -> i32 {
    // The classic long links (File History, backup) wrap to a second line.
    if label.chars().count() > 42 { p(dpi, 34) } else { p(dpi, 18) }
}

fn category_height(cat: &Category, dpi: i32) -> i32 {
    let links: i32 = cat.links.iter().map(|l| link_height(&l.label, dpi)).sum();
    p(dpi, 40) + links
}

fn home_layout(a: &App, content: RECT, scroll: i32) -> HomeLayout {
    let d = a.dpi;
    let body = RECT { left: content.left, top: content.top + p(d, HOME_HEAD), right: content.right, bottom: content.bottom };
    let mut cards = vec![];
    let mut links = vec![];
    let mut tiles = vec![];
    let mut counter = 0usize;
    let mut end = body.top;
    if a.home_view == HomeView::Category {
        if a.categories.is_empty() {
            return HomeLayout { cards, links, tiles, end: body.top + p(d, 40) };
        }
        let gap = p(d, 24);
        let col_w = (body.right - body.left - gap) / 2;
        let split = if a.categories.len() > 4 { (a.categories.len() + 1) / 2 } else { a.categories.len() };
        let mut y = body.top - p(d, scroll);
        for row in 0..4 {
            let li = row;
            let ri = split + row;
            let lcat = a.categories.get(li);
            let rcat = a.categories.get(ri);
            if lcat.is_none() && rcat.is_none() {
                break;
            }
            let lh = lcat.map(|c| category_height(c, d)).unwrap_or(0);
            let rh = rcat.map(|c| category_height(c, d)).unwrap_or(0);
            if let Some(cat) = lcat {
                let r = RECT { left: body.left, top: y, right: body.left + col_w, bottom: y + lh };
                cards.push((r, li));
                let mut ly = r.top + p(d, 34);
                for link in cat.links.iter() {
                    let lh = link_height(&link.label, d);
                    links.push((RECT { left: r.left + p(d, 68), top: ly, right: r.right - p(d, 12), bottom: ly + lh }, link.action.clone(), counter, link.label.clone()));
                    counter += 1;
                    ly += lh;
                }
            }
            if let Some(cat) = rcat {
                let r = RECT { left: body.left + col_w + gap, top: y, right: body.right, bottom: y + rh };
                cards.push((r, ri));
                let mut ly = r.top + p(d, 34);
                for link in cat.links.iter() {
                    let lh = link_height(&link.label, d);
                    links.push((RECT { left: r.left + p(d, 68), top: ly, right: r.right - p(d, 12), bottom: ly + lh }, link.action.clone(), counter, link.label.clone()));
                    counter += 1;
                    ly += lh;
                }
            }
            y += lh.max(rh) + p(d, 20);
            end = y;
        }
    } else {
        if a.applets.is_empty() {
            return HomeLayout { cards, links, tiles, end: body.top + p(d, 40) };
        }
        let cols = if body.right - body.left >= p(d, 560) { 3 } else { 2 };
        let gap = p(d, 12);
        let tw = (body.right - body.left - gap * (cols - 1)) / cols;
        let th = p(d, 58);
        let start = body.top - p(d, scroll);
        for (i, _) in a.applets.iter().enumerate() {
            let col = i % cols as usize;
            let row = i / cols as usize;
            let r = RECT { left: body.left + (tw + gap) * col as i32, top: start + (th + gap) * row as i32, right: body.left + (tw + gap) * col as i32 + tw, bottom: start + (th + gap) * row as i32 + th };
            tiles.push((r, i));
        }
        let rows = (a.applets.len() + cols as usize - 1) / cols as usize;
        end = start + rows as i32 * (th + gap);
    }
    HomeLayout { cards, links, tiles, end }
}

fn home_hot(hwnd: HWND, x: i32, y: i32) -> Option<Hot> {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        APP.with_borrow(|a| {
            if a.page != PageId::Home {
                return None;
            }
            let c = content_rect(rc.right, rc.bottom, a.dpi);
            if a.home_view == HomeView::Category {
                let layout = home_layout(a, c, a.scroll);
                for (r, _, gi, _) in &layout.links {
                    if contains(r, x, y) {
                        return Some(Hot::Link(*gi));
                    }
                }
            } else {
                let layout = home_layout(a, c, a.scroll);
                for (r, i) in &layout.tiles {
                    if contains(r, x, y) {
                        return Some(Hot::Tile(*i));
                    }
                }
            }
            None
        })
    }
}

fn home_click(hwnd: HWND, x: i32, y: i32) -> Option<Action> {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        APP.with_borrow(|a| {
            if a.page != PageId::Home {
                return None;
            }
            let c = content_rect(rc.right, rc.bottom, a.dpi);
            let layout = home_layout(a, c, a.scroll);
            if a.home_view == HomeView::Category {
                for (r, action, _, _) in &layout.links {
                    if contains(r, x, y) {
                        return Some(action.clone());
                    }
                }
            } else {
                for (r, i) in &layout.tiles {
                    if contains(r, x, y) {
                        return Some(a.applets[*i].action.clone());
                    }
                }
            }
            None
        })
    }
}

// ───────────────────────────── hit testing ─────────────────────────────

fn hot_at(hwnd: HWND, x: i32, y: i32) -> Option<Hot> {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (cw, ch, dpi, page, has_menu) = APP.with_borrow(|a| (rc.right, rc.bottom, a.dpi, a.page, a.menu.is_some()));
        if has_menu {
            return APP.with_borrow(|a| a.menu.as_ref().and_then(|m| menu_item_at(m, dpi, x, y).map(|(i, _)| Hot::Menu(i))));
        }
        for (i, r) in title_button_rects(cw, dpi).iter().enumerate() {
            if contains(r, x, y) {
                return Some(Hot::Title(i as u8));
            }
        }
        if page != PageId::Home && contains(&crumb_rect(cw, dpi), x, y) {
            return Some(Hot::Breadcrumb);
        }
        if let Some(i) = nav_index_at(cw, dpi, x, y) {
            return Some(Hot::Nav(i));
        }
        if page == PageId::Home && contains(&selector_rect(cw, dpi), x, y) {
            return Some(Hot::Selector);
        }
        if page == PageId::Home {
            return home_hot(hwnd, x, y);
        }
        let content = content_rect(cw, ch, dpi);
        APP.with_borrow(|a| row_at_pos(a, content, x, y).and_then(|(i, r)| row_hot(hwnd, a, i, r, x, y)))
    }
}

// ───────────────────────────── actions ─────────────────────────────

fn apply(hwnd: HWND, action: Action) {
    match action {
        Action::None => {}
        Action::Go(page) => goto(hwnd, page),
        Action::Open(target) => match sys::open(target) {
            Ok(()) => set_status(t!("Открыто в Windows.", "Opened in Windows.", ja: "Windows で開きました。", pt: "Aberto no Windows.", zh: "已在 Windows 中打开。")),
            Err(code) => set_status(tf!("Windows не смогла открыть этот элемент (код {}).", "Windows could not open that item (code {}).", ja: "Windows はその項目を開けませんでした (コード {})。", pt: "O Windows não conseguiu abrir esse item (código {}).", zh: "Windows 无法打开该项（代码 {}）。", code)),
        },
        Action::Uninstall { name, command } => {
            let q = tf!("Удалить «{}»? Будет запущена собственная программа удаления.", "Uninstall “{}”? Its own uninstaller will start.", ja: "「{}」をアンインストールしますか? 独自のアンインストーラーが起動します。", pt: "Desinstalar “{}”? O próprio desinstalador será iniciado.", zh: "卸载“{}”？将启动程序自带的卸载程序。", name);
            if sys::confirm(hwnd, &q, t!("Удаление программы", "Uninstall a program", ja: "プログラムのアンインストール", pt: "Desinstalar um programa", zh: "卸载程序")) {
                match sys::run_command_line(&command) {
                    Ok(()) => set_status(tf!("Запущено удаление «{}».", "Uninstall started for “{}”.", ja: "「{}」のアンインストールを開始しました。", pt: "Desinstalação iniciada para “{}”.", zh: "已开始卸载“{}”。", name)),
                    Err(e) => set_status(tf!("Не удалось запустить удаление: {}", "Could not start the uninstaller: {}", ja: "アンインストーラーを起動できませんでした: {}", pt: "Não foi possível iniciar o desinstalador: {}", zh: "无法启动卸载程序：{}", sys::error_text(e))),
                }
            }
        }
        Action::ServiceStart { name, label } => {
            if !sys::is_admin() {
                set_status(t!("Для этой операции нужны права администратора. Запустите rust-control от имени администратора.", "This action needs administrator rights. Start rust-control as administrator.", ja: "この操作には管理者権限が必要です。rust-control を管理者として起動してください。", pt: "Esta ação exige direitos de administrador. Inicie o rust-control como administrador.", zh: "此操作需要管理员权限。请以管理员身份启动 rust-control。"));
            } else {
                match pages::services::start(&name) {
                    Ok(()) => set_status(tf!("Служба «{}» запускается.", "Service “{}” is starting.", ja: "サービス「{}」を開始しています。", pt: "O serviço “{}” está iniciando.", zh: "服务“{}”正在启动。", label)),
                    Err(e) => set_status(tf!("Не удалось запустить службу: {}", "Could not start the service: {}", ja: "サービスを開始できませんでした: {}", pt: "Não foi possível iniciar o serviço: {}", zh: "无法启动服务：{}", sys::error_text(e))),
                }
                load_current(hwnd, true);
            }
        }
        Action::ServiceStop { name, label } => {
            if !sys::is_admin() {
                set_status(t!("Для этой операции нужны права администратора. Запустите rust-control от имени администратора.", "This action needs administrator rights. Start rust-control as administrator.", ja: "この操作には管理者権限が必要です。rust-control を管理者として起動してください。", pt: "Esta ação exige direitos de administrador. Inicie o rust-control como administrador.", zh: "此操作需要管理员权限。请以管理员身份启动 rust-control。"));
            } else {
                let q = tf!("Остановить службу «{}»?", "Stop the service “{}”?", ja: "サービス「{}」を停止しますか?", pt: "Parar o serviço “{}”?", zh: "停止服务“{}”？", label);
                if sys::confirm(hwnd, &q, t!("Остановка службы", "Stop a service", ja: "サービスの停止", pt: "Parar um serviço", zh: "停止服务")) {
                    match pages::services::stop(&name) {
                        Ok(()) => set_status(tf!("Служба «{}» останавливается.", "Service “{}” is stopping.", ja: "サービス「{}」を停止しています。", pt: "O serviço “{}” está parando.", zh: "服务“{}”正在停止。", label)),
                        Err(e) => set_status(tf!("Не удалось остановить службу: {}", "Could not stop the service: {}", ja: "サービスを停止できませんでした: {}", pt: "Não foi possível parar o serviço: {}", zh: "无法停止服务：{}", sys::error_text(e))),
                    }
                    load_current(hwnd, true);
                }
            }
        }
        Action::ServiceStartup { name, label, start } => {
            if !sys::is_admin() {
                set_status(t!("Для этой операции нужны права администратора. Запустите rust-control от имени администратора.", "This action needs administrator rights. Start rust-control as administrator.", ja: "この操作には管理者権限が必要です。rust-control を管理者として起動してください。", pt: "Esta ação exige direitos de administrador. Inicie o rust-control como administrador.", zh: "此操作需要管理员权限。请以管理员身份启动 rust-control。"));
            } else {
                let kind = pages::services::start_type_text(start);
                let q = tf!("Изменить запуск службы «{}» на «{}»?", "Change how “{}” starts to “{}”?", ja: "「{}」のスタートアップを「{}」に変更しますか?", pt: "Alterar a inicialização de “{}” para “{}”?", zh: "将“{}”的启动类型改为“{}”？", label, kind);
                if sys::confirm(hwnd, &q, t!("Тип запуска службы", "Service start type", ja: "サービスのスタートアップの種類", pt: "Tipo de inicialização do serviço", zh: "服务启动类型")) {
                    match pages::services::set_start(&name, start) {
                        Ok(()) => set_status(tf!("Тип запуска «{}» изменён на «{}».", "“{}” now starts “{}”.", ja: "「{}」のスタートアップを「{}」に変更しました。", pt: "“{}” agora inicia “{}”.", zh: "“{}”的启动类型已改为“{}”。", label, kind)),
                        Err(e) => set_status(tf!("Не удалось изменить тип запуска: {}", "Could not change the start type: {}", ja: "スタートアップの種類を変更できませんでした: {}", pt: "Não foi possível alterar o tipo de inicialização: {}", zh: "无法更改启动类型：{}", sys::error_text(e))),
                    }
                    load_current(hwnd, true);
                }
            }
        }
        Action::ServiceStartupMenu { .. } => {}
        Action::SetPlan { guid, label } => {
            let q = tf!("Переключиться на схему питания «{}»?", "Switch to the power plan “{}”?", ja: "電源プラン「{}」に切り替えますか?", pt: "Alternar para o plano de energia “{}”?", zh: "切换到电源计划“{}”？", label);
            if sys::confirm(hwnd, &q, t!("Схема питания", "Power plan", ja: "電源プラン", pt: "Plano de energia", zh: "电源计划")) {
                match pages::power::set_plan(guid) {
                    Ok(()) => set_status(tf!("Схема питания «{}» включена.", "Power plan “{}” is active.", ja: "電源プラン「{}」を有効にしました。", pt: "O plano de energia “{}” está ativo.", zh: "电源计划“{}”已启用。", label)),
                    Err(e) => set_status(tf!("Не удалось сменить схему питания: {}", "Could not switch the power plan: {}", ja: "電源プランを切り替えられませんでした: {}", pt: "Não foi possível alternar o plano de energia: {}", zh: "无法切换电源计划：{}", sys::error_text(e))),
                }
                load_current(hwnd, true);
            }
        }
        Action::SetHomeView(icons) => {
            APP.with_borrow_mut(|a| {
                a.home_view = if icons { HomeView::Icons } else { HomeView::Category };
                a.scroll = 0;
                a.menu = None;
            });
            rebuild_filter(hwnd);
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
        Action::Refresh => {
            load_current(hwnd, true);
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
    }
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

// ───────────────────────────── input ─────────────────────────────

fn on_click(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let dpi = APP.with_borrow(|a| a.dpi);

        // An open menu takes the click.
        let has_menu = APP.with_borrow(|a| a.menu.is_some());
        if has_menu {
            let pick = APP.with_borrow(|a| a.menu.as_ref().and_then(|m| menu_item_at(m, dpi, x, y)).map(|(_, action)| action));
            APP.with_borrow_mut(|a| {
                a.menu = None;
                a.hover = None;
            });
            if let Some(action) = pick {
                apply(hwnd, action);
            }
            InvalidateRect(hwnd, null(), 0);
            return;
        }

        for (i, r) in title_button_rects(rc.right, dpi).iter().enumerate() {
            if contains(r, x, y) {
                match i {
                    0 => {
                        ShowWindow(hwnd, SW_MINIMIZE);
                    }
                    1 => {
                        if IsZoomed(hwnd) != 0 {
                            ShowWindow(hwnd, SW_RESTORE);
                        } else {
                            ShowWindow(hwnd, SW_MAXIMIZE);
                        }
                    }
                    _ => {
                        DestroyWindow(hwnd);
                    }
                }
                return;
            }
        }
        let page = APP.with_borrow(|a| a.page);
        if page != PageId::Home && contains(&crumb_rect(rc.right, dpi), x, y) {
            goto(hwnd, PageId::Home);
            return;
        }
        if let Some(i) = nav_index_at(rc.right, dpi, x, y) {
            goto(hwnd, NAV[i]);
            return;
        }
        if page == PageId::Home && contains(&selector_rect(rc.right, dpi), x, y) {
            home_view_menu(hwnd);
            return;
        }
        if page == PageId::Home {
            if let Some(action) = home_click(hwnd, x, y) {
                apply(hwnd, action);
            } else {
                SetFocus(hwnd);
            }
            return;
        }
        let content = content_rect(rc.right, rc.bottom, dpi);
        let hit = APP.with_borrow(|a| row_at_pos(a, content, x, y));
        if let Some((i, r)) = hit {
            let (buttons, action) = APP.with_borrow(|a| (row_buttons(a, &a.view[i]), row_action(&a.view[i])));
            if !buttons.is_empty() {
                let dc = GetDC(hwnd);
                let font = APP.with_borrow(|a| a.fonts[F_SMALL]);
                let rects = button_rects(dc, font, r, &buttons, dpi);
                ReleaseDC(hwnd, dc);
                for (j, br) in rects.iter().enumerate() {
                    if contains(br, x, y) {
                        let act = buttons[j].action.clone();
                        match act {
                            Action::ServiceStartupMenu { name, label, current } => service_menu(hwnd, *br, name, label, current),
                            other => apply(hwnd, other),
                        }
                        return;
                    }
                }
            }
            if let Some(action) = action {
                apply(hwnd, action);
                return;
            }
        }
        SetFocus(hwnd);
    }
}

fn on_move(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let mut te = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
        TrackMouseEvent(&mut te);
        let hot = hot_at(hwnd, x, y);
        let changed = APP.with_borrow_mut(|a| {
            let old = a.hover;
            a.hover = hot;
            old != hot
        });
        if changed {
            InvalidateRect(hwnd, null(), 0);
        }
    }
}

fn on_wheel(hwnd: HWND, delta: i32) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (step, max) = APP.with_borrow(|a| (34 * 3, scroll_max(a, rc.right, rc.bottom)));
        if max > 0 {
            let by = -(delta / 120) * step;
            APP.with_borrow_mut(|a| a.scroll = (a.scroll + by).clamp(0, max));
            InvalidateRect(hwnd, null(), 0);
        }
    }
}

fn on_key(hwnd: HWND, key: u16) {
    unsafe {
        let menu = APP.with_borrow(|a| a.menu.is_some());
        if menu {
            let action = APP.with_borrow_mut(|a| {
                let n = match a.menu.as_ref() {
                    Some(m) => m.items.len(),
                    None => return None,
                };
                match key {
                    VK_UP | VK_DOWN => {
                        let cur = match a.hover {
                            Some(Hot::Menu(i)) => i,
                            _ => 0,
                        };
                        let next = if key == VK_UP { cur.saturating_sub(1) } else { (cur + 1).min(n - 1) };
                        a.hover = Some(Hot::Menu(next));
                        None
                    }
                    VK_RETURN => {
                        let i = match a.hover {
                            Some(Hot::Menu(i)) => i,
                            _ => 0,
                        };
                        a.menu.take().and_then(|m| m.items.get(i).map(|it| it.action.clone()))
                    }
                    VK_ESCAPE => {
                        a.menu = None;
                        a.hover = None;
                        None
                    }
                    _ => None,
                }
            });
            if let Some(action) = action {
                apply(hwnd, action);
            }
            InvalidateRect(hwnd, null(), 0);
            return;
        }
        match key {
            VK_ESCAPE => {
                let q = APP.with_borrow(|a| !a.query.is_empty());
                if q {
                    clear_search(hwnd);
                } else {
                    DestroyWindow(hwnd);
                }
            }
            VK_F5 => {
                load_current(hwnd, true);
                InvalidateRect(hwnd, null(), 0);
            }
            k @ (VK_UP | VK_DOWN) => {
                let cur = APP.with_borrow(|a| a.page);
                let i = cur.index();
                let next = if k == VK_UP { i.saturating_sub(1) } else { (i + 1).min(NAV.len() - 1) };
                goto(hwnd, PageId::from_index(next));
            }
            _ if key == b'F' as u16 && GetKeyState(VK_CONTROL as i32) < 0 => {
                let ed = APP.with_borrow(|a| a.edit);
                if ed != 0 {
                    SetFocus(ed as HWND);
                }
            }
            _ => {}
        }
    }
}

// ───────────────────────────── fonts and search field ─────────────────────────────

unsafe fn refresh_fonts() {
    let mono = style::style().mono();
    let style_id = style::style() as u8;
    let (ed, font) = APP.with_borrow_mut(|a| {
        if a.fonts[0] != 0 && a.mono_ok(mono, style_id) {
            return (0, 0);
        }
        let new = ui::create_fonts(a.dpi);
        let old = std::mem::replace(&mut a.fonts, new);
        ui::delete_fonts(&old);
        a.mono = mono;
        a.font_style = style_id;
        a.font_dpi = a.dpi;
        (a.edit, a.fonts[F_BODY])
    });
    if ed != 0 && font != 0 {
        SendMessageW(ed as HWND, WM_SETFONT, font as usize, 1);
    }
}

impl App {
    fn mono_ok(&self, mono: bool, style_id: u8) -> bool {
        self.mono == mono && self.font_style == style_id && self.font_dpi == self.dpi
    }
}

unsafe fn create_search(hwnd: HWND) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (dpi, font) = APP.with_borrow(|a| (a.dpi, a.fonts[F_BODY]));
        let r = search_edit_rect(rc.right, dpi);
        let style = WS_CHILD | WS_VISIBLE | (ES_AUTOHSCROLL as u32) | (ES_LEFT as u32);
        let inst = GetModuleHandleW(null());
        let ed = CreateWindowExW(0, sys::wide("EDIT").as_ptr(), null(), style, r.left, r.top, r.right - r.left, r.bottom - r.top, hwnd, SEARCH_ID as *mut core::ffi::c_void, inst, null());
        if ed.is_null() {
            return;
        }
        SendMessageW(ed, WM_SETFONT, font as usize, 1);
        let cue = sys::wide(t!("Поиск в панели управления", "Search Control Panel", ja: "コントロール パネルを検索", pt: "Pesquisar no Painel de Controle", zh: "搜索控制面板"));
        SendMessageW(ed, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
        SendMessageW(ed, EM_SETMARGINS, 3, (p(dpi, 8) * 0x10001) as isize);
        SendMessageW(ed, EM_LIMITTEXT, 120, 0);
        SEARCH_PROC.store(SetWindowLongPtrW(ed, GWLP_WNDPROC, search_proc as *const () as isize), Ordering::Relaxed);
        APP.with_borrow_mut(|a| a.edit = ed as isize);
    }
}

unsafe extern "system" fn search_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == WM_KEYDOWN && (wp == VK_ESCAPE as usize || wp == VK_RETURN as usize) {
            PostMessageW(GetParent(hwnd), WM_SEARCH_KEY, wp, 0);
            return 0;
        }
        if msg == WM_CHAR && (wp == 13 || wp == 27) {
            return 0;
        }
        let prev = SEARCH_PROC.load(Ordering::Relaxed);
        CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(prev), hwnd, msg, wp, lp)
    }
}

fn get_text(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

fn reposition_search(hwnd: HWND) {
    unsafe {
        let (ed, dpi) = APP.with_borrow(|a| (a.edit, a.dpi));
        if ed == 0 {
            return;
        }
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let r = search_edit_rect(rc.right, dpi);
        MoveWindow(ed as HWND, r.left, r.top, r.right - r.left, r.bottom - r.top, 1);
    }
}

// ───────────────────────────── window procedure ─────────────────────────────

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg != 0 && msg == style::msg() {
            style::reload();
            refresh_fonts();
            InvalidateRect(hwnd, null(), 0);
            return 0;
        }
        match msg {
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_ERASEBKGND => 1,
            WM_SIZE => {
                reposition_search(hwnd);
                clamp_scroll(hwnd);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_DPICHANGED => {
                let dpi = sys::hiword(wp) as i32;
                if dpi > 0 {
                    APP.with_borrow_mut(|a| a.dpi = dpi);
                }
                refresh_fonts();
                reposition_search(hwnd);
                let r = &*(lp as *const RECT);
                SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                clamp_scroll(hwnd);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_SETTINGCHANGE => {
                refresh_fonts();
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_GETMINMAXINFO => {
                let mm = &mut *(lp as *mut MINMAXINFO);
                let dpi = APP.with_borrow(|a| a.dpi);
                mm.ptMinTrackSize.x = p(dpi, 760);
                mm.ptMinTrackSize.y = p(dpi, 560);
                0
            }
            WM_NCHITTEST => {
                let result = DefWindowProcW(hwnd, msg, wp, lp);
                if result == HTCLIENT as isize {
                    let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
                    ScreenToClient(hwnd, &mut pt);
                    let mut rc: RECT = std::mem::zeroed();
                    GetClientRect(hwnd, &mut rc);
                    let dpi = APP.with_borrow(|a| a.dpi);
                    if pt.y >= 0 && pt.y < p(dpi, TITLE_H) {
                        if title_button_rects(rc.right, dpi).iter().any(|r| contains(r, pt.x, pt.y)) {
                            return HTCLIENT as isize;
                        }
                        return HTCAPTION as isize;
                    }
                }
                result
            }
            WM_MOUSEMOVE => {
                on_move(hwnd, (lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                0
            }
            WM_MOUSELEAVE => {
                if APP.with_borrow_mut(|a| a.hover.take().is_some()) {
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_LBUTTONDOWN => {
                on_click(hwnd, (lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_MOUSEWHEEL => {
                on_wheel(hwnd, ((wp >> 16) & 0xFFFF) as i16 as i32);
                0
            }
            WM_KEYDOWN => {
                on_key(hwnd, wp as u16);
                0
            }
            WM_SEARCH_KEY => {
                if wp == VK_ESCAPE as usize {
                    let q = APP.with_borrow(|a| !a.query.is_empty());
                    if q {
                        clear_search(hwnd);
                    } else {
                        SetFocus(hwnd);
                    }
                } else {
                    SetFocus(hwnd);
                }
                0
            }
            WM_COMMAND => {
                if sys::loword(wp) as usize == SEARCH_ID && sys::hiword(wp) == EN_CHANGE_CODE {
                    let ed = APP.with_borrow(|a| a.edit);
                    if ed != 0 {
                        let s = get_text(ed as HWND);
                        APP.with_borrow_mut(|a| {
                            a.query = s;
                            a.scroll = 0;
                        });
                        rebuild_filter(hwnd);
                        InvalidateRect(hwnd, null(), 0);
                    }
                }
                0
            }
            WM_CTLCOLOREDIT => {
                let t = ui::theme();
                SetTextColor(wp as HDC, t.text);
                SetBkColor(wp as HDC, t.card);
                let b = CreateSolidBrush(t.card);
                let old = APP.with_borrow_mut(|a| std::mem::replace(&mut a.brush, b as isize));
                if old != 0 {
                    DeleteObject(old as _);
                }
                b as LRESULT
            }
            WM_TIMER if wp == 1 => {
                let (page, fresh, menu) = APP.with_borrow(|a| (a.page, a.last_load.elapsed().as_secs(), a.menu.is_some()));
                let dynamic = matches!(page, PageId::System | PageId::Services | PageId::Power | PageId::DateTime);
                if dynamic && fresh >= 8 && !menu {
                    load_current(hwnd, true);
                    InvalidateRect(hwnd, null(), 0);
                }
                let stale = APP.with_borrow(|a| a.status.is_some() && a.status_at.elapsed().as_secs() > 20);
                if stale {
                    APP.with_borrow_mut(|a| a.status = None);
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_DESTROY => {
                let (edit, brush, fonts) = APP.with_borrow_mut(|a| {
                    let e = a.edit;
                    a.edit = 0;
                    let b = a.brush;
                    a.brush = 0;
                    let f = a.fonts;
                    (e, b, f)
                });
                if edit != 0 {
                    DestroyWindow(edit as HWND);
                }
                if brush != 0 {
                    DeleteObject(brush as _);
                }
                ui::delete_fonts(&fonts);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

// ───────────────────────────── entry point ─────────────────────────────

fn main() {
    unsafe {
        let mut token = 0usize;
        let mut input: GdiplusStartupInput = std::mem::zeroed();
        input.GdiplusVersion = 1;
        let _ = GdiplusStartup(&mut token, &input, null_mut());
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_STANDARD_CLASSES };
        let _ = InitCommonControlsEx(&icc);
        SetProcessDPIAware();
        let dpi = GetDpiForSystem() as i32;
        let meta = pages::meta();
        APP.with_borrow_mut(|a| {
            a.meta = meta;
            a.dpi = dpi;
        });
        refresh_fonts();
        let inst = GetModuleHandleW(null());
        let class = sys::wide("WinRustControl");
        let icon = LoadIconW(inst, 1 as *const u16);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: inst,
            hIcon: icon,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let w = 900 * dpi / 96;
        let h = 620 * dpi / 96;
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let title = sys::wide(t!("Панель управления", "Control Panel", ja: "コントロール パネル", pt: "Painel de Controle", zh: "控制面板"));
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU | WS_CLIPCHILDREN | WS_VISIBLE,
            (sw - w) / 2,
            (sh - h) / 2,
            w,
            h,
            null_mut(),
            null_mut(),
            inst,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        create_search(hwnd);
        goto(hwnd, PageId::Home);
        SetTimer(hwnd, 1, 2000, None);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        GdiplusShutdown(token);
    }
}
