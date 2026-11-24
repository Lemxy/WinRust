//! Windows 7 style Start menu: two columns — the app list on the left, the known folders on the right — with the
//! "All programs" bar and the search box along the bottom, like Classic Shell / Start11's Windows 7 style.
//!
//! Reference: the Windows 7 Start menu (Start11 "Windows 7" style, Classic Shell "Windows 7 style").
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{HDC, DT_CENTER, DT_LEFT, DT_RIGHT, SetBkMode, SelectObject, TRANSPARENT},
    UI::WindowsAndMessaging::{DI_NORMAL, DrawIconEx},
};

use crate::{
    fill,
    fly::Act,
    startctx::{accent, bg, dim_color, hover_color, r, sc, text_color, Ctx, Row},
    text,
};
use winrust_common::{style::mix, t};

/// Flip to true once this file draws the whole panel: until then the bar keeps its built-in list.
pub const READY: bool = true;

/// Logical size of the panel.
pub fn size() -> (i32, i32) {
    (430, 600)
}

// ---------- geometry (logical px) ----------

const LEFT_L: i32 = 8;
const LEFT_R: i32 = 244; // 236 px of list: the places column never takes more than the panel can spare
const LIST_TOP: i32 = 8;
const ROW_H: i32 = 36;
/// Air above and below the rule between the pinned rows and the app list.
const PIN_GAP: i32 = 7;
const PIN_RULE: i32 = 1;
const BAR_TOP: i32 = 504;
const BAR_BOTTOM: i32 = 544;
/// The left column's rows that fit between its top and the "All programs" bar.
const ROWS_FIT: usize = ((BAR_TOP - LIST_TOP) / ROW_H) as usize;
const SEARCH_TOP: i32 = 552;
const SEARCH_BOTTOM: i32 = 592;
const SITES_TOP: i32 = 66;
const SITE_H: i32 = 30;
const SITE_COUNT: usize = 11;
const BOTTOM_TOP: i32 = 552;
const BOTTOM_BOTTOM: i32 = 592;
const GEAR_W: i32 = 30;
/// The places column: wide enough for the longest label ("Programas predeterminados") at 13 px.
const SITE_W: i32 = 178;
/// Gap between the places column and the panel's right edge.
const SITE_PAD: i32 = 8;
/// A place's label, inset from its row.
const LABEL_L: i32 = 6;
const LABEL_R: i32 = 1;

/// Right column: `SITE_W` wide, ending `SITE_PAD` before the panel's right edge.
fn right_col(w: i32) -> (i32, i32) {
    let x1 = w - SITE_PAD;
    (x1 - SITE_W, x1)
}

/// A row of the left column starting at `top`.
fn left_row(top: i32, i: usize) -> RECT {
    r(LEFT_L, top + ROW_H * i as i32, LEFT_R, top + ROW_H * (i as i32 + 1))
}

/// The pinned rows that fit above the bar: they never scroll, so the rest is cut off.
fn pin_count(c: &Ctx) -> usize {
    c.pins.len().min(ROWS_FIT)
}

/// One pinned row, at the very top of the column.
fn pin_rect(i: usize) -> RECT {
    left_row(LIST_TOP, i)
}

/// The y of the 1 px rule that closes the pinned section.
fn pin_rule_y(c: &Ctx) -> i32 {
    LIST_TOP + ROW_H * pin_count(c) as i32 + PIN_GAP
}

/// Where the app list starts: under the pins and their rule, or at the top when nothing is pinned.
fn list_top(c: &Ctx) -> i32 {
    match pin_count(c) {
        0 => LIST_TOP,
        n => LIST_TOP + ROW_H * n as i32 + PIN_GAP + PIN_RULE + PIN_GAP,
    }
}

/// One row of the app list, below the pinned section.
fn app_rect(c: &Ctx, i: usize) -> RECT {
    left_row(list_top(c), i)
}

/// The app rows that fit between the start of the list and the "All programs" bar.
fn app_capacity(c: &Ctx) -> usize {
    ((BAR_TOP - list_top(c)) / ROW_H).max(0) as usize
}

/// The app rows to show: `c.scroll` rows in and as many as fit, skipping the apps already pinned above.
fn app_rows<'a>(c: &'a Ctx<'_>) -> Vec<&'a Row> {
    let first = c.scroll.min(c.rows.len());
    c.rows[first..]
        .iter()
        .filter(|row| !c.pins.iter().any(|p| p.act == row.act))
        .take(app_capacity(c))
        .collect()
}

/// One row of the right column's places list.
fn site_rect(w: i32, i: usize) -> RECT {
    let (x0, x1) = right_col(w);
    r(x0, SITES_TOP + SITE_H * i as i32, x1, SITES_TOP + SITE_H * (i as i32 + 1))
}

/// The label box of a place row: the row inset by the column's text margins.
fn site_text_rect(rc: RECT) -> RECT {
    RECT { left: rc.left + sc(LABEL_L), top: rc.top, right: rc.right - sc(LABEL_R), bottom: rc.bottom }
}

fn bar_rect() -> RECT {
    r(LEFT_L, BAR_TOP, LEFT_R, BAR_BOTTOM)
}

fn search_rect() -> RECT {
    r(LEFT_L, SEARCH_TOP, LEFT_R, SEARCH_BOTTOM)
}

fn avatar_rect(w: i32) -> RECT {
    let (x0, _) = right_col(w);
    r(x0, 14, x0 + 40, 54)
}

fn gear_rect(w: i32) -> RECT {
    let (x0, _) = right_col(w);
    r(x0, BOTTOM_TOP, x0 + GEAR_W, BOTTOM_BOTTOM)
}

fn shutdown_rect(w: i32) -> RECT {
    let (x0, x1) = right_col(w);
    r(x0 + GEAR_W + 4, BOTTOM_TOP, x1, BOTTOM_BOTTOM)
}

fn inset(rc: RECT, d: i32) -> RECT {
    RECT { left: rc.left + d, top: rc.top + d, right: rc.right - d, bottom: rc.bottom - d }
}

/// One icon + label row of the left column, shared by the pinned section and the app list.
unsafe fn draw_row(dc: HDC, rc: RECT, row: &Row, hover: bool) {
    if hover {
        fill(dc, inset(rc, sc(2)), hover_color(), sc(3));
    }
    if !row.icon.is_null() {
        let d = sc(24);
        DrawIconEx(dc, sc(12), (rc.top + rc.bottom - d) / 2, row.icon, d, d, 0, std::ptr::null_mut(), DI_NORMAL);
    }
    let tr = RECT { left: sc(46), top: rc.top, right: sc(LEFT_R - 10), bottom: rc.bottom };
    text(dc, &row.label, tr, text_color(), DT_LEFT);
}

// ---------- the places column ----------

/// Label of place `i` (English is the key; Spanish comes from the shared table).
fn site_label(i: usize) -> &'static str {
    match i {
        0 => t!("Документы", "Documents", ja: "ドキュメント", pt: "Documentos", zh: "文档"),
        1 => t!("Изображения", "Pictures", ja: "ピクチャ", pt: "Imagens", zh: "图片"),
        2 => t!("Музыка", "Music", ja: "ミュージック", pt: "Música", zh: "音乐"),
        3 => t!("Недавние документы", "Recent items", ja: "最近使った項目", pt: "Itens recentes", zh: "最近使用的项目"),
        4 => t!("Компьютер", "This PC", ja: "この PC", pt: "Este Computador", zh: "此电脑"),
        5 => t!("Панель управления", "Control Panel", ja: "コントロール パネル", pt: "Painel de Controle", zh: "控制面板"),
        6 => t!("Параметры", "Settings", ja: "設定", pt: "Configurações", zh: "设置"),
        7 => t!("Устройства и принтеры", "Devices and printers", ja: "デバイスとプリンター", pt: "Dispositivos e impressoras", zh: "设备和打印机"),
        8 => t!("Программы по умолчанию", "Default programs", ja: "既定のプログラム", pt: "Programas padrão", zh: "默认程序"),
        9 => t!("Справка", "Help", ja: "ヘルプ", pt: "Ajuda", zh: "帮助"),
        _ => t!("Выполнить", "Run", ja: "実行", pt: "Executar", zh: "运行"),
    }
}

// ---------- public API ----------

/// Every clickable area of the panel.
pub unsafe fn rects(c: &Ctx) -> Vec<(RECT, Act)> {
    let mut v = Vec::new();
    for (i, _) in c.pins.iter().take(pin_count(c)).enumerate() {
        v.push((pin_rect(i), Act::Pin(i)));
    }
    for (vis, row) in app_rows(c).into_iter().enumerate() {
        // `row.act` keeps the app's real index in `c.rows`; only its position on screen moves
        v.push((app_rect(c, vis), row.act));
    }
    v.push((bar_rect(), Act::AllApps));
    for i in 0..SITE_COUNT {
        v.push((site_rect(c.w, i), Act::Place(i as u8)));
    }
    v.push((gear_rect(c.w), Act::StartSettings));
    v.push((shutdown_rect(c.w), Act::Power(3)));
    v
}

/// Draws the panel (the background and the blur are already painted by `fly`).
pub unsafe fn paint(dc: HDC, c: &Ctx) {
    if c.w <= 0 || c.h <= 0 {
        return;
    }
    SetBkMode(dc, TRANSPARENT as i32);
    let old = SelectObject(dc, c.fonts[0]);
    let (x0, x1) = right_col(c.w);

    // ---- left column: the pinned apps on top, then the list below them ----
    for (i, row) in c.pins.iter().take(pin_count(c)).enumerate() {
        draw_row(dc, pin_rect(i), row, c.hover == Some(Act::Pin(i)));
    }
    if pin_count(c) > 0 {
        let y = pin_rule_y(c);
        // a clear rule between what is pinned and the rest of the list, with a faint light line under it
        fill(dc, r(LEFT_L, y, LEFT_R, y + PIN_RULE), mix(bg(), text_color(), 0.26), 0);
        fill(dc, r(LEFT_L, y + PIN_RULE, LEFT_R, y + PIN_RULE + 1), mix(bg(), text_color(), 0.07), 0);
    }
    for (vis, row) in app_rows(c).into_iter().enumerate() {
        draw_row(dc, app_rect(c, vis), row, c.hover == Some(row.act));
    }
    if c.rows.is_empty() && app_capacity(c) > 0 {
        let empty = if c.query.trim().is_empty() {
            t!("Здесь пока нет приложений", "No apps here yet", ja: "ここにはまだアプリがありません", pt: "Ainda não há apps aqui", zh: "此处还没有应用")
        } else {
            t!("Приложения не найдены", "No apps found", ja: "アプリが見つかりません", pt: "Nenhum app encontrado", zh: "未找到应用")
        };
        let top = list_top(c);
        text(dc, empty, r(16, top + 4, LEFT_R - 8, top + 32), dim_color(), DT_LEFT);
    }

    // ---- "All programs": the wide accent bar under the list ----
    let br = bar_rect();
    fill(dc, br, mix(bg(), accent(), 0.35), sc(3));
    if c.hover == Some(Act::AllApps) {
        fill(dc, inset(br, sc(1)), hover_color(), sc(4));
    }
    text(dc, t!("Все программы", "All programs", ja: "すべてのプログラム", pt: "Todos os programas", zh: "所有程序"), r(16, BAR_TOP, LEFT_R - 34, BAR_BOTTOM), text_color(), DT_LEFT);
    SelectObject(dc, c.fonts[2]);
    text(dc, "\u{E76C}", r(LEFT_R - 32, BAR_TOP, LEFT_R - 10, BAR_BOTTOM), text_color(), DT_RIGHT); // chevron
    SelectObject(dc, c.fonts[0]);

    // ---- search field ----
    let sr = search_rect();
    fill(dc, sr, mix(bg(), text_color(), 0.20), sc(3)); // hairline border
    fill(dc, inset(sr, sc(1)), mix(c.card, text_color(), 0.04), sc(2));
    SelectObject(dc, c.fonts[2]);
    text(dc, "\u{E721}", RECT { left: sc(12), top: sr.top, right: sc(40), bottom: sr.bottom }, dim_color(), DT_CENTER); // magnifier
    SelectObject(dc, c.fonts[0]);
    let empty = c.query.is_empty();
    let shown = if empty { t!("Поиск программ и файлов", "Search programs and files", ja: "プログラムとファイルの検索", pt: "Pesquisar programas e arquivos", zh: "搜索程序和文件") } else { c.query };
    let tr = RECT { left: sc(44), top: sr.top, right: sc(LEFT_R - 8), bottom: sr.bottom };
    text(dc, shown, tr, if empty { dim_color() } else { text_color() }, DT_LEFT);

    // ---- right column: the user, then the places ----
    let user = std::env::var("USERNAME").unwrap_or_default();
    let av = avatar_rect(c.w);
    fill(dc, av, mix(bg(), accent(), 0.55), sc(40)); // a circle: the radius is the diameter
    let initial: String = user.chars().next().and_then(|ch| ch.to_uppercase().next()).map(String::from).unwrap_or_default();
    SelectObject(dc, c.fonts[3]);
    text(dc, &initial, av, text_color(), DT_CENTER);
    SelectObject(dc, c.fonts[0]);
    let nr = RECT { left: av.right + sc(8), top: av.top, right: x1, bottom: av.bottom };
    text(dc, &user, nr, text_color(), DT_LEFT);

    for i in 0..SITE_COUNT {
        let rc = site_rect(c.w, i);
        if c.hover == Some(Act::Place(i as u8)) {
            fill(dc, inset(rc, sc(1)), hover_color(), sc(4));
        }
        text(dc, site_label(i), site_text_rect(rc), text_color(), DT_LEFT);
    }
    // Thin rules after Music and after This PC.
    let line = mix(bg(), text_color(), 0.10);
    for i in [2usize, 4] {
        let y = sc(SITES_TOP + SITE_H * (i as i32 + 1));
        fill(dc, RECT { left: x0 + sc(4), top: y, right: x1 - sc(4), bottom: y + 1 }, line, 0);
    }

    // ---- bottom right: the gear and the "Shut down" button ----
    let gr = gear_rect(c.w);
    if c.hover == Some(Act::StartSettings) {
        fill(dc, inset(gr, sc(2)), hover_color(), sc(4));
    }
    SelectObject(dc, c.fonts[2]);
    text(dc, "\u{E713}", gr, text_color(), DT_CENTER); // gear
    SelectObject(dc, c.fonts[0]);

    let sd = shutdown_rect(c.w);
    fill(dc, sd, mix(bg(), text_color(), 0.20), sc(3)); // button border
    fill(dc, inset(sd, sc(1)), if c.hover == Some(Act::Power(3)) { hover_color() } else { c.card }, sc(2));
    SelectObject(dc, c.fonts[2]);
    text(dc, "\u{E777}", RECT { left: sd.left + sc(7), top: sd.top, right: sd.left + sc(25), bottom: sd.bottom }, text_color(), DT_CENTER); // power
    SelectObject(dc, c.fonts[0]);
    let tr = RECT { left: sd.left + sc(27), top: sd.top, right: sd.right - sc(20), bottom: sd.bottom };
    text(dc, t!("Завершение работы", "Shut down", ja: "シャットダウン", pt: "Desligar", zh: "关机"), tr, text_color(), DT_LEFT);
    SelectObject(dc, c.fonts[2]);
    text(dc, "\u{E76C}", RECT { left: sd.right - sc(20), top: sd.top, right: sd.right - sc(4), bottom: sd.bottom }, text_color(), DT_CENTER); // chevron
    SelectObject(dc, c.fonts[0]);

    SelectObject(dc, old);
}
