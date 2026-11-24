//! Windows 10 style Start menu: a narrow rail, the alphabetical app list on the left and the pinned tiles on the
//! right, like Start11's "Windows 10" style.
//!
//! Reference: the Windows 10 Start menu (Start11 "Windows 10" style).
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::{SelectObject, SetBkMode, HDC, DT_CENTER, DT_LEFT, TRANSPARENT},
    UI::WindowsAndMessaging::{DrawIconEx, DI_NORMAL},
};

use crate::fly::Act;
use crate::startctx::{accent, bg, dim_color, hover_color, r, sc, text_color, Ctx};
use crate::{fill, text};
use winrust_common::style::mix;

/// Flip to true once this file draws the whole panel: until then the bar keeps its built-in list.
pub const READY: bool = true;

/// Logical size of the panel.
pub fn size() -> (i32, i32) {
    (620, 660)
}

// ---------- layout ----------
// Everything is in logical pixels: `r` and `sc` scale it at paint time, and `rects` and `paint` run the same layout
// helpers, so the hit boxes and the drawing can never drift apart.

const RAIL: i32 = 48; // width of the left rail
const BTN: i32 = 36; // one rail button
const LIST_L: i32 = 56; // app list, flush with the rail
const LIST_R: i32 = 300;
const LIST_TOP: i32 = 46; // where the app list starts, under its section title
const ROW_H: i32 = 34; // one app row, including its 1 px gap
const HEAD_H: i32 = 20; // a letter heading

const GRID_X: i32 = 310; // first tile column
const GRID_TOP: i32 = 42; // under the "Pinned" title: six rows of 96 plus five 8 px gaps end at 658
const TILE: i32 = 96; // Windows 10 tiles are square
const GAP: i32 = 8; // between tiles, both ways
const TILE_COLS: i32 = 3; // three columns fill the right column
const TILE_MAX: usize = 18; // fly hands over at most 18 pins
const TILE_DX: i32 = TILE + GAP;
const TILE_DY: i32 = TILE + GAP;
const TILE_ICON: i32 = 32; // the icon inside a tile

/// One of the two rail buttons: 0 is the settings gear, 1 the power button, stacked at the bottom. They are the only
/// rail entries with a real `Act`; the user avatar and the hamburger were removed instead of being dead buttons.
fn rail_btn(c: &Ctx, slot: i32) -> RECT {
    let y = c.h - 10 - BTN - (1 - slot) * 42;
    r(6, y, 6 + BTN, y + BTN)
}

/// A line of the left column: a letter heading or an app row (`i` indexes `ctx.rows`).
#[derive(Clone, Copy)]
enum Line {
    Head(char, RECT),
    App { rc: RECT, i: usize, act: Act },
}

/// The letter a row is headed under: the first visible character, in upper case.
fn initial(label: &str) -> char {
    label
        .chars()
        .find(|ch| !ch.is_whitespace())
        .and_then(|ch| ch.to_uppercase().next())
        .unwrap_or('#')
}

/// Every row of `ctx.rows`, alphabetically by label and case-insensitive; equal labels keep their original order.
/// `rects` and `paint` both call this, so both draw (and hit-test) the exact same list.
fn ordered(c: &Ctx) -> Vec<usize> {
    let mut v: Vec<(String, usize)> = c.rows.iter().enumerate().map(|(i, row)| (row.label.to_lowercase(), i)).collect();
    v.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    v.into_iter().map(|(_, i)| i).collect()
}

/// The alphabetical list, letter headings included, cut off where the panel ends. The wheel moves `c.scroll`, so the
/// list starts that many rows in; headings are derived from the visible rows only, so a list scrolled past "A" starts
/// with the letter it is actually showing. A heading is only emitted together with a row under it, so no letter is
/// ever left dangling at the bottom.
fn lines(c: &Ctx) -> Vec<Line> {
    let mut out = Vec::new();
    let (mut y, mut last): (i32, Option<char>) = (LIST_TOP, None);
    let bottom = c.h - 12;
    // `rows` arrives alphabetical from fly, so skipping `c.scroll` here is the same as starting at `c.rows[c.scroll]`.
    // Every row keeps its own absolute `Act::App` index, so a click always opens the row that is drawn. The clamp only
    // guards against a stale scroll value after the list shrinks: it never leaves the panel blank.
    let skip = c.scroll.min(c.rows.len().saturating_sub(1));
    for i in ordered(c).into_iter().skip(skip) {
        let row = &c.rows[i];
        let ch = initial(&row.label);
        let head = last != Some(ch);
        let need = ROW_H + if head { HEAD_H } else { 0 };
        if y + need > bottom {
            break;
        }
        if head {
            out.push(Line::Head(ch, r(LIST_L + 10, y, LIST_R, y + HEAD_H)));
            y += HEAD_H;
            last = Some(ch);
        }
        out.push(Line::App { rc: r(LIST_L, y + 1, LIST_R, y + ROW_H - 1), i, act: row.act });
        y += ROW_H;
    }
    out
}

/// One tile of the right grid: `i` indexes `ctx.pins`, `act` is the `Act::Pin` it answers to.
#[derive(Clone, Copy)]
struct Tile {
    rc: RECT,
    i: usize,
    act: Act,
}

/// How many tile columns fit beside the list: three at the normal size, two when the Start menu is set to 90 %.
fn tile_cols(c: &Ctx) -> i32 {
    ((c.w - 4 - GRID_X + GAP) / TILE_DX).clamp(1, TILE_COLS)
}

/// The tiles that fit: up to 18 pins, row-major. Ones past the panel edge (a smaller window at a fractional DPI, say)
/// are dropped rather than drawn outside the panel. Each tile gets `Act::Pin` pointing at its row in `ctx.rows`, so the
/// grid and the list highlight independently even though both open the same app.
fn tiles(c: &Ctx) -> Vec<Tile> {
    let mut out = Vec::new();
    let (cols, bottom) = (tile_cols(c), c.h - 2);
    for (i, row) in c.pins.iter().enumerate().take(TILE_MAX) {
        let (col, line) = (i as i32 % cols, i as i32 / cols);
        let (x, y) = (GRID_X + TILE_DX * col, GRID_TOP + TILE_DY * line);
        if x + TILE > c.w - 4 || y + TILE > bottom {
            break;
        }
        // `pins` is a prefix slice of `rows` today, so `i` is already that row's position; the lookup keeps the tile
        // honest if the two ever stop being the same slice.
        let j = c.rows.iter().position(|r| std::ptr::eq(r, row)).unwrap_or(i);
        out.push(Tile { rc: r(x, y, x + TILE, y + TILE), i, act: Act::Pin(j) });
    }
    out
}

/// Selects one of the five fonts `fly` created; a null handle (as in the hit-test context) keeps the current one.
unsafe fn use_font(dc: HDC, c: &Ctx, i: usize) {
    if let Some(f) = c.fonts.get(i).copied() {
        if !f.is_null() {
            let _ = SelectObject(dc, f);
        }
    }
}

/// Every clickable area of the panel: every listed app, every drawn tile and the two rail buttons with a real action.
pub unsafe fn rects(c: &Ctx) -> Vec<(RECT, Act)> {
    let mut v = Vec::new();
    // tiles first, the list after: they never overlap, but the list stays on top if they ever do
    for t in tiles(c) {
        v.push((t.rc, t.act));
    }
    for line in lines(c) {
        if let Line::App { rc, act, .. } = line {
            v.push((rc, act));
        }
    }
    v.push((rail_btn(c, 0), Act::StartSettings));
    v.push((rail_btn(c, 1), Act::Power(3)));
    v
}

/// Draws the panel (the background and the blur are already painted by `fly`).
pub unsafe fn paint(dc: HDC, c: &Ctx) {
    SetBkMode(dc, TRANSPARENT as i32);

    // Rail: a hairline against the columns (the panel itself stays translucent), then the two buttons that actually do
    // something: the settings gear and the power glyph. The user avatar and the hamburger had no `Act`, so they are no
    // longer drawn instead of being dead buttons.
    fill(dc, r(RAIL, 8, RAIL + 1, c.h - 8), mix(bg(), text_color(), 0.10), 0);
    use_font(dc, c, 2);
    let buttons: [(i32, &str, Act); 2] = [(0, "\u{E713}", Act::StartSettings), (1, "\u{E7E8}", Act::Power(3))];
    for (slot, glyph, act) in buttons {
        let rc = rail_btn(c, slot);
        let hot = c.hover == Some(act);
        if hot {
            fill(dc, rc, hover_color(), sc(4));
        }
        text(dc, glyph, rc, text_color(), DT_CENTER);
    }

    // Section titles shared by both columns.
    use_font(dc, c, 1);
    text(dc, t!("Недавно добавленные", "Recently added", ja: "最近追加されたもの", pt: "Adicionado recentemente", zh: "最近添加"), r(LIST_L + 4, 16, LIST_R, 36), dim_color(), DT_LEFT);
    text(dc, t!("Закреплено", "Pinned", ja: "ピン留め", pt: "Fixado", zh: "已固定"), r(GRID_X, 16, c.w - 16, 36), dim_color(), DT_LEFT);

    // Left column: letter headings and app rows with a 24 px icon; only the row under the pointer (Act::App) is lit.
    for line in lines(c) {
        match line {
            Line::Head(ch, rc) => {
                use_font(dc, c, 1);
                text(dc, &ch.to_string(), rc, dim_color(), DT_LEFT);
            }
            Line::App { rc, i, act } => {
                let row = &c.rows[i];
                use_font(dc, c, 0);
                if c.hover == Some(act) {
                    fill(dc, rc, hover_color(), sc(4));
                }
                let sz = sc(24);
                let iy = rc.top + (rc.bottom - rc.top - sz) / 2;
                DrawIconEx(dc, rc.left + sc(8), iy, row.icon, sz, sz, 0, std::ptr::null_mut(), DI_NORMAL);
                text(dc, &row.label, RECT { left: rc.left + sc(40), right: rc.right - sc(8), ..rc }, text_color(), DT_LEFT);
            }
        }
    }
    if c.rows.is_empty() {
        use_font(dc, c, 0);
        text(dc, t!("Приложения не найдены", "No apps found", ja: "アプリが見つかりません", pt: "Nenhum app encontrado", zh: "未找到应用"), r(LIST_L + 10, LIST_TOP, LIST_R, LIST_TOP + ROW_H), dim_color(), DT_LEFT);
    }

    // Right column: the pinned tiles, tinted with the accent. A tile answers to `Act::Pin`, so the same app listed on
    // the left never lights together with its tile. The label sits under the 32 px icon and `text` adds
    // DT_END_ELLIPSIS, so a long name ends in dots instead of spilling out.
    let base = mix(bg(), accent(), 0.35);
    for tile in tiles(c) {
        let row = &c.pins[tile.i];
        let rc = tile.rc;
        let hot = c.hover == Some(tile.act);
        fill(dc, rc, if hot { mix(base, text_color(), 0.12) } else { base }, 0);
        if !row.icon.is_null() {
            let sz = sc(TILE_ICON);
            DrawIconEx(dc, (rc.left + rc.right - sz) / 2, rc.top + sc(16), row.icon, sz, sz, 0, std::ptr::null_mut(), DI_NORMAL);
        }
        use_font(dc, c, 1);
        text(dc, &row.label, RECT { left: rc.left + sc(6), top: rc.top + sc(58), right: rc.right - sc(6), bottom: rc.top + sc(86) }, text_color(), DT_CENTER);
    }
}
