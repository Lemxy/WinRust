//! WinRust Disk's other page: the cleaner (winrust_common::clean) over every drive, as a list to tick.
//! Green and yellow come ticked, red waits for the user. Click: tick. Click a heading: the whole group. Right click: show it
//! in Explorer. The files among the red ones go to a 30-day quarantine, and "Restore" brings them all back.
use super::*;
use winrust_common::clean::{self as engine, Done, How, Item, Risk};

pub const WM_STEP: u32 = WM_APP + 2; // lparam = Box<String>: what the engine is on
pub const WM_PLAN: u32 = WM_APP + 3; // lparam = Box<Vec<Item>>
pub const WM_CLEANED: u32 = WM_APP + 4; // lparam = Box<Done>

const ROW: f64 = 54.0;
const HEAD: f64 = 36.0;
const FOOT: f64 = 60.0;
const TEXT: u32 = rgb(235, 235, 235);
const LINE: u32 = rgb(44, 44, 48);
const HOVER: u32 = rgb(33, 33, 37);

#[derive(Clone, Copy, PartialEq)]
pub enum Hit {
    Group(Risk),
    Item(usize),
    Go,
    Restore,
}

#[derive(Default)]
pub struct Clean {
    started: bool,
    busy: bool,
    step: String,
    items: Vec<Item>,
    pick: Vec<bool>,
    scroll: i32,
    hits: Vec<(RECT, Hit)>,
    hot: Option<Hit>,
    note: String, // how the last clean or restore went
    quar: usize,  // items waiting in the quarantine, every drive
}

impl Clean {
    pub fn started(&self) -> bool {
        self.started
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
}

fn colour(r: Risk) -> u32 {
    match r {
        Risk::Safe => rgb(48, 209, 88),
        Risk::Rebuild => rgb(255, 204, 0),
        Risk::Ask => rgb(255, 69, 58),
    }
}

fn post<T>(h: isize, msg: u32, v: T) {
    unsafe { PostMessageW(h as HWND, msg, 0, Box::into_raw(Box::new(v)) as isize) };
}

fn quarantined() -> usize {
    engine::fixed_drives().into_iter().map(engine::quarantined).sum()
}

/// Reads every drive and works out what can go (a few seconds per drive, more when there are big copies to compare).
pub fn start(hwnd: HWND) {
    APP.with_borrow_mut(|a| {
        a.clean = Clean { started: true, busy: true, note: std::mem::take(&mut a.clean.note), quar: a.clean.quar, ..Default::default() };
    });
    unsafe { InvalidateRect(hwnd, null(), 0) };
    let h = hwnd as isize;
    std::thread::spawn(move || {
        let mut say = |s: String| post(h, WM_STEP, s);
        let ds = engine::scan_all(&mut say);
        post(h, WM_PLAN, engine::plan(&ds, &mut say));
    });
}

/// The engine's answers, from the worker threads.
pub unsafe fn on_msg(hwnd: HWND, msg: u32, lp: LPARAM) {
    unsafe {
        match msg {
            WM_STEP => {
                let s = *Box::from_raw(lp as *mut String);
                APP.with_borrow_mut(|a| a.clean.step = s);
            }
            WM_PLAN => {
                let items = *Box::from_raw(lp as *mut Vec<Item>);
                let quar = quarantined();
                APP.with_borrow_mut(|a| {
                    let c = &mut a.clean;
                    c.pick = items.iter().map(Item::default_pick).collect();
                    (c.items, c.busy, c.quar) = (items, false, quar);
                });
            }
            WM_CLEANED => {
                let d = *Box::from_raw(lp as *mut Done);
                let quar = quarantined();
                APP.with_borrow_mut(|a| {
                    let c = &mut a.clean;
                    // what was cleaned leaves the list; what partly stayed says so in the footer
                    let keep: Vec<bool> = c.pick.iter().map(|p| !p).collect();
                    let mut k = keep.iter();
                    c.items.retain(|_| *k.next().unwrap_or(&true));
                    c.pick = vec![false; c.items.len()];
                    c.note = tf!("Освобождено {}. В карантине {} (можно вернуть 30 дней).", "Freed {}. In quarantine {} (restorable for 30 days).", ja: "{} を解放しました。隔離中 {} (30 日間は復元できます)。", pt: "Liberado {}. Em quarentena {} (pode ser restaurado por 30 dias).", zh: "已释放 {}。隔离区中 {}（30 天内可恢复）。", es: "Liberado {}. En cuarentena {} (se puede restaurar durante 30 días).", human(d.freed), human(d.held));
                    if !d.partly.is_empty() {
                        let left: u64 = d.partly.iter().map(|p| p.1).sum();
                        c.note += &tf!(" {} осталось: занято программами.", " {} stayed: in use by programs.", ja: " {} はプログラムが使用中のため残りました。", pt: " {} ficaram: em uso por programas.", zh: " {} 未删除：正被程序使用。", es: " Quedaron {}: en uso por programas.", human(left));
                    }
                    (c.busy, c.quar, c.scroll) = (false, quar, 0);
                });
            }
            _ => {}
        }
        InvalidateRect(hwnd, null(), 0);
    }
}

/// Cleans what is ticked; asks first when something red would be gone for good (the recycle bin, Windows.old).
unsafe fn go(hwnd: HWND) {
    unsafe {
        let (picked, for_good) = APP.with_borrow(|a| {
            let c = &a.clean;
            let picked: Vec<Item> = c.items.iter().zip(&c.pick).filter(|(_, p)| **p).map(|(i, _)| i.clone()).collect();
            let for_good = picked.iter().any(|i| i.risk == Risk::Ask && i.how != How::Quarantine);
            (picked, for_good)
        });
        if picked.is_empty() {
            return;
        }
        let q = t!("Отмеченные красные пункты (например, корзина) удалятся навсегда, их не вернуть. Продолжить?", "The red items you ticked (the Recycle Bin, for one) are deleted for good and cannot be brought back. Go on?", ja: "選択した赤い項目 (ごみ箱など) は完全に削除され、元に戻せません。続行しますか?", pt: "Os itens vermelhos marcados (a Lixeira, por exemplo) serão excluídos para sempre e não poderão ser recuperados. Continuar?", zh: "所选的红色项目（例如回收站）将被永久删除，无法恢复。要继续吗？", es: "Los elementos rojos que marcaste (la Papelera, por ejemplo) se eliminan para siempre y no se pueden recuperar. ¿Continuar?");
        if for_good && MessageBoxW(hwnd, wide(q).as_ptr(), wide("WinRust Disk").as_ptr(), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) != IDYES {
            return;
        }
        APP.with_borrow_mut(|a| (a.clean.busy, a.clean.note) = (true, String::new()));
        InvalidateRect(hwnd, null(), 0);
        let h = hwnd as isize;
        std::thread::spawn(move || {
            let refs: Vec<&Item> = picked.iter().collect();
            let mut say = |s: String| post(h, WM_STEP, tf!("Очистка: {}", "Cleaning: {}", ja: "削除しています: {}", pt: "Limpando: {}", zh: "正在清理：{}", es: "Limpiando: {}", s));
            post(h, WM_CLEANED, engine::apply(&refs, &mut say));
        });
    }
}

/// Everything in the quarantine goes back where it was (a rename each: instant).
fn restore() {
    let (mut back, mut failed) = (0, 0);
    for d in engine::fixed_drives() {
        let (b, f) = engine::restore(d);
        (back, failed) = (back + b, failed + f.len());
    }
    let quar = quarantined();
    APP.with_borrow_mut(|a| {
        a.clean.quar = quar;
        a.clean.note = tf!("Восстановлено {0}, не удалось {1}.", "{0} restored, {1} could not be.", ja: "{0} 個を復元、{1} 個は復元できませんでした。", pt: "{0} restaurados, {1} não puderam ser.", zh: "已恢复 {0} 项，{1} 项无法恢复。", es: "{0} restaurados, {1} no se pudieron restaurar.", back, failed);
    });
}

fn hit_at(x: i32, y: i32) -> Option<Hit> {
    APP.with_borrow(|a| a.clean.hits.iter().find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|h| h.1))
}

/// Hover; true when the hot spot changed (repaint).
pub fn hover(x: i32, y: i32) -> bool {
    let h = hit_at(x, y);
    APP.with_borrow_mut(|a| std::mem::replace(&mut a.clean.hot, h) != h)
}

pub unsafe fn click(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let busy = APP.with_borrow(|a| a.clean.busy);
        match hit_at(x, y) {
            _ if busy => {}
            Some(Hit::Item(i)) => APP.with_borrow_mut(|a| {
                if let Some(p) = a.clean.pick.get_mut(i) {
                    *p = !*p;
                }
            }),
            Some(Hit::Group(r)) => APP.with_borrow_mut(|a| {
                let c = &mut a.clean;
                let all = c.items.iter().zip(&c.pick).filter(|(i, _)| i.risk == r).all(|(_, p)| *p);
                for (i, p) in c.items.iter().zip(c.pick.iter_mut()) {
                    if i.risk == r {
                        *p = !all;
                    }
                }
            }),
            Some(Hit::Go) => go(hwnd),
            Some(Hit::Restore) => restore(),
            None => {}
        }
        InvalidateRect(hwnd, null(), 0);
    }
}

/// Right click on an item: show its first path in Explorer.
pub fn right_click(x: i32, y: i32) {
    if let Some(Hit::Item(i)) = hit_at(x, y)
        && let Some(p) = APP.with_borrow(|a| a.clean.items.get(i).and_then(|i| i.paths.first().cloned()))
    {
        winrust_common::reveal(&p.to_string_lossy());
    }
}

pub unsafe fn wheel(hwnd: HWND, wp: WPARAM) {
    let notches = (wp >> 16) as i16 as i32 / 120;
    APP.with_borrow_mut(|a| a.clean.scroll -= notches * (ROW * a.dpi as f64 / 96.0) as i32 * 2);
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

unsafe fn rect(dc: HDC, r: &RECT, c: u32) {
    unsafe {
        let b = CreateSolidBrush(c);
        FillRect(dc, r, b);
        DeleteObject(b as _);
    }
}

/// A rounded box: filled with `fill`, or only its outline in `edge` when `fill` is None.
unsafe fn round(dc: HDC, r: &RECT, fill: Option<u32>, edge: u32, rad: i32) {
    unsafe {
        let b = fill.map_or(GetStockObject(NULL_BRUSH), |c| CreateSolidBrush(c) as _);
        let p = CreatePen(PS_SOLID, 1, edge);
        let (ob, op) = (SelectObject(dc, b), SelectObject(dc, p as _));
        RoundRect(dc, r.left, r.top, r.right, r.bottom, rad, rad);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        DeleteObject(p as _);
        if fill.is_some() {
            DeleteObject(b);
        }
    }
}

unsafe fn text(dc: HDC, s: &str, mut r: RECT, c: u32, fmt: u32) {
    unsafe {
        SetTextColor(dc, c);
        let t = wide(s);
        DrawTextW(dc, t.as_ptr(), -1, &mut r, fmt | DT_NOPREFIX);
    }
}

unsafe fn text_w(dc: HDC, s: &str) -> i32 {
    let t: Vec<u16> = s.encode_utf16().collect();
    let mut sz: SIZE = unsafe { std::mem::zeroed() };
    unsafe { GetTextExtentPoint32W(dc, t.as_ptr(), t.len() as i32, &mut sz) };
    sz.cx
}

/// The list between the title bar (`top`) and the footer, and the footer with its buttons.
pub unsafe fn paint(dc: HDC, a: &mut App, w: i32, h: i32, top: i32) {
    unsafe {
        let k = a.dpi as f64 / 96.0;
        let s = |v: f64| (v * k) as i32;
        let (fonts, c) = ((a.font, a.bold, a.small), &mut a.clean);
        let foot = h - s(FOOT);
        let one = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS;
        c.hits.clear();
        SelectObject(dc, fonts.0 as _);
        if c.items.is_empty() {
            let msg = if c.busy { c.step.clone() } else { t!("Чистить нечего: всё уже убрано.", "Nothing to clean: it is all tidy already.", ja: "削除するものはありません。すでに片付いています。", pt: "Nada para limpar: já está tudo em ordem.", zh: "没有可清理的内容：已经很整洁了。", es: "Nada que limpiar: ya está todo en orden.").into() };
            text(dc, &msg, RECT { left: s(24.0), top, right: w - s(24.0), bottom: foot }, DIM, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_PATH_ELLIPSIS);
        } else {
            // the rows scroll under a clip; the scroll stops at both ends
            let groups: Vec<Risk> = [Risk::Safe, Risk::Rebuild, Risk::Ask].into_iter().filter(|r| c.items.iter().any(|i| i.risk == *r)).collect();
            let content = groups.len() as i32 * s(HEAD) + c.items.len() as i32 * s(ROW) + s(12.0);
            c.scroll = c.scroll.clamp(0, (content - (foot - top)).max(0));
            IntersectClipRect(dc, 0, top, w, foot);
            let mut y = top - c.scroll;
            for r in groups {
                let hr = RECT { left: s(12.0), top: y, right: w - s(12.0), bottom: y + s(HEAD) };
                let total: u64 = c.items.iter().filter(|i| i.risk == r).map(|i| i.bytes).sum();
                let dot = RECT { left: hr.left + s(6.0), top: y + s(HEAD) / 2 - s(4.0), right: hr.left + s(14.0), bottom: y + s(HEAD) / 2 + s(4.0) };
                round(dc, &dot, Some(colour(r)), colour(r), s(8.0));
                SelectObject(dc, fonts.2 as _);
                text(dc, engine::risk_title(r), RECT { left: hr.left + s(24.0), right: hr.right - s(110.0), ..hr }, colour(r), one);
                text(dc, &human(total), RECT { right: hr.right - s(8.0), ..hr }, DIM, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
                c.hits.push((hr, Hit::Group(r)));
                y += s(HEAD);
                for (n, i) in c.items.iter().enumerate().filter(|(_, i)| i.risk == r) {
                    let rr = RECT { left: s(12.0), top: y, right: w - s(12.0), bottom: y + s(ROW) - s(4.0) };
                    if c.hot == Some(Hit::Item(n)) {
                        round(dc, &rr, Some(HOVER), HOVER, s(10.0));
                    }
                    // the tick box
                    let bx = RECT { left: rr.left + s(12.0), top: rr.top + s(9.0), right: rr.left + s(30.0), bottom: rr.top + s(27.0) };
                    if c.pick[n] {
                        round(dc, &bx, Some(ACCENT), ACCENT, s(6.0));
                        let pen = CreatePen(PS_SOLID, s(2.0).max(2), rgb(255, 255, 255));
                        let op = SelectObject(dc, pen as _);
                        MoveToEx(dc, bx.left + s(4.0), bx.top + s(9.0), null_mut());
                        LineTo(dc, bx.left + s(8.0), bx.top + s(13.0));
                        LineTo(dc, bx.left + s(14.0), bx.top + s(5.0));
                        SelectObject(dc, op);
                        DeleteObject(pen as _);
                    } else {
                        round(dc, &bx, None, DIM, s(6.0));
                    }
                    let lx = rr.left + s(44.0);
                    SelectObject(dc, fonts.1 as _);
                    let path = i.label.contains(":\\");
                    text(dc, &i.label, RECT { left: lx, top: rr.top + s(6.0), right: rr.right - s(110.0), bottom: rr.top + s(28.0) }, TEXT, if path { DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_PATH_ELLIPSIS } else { one });
                    text(dc, &human(i.bytes), RECT { left: rr.right - s(110.0), top: rr.top + s(6.0), right: rr.right - s(12.0), bottom: rr.top + s(28.0) }, TEXT, DT_RIGHT | DT_VCENTER | DT_SINGLELINE);
                    SelectObject(dc, fonts.2 as _);
                    let note = if i.fate().is_empty() { i.note.clone() } else { format!("{} {}", i.note, i.fate()) };
                    text(dc, &note, RECT { left: lx, top: rr.top + s(28.0), right: rr.right - s(12.0), bottom: rr.bottom - s(2.0) }, DIM, one);
                    c.hits.push((rr, Hit::Item(n)));
                    y += s(ROW);
                }
            }
            SelectClipRgn(dc, null_mut());
            c.hits.retain(|(r, _)| r.bottom > top && r.top < foot); // a row scrolled away is not clickable
        }

        // footer: what is ticked (or how the last run went) and the buttons
        rect(dc, &RECT { left: 0, top: foot, right: w, bottom: foot + 1 }, LINE);
        let picked: u64 = c.items.iter().zip(&c.pick).filter(|(_, p)| **p).map(|(i, _)| i.bytes).sum();
        let all: u64 = c.items.iter().map(|i| i.bytes).sum();
        SelectObject(dc, fonts.1 as _);
        let go = if c.busy && !c.items.is_empty() { t!("Очистка...", "Cleaning...", ja: "削除しています...", pt: "Limpando...", zh: "正在清理...", es: "Limpiando...").to_string() } else { tf!("Освободить {}", "Free {}", ja: "{} を解放", pt: "Liberar {}", zh: "释放 {}", es: "Liberar {}", human(picked)) };
        let (bh, pad) = (s(36.0), s(18.0));
        let by = foot + (s(FOOT) - bh) / 2;
        let gw = text_w(dc, &go) + 2 * pad;
        let gr = RECT { left: w - s(16.0) - gw, top: by, right: w - s(16.0), bottom: by + bh };
        let live = !c.busy && picked > 0;
        let lift = |c0: u32, on: bool| if on { [0, 8, 16].iter().map(|sh| ((c0 >> sh & 255) + 20).min(255) << sh).sum() } else { c0 }; // hover: a touch lighter
        round(dc, &gr, Some(if live { lift(ACCENT, c.hot == Some(Hit::Go)) } else { rgb(58, 58, 62) }), if live { ACCENT } else { rgb(58, 58, 62) }, s(10.0));
        text(dc, &go, gr, if live { rgb(255, 255, 255) } else { DIM }, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        c.hits.push((gr, Hit::Go));
        let mut left_edge = gr.left;
        if c.quar > 0 && !c.busy {
            SelectObject(dc, fonts.0 as _);
            let rs = tf!("Вернуть из карантина ({})", "Restore from quarantine ({})", ja: "隔離から復元 ({})", pt: "Restaurar da quarentena ({})", zh: "从隔离区恢复（{}）", es: "Restaurar de la cuarentena ({})", c.quar);
            let rw = text_w(dc, &rs) + 2 * pad;
            let rr = RECT { left: gr.left - s(10.0) - rw, top: by, right: gr.left - s(10.0), bottom: by + bh };
            round(dc, &rr, if c.hot == Some(Hit::Restore) { Some(HOVER) } else { None }, rgb(90, 90, 96), s(10.0));
            text(dc, &rs, rr, TEXT, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            c.hits.push((rr, Hit::Restore));
            left_edge = rr.left;
        }
        SelectObject(dc, fonts.0 as _);
        let status = if c.busy {
            c.step.clone()
        } else if !c.note.is_empty() {
            c.note.clone()
        } else {
            tf!("Выбрано {0} из {1}", "{0} of {1} selected", ja: "{1} のうち {0} を選択", pt: "{0} de {1} selecionados", zh: "已选择 {0}，共 {1}", es: "{0} de {1} seleccionados", human(picked), human(all))
        };
        text(dc, &status, RECT { left: s(16.0), top: foot, right: left_edge - s(12.0), bottom: h }, DIM, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_PATH_ELLIPSIS);
    }
}
