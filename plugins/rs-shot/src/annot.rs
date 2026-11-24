//! Annotation model and renderer. Shapes live in original-image coordinates; the editor passes the crop origin so a
//! cropped image draws the same shapes in its own space. The renderer writes straight into the editor's 32-bit DIB, so
//! highlighter alpha, pixelation and blur are pixel operations while lines, boxes, ellipses and text use GDI.
use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::{
        CreateFontW, CreatePen, CreateSolidBrush, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, Ellipse, GetStockObject,        HDC, HGDIOBJ, LineTo, MoveToEx, NULL_BRUSH, NULL_PEN, PS_SOLID, Polygon, Rectangle, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    },
};

use winrust_common::style::{WARN, pal};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    Crop,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Text,
    Highlighter,
    Pixelate,
    Blur,
    Number,
}

impl Tool {
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => t!("Щёлкните фигуру, чтобы выбрать её; Delete удаляет", "Click a shape to select it; Delete removes it", ja: "図形をクリックして選択、Delete で削除", pt: "Clique em uma forma para selecioná-la; Delete remove", zh: "单击图形以选择，Delete 删除"),
            Tool::Crop => t!("Протяните рамку и отпустите, чтобы обрезать", "Drag a box and release to crop", ja: "ドラッグして切り抜き", pt: "Arraste uma caixa e solte para recortar", zh: "拖动框选并释放以裁剪"),
            Tool::Arrow | Tool::Line => t!("Перетащите; Shift — под углом 45°", "Drag; Shift snaps to 45°", ja: "ドラッグ。Shift で 45° 単位", pt: "Arraste; Shift ajusta em 45°", zh: "拖动；Shift 以 45° 吸附"),
            Tool::Rect | Tool::Ellipse => t!("Перетащите; Shift — квадрат/круг", "Drag; Shift makes a square/circle", ja: "ドラッグ。Shift で正方形/円", pt: "Arraste; Shift faz quadrado/círculo", zh: "拖动；Shift 生成正方形/圆"),
            Tool::Text => t!("Щёлкните и печатайте; Enter — готово", "Click and type; Enter commits", ja: "クリックして入力、Enter で確定", pt: "Clique e digite; Enter confirma", zh: "单击并输入；Enter 确认"),
            Tool::Highlighter => t!("Проведите мышью; Shift — прямая", "Drag to highlight; Shift draws straight", ja: "ドラッグでハイライト。Shift で直線", pt: "Arraste para destacar; Shift desenha reto", zh: "拖动以高亮；Shift 画直线"),
            Tool::Pixelate | Tool::Blur => t!("Протяните прямоугольник", "Drag a rectangle", ja: "長方形をドラッグ", pt: "Arraste um retângulo", zh: "拖动一个矩形"),
            Tool::Number => t!("Щёлкните, чтобы поставить следующий номер", "Click to stamp the next number", ja: "クリックで次の番号を追加", pt: "Clique para carimbar o próximo número", zh: "单击以盖上下一个编号"),
        }
    }
}

#[derive(Clone)]
pub enum Shape {
    Line { a: (i32, i32), b: (i32, i32), arrow: bool, color: u32, width: i32 },
    Rect { a: (i32, i32), b: (i32, i32), color: u32, width: i32 },
    Ellipse { a: (i32, i32), b: (i32, i32), color: u32, width: i32 },
    Highlighter { pts: Vec<(i32, i32)>, color: u32, width: i32 },
    Text { pos: (i32, i32), text: String, color: u32, size: i32 },
    Pixelate { a: (i32, i32), b: (i32, i32), block: i32 },
    Blur { a: (i32, i32), b: (i32, i32), radius: i32 },
    Number { pos: (i32, i32), n: u32, color: u32 },
}

fn norm(a: (i32, i32), b: (i32, i32)) -> RECT {
    RECT { left: a.0.min(b.0), top: a.1.min(b.1), right: a.0.max(b.0), bottom: a.1.max(b.1) }
}

impl Shape {
    pub fn bounds(&self) -> RECT {
        match self {
            Shape::Line { a, b, .. } => norm(*a, *b),
            Shape::Rect { a, b, .. } | Shape::Ellipse { a, b, .. } | Shape::Pixelate { a, b, .. } | Shape::Blur { a, b, .. } => norm(*a, *b),
            Shape::Highlighter { pts, width, .. } => {
                let mut r = RECT { left: i32::MAX, top: i32::MAX, right: i32::MIN, bottom: i32::MIN };
                for p in pts {
                    r.left = r.left.min(p.0);
                    r.top = r.top.min(p.1);
                    r.right = r.right.max(p.0);
                    r.bottom = r.bottom.max(p.1);
                }
                if r.left > r.right {
                    return RECT { left: 0, top: 0, right: 0, bottom: 0 };
                }
                r.left -= width;
                r.top -= width;
                r.right += width;
                r.bottom += width;
                r
            }
            Shape::Text { pos, text, size, .. } => RECT {
                left: pos.0,
                top: pos.1,
                right: pos.0 + (text.chars().count() as i32 * size / 2).max(*size),
                bottom: pos.1 + size * 2,
            },
            Shape::Number { pos, .. } => RECT { left: pos.0 - 20, top: pos.1 - 20, right: pos.0 + 20, bottom: pos.1 + 20 },
        }
    }

    pub fn translate(&mut self, dx: i32, dy: i32) {
        let mv = |a: &mut (i32, i32)| {
            a.0 += dx;
            a.1 += dy;
        };
        match self {
            Shape::Line { a, b, .. } => {
                mv(a);
                mv(b);
            }
            Shape::Rect { a, b, .. } | Shape::Ellipse { a, b, .. } | Shape::Pixelate { a, b, .. } | Shape::Blur { a, b, .. } => {
                mv(a);
                mv(b);
            }
            Shape::Highlighter { pts, .. } => pts.iter_mut().for_each(mv),
            Shape::Text { pos, .. } | Shape::Number { pos, .. } => mv(pos),
        }
    }

    pub fn hit(&self, x: i32, y: i32) -> bool {
        let r = self.bounds();
        x >= r.left - 4 && x <= r.right + 4 && y >= r.top - 4 && y <= r.bottom + 4
    }
}

pub fn make(tool: Tool, a: (i32, i32), b: (i32, i32), next: u32) -> Option<Shape> {
    let accent = pal().accent;
    let w = ((a.0 - b.0).abs()).max((a.1 - b.1).abs());
    if w < 3 && !matches!(tool, Tool::Number) {
        return None;
    }
    Some(match tool {
        Tool::Arrow => Shape::Line { a, b, arrow: true, color: accent, width: 3 },
        Tool::Line => Shape::Line { a, b, arrow: false, color: accent, width: 3 },
        Tool::Rect => Shape::Rect { a, b, color: accent, width: 3 },
        Tool::Ellipse => Shape::Ellipse { a, b, color: accent, width: 3 },
        Tool::Highlighter => Shape::Highlighter { pts: vec![a, b], color: WARN, width: 14 },
        Tool::Text => Shape::Text { pos: a, text: String::new(), color: accent, size: 24 },
        Tool::Pixelate => Shape::Pixelate { a, b, block: 14 },
        Tool::Blur => Shape::Blur { a, b, radius: 10 },
        Tool::Number => Shape::Number { pos: a, n: next, color: accent },
        Tool::Select | Tool::Crop => return None,
    })
}

/// Shift snapping for lines/arrows (45°) and boxes (square).
pub fn snap(tool: Tool, a: (i32, i32), b: (i32, i32)) -> (i32, i32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    match tool {
        Tool::Line | Tool::Arrow => {
            let adx = dx.abs();
            let ady = dy.abs();
            if adx > ady * 2 {
                (b.0, a.1)
            } else if ady > adx * 2 {
                (a.0, b.1)
            } else {
                let d = adx.max(ady);
                (a.0 + d * dx.signum(), a.1 + d * dy.signum())
            }
        }
        Tool::Rect | Tool::Ellipse | Tool::Pixelate | Tool::Blur => {
            let d = dx.abs().max(dy.abs());
            (a.0 + d * dx.signum(), a.1 + d * dy.signum())
        }
        _ => b,
    }
}

pub unsafe fn render(dc: HDC, bits: *mut u8, w: i32, h: i32, ox: i32, oy: i32, shapes: &[Shape], preview: Option<&Shape>) {
    for s in shapes {
        draw(dc, bits, w, h, ox, oy, s);
    }
    if let Some(s) = preview {
        draw(dc, bits, w, h, ox, oy, s);
    }
}

unsafe fn draw(dc: HDC, bits: *mut u8, w: i32, h: i32, ox: i32, oy: i32, s: &Shape) {
    match s {
        Shape::Line { a, b, arrow, color, width } => {
            let (a, b) = ((a.0 - ox, a.1 - oy), (b.0 - ox, b.1 - oy));
            let pen = CreatePen(PS_SOLID, *width, *color);
            let old = SelectObject(dc, pen as HGDIOBJ);
            MoveToEx(dc, a.0, a.1, null_mut());
            LineTo(dc, b.0, b.1);
            SelectObject(dc, old);
            DeleteObject(pen as HGDIOBJ);
            if *arrow {
                let (dx, dy) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
                let len = (dx * dx + dy * dy).sqrt().max(1.0);
                let (ux, uy) = (dx / len, dy / len);
                let size = (14.0 + *width as f32 * 2.0).min(len * 0.6);
                let (px, py) = (-uy, ux);
                let tip = (b.0 as f32, b.1 as f32);
                let base = (tip.0 - ux * size, tip.1 - uy * size);
                let pts = [
                    POINT { x: tip.0 as i32, y: tip.1 as i32 },
                    POINT { x: (base.0 + px * size * 0.55) as i32, y: (base.1 + py * size * 0.55) as i32 },
                    POINT { x: (base.0 - px * size * 0.55) as i32, y: (base.1 - py * size * 0.55) as i32 },
                ];
                let brush = CreateSolidBrush(*color);
                let old_b = SelectObject(dc, brush as HGDIOBJ);
                let old_p = SelectObject(dc, GetStockObject(NULL_PEN) as HGDIOBJ);
                Polygon(dc, pts.as_ptr(), 3);
                SelectObject(dc, old_b);
                SelectObject(dc, old_p);
                DeleteObject(brush as HGDIOBJ);
            }
        }
        Shape::Rect { a, b, color, width } => {
            let (a, b) = ((a.0 - ox, a.1 - oy), (b.0 - ox, b.1 - oy));
            let pen = CreatePen(PS_SOLID, *width, *color);
            let old = SelectObject(dc, pen as HGDIOBJ);
            let old_b = SelectObject(dc, GetStockObject(NULL_BRUSH) as HGDIOBJ);
            Rectangle(dc, a.0, a.1, b.0, b.1);
            SelectObject(dc, old);
            SelectObject(dc, old_b);
            DeleteObject(pen as HGDIOBJ);
        }
        Shape::Ellipse { a, b, color, width } => {
            let (a, b) = ((a.0 - ox, a.1 - oy), (b.0 - ox, b.1 - oy));
            let pen = CreatePen(PS_SOLID, *width, *color);
            let old = SelectObject(dc, pen as HGDIOBJ);
            let old_b = SelectObject(dc, GetStockObject(NULL_BRUSH) as HGDIOBJ);
            Ellipse(dc, a.0, a.1, b.0, b.1);
            SelectObject(dc, old);
            SelectObject(dc, old_b);
            DeleteObject(pen as HGDIOBJ);
        }
        Shape::Highlighter { pts, color, width } => {
            let p: Vec<(i32, i32)> = pts.iter().map(|p| (p.0 - ox, p.1 - oy)).collect();
            for pair in p.windows(2) {
                thick_line(bits, w, h, pair[0], pair[1], *width, *color, 115);
            }
            if p.len() == 1 {
                thick_line(bits, w, h, p[0], p[0], *width, *color, 115);
            }
        }
        Shape::Text { pos, text, color, size } => {
            if text.is_empty() {
                return;
            }
            let font = CreateFontW(-*size, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, crate::ui::wide("Segoe UI").as_ptr());
            let old = SelectObject(dc, font as HGDIOBJ);
            SetTextColor(dc, *color);
            SetBkMode(dc, TRANSPARENT as i32);
            let mut r = RECT { left: pos.0 - ox, top: pos.1 - oy, right: w + 4000, bottom: pos.1 - oy + *size * 3 };
            let u: Vec<u16> = text.encode_utf16().collect();
            DrawTextW(dc, u.as_ptr(), u.len() as i32, &mut r, DT_SINGLELINE as u32 | DT_NOPREFIX as u32);
            SelectObject(dc, old);
            DeleteObject(font as HGDIOBJ);
        }
        Shape::Pixelate { a, b, block } => {
            let r = norm(*a, *b);
            pixelate(bits, w, h, r.left - ox, r.top - oy, r.right - ox, r.bottom - oy, (*block).max(4));
        }
        Shape::Blur { a, b, radius } => {
            let r = norm(*a, *b);
            blur(bits, w, h, r.left - ox, r.top - oy, r.right - ox, r.bottom - oy, (*radius).clamp(2, 40));
        }
        Shape::Number { pos, n, color } => {
            let (x, y) = (pos.0 - ox, pos.1 - oy);
            let rad = 20;
            let brush = CreateSolidBrush(*color);
            let old_b = SelectObject(dc, brush as HGDIOBJ);
            let old_p = SelectObject(dc, GetStockObject(NULL_PEN) as HGDIOBJ);
            Ellipse(dc, x - rad, y - rad, x + rad, y + rad);
            SelectObject(dc, old_b);
            SelectObject(dc, old_p);
            DeleteObject(brush as HGDIOBJ);
            let font = CreateFontW(-24, 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, crate::ui::wide("Segoe UI").as_ptr());
            let old = SelectObject(dc, font as HGDIOBJ);
            SetTextColor(dc, winrust_common::style::rgb(255, 255, 255));
            SetBkMode(dc, TRANSPARENT as i32);
            let s = n.to_string();
            let u: Vec<u16> = s.encode_utf16().collect();
            let mut r = RECT { left: x - rad, top: y - rad, right: x + rad, bottom: y + rad };
            DrawTextW(dc, u.as_ptr(), u.len() as i32, &mut r, DT_CENTER as u32 | DT_VCENTER as u32 | DT_SINGLELINE as u32);
            SelectObject(dc, old);
            DeleteObject(font as HGDIOBJ);
        }
    }
}

fn px(_bits: *mut u8, w: i32, h: i32, x: i32, y: i32) -> Option<usize> {
    (x >= 0 && y >= 0 && x < w && y < h).then_some(((y * w + x) * 4) as usize)
}

/// A disc stamped along the segment, blending in place: the highlighter's translucent look on any background.
fn thick_line(bits: *mut u8, w: i32, h: i32, a: (i32, i32), b: (i32, i32), width: i32, color: u32, alpha: u32) {
    let rad = (width / 2).max(1);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let steps = dx.abs().max(dy.abs()).max(1);
    let src = ((color & 255) as u32, ((color >> 8) & 255) as u32, ((color >> 16) & 255) as u32);
    for i in 0..=steps {
        let x = a.0 + dx * i / steps;
        let y = a.1 + dy * i / steps;
        for oy in -rad..=rad {
            for ox in -rad..=rad {
                if ox * ox + oy * oy > rad * rad {
                    continue;
                }
                if let Some(p) = px(bits, w, h, x + ox, y + oy) {
                    let d = unsafe { std::slice::from_raw_parts_mut(bits.add(p), 4) };
                    for c in 0..3 {
                        let old = d[c] as u32;
                        let new = match c {
                            0 => src.0,
                            1 => src.1,
                            _ => src.2,
                        };
                        d[c] = ((new * alpha + old * (255 - alpha)) / 255) as u8;
                    }
                }
            }
        }
    }
}

fn pixelate(bits: *mut u8, w: i32, h: i32, l: i32, t: i32, r: i32, b: i32, block: i32) {
    let (l, t, r, b) = (l.max(0), t.max(0), r.min(w), b.min(h));
    if r - l < 2 || b - t < 2 {
        return;
    }
    let mut y = t;
    while y < b {
        let mut x = l;
        while x < r {
            let (bx, by) = ((x + block).min(r), (y + block).min(b));
            let mut sum = [0u32; 3];
            let mut n = 0u32;
            let mut yy = y;
            while yy < by {
                let mut xx = x;
                while xx < bx {
                    if let Some(p) = px(bits, w, h, xx, yy) {
                        let d = unsafe { std::slice::from_raw_parts(bits.add(p), 4) };
                        for c in 0..3 {
                            sum[c] += d[c] as u32;
                        }
                        n += 1;
                    }
                    xx += 1;
                }
                yy += 1;
            }
            if n == 0 {
                break;
            }
            let avg = [sum[0] / n, sum[1] / n, sum[2] / n];
            let mut yy = y;
            while yy < by {
                let mut xx = x;
                while xx < bx {
                    if let Some(p) = px(bits, w, h, xx, yy) {
                        let d = unsafe { std::slice::from_raw_parts_mut(bits.add(p), 4) };
                        d[0] = avg[0] as u8;
                        d[1] = avg[1] as u8;
                        d[2] = avg[2] as u8;
                    }
                    xx += 1;
                }
                yy += 1;
            }
            x += block;
        }
        y += block;
    }
}

fn blur(bits: *mut u8, w: i32, h: i32, l: i32, t: i32, r: i32, b: i32, radius: i32) {
    let (l, t, r, b) = (l.max(0), t.max(0), r.min(w), b.min(h));
    let (rw, rh) = (r - l, b - t);
    if rw < 2 || rh < 2 {
        return;
    }
    let mut copy = vec![0u8; (rw * rh * 4) as usize];
    for y in 0..rh {
        let src = (((t + y) * w + l) * 4) as usize;
        let dst = (y * rw * 4) as usize;
        unsafe { std::ptr::copy_nonoverlapping(bits.add(src), copy.as_mut_ptr().add(dst), (rw * 4) as usize) };
    }
    let rad = radius as usize;
    for y in 0..rh {
        for x in 0..rw {
            let (mut sum, mut n) = ([0u32; 3], 0u32);
            let y0 = (y as usize).saturating_sub(rad);
            let y1 = (y as usize + rad).min(rh as usize - 1);
            let x0 = (x as usize).saturating_sub(rad);
            let x1 = (x as usize + rad).min(rw as usize - 1);
            for sy in y0..=y1 {
                for sx in x0..=x1 {
                    let i = (sy * rw as usize + sx) * 4;
                    for c in 0..3 {
                        sum[c] += copy[i + c] as u32;
                    }
                    n += 1;
                }
            }
            let i = ((y * rw + x) * 4) as usize;
            let d = unsafe { std::slice::from_raw_parts_mut(bits.add((((t + y) * w + l) * 4) as usize), 4) };
            for c in 0..3 {
                d[c] = (sum[c] / n.max(1)) as u8;
            }
            let _ = i;
        }
    }
}

