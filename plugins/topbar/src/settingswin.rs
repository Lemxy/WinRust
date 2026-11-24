//! The settings window: a real popup with a tab rail on the left and the options of each tab on the right, instead of
//! one long dropdown. Rows are toggles, choices and actions; the geometry of `rows` is used by both the painter and the
//! hit test, so a click always lands where the row is drawn.
use std::{cell::Cell, mem::zeroed, ptr};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::VK_ESCAPE, WindowsAndMessaging::*},
};

use crate::fly;

const W: i32 = 780;
const H: i32 = 540;
const RAIL: i32 = 196; // the tab rail

fn sc(v: i32) -> i32 {
    crate::startctx::sc(v)
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    None,
    Tab(u8),
    Theme(u8),
    StartPos(bool),
    StartStyle(u8),
    StartScale(u8),
    BarSide(u8),
    BarMonitor(u8),
    BarMonitorAll,
    BarSize(u8),
    Autohide,
    AlignLeft,
    AltTab,
    Notify,
    NotifyTimeoutNext,
    BarBlur,
    BarRadius(i32),
    MenuBlur,
    MenuAlpha(u8),
    MenuRadius(i32),
    Lang(u8),
    ColorsPage,
    TemplatesPage,
    Reset,
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Head,
    Check(bool),
    Choice(bool),
    Action,
    Danger,
}

struct Row {
    label: String,
    hint: String,
    kind: Kind,
    op: Op,
}

fn head(label: &str) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Head, op: Op::None }
}
fn check(label: &str, on: bool, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Check(on), op }
}
fn choice(label: &str, on: bool, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Choice(on), op }
}
fn action(label: &str, hint: &str, op: Op) -> Row {
    Row { label: label.into(), hint: hint.into(), kind: Kind::Action, op }
}
fn danger(label: &str, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Danger, op }
}

const TABS: [&str; 5] = ["Appearance", "Start", "Blur", "Bar", "Language"];

fn tab_title(i: usize) -> String {
    use winrust_common::t;
    match i {
        0 => t!("Внешний вид", "Appearance", ja: "外観", pt: "Aparência", zh: "外观"),
        1 => t!("Меню «Пуск»", "Start menu", ja: "スタート メニュー", pt: "Menu Iniciar", zh: "开始菜单"),
        2 => t!("Размытие", "Blur", ja: "ぼかし", pt: "Desfoque", zh: "模糊"),
        3 => t!("Панель", "Bar", ja: "バー", pt: "Barra", zh: "任务栏"),
        _ => t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言"),
    }
    .to_string()
}

fn rows(tab: u8) -> Vec<Row> {
    use winrust_common::{look as lk, t};
    let mut v = Vec::new();
    match tab {
        0 => {
            v.push(head(t!("Тема", "Theme", ja: "テーマ", pt: "Tema", zh: "主题")));
            for (i, s) in winrust_common::style::ALL.iter().enumerate() {
                v.push(choice(style_name(*s), winrust_common::style::style() == *s, Op::Theme(i as u8)));
            }
            v.push(head(t!("Свои цвета", "Own colors", ja: "独自の色", pt: "Cores próprias", zh: "自定义颜色")));
            v.push(action(t!("Цвета и прозрачность…", "Colors and transparency…", ja: "色と透明度…", pt: "Cores e transparência…", zh: "颜色与透明度…"), "›", Op::ColorsPage));
            v.push(action(t!("Шаблоны…", "Templates…", ja: "テンプレート…", pt: "Modelos…", zh: "模板…"), "›", Op::TemplatesPage));
        }
        1 => {
            v.push(head(t!("Положение", "Position", ja: "位置", pt: "Posição", zh: "位置")));
            v.push(choice(t!("По центру", "Centered", ja: "中央", pt: "Centralizado", zh: "居中"), !lk::start_left(), Op::StartPos(false)));
            v.push(choice(t!("Слева", "On the left", ja: "左", pt: "À esquerda", zh: "靠左"), lk::start_left(), Op::StartPos(true)));
            v.push(head(t!("Стиль", "Style", ja: "スタイル", pt: "Estilo", zh: "样式")));
            for (i, name) in ["Windows 11", "Windows 10", "Windows 7"].iter().enumerate() {
                v.push(choice(name, lk::start_style() == i as i32, Op::StartStyle(i as u8)));
            }
            v.push(head(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小")));
            for (i, name) in [(0u8, t!("Мелкий", "Small", ja: "小", pt: "Pequeno", zh: "小")), (1, t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常")), (2, t!("Крупный", "Large", ja: "大", pt: "Grande", zh: "大"))] {
                v.push(choice(name, lk::start_scale() == match i { 0 => 90, 2 => 115, _ => 100 }, Op::StartScale(i)));
            }
        }
        2 => {
            v.push(head(t!("Панель", "Bar", ja: "バー", pt: "Barra", zh: "任务栏")));
            v.push(check(t!("Размытие за панелью", "Blur behind the bar", ja: "バーの後ろをぼかす", pt: "Desfoque atrás da barra", zh: "任务栏后模糊"), lk::bar_blur(), Op::BarBlur));
            for (r, name) in [(4, t!("Слабое", "Soft", ja: "弱い", pt: "Suave", zh: "弱")), (8, t!("Среднее", "Medium", ja: "中", pt: "Média", zh: "中")), (14, t!("Сильное", "Strong", ja: "強い", pt: "Forte", zh: "强"))] {
                v.push(choice(name, lk::bar_radius() == r, Op::BarRadius(r)));
            }
            v.push(head(t!("Контекстное меню", "Context menu", ja: "コンテキスト メニュー", pt: "Menu de contexto", zh: "上下文菜单")));
            v.push(check(t!("Размытие за меню", "Blur behind the menu", ja: "メニューの後ろをぼかす", pt: "Desfoque atrás do menu", zh: "菜单后模糊"), lk::menu_blur(), Op::MenuBlur));
            v.push(head(t!("Непрозрачность меню", "Menu opacity", ja: "メニューの不透明度", pt: "Opacidade do menu", zh: "菜单不透明度")));
            for a in [90u8, 75, 67, 50, 35] {
                v.push(choice(&format!("{a} %"), lk::menu_alpha() == a, Op::MenuAlpha(a)));
            }
            v.push(head(t!("Сила размытия", "Blur strength", ja: "ぼかしの強さ", pt: "Intensidade do desfoque", zh: "模糊强度")));
            for r in [12, 18, 26] {
                v.push(choice(&format!("{r} px"), lk::menu_radius() == r, Op::MenuRadius(r)));
            }
        }
        3 => {
            use winrust_common::style::Side;
            v.push(head(t!("Положение", "Position", ja: "位置", pt: "Posição", zh: "位置")));
            for (i, (s, name)) in [(Side::Bottom, t!("Снизу", "Bottom", ja: "下", pt: "Embaixo", zh: "底部")), (Side::Left, t!("Слева", "Left", ja: "左", pt: "Esquerda", zh: "左侧")), (Side::Right, t!("Справа", "Right", ja: "右", pt: "Direita", zh: "右侧"))].into_iter().enumerate() {
                v.push(choice(name, winrust_common::style::side() == s, Op::BarSide(i as u8)));
            }
            v.push(head(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小")));
            for (i, name) in [(0u8, t!("Мелкий", "Small", ja: "小", pt: "Pequeno", zh: "小")), (1, t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常")), (2, t!("Крупный", "Large", ja: "大", pt: "Grande", zh: "大"))] {
                v.push(choice(name, winrust_common::style::bar_size() == i, Op::BarSize(i)));
            }
            // one bar on every monitor, or one on a chosen one
            let mons = winrust_common::monitors::all();
            if mons.len() > 1 {
                v.push(head(t!("Монитор", "Monitor", ja: "モニター", pt: "Monitor", zh: "显示器")));
                v.push(choice(t!("Все экраны", "All screens", ja: "すべての画面", pt: "Todas as telas", zh: "所有屏幕"), lk::bar_monitor() < 0, Op::BarMonitorAll));
                for i in 0..mons.len() {
                    let name = format!("{} {}", t!("Монитор", "Monitor", ja: "モニター", pt: "Monitor", zh: "显示器"), i + 1);
                    v.push(choice(&name, lk::bar_monitor() == i as i32, Op::BarMonitor(i as u8)));
                }
            }
            // one switch and one value row: the fixed-height window has no room for a second list of choices
            v.push(check(t!("Уведомления", "Notifications", ja: "通知", pt: "Notificações", zh: "通知"), lk::notify(), Op::Notify));
            v.push(action(t!("Время на экране", "On screen for", ja: "表示時間", pt: "Tempo na tela", zh: "显示时长"), &format!("{} s ›", lk::notify_timeout()), Op::NotifyTimeoutNext));
            v.push(head(t!("Поведение", "Behavior", ja: "動作", pt: "Comportamento", zh: "行为")));
            v.push(check(t!("Скрывать автоматически", "Hide automatically", ja: "自動的に隠す", pt: "Ocultar automaticamente", zh: "自动隐藏"), winrust_common::style::autohide(), Op::Autohide));
            v.push(check(t!("Кнопки слева, как в Windows 10", "Buttons on the left, like Windows 10", ja: "ボタンを左に配置 (Windows 10 風)", pt: "Botões à esquerda, como no Windows 10", zh: "按钮靠左，如同 Windows 10"), winrust_common::style::align_left(), Op::AlignLeft));
            v.push(check(t!("Alt+Tab: только этот монитор", "Alt+Tab: this monitor only", ja: "Alt+Tab: このモニターのみ", pt: "Alt+Tab: somente este monitor", zh: "Alt+Tab：仅限此显示器"), winrust_common::style::alttab_per_monitor(), Op::AltTab));
        }
        _ => {
            v.push(head(t!("Язык всех программ WinRust", "The language of every WinRust program", ja: "すべての WinRust プログラムの言語", pt: "O idioma de todos os programas da WinRust", zh: "所有 WinRust 程序的语言")));
            for (i, l) in winrust_common::Lang::ALL.iter().enumerate() {
                v.push(choice(l.name(), winrust_common::lang() == *l, Op::Lang(i as u8)));
            }
        }
    }
    v.push(head(""));
    v.push(danger(t!("Сбросить всё по умолчанию…", "Reset everything to defaults…", ja: "すべてを既定に戻す…", pt: "Restaurar tudo para o padrão…", zh: "全部恢复默认…"), Op::Reset));
    v
}

fn style_name(s: winrust_common::style::Style) -> &'static str {
    match s {
        winrust_common::style::Style::Normal => t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常"),
        winrust_common::style::Style::Terminal => "Terminal",
        winrust_common::style::Style::Dock => t!("Плавающая панель", "Floating dock", ja: "フローティング ドック", pt: "Dock flutuante", zh: "悬浮停靠"),
        winrust_common::style::Style::Tiling => t!("Плитка", "Tiling", ja: "タイル表示", pt: "Lado a lado", zh: "平铺"),
        winrust_common::style::Style::Road => t!("Белая дорога", "White road", ja: "白い道", pt: "Estrada branca", zh: "白色道路"),
        winrust_common::style::Style::Ruby => "Ruby",
        winrust_common::style::Style::Win98 => "Windows 98",
        winrust_common::style::Style::Win7 => "Windows 7",
    }
}

thread_local! {
    static TAB: Cell<u8> = const { Cell::new(0) };
    static PX: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    static FONT: Cell<[usize; 4]> = const { Cell::new([0; 4]) };
    static BITS: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Row geometry: x, y, w, h of every row of the current tab.
fn layout(tab: u8, top: i32) -> Vec<(RECT, Row)> {
    let mut y = top;
    let mut v = Vec::new();
    for r in rows(tab) {
        let h = match r.kind {
            Kind::Head => sc(30),
            _ => sc(36),
        };
        v.push((RECT { left: sc(RAIL + 24), top: y, right: sc(W - 24), bottom: y + h }, r));
        y += h;
    }
    v
}

unsafe fn font(px: i32, weight: i32) -> HFONT {
    let face: Vec<u16> = winrust_common::style::ui_face().encode_utf16().chain([0]).collect();
    CreateFontW(-sc(px), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr())
}

unsafe fn paint(h: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let wdc = BeginPaint(h, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(h, &mut cr);
    // double buffer: the blurred backdrop is expensive, and painting it straight to the window flickers
    let dc = CreateCompatibleDC(wdc);
    let bmp = CreateCompatibleBitmap(wdc, cr.right, cr.bottom);
    let obmp = SelectObject(dc, bmp);
    let p = winrust_common::style::pal();
    let tab = TAB.with(|t| t.get());
    let inner = RECT { left: 1, top: 1, right: cr.right - 1, bottom: cr.bottom - 1 };
    // backdrop: the blurred screen captured when the window opened, then the theme colour with a floor of 62 %
    let drew = BITS.with(|b| {
        let b = b.borrow();
        if b.is_empty() {
            return false;
        }
        crate::blur::paint_capture(dc, &b, inner, p.bg, (255 * winrust_common::look::menu_alpha().max(60) as u32 / 100) as u8);
        true
    });
    if !drew {
        let br = CreateSolidBrush(p.bg);
        FillRect(dc, &inner, br);
        DeleteObject(br);
    }
    let edge = CreateSolidBrush(winrust_common::style::mix(p.bg, p.text, 0.12));
    FrameRect(dc, &cr, edge);
    DeleteObject(edge);
    // tab rail
    let rail = CreateSolidBrush(winrust_common::style::mix(p.bg, p.text, 0.05));
    FillRect(dc, &RECT { left: sc(1), top: sc(1), right: sc(RAIL), bottom: cr.bottom - sc(1) }, rail);
    DeleteObject(rail);
    let (f, f_head, f_small, f_tab) = FONT.with(|x| {
        let a = x.get();
        (a[0] as HFONT, a[1] as HFONT, a[2] as HFONT, a[3] as HFONT)
    });
    SetBkMode(dc, TRANSPARENT as i32);
    let old = SelectObject(dc, f_tab);
    for i in 0..TABS.len() {
        let name = tab_title(i);
        let r = tab_rect(i as u8);
        if i as u8 == tab {
            let br = CreateSolidBrush(winrust_common::style::mix(p.bg, p.accent, 0.18));
            let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
            RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, sc(8), sc(8));
            SelectObject(dc, o1);
            SelectObject(dc, o2);
            DeleteObject(br);
        }
        SelectObject(dc, f_tab);
        SetTextColor(dc, if i as u8 == tab { p.text } else { p.dim });
        let mut rr = r;
        let w: Vec<u16> = name.encode_utf16().chain([0]).collect();
        DrawTextW(dc, w.as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
    // header of the tab
    SelectObject(dc, f_head);
    SetTextColor(dc, p.text);
    let mut hr = RECT { left: sc(RAIL + 24), top: sc(18), right: cr.right - sc(24), bottom: sc(52) };
    let w: Vec<u16> = tab_title(tab as usize).encode_utf16().chain([0]).collect();
    DrawTextW(dc, w.as_ptr(), -1, &mut hr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    let line = CreateSolidBrush(winrust_common::style::mix(p.bg, p.text, 0.10));
    FillRect(dc, &RECT { left: sc(RAIL + 24), top: sc(56), right: cr.right - sc(24), bottom: sc(57) }, line);
    DeleteObject(line);
    // rows
    for (r, row) in layout(tab, sc(70)) {
        match row.kind {
            Kind::Head => {
                if row.label.is_empty() {
                    continue;
                }
                SelectObject(dc, f_small);
                SetTextColor(dc, p.dim);
                let mut rr = RECT { left: r.left, top: r.top + sc(8), right: r.right, bottom: r.bottom };
                let w: Vec<u16> = row.label.encode_utf16().chain([0]).collect();
                DrawTextW(dc, w.as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
            _ => {
                let hot = HOT.with(|x| x.get()) == r.top;
                if hot {
                    let br = CreateSolidBrush(p.hover);
                    let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
                    RoundRect(dc, r.left - sc(6), r.top, r.right + 1, r.bottom, sc(6), sc(6));
                    SelectObject(dc, o1);
                    SelectObject(dc, o2);
                    DeleteObject(br);
                }
                let col = if row.kind == Kind::Danger { winrust_common::style::rgb(232, 92, 92) } else { p.text };
                // the marker: a dot for the chosen choice, a tick for a toggle that is on
                let mark = match row.kind {
                    Kind::Choice(on) => Some(if on { "\u{25CF}" } else { "\u{25CB}" }),
                    Kind::Check(on) => Some(if on { "\u{2713}" } else { "" }),
                    _ => None,
                };
                if let Some(m) = mark {
                    if !m.is_empty() {
                        SelectObject(dc, f);
                        SetTextColor(dc, if row.kind == Kind::Check(true) { p.accent } else { p.text });
                        let mut mr = RECT { left: r.left + sc(4), top: r.top, right: r.left + sc(28), bottom: r.bottom };
                        let w: Vec<u16> = m.encode_utf16().chain([0]).collect();
                        DrawTextW(dc, w.as_ptr(), -1, &mut mr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                    }
                }
                SelectObject(dc, f);
                SetTextColor(dc, col);
                let left = if mark.is_some() { r.left + sc(32) } else { r.left + sc(6) };
                let mut rr = RECT { left, top: r.top, right: if row.hint.is_empty() { r.right } else { r.right - sc(54) }, bottom: r.bottom };
                let w: Vec<u16> = row.label.encode_utf16().chain([0]).collect();
                DrawTextW(dc, w.as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
                if !row.hint.is_empty() {
                    SelectObject(dc, f_small);
                    SetTextColor(dc, p.dim);
                    let mut hr = RECT { left: r.right - sc(54), top: r.top, right: r.right, bottom: r.bottom };
                    let w: Vec<u16> = row.hint.encode_utf16().chain([0]).collect();
                    DrawTextW(dc, w.as_ptr(), -1, &mut hr, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                }
            }
        }
    }
    SelectObject(dc, old);
    BitBlt(wdc, 0, 0, cr.right, cr.bottom, dc, 0, 0, SRCCOPY);
    SelectObject(dc, obmp);
    DeleteObject(bmp);
    DeleteDC(dc);
    EndPaint(h, &ps);
}

thread_local! {
    static HOT: Cell<i32> = const { Cell::new(0) };
}

fn tab_rect(i: u8) -> RECT {
    let top = sc(20) + i as i32 * sc(44);
    RECT { left: sc(12), top, right: sc(RAIL - 12), bottom: top + sc(38) }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(h);
            0
        }
        WM_MOUSEMOVE => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let tab = TAB.with(|t| t.get());
            // rows only exist to the right of the rail: moving over the tabs must not light a row up
            let on_rows = x >= sc(RAIL + 18) && x <= sc(W - 24);
            let hit = if on_rows {
                layout(tab, sc(70)).into_iter().find(|(r, _)| y >= r.top && y < r.bottom).map_or(0, |(r, _)| r.top)
            } else {
                0
            };
            // only repaint when the hovered row changes, not on every pixel of movement
            if HOT.with(|x| x.replace(hit)) != hit {
                InvalidateRect(h, ptr::null(), 0);
            }
            // and ask to be told when the pointer leaves, so the last row does not stay lit
            let mut tme: windows_sys::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT = zeroed();
            tme.cbSize = std::mem::size_of_val(&tme) as u32;
            tme.dwFlags = windows_sys::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE;
            tme.hwndTrack = h;
            windows_sys::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut tme);
            0
        }
        WM_MOUSELEAVE => {
            if HOT.with(|x| x.replace(0)) != 0 {
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            // the tab rail first
            for i in 0..TABS.len() as u8 {
                let r = tab_rect(i);
                if x >= r.left && x < r.right && y >= r.top && y < r.bottom {
                    TAB.with(|t| t.set(i));
                    InvalidateRect(h, ptr::null(), 0);
                    return 0;
                }
            }
            let tab = TAB.with(|t| t.get());
            let op = layout(tab, sc(70)).into_iter().find(|(r, _)| x >= r.left - sc(6) && x < r.right && y >= r.top && y < r.bottom).map(|(_, row)| row.op);
            if let Some(op) = op {
                apply(op, h);
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            if wp as u16 == VK_ESCAPE {
                DestroyWindow(h);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(h);
            0
        }
        WM_DESTROY => {
            for f in FONT.with(|x| x.get()) {
                if f != 0 {
                    DeleteObject(f as HFONT);
                }
            }
            FONT.with(|x| x.set([0; 4]));
            BITS.with(|b| b.borrow_mut().clear());
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn apply(op: Op, h: HWND) {
    use winrust_common::{look as lk, style as st};
    let bar = crate::leader_bar();
    match op {
        Op::Tab(t) => TAB.with(|x| x.set(t)),
        Op::Theme(i) => {
            if let Some(s) = st::ALL.get(i as usize) {
                st::set_style(*s);
            }
            crate::relook(true);
        }
        Op::StartPos(left) => lk::start_left_set(left),
        Op::StartStyle(i) => lk::start_style_set(i as i32),
        Op::StartScale(i) => lk::start_scale_set(i as i32),
        Op::BarMonitor(i) => {
            winrust_common::look::bar_monitor_set(i as i32);
            crate::bars_sync(); // close the bars that are not wanted, create the missing ones, place them all
            crate::relook(true);
        }
        Op::BarMonitorAll => {
            winrust_common::look::bar_monitor_set(-1);
            crate::bars_sync();
            crate::relook(true);
        }
        Op::BarSide(i) => {
            st::set_side([st::Side::Bottom, st::Side::Left, st::Side::Right][i as usize]);
            crate::relook(true);
        }
        Op::BarSize(i) => {
            st::set_bar_size(i);
            crate::relook(true);
        }
        Op::Autohide => {
            st::set_autohide(!st::autohide());
            crate::relook(true);
        }
        Op::AlignLeft => {
            st::set_align_left(!st::align_left());
            crate::relook(true);
        }
        Op::AltTab => st::set_alttab_per_monitor(!st::alttab_per_monitor()),
        Op::Notify => {
            lk::notify_set(!lk::notify());
            crate::notify::refresh();
        }
        Op::NotifyTimeoutNext => {
            let next = match lk::notify_timeout() {
                3 => 5,
                5 => 8,
                8 => 10,
                _ => 3,
            };
            lk::notify_timeout_set(next);
        }
        Op::BarBlur => {
            lk::bar_blur_set(!lk::bar_blur());
            crate::settings::apply_opacity(bar);
        }
        Op::BarRadius(r) => lk::bar_radius_set(r),
        Op::MenuBlur => lk::menu_blur_set(!lk::menu_blur()),
        Op::MenuAlpha(a) => lk::menu_alpha_set(a),
        Op::MenuRadius(r) => lk::menu_radius_set(r),
        Op::Lang(i) => {
            if let Some(l) = winrust_common::Lang::ALL.get(i as usize) {
                winrust_common::set_lang(*l);
            }
        }
        Op::ColorsPage => {
            DestroyWindow(h);
            crate::settings::colors_page();
            return;
        }
        Op::TemplatesPage => {
            DestroyWindow(h);
            crate::settings::templates_page();
            return;
        }
        Op::Reset => {
            st::reset_custom();
            crate::relook(true);
        }
        Op::Close => {
            DestroyWindow(h);
            return;
        }
        Op::None => return,
    }
    crate::invalidate_bars();
}

/// Opens the settings window (one at a time: opening it again brings it to the front).
pub unsafe fn open() {
    // the fading ghost of the menu that opened this window would sit on top and eat the clicks
    crate::fly::kill_ghost();
    if let Some(h) = existing() {
        SetForegroundWindow(h);
        return;
    }
    let class: Vec<u16> = "topbar_settings".encode_utf16().chain([0]).collect();
    if GetClassInfoW(GetModuleHandleW(ptr::null()), class.as_ptr(), &mut zeroed()) == 0 {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc),
            hInstance: GetModuleHandleW(ptr::null()),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..zeroed()
        };
        RegisterClassW(&wc);
    }
    let (w, h) = (sc(W), sc(H));
    let sw = GetSystemMetrics(SM_CXSCREEN);
    let sh = GetSystemMetrics(SM_CYSCREEN);
    let x = (sw - w) / 2;
    let y = ((sh - h) / 2).max(sc(24));
    let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, class.as_ptr(), class.as_ptr(), WS_POPUP, x, y, w, h, ptr::null_mut(), ptr::null_mut(), GetModuleHandleW(ptr::null()), ptr::null());
    if hwnd.is_null() {
        return;
    }
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, sc(24), sc(24)), 1);
    // the blurred screen behind it, then the fonts once
    let radius = sc(winrust_common::look::menu_radius()).max(h / 22).max(sc(10));
    let px = crate::blur::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + h }, radius);
    BITS.with(|b| *b.borrow_mut() = px);
    let f = font(13, 400);
    let f_head = font(16, 600);
    let f_small = font(11, 400);
    let f_tab = font(13, 600);
    FONT.with(|x| x.set([f as usize, f_head as usize, f_small as usize, f_tab as usize]));
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    InvalidateRect(hwnd, ptr::null(), 0);
}

unsafe fn existing() -> Option<HWND> {
    let class: Vec<u16> = "topbar_settings".encode_utf16().chain([0]).collect();
    let h = FindWindowW(class.as_ptr(), ptr::null());
    (!h.is_null()).then_some(h)
}
