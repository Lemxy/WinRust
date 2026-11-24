//! The annotation editor: dark toolbar with Segoe MDL2 icons, the capture on a canvas, Spectacle's tools (crop/select,
//! arrow, line, rectangle, ellipse, text, highlighter, pixelate, blur, automatic numbers), undo/redo and save/copy.
use std::{
    cell::RefCell,
    mem::zeroed,
    path::PathBuf,
    ptr::{null, null_mut},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::{
        Dwm::DwmSetWindowAttribute,
        Gdi::*,
    },
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount64},
    UI::{
        HiDpi::GetDpiForSystem,
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::*,
    },
};

use winrust_common::{
    monitors,
    style::{mix, pal, rgb},
};

use crate::{
    annot::{self, Shape, Tool},
    img::{Image, make_dib},
    save,
    ui,
};

const BG: u32 = 0x0014_1416;
const BAR_H: i32 = 52;
const STATUS_H: i32 = 30;
const BTN: i32 = 38;

#[derive(Clone)]
struct Snap {
    shapes: Vec<Shape>,
    crop: RECT,
    next_num: u32,
}

struct Drag {
    start: (i32, i32),
    end: (i32, i32),
    pts: Vec<(i32, i32)>,
}

struct TextIn {
    pos: (i32, i32),
    buf: String,
}

struct Ed {
    base: Image,
    crop: RECT,
    shapes: Vec<Shape>,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    tool: Tool,
    next_num: u32,
    drag: Option<Drag>,
    text: Option<TextIn>,
    selected: Option<usize>,
    moving: Option<(i32, i32)>,
    move_pushed: bool,
    hover: Option<usize>,
    zoom: f32,
    fit: bool,
    file: Option<PathBuf>,
    last_saved: Option<PathBuf>,
    status: Option<(String, u64)>,
    canvas: HBITMAP,
    cdc: HDC,
    bits: *mut u8,
    cw: i32,
    ch: i32,
    dirty: bool,
    back: HBITMAP,
    bdc: HDC,
    bw: i32,
    bh: i32,
}

thread_local! {
    static ED: RefCell<Option<Ed>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy)]
enum Act {
    Tool(Tool),
    Undo,
    Redo,
    Clear,
    Save,
    Copy,
    Close,
}

fn buttons() -> Vec<(&'static str, String, Act)> {
    vec![
        ("\u{E7A8}", t!("Обрезать", "Crop", ja: "切り抜き", pt: "Recortar", zh: "裁剪").to_string(), Act::Tool(Tool::Crop)),
        ("\u{E8B0}", t!("Выбор", "Select", ja: "選択", pt: "Selecionar", zh: "选择").to_string(), Act::Tool(Tool::Select)),
        ("\u{E72A}", t!("Стрелка", "Arrow", ja: "矢印", pt: "Seta", zh: "箭头").to_string(), Act::Tool(Tool::Arrow)),
        ("\u{E738}", t!("Линия", "Line", ja: "線", pt: "Linha", zh: "直线").to_string(), Act::Tool(Tool::Line)),
        ("\u{E7FB}", t!("Прямоугольник", "Rectangle", ja: "長方形", pt: "Retângulo", zh: "矩形").to_string(), Act::Tool(Tool::Rect)),
        ("\u{E7F4}", t!("Эллипс", "Ellipse", ja: "楕円", pt: "Elipse", zh: "椭圆").to_string(), Act::Tool(Tool::Ellipse)),
        ("\u{E8D2}", t!("Текст", "Text", ja: "テキスト", pt: "Texto", zh: "文字").to_string(), Act::Tool(Tool::Text)),
        ("\u{E7E6}", t!("Маркер", "Highlighter", ja: "マーカー", pt: "Marcador", zh: "荧光笔").to_string(), Act::Tool(Tool::Highlighter)),
        ("\u{E7F8}", t!("Пиксели", "Pixelate", ja: "モザイク", pt: "Pixelizar", zh: "马赛克").to_string(), Act::Tool(Tool::Pixelate)),
        ("\u{E81A}", t!("Размытие", "Blur", ja: "ぼかし", pt: "Desfoque", zh: "模糊").to_string(), Act::Tool(Tool::Blur)),
        ("\u{E8EF}", t!("Номер", "Number", ja: "番号", pt: "Número", zh: "编号").to_string(), Act::Tool(Tool::Number)),
        ("\u{E7A7}", t!("Отменить", "Undo", ja: "元に戻す", pt: "Desfazer", zh: "撤销").to_string(), Act::Undo),
        ("\u{E7A6}", t!("Повторить", "Redo", ja: "やり直す", pt: "Refazer", zh: "重做").to_string(), Act::Redo),
        ("\u{E74D}", t!("Очистить всё", "Clear all", ja: "すべて消去", pt: "Limpar tudo", zh: "全部清除").to_string(), Act::Clear),
        ("\u{E74E}", t!("Сохранить и скопировать", "Save and copy", ja: "保存してコピー", pt: "Salvar e copiar", zh: "保存并复制").to_string(), Act::Save),
        ("\u{E8C8}", t!("Копировать", "Copy", ja: "コピー", pt: "Copiar", zh: "复制").to_string(), Act::Copy),
        ("\u{E711}", t!("Закрыть", "Close", ja: "閉じる", pt: "Fechar", zh: "关闭").to_string(), Act::Close),
    ]
}

fn toolbar_h(hwnd: HWND) -> i32 {
    unsafe { ui::sc(hwnd, BAR_H) }
}
fn status_h(hwnd: HWND) -> i32 {
    unsafe { ui::sc(hwnd, STATUS_H) }
}
fn btn_size(hwnd: HWND) -> i32 {
    unsafe { ui::sc(hwnd, BTN) }
}

fn btn_rect(hwnd: HWND, i: usize) -> RECT {
    unsafe {
        let s = btn_size(hwnd);
        let gap = ui::sc(hwnd, 4);
        let left = ui::sc(hwnd, 8) + i as i32 * (s + gap) + if i >= 11 { ui::sc(hwnd, 10) } else { 0 } + if i >= 14 { ui::sc(hwnd, 10) } else { 0 };
        let top = (toolbar_h(hwnd) - s) / 2;
        RECT { left, top, right: left + s, bottom: top + s }
    }
}

fn in_rect(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn rects_intersect(a: RECT, b: RECT) -> bool {
    a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top
}

/// Starts the editor for an image. Returns the process exit code.
pub fn run(img: Image, file: Option<PathBuf>) -> i32 {
    unsafe { open(img, file) }
}

unsafe fn open(img: Image, file: Option<PathBuf>) -> i32 {
    let cursor = crate::cap::cursor_pos();
    let work = monitors::at(cursor.x, cursor.y).work;
    let (cw, ch) = (img.w, img.h);
    let hwnd0 = null_mut();
    let cls = ui::wide("rs-shot-editor");
    let hi = GetModuleHandleW(null());
    let mut wc: WNDCLASSW = zeroed();
    wc.lpfnWndProc = Some(proc_);
    wc.hInstance = hi;
    wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
    wc.hbrBackground = null_mut();
    wc.lpszClassName = cls.as_ptr();
    RegisterClassW(&wc);
    let sys_dpi = GetDpiForSystem() as i32;
    let min_w = 760 * sys_dpi.max(96) / 96;
    let want_w = (cw + 80).clamp(min_w, (work.right - work.left) * 9 / 10);
    let want_h = (ch + 110).clamp(560 * sys_dpi.max(96) / 96, (work.bottom - work.top) * 9 / 10);
    let x = work.left + (work.right - work.left - want_w) / 2;
    let y = work.top + (work.bottom - work.top - want_h) / 2;
    let title = t!("rs-shot — Снимок", "rs-shot — Screenshot", ja: "rs-shot — スクリーンショット", pt: "rs-shot — Captura", zh: "rs-shot — 截图");
    let hwnd = CreateWindowExW(
        0,
        cls.as_ptr(),
        ui::wide(title).as_ptr(),
        WS_OVERLAPPEDWINDOW,
        x,
        y,
        want_w,
        want_h,
        hwnd0,
        null_mut(),
        hi,
        null(),
    );
    if hwnd.is_null() {
        return 2;
    }
    let dark: u32 = 1;
    DwmSetWindowAttribute(hwnd, 20, &dark as *const u32 as *const _, 4); // dark title bar where supported
    let crop = RECT { left: 0, top: 0, right: img.w, bottom: img.h };
    ED.set(Some(Ed {
        base: img,
        crop,
        shapes: Vec::new(),
        undo: Vec::new(),
        redo: Vec::new(),
        tool: Tool::Arrow,
        next_num: 1,
        drag: None,
        text: None,
        selected: None,
        moving: None,
        move_pushed: false,
        hover: None,
        zoom: 1.0,
        fit: true,
        file,
        last_saved: None,
        status: None,
        canvas: null_mut(),
        cdc: null_mut(),
        bits: null_mut(),
        cw: 0,
        ch: 0,
        dirty: true,
        back: null_mut(),
        bdc: null_mut(),
        bw: 0,
        bh: 0,
    }));
    ShowWindow(hwnd, SW_SHOW);
    UpdateWindow(hwnd);
    winrust_common::fg::force(hwnd);
    let mut m: MSG = zeroed();
    while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    cleanup();
    0
}

unsafe fn cleanup() {
    ED.with_borrow_mut(|e| {
        if let Some(e) = e {
            if !e.canvas.is_null() {
                DeleteObject(e.canvas as HGDIOBJ);
            }
            if !e.cdc.is_null() {
                DeleteDC(e.cdc);
            }
            if !e.back.is_null() {
                DeleteObject(e.back as HGDIOBJ);
            }
            if !e.bdc.is_null() {
                DeleteDC(e.bdc);
            }
            e.canvas = null_mut();
            e.cdc = null_mut();
            e.back = null_mut();
            e.bdc = null_mut();
        }
    });
    ED.set(None);
}

fn crop_size(e: &Ed) -> (i32, i32) {
    ((e.crop.right - e.crop.left).max(1), (e.crop.bottom - e.crop.top).max(1))
}

fn snapshot(e: &Ed) -> Snap {
    Snap { shapes: e.shapes.clone(), crop: e.crop, next_num: e.next_num }
}

fn push_undo(e: &mut Ed) {
    e.undo.push(snapshot(e));
    if e.undo.len() > 60 {
        e.undo.remove(0);
    }
    e.redo.clear();
}

fn restore(e: &mut Ed, s: Snap) {
    e.shapes = s.shapes;
    e.crop = s.crop;
    e.next_num = s.next_num;
    e.selected = None;
    e.text = None;
    e.dirty = true;
}

fn do_undo(e: &mut Ed) {
    if let Some(prev) = e.undo.pop() {
        let cur = snapshot(e);
        e.redo.push(cur);
        restore(e, prev);
    }
}

fn do_redo(e: &mut Ed) {
    if let Some(next) = e.redo.pop() {
        let cur = snapshot(e);
        e.undo.push(cur);
        restore(e, next);
    }
}

fn preview(e: &Ed) -> Option<Shape> {
    if let Some(t) = &e.text {
        if !t.buf.is_empty() {
            return Some(Shape::Text { pos: t.pos, text: t.buf.clone(), color: pal().accent, size: 24 });
        }
        return None;
    }
    let d = e.drag.as_ref()?;
    if matches!(e.tool, Tool::Crop | Tool::Select) || e.tool == Tool::Text || e.tool == Tool::Number {
        return None;
    }
    if e.tool == Tool::Highlighter {
        let mut pts = d.pts.clone();
        if pts.len() < 2 {
            pts = vec![d.start, d.end];
        }
        return Some(Shape::Highlighter { pts, color: winrust_common::style::WARN, width: 14 });
    }
    annot::make(e.tool, d.start, d.end, e.next_num)
}

/// Rebuilds the canvas from the base image and every committed shape. Runs only when something changed.
unsafe fn ensure_canvas(e: &mut Ed) {
    let (cw, ch) = crop_size(e);
    if e.canvas.is_null() || e.cw != cw || e.ch != ch {
        if !e.canvas.is_null() {
            DeleteObject(e.canvas as HGDIOBJ);
            DeleteDC(e.cdc);
        }
        let dc = GetDC(null_mut());
        let Some((hbm, bits)) = make_dib(dc, cw, ch, 32) else {
            ReleaseDC(null_mut(), dc);
            return;
        };
        let cdc = CreateCompatibleDC(dc);
        SelectObject(cdc, hbm as HGDIOBJ);
        ReleaseDC(null_mut(), dc);
        e.canvas = hbm;
        e.bits = bits;
        e.cdc = cdc;
        e.cw = cw;
        e.ch = ch;
        e.dirty = true;
    }
    if !e.dirty {
        return;
    }
    // base pixels (the crop rect, row by row)
    for y in 0..ch {
        let sy = e.crop.top + y;
        if sy < 0 || sy >= e.base.h {
            continue;
        }
        let sx = e.crop.left.max(0);
        let n = cw.min(e.base.w - sx).max(0) as usize;
        if n == 0 {
            continue;
        }
        let src = ((sy * e.base.w + sx) * 4) as usize;
        let dst = (y * cw * 4) as usize;
        std::ptr::copy_nonoverlapping(e.base.px.as_ptr().add(src), e.bits.add(dst), n * 4);
    }
    let pv = preview(e);
    annot::render(e.cdc, e.bits, cw, ch, e.crop.left, e.crop.top, &e.shapes, pv.as_ref());
    // text caret
    if let Some(t) = &e.text {
        let cxp = t.pos.0 - e.crop.left + t.buf.chars().count() as i32 * 13;
        let cyp = t.pos.1 - e.crop.top;
        let r = RECT { left: cxp, top: cyp, right: cxp + 2, bottom: cyp + 26 };
        let b = CreateSolidBrush(pal().accent);
        FillRect(e.cdc, &r, b);
        DeleteObject(b as HGDIOBJ);
    }
    e.dirty = false;
}

/// (scale, x, y) of the crop inside the client's canvas area.
fn view(e: &Ed, hwnd: HWND) -> (f32, i32, i32) {
    unsafe {
        let mut c: RECT = zeroed();
        GetClientRect(hwnd, &mut c);
        let area = RECT { left: 0, top: toolbar_h(hwnd), right: c.right, bottom: (c.bottom - status_h(hwnd)).max(toolbar_h(hwnd) + 1) };
        let (cw, ch) = crop_size(e);
        let aw = (area.right - area.left - 24).max(32) as f32;
        let ah = (area.bottom - area.top - 24).max(32) as f32;
        let fit = (aw / cw as f32).min(ah / ch as f32).clamp(0.02, 3.0);
        let s = if e.fit { fit } else { e.zoom };
        let ox = area.left + ((area.right - area.left) - (cw as f32 * s) as i32) / 2;
        let oy = area.top + ((area.bottom - area.top) - (ch as f32 * s) as i32) / 2;
        (s, ox, oy)
    }
}

fn to_image(e: &Ed, hwnd: HWND, x: i32, y: i32) -> (i32, i32) {
    let (s, ox, oy) = view(e, hwnd);
    (e.crop.left + ((x - ox) as f32 / s) as i32, e.crop.top + ((y - oy) as f32 / s) as i32)
}

unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let screen = BeginPaint(hwnd, &mut ps);
    let mut c: RECT = zeroed();
    GetClientRect(hwnd, &mut c);
    let p = pal();
    // Persistent client-sized back buffer: hover, drag and typing never draw straight to the window DC.
    let (back_dc, made) = ED.with_borrow_mut(|slot| {
        if let Some(e) = slot {
            if e.bdc.is_null() || e.bw != c.right || e.bh != c.bottom {
                if !e.back.is_null() {
                    DeleteObject(e.back as HGDIOBJ);
                }
                if !e.bdc.is_null() {
                    DeleteDC(e.bdc);
                }
                let g = GetDC(null_mut());
                let bmp = CreateCompatibleBitmap(g, c.right.max(1), c.bottom.max(1));
                let dc = CreateCompatibleDC(g);
                if !bmp.is_null() && !dc.is_null() {
                    SelectObject(dc, bmp as HGDIOBJ);
                }
                ReleaseDC(null_mut(), g);
                e.back = bmp;
                e.bdc = dc;
                e.bw = c.right;
                e.bh = c.bottom;
                (dc, true)
            } else {
                (e.bdc, false)
            }
        } else {
            (null_mut(), false)
        }
    });
    let mut p = ps.rcPaint;
    if made {
        p = c;
    }
    let hdc = if back_dc.is_null() { screen } else { back_dc };
    let toolbar = RECT { left: 0, top: 0, right: c.right, bottom: toolbar_h(hwnd) };
    let status = RECT { left: 0, top: c.bottom - status_h(hwnd), right: c.right, bottom: c.bottom };
    ui::fill(hdc, c, BG);
    ui::fill(hdc, toolbar, p.bg);
    ui::fill(hdc, RECT { left: 0, top: toolbar.bottom - 1, right: c.right, bottom: toolbar.bottom }, p.line);
    ui::fill(hdc, status, p.bg);
    ui::fill(hdc, RECT { left: 0, top: status.top, right: c.right, bottom: status.top + 1 }, p.line);

    let canvas_area = RECT { left: 0, top: toolbar_h(hwnd), right: c.right, bottom: (c.bottom - status_h(hwnd)).max(toolbar_h(hwnd) + 1) };
    if made || rects_intersect(canvas_area, p) {
        ED.with_borrow_mut(|slot| {
            if let Some(e) = slot {
                ensure_canvas(e);
                let (cw, ch) = (e.cw, e.ch);
                let (s, ox, oy) = view(e, hwnd);
                let area = canvas_area;
                let clip = CreateRectRgn(area.left, area.top, area.right, area.bottom);
                SelectClipRgn(hdc, clip);
                if !e.canvas.is_null() && cw > 0 && ch > 0 {
                    SetStretchBltMode(hdc, HALFTONE);
                    SetBrushOrgEx(hdc, 0, 0, null_mut());
                    StretchBlt(hdc, ox, oy, (cw as f32 * s) as i32, (ch as f32 * s) as i32, e.cdc, 0, 0, cw, ch, SRCCOPY);
                }
                // crop drag: dashed box in screen space
                if let Some(d) = &e.drag {
                    if e.tool == Tool::Crop {
                        let a = (d.start.0 - e.crop.left, d.start.1 - e.crop.top);
                        let b = (d.end.0 - e.crop.left, d.end.1 - e.crop.top);
                        let rr = RECT {
                            left: ox + (a.0.min(b.0) as f32 * s) as i32,
                            top: oy + (a.1.min(b.1) as f32 * s) as i32,
                            right: ox + (a.0.max(b.0) as f32 * s) as i32,
                            bottom: oy + (a.1.max(b.1) as f32 * s) as i32,
                        };
                        ui::dashed(hdc, rr, p.accent);
                    }
                }
                if let Some(i) = e.selected {
                    if let Some(sh) = e.shapes.get(i) {
                        let r = sh.bounds();
                        let rr = RECT {
                            left: ox + ((r.left - e.crop.left) as f32 * s) as i32 - 3,
                            top: oy + ((r.top - e.crop.top) as f32 * s) as i32 - 3,
                            right: ox + ((r.right - e.crop.left) as f32 * s) as i32 + 3,
                            bottom: oy + ((r.bottom - e.crop.top) as f32 * s) as i32 + 3,
                        };
                        ui::dashed(hdc, rr, p.accent);
                    }
                }
                // frame around the canvas
                let ir = RECT {
                    left: ox - 1,
                    top: oy - 1,
                    right: ox + (cw as f32 * s) as i32 + 1,
                    bottom: oy + (ch as f32 * s) as i32 + 1,
                };
                ui::outline(hdc, ir, p.line, 1);
                SelectClipRgn(hdc, null_mut());
                DeleteObject(clip as HGDIOBJ);
            }
        });
    }
    // toolbar buttons
    let list = buttons();
    let (hover, tool, undo_len, redo_len, has_sel) = ED.with_borrow(|s| {
        s.as_ref().map(|e| (e.hover, e.tool, e.undo.len(), e.redo.len(), e.selected.is_some())).unwrap_or((None, Tool::Arrow, 0, 0, false))
    });
    let glyph_font = ui::make_font(ui::sc(hwnd, 18), 400, winrust_common::style::icon_face());
    for (i, (g, _, act)) in list.iter().enumerate() {
        let r = btn_rect(hwnd, i);
        if r.right > c.right {
            break;
        }
        let is_active = matches!(act, Act::Tool(t) if *t == tool);
        let enabled = match act {
            Act::Undo => undo_len > 0,
            Act::Redo => redo_len > 0,
            Act::Clear => true,
            _ => true,
        };
        let bg = if matches!(act, Act::Save) { mix(p.accent, BG, 0.75) } else { ui::button_bg(hover == Some(i), is_active) };
        ui::rounded(hdc, r, bg, ui::sc(hwnd, 8));
        if is_active {
            ui::fill(hdc, RECT { left: r.left + 8, top: r.bottom - 3, right: r.right - 8, bottom: r.bottom - 1 }, p.accent);
        }
        let col = if !enabled {
            p.line
        } else if matches!(act, Act::Save) {
            p.accent
        } else if is_active {
            p.accent
        } else {
            p.text
        };
        ui::text(hdc, g, r, glyph_font, col, DT_CENTER as u32);
        if matches!(act, Act::Close) && hover == Some(i) {
            ui::rounded(hdc, r, mix(rgb(255, 69, 58), BG, 0.65), ui::sc(hwnd, 8));
            ui::text(hdc, g, r, glyph_font, rgb(255, 255, 255), DT_CENTER as u32);
        }
    }
    DeleteObject(glyph_font as HGDIOBJ);

    // status line
    let small = ui::make_font(ui::sc(hwnd, 12), 400, winrust_common::style::ui_face());
    let (dims, zoom_txt, hint, msg) = ED.with_borrow(|s| {
        s.as_ref()
            .map(|e| {
                let (cw, ch) = crop_size(e);
                let (sc, _, _) = view(e, hwnd);
                let hovered = e.hover.and_then(|i| list.get(i)).map(|b| b.1.clone());
                let hint = hovered.unwrap_or_else(|| if has_sel { t!("Delete — удалить выбранное", "Delete removes the selected shape", ja: "Delete で選択した図形を削除", pt: "Delete remove a forma selecionada", zh: "Delete 删除选中的图形").to_string() } else { e.tool.hint().to_string() });
                let msg = e.status.as_ref().filter(|(_, t)| *t + 4000 > GetTickCount64()).map(|(s, _)| s.clone());
                (format!("{cw} × {ch}"), format!("{:.0}%", sc * 100.0), hint, msg)
            })
            .unwrap_or_default()
    });
    let left = RECT { left: ui::sc(hwnd, 12), top: status.top, right: c.right / 2, bottom: status.bottom };
    ui::text(hdc, &format!("{dims}   ·   {zoom_txt}"), left, small, p.dim, 0);
    let right = RECT { left: c.right / 2, top: status.top, right: c.right - ui::sc(hwnd, 12), bottom: status.bottom };
    ui::text(hdc, msg.as_deref().unwrap_or(&hint), right, small, if msg.is_some() { p.accent } else { p.dim }, DT_RIGHT as u32);
    DeleteObject(small as HGDIOBJ);
    if !back_dc.is_null() {
        BitBlt(screen, p.left, p.top, p.right - p.left, p.bottom - p.top, back_dc, p.left, p.top, SRCCOPY);
    }
    EndPaint(hwnd, &ps);
}

unsafe fn commit_text(e: &mut Ed) {
    if let Some(t) = e.text.take() {
        if !t.buf.trim().is_empty() {
            push_undo(e);
            e.shapes.push(Shape::Text { pos: t.pos, text: t.buf, color: pal().accent, size: 24 });
            e.dirty = true;
        }
    }
}

unsafe fn apply_crop(e: &mut Ed, a: (i32, i32), b: (i32, i32)) {
    let r = RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0), bottom: a.1.max(b.1) };
    let n = RECT {
        left: r.left.max(e.crop.left),
        top: r.top.max(e.crop.top),
        right: r.right.min(e.crop.right),
        bottom: r.bottom.min(e.crop.bottom),
    };
    if n.right - n.left >= 8 && n.bottom - n.top >= 8 {
        push_undo(e);
        e.crop = n;
        e.selected = None;
        e.fit = true;
        e.dirty = true;
    }
}

fn set_status(e: &mut Ed, s: String) {
    e.status = Some((s, unsafe { GetTickCount64() }));
}

unsafe fn save_it(e: &mut Ed, copy_only: bool) {
    ensure_canvas(e);
    let (cw, ch) = crop_size(e);
    if e.bits.is_null() {
        set_status(e, t!("Нет холста для сохранения", "No canvas to save", ja: "保存するキャンバスがありません", pt: "Não há tela para salvar", zh: "没有可保存的画布").to_string());
        return;
    }
    let px = std::slice::from_raw_parts(e.bits, (cw * ch * 4) as usize).to_vec();
    let img = Image::from_bgra(cw, ch, px);
    if copy_only {
        if save::copy_to_clipboard(&img) {
            set_status(e, t!("Скопировано в буфер обмена", "Copied to the clipboard", ja: "クリップボードにコピーしました", pt: "Copiado para a área de transferência", zh: "已复制到剪贴板").to_string());
        } else {
            set_status(e, t!("Не удалось скопировать", "Could not copy to the clipboard", ja: "コピーできませんでした", pt: "Não foi possível copiar", zh: "无法复制").to_string());
        }
        return;
    }
    let mut path = e.file.clone().unwrap_or_else(save::auto_path);
    if path.extension().is_none() {
        path.set_extension("png");
    }
    let ok = save::save(&img, &path);
    let clip = save::copy_to_clipboard(&img);
    if ok {
        e.last_saved = Some(path.clone());
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        set_status(e, if clip { tf!("Сохранено и скопировано: {name}", "Saved and copied: {name}", ja: "保存してコピーしました: {name}", pt: "Salvo e copiado: {name}", zh: "已保存并复制：{name}") } else { tf!("Сохранено: {name}", "Saved: {name}", ja: "保存しました: {name}", pt: "Salvo: {name}", zh: "已保存：{name}") });
    } else {
        set_status(e, t!("Не удалось сохранить файл", "Could not save the file", ja: "ファイルを保存できませんでした", pt: "Não foi possível salvar", zh: "无法保存文件").to_string());
    }
}

unsafe fn act(hwnd: HWND, a: Act) {
    ED.with_borrow_mut(|slot| {
        let Some(e) = slot else { return };
        match a {
            Act::Tool(t) => {
                commit_text(e);
                e.tool = t;
                e.selected = None;
                e.drag = None;
                e.dirty = true;
            }
            Act::Undo => do_undo(e),
            Act::Redo => do_redo(e),
            Act::Clear => {
                if !e.shapes.is_empty() {
                    push_undo(e);
                    e.shapes.clear();
                    e.selected = None;
                    e.dirty = true;
                }
            }
            Act::Save => save_it(e, false),
            Act::Copy => save_it(e, true),
            Act::Close => {
                DestroyWindow(hwnd);
            }
        }
    });
    InvalidateRect(hwnd, null(), 0);
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp & 0xffff) as i16 as i32;
    let y = ((lp >> 16) & 0xffff) as i16 as i32;
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(hwnd);
            0
        }
        WM_SIZE => {
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_DPICHANGED => {
            let r = &*(lp as *const RECT);
            SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEMOVE => {
            let list_len = buttons().len();
            let hover = (0..list_len).find(|i| in_rect(btn_rect(hwnd, *i), x, y));
            let old_hover = ED.with_borrow(|s| s.as_ref().and_then(|e| e.hover));
            ED.with_borrow_mut(|slot| {
                let Some(e) = slot else { return };
                e.hover = hover;
                let p = to_image(e, hwnd, x, y);
                let tool = e.tool;
                if let Some(d) = &mut e.drag {
                    let snapped = if GetKeyState(VK_SHIFT as i32) < 0 { annot::snap(tool, d.start, p) } else { p };
                    d.end = snapped;
                    if tool == Tool::Highlighter {
                        if d.pts.last().map(|q| (q.0 - p.0).abs() + (q.1 - p.1).abs() > 3).unwrap_or(true) {
                            d.pts.push(p);
                        }
                    }
                    e.dirty = true;
                }
                let last = e.moving;
                if let Some(last) = last {
                    let (dx, dy) = (p.0 - last.0, p.1 - last.1);
                    if dx != 0 || dy != 0 {
                        if !e.move_pushed {
                            push_undo(e);
                            e.move_pushed = true;
                        }
                        if let Some(i) = e.selected {
                            if let Some(sh) = e.shapes.get_mut(i) {
                                sh.translate(dx, dy);
                            }
                        }
                        e.moving = Some(p);
                        e.dirty = true;
                    }
                }
            });
            let busy = ED.with_borrow(|s| s.as_ref().map(|e| e.drag.is_some() || e.moving.is_some()).unwrap_or(false));
            if busy {
                let mut cr: RECT = zeroed();
                GetClientRect(hwnd, &mut cr);
                InvalidateRect(
                    hwnd,
                    &RECT { left: 0, top: toolbar_h(hwnd), right: cr.right, bottom: (cr.bottom - status_h(hwnd)).max(toolbar_h(hwnd) + 1) },
                    0,
                );
            } else if hover != old_hover {
                if let Some(i) = old_hover {
                    let r = btn_rect(hwnd, i);
                    InvalidateRect(hwnd, &r, 0);
                }
                if let Some(i) = hover {
                    let r = btn_rect(hwnd, i);
                    InvalidateRect(hwnd, &r, 0);
                }
                let mut cr: RECT = zeroed();
                GetClientRect(hwnd, &mut cr);
                // the tooltip/hint in the status bar changes with the hovered button
                InvalidateRect(hwnd, &RECT { left: cr.right / 2, top: (cr.bottom - status_h(hwnd)), right: cr.right, bottom: cr.bottom }, 0);
            }
            0
        }
        WM_LBUTTONDOWN => {
            let list = buttons();
            if let Some(i) = (0..list.len()).find(|i| in_rect(btn_rect(hwnd, *i), x, y)) {
                let a = list[i].2;
                act(hwnd, a);
                return 0;
            }
            ED.with_borrow_mut(|slot| {
                let Some(e) = slot else { return };
                let p = to_image(e, hwnd, x, y);
                commit_text(e);
                match e.tool {
                    Tool::Text => {
                        e.text = Some(TextIn { pos: p, buf: String::new() });
                        e.dirty = true;
                    }
                    Tool::Number => {
                        push_undo(e);
                        e.shapes.push(Shape::Number { pos: p, n: e.next_num, color: pal().accent });
                        e.next_num += 1;
                        e.dirty = true;
                    }
                    Tool::Select => {
                        let hit = e.shapes.iter().rposition(|s| s.hit(p.0, p.1));
                        e.selected = hit;
                        if hit.is_some() {
                            e.moving = Some(p);
                            e.move_pushed = false;
                        }
                    }
                    _ => {
                        e.drag = Some(Drag { start: p, end: p, pts: vec![p] });
                        e.dirty = true;
                    }
                }
            });
            SetCapture(hwnd);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_LBUTTONUP => {
            ReleaseCapture();
            ED.with_borrow_mut(|slot| {
                let Some(e) = slot else { return };
                if let Some(_) = e.moving.take() {
                    e.dirty = true;
                    return;
                }
                let Some(d) = e.drag.take() else { return };
                if e.tool == Tool::Crop {
                    apply_crop(e, d.start, d.end);
                } else {
                    let shape = if e.tool == Tool::Highlighter {
                        let mut pts = d.pts.clone();
                        if pts.len() < 2 {
                            pts = vec![d.start, d.end];
                        }
                        if (d.start.0 - d.end.0).abs() + (d.start.1 - d.end.1).abs() > 3 {
                            Some(Shape::Highlighter { pts, color: winrust_common::style::WARN, width: 14 })
                        } else {
                            None
                        }
                    } else {
                        annot::make(e.tool, d.start, d.end, e.next_num)
                    };
                    if let Some(s) = shape {
                        push_undo(e);
                        e.shapes.push(s);
                        e.dirty = true;
                    }
                }
            });
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_RBUTTONUP => {
            ED.with_borrow_mut(|slot| {
                if let Some(e) = slot {
                    e.drag = None;
                    e.moving = None;
                    e.text = None;
                    e.selected = None;
                    e.dirty = true;
                }
            });
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xffff) as i16 as i32;
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            if ctrl {
                ED.with_borrow_mut(|slot| {
                    if let Some(e) = slot {
                        let s = if e.fit { let (s, _, _) = view(e, hwnd); s } else { e.zoom };
                        e.zoom = (s * if delta > 0 { 1.15 } else { 1.0 / 1.15 }).clamp(0.1, 8.0);
                        e.fit = false;
                    }
                });
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_CHAR => {
            let ch = wp as u32;
            if ch == 13 {
                ED.with_borrow_mut(|slot| {
                    if let Some(e) = slot {
                        commit_text(e);
                    }
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            if ch == 27 {
                ED.with_borrow_mut(|slot| {
                    if let Some(e) = slot {
                        e.text = None;
                        e.dirty = true;
                    }
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            let active = ED.with_borrow(|s| s.as_ref().map(|e| e.text.is_some()).unwrap_or(false));
            if active {
                ED.with_borrow_mut(|slot| {
                    if let Some(e) = slot {
                        if let Some(t) = &mut e.text {
                            if ch == 8 {
                                t.buf.pop();
                            } else if ch >= 32 {
                                t.buf.push(char::from_u32(ch).unwrap_or(' '));
                            }
                            e.dirty = true;
                        }
                    }
                });
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_KEYDOWN => {
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            let shift = GetKeyState(VK_SHIFT as i32) < 0;
            let text_active = ED.with_borrow(|s| s.as_ref().map(|e| e.text.is_some()).unwrap_or(false));
            if text_active && wp != VK_ESCAPE as usize {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            if wp == VK_ESCAPE as usize {
                if text_active {
                    ED.with_borrow_mut(|slot| {
                        if let Some(e) = slot {
                            e.text = None;
                            e.dirty = true;
                        }
                    });
                    InvalidateRect(hwnd, null(), 0);
                } else {
                    DestroyWindow(hwnd);
                }
                return 0;
            }
            if ctrl && (wp == b'Z' as usize) {
                ED.with_borrow_mut(|slot| {
                    if let Some(e) = slot {
                        if shift { do_redo(e) } else { do_undo(e) }
                    }
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            if ctrl && (wp == b'Y' as usize) {
                ED.with_borrow_mut(|slot| {
                    if let Some(e) = slot { do_redo(e) }
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            if ctrl && (wp == b'S' as usize) {
                act(hwnd, Act::Save);
                return 0;
            }
            if ctrl && (wp == b'C' as usize) {
                act(hwnd, Act::Copy);
                return 0;
            }
            if wp == VK_DELETE as usize {
                ED.with_borrow_mut(|slot| {
                    if let Some(e) = slot {
                        if let Some(i) = e.selected.take() {
                            push_undo(e);
                            e.shapes.remove(i);
                            e.dirty = true;
                        }
                    }
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            let tool = match wp as u16 {
                c if !ctrl && c == b'V' as u16 => Some(Tool::Select),
                c if !ctrl && c == b'C' as u16 => Some(Tool::Crop),
                c if !ctrl && c == b'A' as u16 => Some(Tool::Arrow),
                c if !ctrl && c == b'L' as u16 => Some(Tool::Line),
                c if !ctrl && c == b'R' as u16 => Some(Tool::Rect),
                c if !ctrl && c == b'E' as u16 => Some(Tool::Ellipse),
                c if !ctrl && c == b'T' as u16 => Some(Tool::Text),
                c if !ctrl && c == b'H' as u16 => Some(Tool::Highlighter),
                c if !ctrl && c == b'P' as u16 => Some(Tool::Pixelate),
                c if !ctrl && c == b'B' as u16 => Some(Tool::Blur),
                c if !ctrl && c == b'N' as u16 => Some(Tool::Number),
                _ => None,
            };
            if let Some(t) = tool {
                act(hwnd, Act::Tool(t));
                return 0;
            }
            if wp == b'0' as usize || wp == VK_ADD as usize || wp == VK_OEM_PLUS as usize {
                ED.with_borrow_mut(|slot| {
                    if let Some(e) = slot {
                        e.fit = true;
                    }
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            0
        }
        WM_SETCURSOR => {
            if (lp >> 16) as u16 == HTCLIENT as u16 {
                SetCursor(LoadCursorW(null_mut(), IDC_CROSS));
                return 1;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}


