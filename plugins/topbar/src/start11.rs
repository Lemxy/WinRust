//! Windows 11 style Start menu: the search box on top, the pinned grid, the "Recommended" list and the footer with
//! the user and the power buttons, like Start11's "Windows 11" style.
//!
//! Reference: the Windows 11 Start menu (Start11 "Windows 11" style).
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{SelectObject, HDC, DT_CENTER, DT_LEFT, DT_RIGHT},
    UI::WindowsAndMessaging::{DrawIconEx, DI_NORMAL},
};

use crate::fly::Act;
use crate::startctx::{accent, dim_color, hover_color, r, sc, text_color, Ctx};
use crate::{fill, text};
use winrust_common::style::{mix, on_accent};

/// Flip to true once this file draws the whole panel: until then the bar keeps its built-in list.
pub const READY: bool = true;

/// Logical size of the panel.
pub fn size() -> (i32, i32) {
    (560, 640)
}

// ---------- layout (all numbers are logical px, scaled by `sc`) ----------

/// The panel is always 560x640 logical px; right and bottom edges follow the real window size instead of `sc(560)`
/// so rounding at fractional DPIs never leaves a one-pixel gap at the edge.
const LOGICAL_W: i32 = 560;
const LOGICAL_H: i32 = 640;

const PAD: i32 = 16; // the content margin (search box, list)
const SIDE: i32 = 24; // the margin used by the section headers and the footer

const COLS: i32 = 6; // pinned grid: 6 columns
const PIN_ROWS: i32 = 3; // and 3 rows, so up to 18 shortcuts
const PIN_MAX: usize = (COLS * PIN_ROWS) as usize;
const PIN_X0: i32 = 30; // first column, centred so the 6 cells leave equal side margins
const PIN_STEP: i32 = 88; // cell pitch, both across and down
const PIN_W: i32 = 60;
const PIN_H: i32 = 76;
const GRID_T: i32 = 86; // below the "Pinned" header
const HEAD_GAP: i32 = 10; // between the last grid row and the "Recommended" header

const REC_COLS: i32 = 2; // recommended list: two columns
const REC_H: i32 = 44; // one "Recommended" row
const REC_W: i32 = 256; // one of its two columns
const REC_STEP: i32 = 272; // 16 px gap between the columns
const LIST_BOTTOM: i32 = 572; // the list stops above the footer

const FOOT_T: i32 = 584; // h - 56: the footer band
const BTN: i32 = 28; // gear and power buttons
const BTN_GAP: i32 = 8;
const BTN_RIGHT: i32 = LOGICAL_W - SIDE; // the buttons end where the headers do

const POWER: [&str; 4] = ["\u{E72E}", "\u{E708}", "\u{E777}", "\u{E7E8}"]; // lock, sleep, restart, shut down

/// A logical rectangle anchored to the real panel size (top-left scaled, right/bottom measured from the panel edge).
fn ar(c: &Ctx, l: i32, t: i32, rr: i32, b: i32) -> RECT {
    r(sc(l), sc(t), c.w - sc(LOGICAL_W - rr), c.h - sc(LOGICAL_H - b))
}

fn search(c: &Ctx) -> RECT {
    ar(c, PAD, 16, LOGICAL_W - PAD, 56)
}

fn keyboard(c: &Ctx) -> RECT {
    ar(c, 508, 22, 536, 50) // 28x28 inside the search field, at its right end
}

/// Pins actually drawn: at most the 18 cells of the grid.
fn visible_pins(c: &Ctx) -> usize {
    c.pins.len().min(PIN_MAX)
}

fn grid_rows(c: &Ctx) -> i32 {
    (visible_pins(c) as i32 + COLS - 1) / COLS
}

fn pin_slot(c: &Ctx, i: usize) -> RECT {
    let (col, row) = (i as i32 % COLS, i as i32 / COLS);
    let x = PIN_X0 + col * PIN_STEP;
    let y = GRID_T + row * PIN_STEP;
    ar(c, x, y, x + PIN_W, y + PIN_H)
}

/// Bottom of the last filled grid row: empty rows are not reserved, so the list below it moves up.
fn grid_bottom(c: &Ctx) -> i32 {
    let rows = grid_rows(c);
    if rows == 0 {
        GRID_T
    } else {
        GRID_T + (rows - 1) * PIN_STEP + PIN_H
    }
}

/// Top of the "Recommended" header: it moves up when the pinned grid is shorter.
fn rec_head_t(c: &Ctx) -> i32 {
    grid_bottom(c) + HEAD_GAP
}

fn rec_top(c: &Ctx) -> i32 {
    rec_head_t(c) + 28 // header band (24) plus a small gap
}

/// Rows per column in the Recommended list: as many as fit above the footer.
fn rec_per_col(c: &Ctx) -> i32 {
    ((LIST_BOTTOM - rec_top(c)) / REC_H).max(0)
}

/// The `j`-th recommended item: column 0 fills first, then column 1.
fn rec_slot(c: &Ctx, j: usize) -> RECT {
    let per = rec_per_col(c).max(1);
    let (col, row) = (j as i32 / per, j as i32 % per);
    let x = PAD + col * REC_STEP;
    let y = rec_top(c) + row * REC_H;
    ar(c, x, y, x + REC_W, y + REC_H)
}

/// First `c.rows` entry that is not already on the grid: the pins are the first rows, so only that prefix is skipped.
fn rec_start(c: &Ctx) -> usize {
    visible_pins(c).min(c.rows.len())
}

/// First recommended row on screen: the pins, then the wheel offset. Saturating so a huge `c.scroll` stays in range.
fn rec_first(c: &Ctx) -> usize {
    rec_start(c).saturating_add(c.scroll)
}

/// Recommended items actually shown: what is left after the pins and the wheel offset and fits above the footer.
fn rec_shown(c: &Ctx) -> usize {
    let per = rec_per_col(c);
    if per <= 0 {
        return 0;
    }
    c.rows.len().saturating_sub(rec_first(c)).min((per * REC_COLS) as usize)
}

/// Footer button `k`: 0 is the gear, 1..=4 the power buttons, laid out right to left.
fn footer_btn(c: &Ctx, k: i32) -> RECT {
    let x = BTN_RIGHT - BTN - (4 - k) * (BTN + BTN_GAP);
    ar(c, x, FOOT_T + 14, x + BTN, FOOT_T + 14 + BTN)
}

/// The user picture / initials circle.
fn avatar(c: &Ctx) -> RECT {
    ar(c, SIDE, 596, SIDE + 32, 628)
}

// ---------- interaction ----------

/// Every clickable area of the panel.
pub unsafe fn rects(c: &Ctx) -> Vec<(RECT, Act)> {
    let mut v = Vec::new();
    for i in 0..visible_pins(c) {
        if c.pins.get(i).is_some() {
            // Pins mirror the first rows, so the pin index is also the row index. `Pin` gives the cell its
            // own hover identity, separate from the Recommended row that shows the same app.
            v.push((pin_slot(c, i), Act::Pin(i)));
        }
    }
    let first = rec_first(c);
    for j in 0..rec_shown(c) {
        if let Some(row) = c.rows.get(first + j) {
            // `row.act` is `Act::App(<absolute row>)`, never the on-screen slot `j`, so clicks launch the right app.
            v.push((rec_slot(c, j), row.act));
        }
    }
    v.push((footer_btn(c, 0), Act::StartSettings));
    for p in 0..4u8 {
        v.push((footer_btn(c, p as i32 + 1), Act::Power(p)));
    }
    v.push((keyboard(c), Act::Keyboard)); // last: nothing else may sit on top of it
    v
}

// ---------- painting ----------

/// Draws the panel (the background and the blur are already painted by `fly`).
pub unsafe fn paint(dc: HDC, c: &Ctx) {
    // --- search field -------------------------------------------------------
    fill(dc, search(c), c.card, sc(36)); // radius 18
    SelectObject(dc, c.fonts[2]);
    text(dc, "\u{E721}", ar(c, 28, 16, 56, 56), dim_color(), DT_CENTER); // magnifier
    SelectObject(dc, c.fonts[0]);
    let typed = !c.query.is_empty();
    let query = if typed {
        c.query
    } else {
        t!(
            "Поиск приложений, параметров и документов",
            "Search for apps, settings, and documents",
            ja: "アプリ、設定、ドキュメントを検索",
            pt: "Pesquisar apps, configurações e documentos",
            zh: "搜索应用、设置和文档"
        )
    };
    text(dc, query, ar(c, 58, 16, 500, 56), if typed { text_color() } else { dim_color() }, DT_LEFT);

    // the touch keyboard button, with the hover highlight of the field's rounded style
    let kb = keyboard(c);
    let kb_hot = c.hover == Some(Act::Keyboard);
    if kb_hot {
        fill(dc, kb, hover_color(), sc(12));
    }
    SelectObject(dc, c.fonts[2]);
    text(dc, "\u{E765}", kb, if kb_hot { text_color() } else { dim_color() }, DT_CENTER);

    // --- pinned -------------------------------------------------------------
    SelectObject(dc, c.fonts[1]);
    let h1 = ar(c, SIDE, 60, LOGICAL_W - SIDE, 84);
    text(dc, t!("Закреплено", "Pinned", ja: "ピン留め", pt: "Fixado", zh: "已固定"), h1, dim_color(), DT_LEFT);
    text(dc, t!("Показать все", "Show all", ja: "すべて表示", pt: "Mostrar tudo", zh: "显示全部"), h1, dim_color(), DT_RIGHT);

    for i in 0..visible_pins(c) {
        let Some(p) = c.pins.get(i) else { continue };
        let rc = pin_slot(c, i);
        if c.hover == Some(Act::Pin(i)) {
            fill(dc, rc, hover_color(), sc(16)); // radius 16
        }
        if !p.icon.is_null() {
            let sz = sc(32);
            DrawIconEx(dc, (rc.left + rc.right - sz) / 2, rc.top + sc(8), p.icon, sz, sz, 0, std::ptr::null_mut(), DI_NORMAL);
        }
        SelectObject(dc, c.fonts[1]);
        text(dc, &p.label, r(rc.left, rc.top + sc(46), rc.right, rc.top + sc(70)), text_color(), DT_CENTER);
    }

    // --- recommended --------------------------------------------------------
    // The whole section disappears when the pins use every row, so no empty header is left behind.
    let shown = rec_shown(c);
    if shown > 0 {
        let rh = ar(c, SIDE, rec_head_t(c), LOGICAL_W - SIDE, rec_head_t(c) + 24);
        SelectObject(dc, c.fonts[1]);
        text(
            dc,
            t!("Рекомендуем", "Recommended", ja: "おすすめ", pt: "Recomendado", zh: "推荐"),
            rh,
            dim_color(),
            DT_LEFT,
        );

        let start = rec_first(c);
        for j in 0..shown {
            let Some(row) = c.rows.get(start + j) else { continue };
            let rc = rec_slot(c, j);
            if c.hover == Some(row.act) {
                fill(dc, rc, hover_color(), sc(16));
            }
            if !row.icon.is_null() {
                let sz = sc(20);
                DrawIconEx(dc, rc.left + sc(8), (rc.top + rc.bottom - sz) / 2, row.icon, sz, sz, 0, std::ptr::null_mut(), DI_NORMAL);
            }
            SelectObject(dc, c.fonts[0]);
            text(dc, &row.label, r(rc.left + sc(36), rc.top, rc.right - sc(8), rc.top + sc(24)), text_color(), DT_LEFT);
            SelectObject(dc, c.fonts[1]);
            text(
                dc,
                t!("Недавно добавлено", "Recently added", ja: "最近追加されたもの", pt: "Adicionado recentemente", zh: "最近添加"),
                r(rc.left + sc(36), rc.top + sc(22), rc.right - sc(8), rc.top + sc(42)),
                dim_color(),
                DT_LEFT,
            );
        }
    }

    // empty panel / no search results
    if c.rows.is_empty() {
        let msg = if typed {
            t!("Приложения не найдены", "No apps found", ja: "アプリが見つかりません", pt: "Nenhum app encontrado", zh: "未找到应用")
        } else {
            t!("Здесь пока пусто", "Nothing here yet", ja: "まだ何もありません", pt: "Nada aqui ainda", zh: "这里还什么都没有")
        };
        let mid = (rec_top(c) + LIST_BOTTOM) / 2;
        SelectObject(dc, c.fonts[0]);
        text(dc, msg, ar(c, SIDE, mid - 16, LOGICAL_W - SIDE, mid + 16), dim_color(), DT_CENTER);
    }

    // --- footer -------------------------------------------------------------
    fill(dc, ar(c, SIDE, FOOT_T - 4, LOGICAL_W - SIDE, FOOT_T - 3), mix(c.card, dim_color(), 0.25), 0); // hairline

    let av = avatar(c);
    fill(dc, av, accent(), sc(32)); // circle
    let user = std::env::var("USERNAME").unwrap_or_default();
    let initial = user
        .chars()
        .find(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_uppercase().next().unwrap_or(ch))
        .unwrap_or('?');
    SelectObject(dc, c.fonts[3]);
    text(dc, &initial.to_string(), av, on_accent(), DT_CENTER);
    SelectObject(dc, c.fonts[0]);
    text(dc, &user, ar(c, SIDE + 40, 596, LOGICAL_W - SIDE - 176, 628), text_color(), DT_LEFT);

    let gear = footer_btn(c, 0);
    let gear_hot = c.hover == Some(Act::StartSettings);
    if gear_hot {
        fill(dc, gear, hover_color(), sc(12));
    }
    SelectObject(dc, c.fonts[2]);
    text(dc, "\u{E713}", gear, if gear_hot { text_color() } else { dim_color() }, DT_CENTER);

    for p in 0..4u8 {
        let rc = footer_btn(c, p as i32 + 1);
        if c.hover == Some(Act::Power(p)) {
            fill(dc, rc, hover_color(), sc(12));
        }
        SelectObject(dc, c.fonts[2]);
        text(dc, POWER[p as usize], rc, text_color(), DT_CENTER);
    }

    SelectObject(dc, c.fonts[0]); // leave the DC with the body font, like the built-in panel
}
