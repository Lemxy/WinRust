//! Win+V: the clipboard history popup (420 x 520 logical, frost, same palette as the bar).
//!
//! Rows are the newest-first entries of `store`. Typing filters on the full text/paths, arrows move, Enter or a click
//! pastes into the window that had the foreground when the hotkey fired, Del removes the selected row and the footer
//! empties the history. Esc, a click outside or a second Win+V closes it.
use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    ptr::null_mut,
    sync::{Arc, Mutex},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount64},
    UI::{
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::*,
    },
};

use winrust_common::style::{mix, pal};

use crate::{
    store::{self, Entry, Kind, Store},
    ui,
};

const CLASS: &str = "WinRustClipHistory";
const TIMER_CARET: usize = 1;

thread_local! {
    static PASTE_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

struct Popup {
    hwnd: HWND,
    store: Arc<Mutex<Store>>,
    entries: Vec<Entry>,
    search: Vec<String>, // lowercase text/paths per entry, built once when the popup opens
    filt: Vec<usize>,
    query: String,
    sel: usize,
    scroll: usize,
    hover: i32,
    clear_hot: bool,
    caret: bool,
    ready: bool,
    opened: u64,
    px: Vec<u32>,
    thumbs: HashMap<PathBuf, HBITMAP>,
    f_text: HFONT,
    f_small: HFONT,
    f_title: HFONT,
    f_icon: HFONT,
    f_empty: HFONT,
}

pub fn open(store: Arc<Mutex<Store>>, target: isize) {
    let target = target as HWND;
    unsafe {
        let inst = GetModuleHandleW(null_mut());
        let class = ui::wide(CLASS);
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: inst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc); // 0 when another popup already registered it: fine

        let (x, y, w, h) = ui::geometry(ui::POPUP_W, ui::POPUP_H);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            ui::wide("rs-clip").as_ptr(),
            WS_POPUP,
            x,
            y,
            w,
            h,
            null_mut(),
            null_mut(),
            inst,
            null_mut(),
        );
        if hwnd.is_null() {
            crate::log("history: the window could not be created");
            return;
        }

        let entries = {
            let mut s = store.lock().unwrap_or_else(|e| e.into_inner());
            s.refresh_limit();
            s.entries()
        };
        let search = entries.iter().map(|e| store::entry_text(e).to_lowercase()).collect();
        let mut p = Box::new(Popup {
            hwnd,
            store,
            entries,
            search,
            filt: Vec::new(),
            query: String::new(),
            sel: 0,
            scroll: 0,
            hover: -1,
            clear_hot: false,
            caret: true,
            ready: false,
            opened: GetTickCount64(),
            px: Vec::new(),
            thumbs: HashMap::new(),
            f_text: ui::font(hwnd, 13, 400, winrust_common::style::ui_face()),
            f_small: ui::font(hwnd, 11, 400, winrust_common::style::ui_face()),
            f_title: ui::font(hwnd, 15, 600, winrust_common::style::ui_face()),
            f_icon: ui::font(hwnd, 16, 400, winrust_common::style::icon_face()),
            f_empty: ui::font(hwnd, 26, 400, winrust_common::style::icon_face()),
        });
        p.refilter();
        p.px = ui::frost_bits(w, h, x, y);
        SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, ui::sc(hwnd, 18), ui::sc(hwnd, 18)), 1);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(p) as isize);
        crate::popup_created(hwnd);
        PASTE_ID.with(|c| *c.borrow_mut() = None);
        ShowWindow(hwnd, SW_SHOW);
        winrust_common::fg::force(hwnd);
        SetTimer(hwnd, TIMER_CARET, 530, None);
        if let Some(p) = popup(hwnd) {
            p.ready = true;
        }

        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }

    // The window is gone: the entry is put on the clipboard, then Ctrl+V goes to the window that opened the popup.
    // No store lock while the clipboard is touched: a notification can arrive and its handler wants that lock.
    if let Some(id) = PASTE_ID.with(|c| c.borrow_mut().take()) {
        let entry = crate::STORE.get().and_then(|s| s.lock().unwrap_or_else(|e| e.into_inner()).get(&id));
        let ok = entry.as_ref().is_some_and(store::copy_entry);
        if ok {
            if let Some(s) = crate::STORE.get() {
                s.lock().unwrap_or_else(|e| e.into_inner()).touch(&id);
            }
            ui::paste_into(target);
        } else {
            crate::log("history: the entry could not be put back on the clipboard");
        }
    }
}

unsafe fn popup(h: HWND) -> Option<&'static mut Popup> {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Popup;
    (!p.is_null()).then(|| &mut *p)
}

impl Popup {
    fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        self.filt = if q.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            (0..self.entries.len()).filter(|i| self.search[*i].contains(&q)).collect()
        };
        if self.sel >= self.filt.len() {
            self.sel = self.filt.len().saturating_sub(1);
        }
        self.clamp_scroll();
    }

    unsafe fn visible(&self) -> usize {
        let m = ui::metrics(self.hwnd);
        let row = ui::sc(self.hwnd, ui::ROW_H).max(1);
        ((m.body.bottom - m.body.top) / row).max(0) as usize
    }

    fn clamp_scroll(&mut self) {
        let vis = unsafe { self.visible() };
        self.scroll = self.scroll.min(self.filt.len().saturating_sub(vis));
    }

    unsafe fn ensure_visible(&mut self) {
        let vis = self.visible().max(1);
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + vis {
            self.scroll = self.sel + 1 - vis;
        }
        self.clamp_scroll();
    }

    /// Reloads after a delete/clear: the store owns the files, the popup only keeps a snapshot.
    fn reload(&mut self) {
        self.entries = self.store.lock().unwrap_or_else(|e| e.into_inner()).entries();
        self.search = self.entries.iter().map(|e| store::entry_text(e).to_lowercase()).collect();
        for (_, h) in self.thumbs.drain() {
            unsafe { DeleteObject(h as HGDIOBJ) };
        }
        self.hover = -1;
        self.refilter();
    }

    unsafe fn paste_selected(&mut self) {
        if let Some(&i) = self.filt.get(self.sel) {
            let id = self.entries[i].id.clone();
            PASTE_ID.with(|c| *c.borrow_mut() = Some(id));
        }
        DestroyWindow(self.hwnd);
    }

    unsafe fn delete_selected(&mut self) {
        let Some(&i) = self.filt.get(self.sel) else { return };
        let id = self.entries[i].id.clone();
        self.store.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        self.reload();
        InvalidateRect(self.hwnd, null_mut(), 0);
    }

    unsafe fn clear(&mut self) {
        self.store.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.reload();
        InvalidateRect(self.hwnd, null_mut(), 0);
    }

    unsafe fn thumb(&mut self, path: &PathBuf) -> HBITMAP {
        if let Some(h) = self.thumbs.get(path) {
            return *h;
        }
        let hb = LoadImageW(null_mut(), ui::wide(&path.to_string_lossy()).as_ptr(), IMAGE_BITMAP, 0, 0, LR_LOADFROMFILE);
        self.thumbs.insert(path.clone(), hb);
        hb
    }

    unsafe fn clear_rect(&self, m: &ui::Metrics) -> RECT {
        let label = t!("Очистить историю", "Clear history", ja: "履歴を消去", pt: "Limpar histórico", zh: "清空历史记录");
        let dc = GetDC(self.hwnd);
        let w = ui::text_w(dc, label, self.f_text) + ui::sc(self.hwnd, 22);
        ReleaseDC(self.hwnd, dc);
        RECT { left: m.footer.right - w, top: m.footer.top + ui::sc(self.hwnd, 3), right: m.footer.right, bottom: m.footer.bottom - ui::sc(self.hwnd, 3) }
    }

    unsafe fn paint(&mut self) {
        let h = self.hwnd;
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let wdc = BeginPaint(h, &mut ps);
        let mut cr: RECT = std::mem::zeroed();
        GetClientRect(h, &mut cr);
        if cr.right <= 0 || cr.bottom <= 0 {
            EndPaint(h, &ps);
            return;
        }
        let mem = CreateCompatibleDC(wdc);
        let bmp = CreateCompatibleBitmap(wdc, cr.right, cr.bottom);
        let old = SelectObject(mem, bmp);
        let p = pal();
        ui::surface(mem, &self.px, cr);
        let m = ui::metrics(h);

        ui::title(mem, &m, t!("Буфер обмена", "Clipboard", ja: "クリップボード", pt: "Área de transferência", zh: "剪贴板"), self.f_title);
        ui::search(
            mem,
            h,
            m.search,
            &self.query,
            t!("Поиск по буферу обмена", "Search the clipboard", ja: "クリップボードを検索", pt: "Pesquisar na área de transferência", zh: "搜索剪贴板"),
            self.f_text,
            self.f_icon,
            self.caret,
        );

        let row_h = ui::sc(h, ui::ROW_H).max(1);
        let visible = ((m.body.bottom - m.body.top) / row_h).max(0) as usize;
        self.scroll = self.scroll.min(self.filt.len().saturating_sub(visible));

        if self.filt.is_empty() {
            let cy = m.body.top + (m.body.bottom - m.body.top) / 2;
            let icon = RECT { left: m.body.left, top: cy - ui::sc(h, 52), right: m.body.right, bottom: cy - ui::sc(h, 10) };
            ui::text(mem, "\u{E8C8}", icon, self.f_empty, p.dim, DT_CENTER);
            let (head, hint) = if self.entries.is_empty() {
                (
                    t!("История буфера обмена пуста", "No clipboard history", ja: "クリップボードの履歴はありません", pt: "Sem histórico da área de transferência", zh: "剪贴板历史为空"),
                    t!("Скопируйте что-нибудь, и это появится здесь", "Copy something and it will appear here", ja: "何かをコピーするとここに表示されます", pt: "Copie algo e aparecerá aqui", zh: "复制内容后会显示在这里"),
                )
            } else {
                (
                    t!("Ничего не найдено", "No matches", ja: "一致する項目はありません", pt: "Nenhum resultado", zh: "没有匹配项"),
                    "",
                )
            };
            ui::text(mem, head, RECT { left: m.body.left, top: cy, right: m.body.right, bottom: cy + ui::sc(h, 26) }, self.f_text, p.text, DT_CENTER);
            if !hint.is_empty() {
                ui::text(
                    mem,
                    hint,
                    RECT { left: m.body.left, top: cy + ui::sc(h, 26), right: m.body.right, bottom: cy + ui::sc(h, 48) },
                    self.f_small,
                    p.dim,
                    DT_CENTER,
                );
            }
        } else {
            let last = (self.scroll + visible).min(self.filt.len());
            for i in self.scroll..last {
                let entry = self.entries[self.filt[i]].clone();
                let top = m.body.top + ((i - self.scroll) as i32) * row_h;
                let r = RECT { left: m.body.left + ui::sc(h, 2), top: top + ui::sc(h, 2), right: m.body.right - ui::sc(h, 2), bottom: top + row_h - ui::sc(h, 2) };
                if i == self.sel {
                    ui::rounded(mem, r, mix(p.bg, p.accent, 0.20), ui::sc(h, 9));
                } else if self.hover == i as i32 {
                    ui::rounded(mem, r, p.hover, ui::sc(h, 9));
                }
                let icon = ui::sc(h, 36);
                let ir = RECT { left: r.left + ui::sc(h, 6), top: r.top + (row_h - ui::sc(h, 4) - icon) / 2, right: r.left + ui::sc(h, 6) + icon, bottom: r.top + (row_h - ui::sc(h, 4) - icon) / 2 + icon };
                let glyph = match entry.kind {
                    Kind::Text => "\u{E8A5}",
                    Kind::Image => "\u{E91B}",
                    Kind::Files => "\u{E8B7}",
                };
                let mut drew_thumb = false;
                if entry.kind == Kind::Image {
                    if let Some(t) = &entry.thumb {
                        let hb = self.thumb(t);
                        if !hb.is_null() {
                            ui::bitmap_fit(mem, hb, ir);
                            drew_thumb = true;
                        }
                    }
                }
                if !drew_thumb {
                    ui::text(mem, glyph, ir, self.f_icon, if i == self.sel { p.accent } else { p.dim }, DT_CENTER);
                }
                let left = ir.right + ui::sc(h, 12);
                let right = r.right - ui::sc(h, 10);
                let line1 = ui::one_line(&entry.label);
                ui::text(
                    mem,
                    &line1,
                    RECT { left, top: r.top + ui::sc(h, 6), right, bottom: r.top + row_h / 2 },
                    self.f_text,
                    p.text,
                    DT_END_ELLIPSIS,
                );
                let kind = match entry.kind {
                    Kind::Text => t!("Текст", "Text", ja: "テキスト", pt: "Texto", zh: "文本"),
                    Kind::Image => t!("Изображение", "Image", ja: "画像", pt: "Imagem", zh: "图片"),
                    Kind::Files => t!("Файлы", "Files", ja: "ファイル", pt: "Arquivos", zh: "文件"),
                };
                let sub = format!("{kind} · {}", ui::hhmm(entry.at));
                ui::text(
                    mem,
                    &sub,
                    RECT { left, top: r.top + row_h / 2, right, bottom: r.bottom - ui::sc(h, 4) },
                    self.f_small,
                    p.dim,
                    DT_END_ELLIPSIS,
                );
            }
            ui::scrollbar(mem, h, m.body, self.filt.len(), visible, self.scroll);
        }

        // footer
        ui::footer_sep(mem, &m, h);
        let n = if self.query.is_empty() {
            tf!("{} записей", "{} items", ja: "{} 件", pt: "{} itens", zh: "{} 项", self.filt.len())
        } else {
            tf!("{} найдено", "{} found", ja: "{} 件", pt: "{} encontrados", zh: "{} 个结果", self.filt.len())
        };
        ui::text(mem, &n, m.footer, self.f_small, p.dim, 0);
        let btn = self.clear_rect(&m);
        let label = t!("Очистить историю", "Clear history", ja: "履歴を消去", pt: "Limpar histórico", zh: "清空历史记录");
        if self.clear_hot && !self.entries.is_empty() {
            ui::rounded(mem, btn, p.hover, ui::sc(h, 8));
        }
        ui::text(mem, label, btn, self.f_text, if self.entries.is_empty() { p.dim } else if self.clear_hot { p.accent } else { p.text }, DT_RIGHT);

        BitBlt(wdc, 0, 0, cr.right, cr.bottom, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp);
        DeleteDC(mem);
        EndPaint(h, &ps);
    }
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            if let Some(p) = popup(h) {
                p.paint();
                return 0;
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_TIMER if wp == TIMER_CARET => {
            if let Some(p) = popup(h) {
                p.caret = !p.caret;
                let m = ui::metrics(h);
                InvalidateRect(h, &m.search, 0);
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(p) = popup(h) {
                let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
                let m = ui::metrics(h);
                let row_h = ui::sc(h, ui::ROW_H).max(1);
                let mut hover = -1i32;
                let mut clear_hot = false;
                if y >= m.body.top && y < m.body.bottom && x >= m.body.left && x <= m.body.right {
                    let row = p.scroll + ((y - m.body.top) / row_h) as usize;
                    if row < p.filt.len() {
                        hover = row as i32;
                    }
                }
                if x >= m.footer.left && y >= m.footer.top {
                    let cr = p.clear_rect(&m);
                    clear_hot = x >= cr.left;
                }
                if hover != p.hover || clear_hot != p.clear_hot {
                    p.hover = hover;
                    p.clear_hot = clear_hot;
                    InvalidateRect(h, null_mut(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if let Some(p) = popup(h) {
                let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
                let m = ui::metrics(h);
                if x >= m.footer.left && y >= m.footer.top {
                    let cr = p.clear_rect(&m);
                    if x >= cr.left && !p.entries.is_empty() {
                        p.clear();
                    }
                    return 0;
                }
                let row_h = ui::sc(h, ui::ROW_H).max(1);
                if y >= m.body.top && y < m.body.bottom && x >= m.body.left && x <= m.body.right {
                    let row = p.scroll + ((y - m.body.top) / row_h) as usize;
                    if row < p.filt.len() {
                        p.sel = row;
                        p.paste_selected();
                    }
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if let Some(p) = popup(h) {
                let delta = ((wp >> 16) & 0xffff) as i16 as i32;
                let step = 3usize;
                if delta > 0 {
                    p.scroll = p.scroll.saturating_sub(step);
                } else {
                    p.scroll = (p.scroll + step).min(p.filt.len().saturating_sub(1));
                }
                p.clamp_scroll();
                InvalidateRect(h, null_mut(), 0);
            }
            0
        }
        WM_CHAR => {
            if let Some(p) = popup(h) {
                let c = char::from_u32(wp as u32);
                if let Some(c) = c {
                    if !c.is_control() && c != '\u{7f}' {
                        p.query.push(c);
                        p.sel = 0;
                        p.scroll = 0;
                        p.refilter();
                        InvalidateRect(h, null_mut(), 0);
                    }
                }
            }
            0
        }
        WM_KEYDOWN => {
            if let Some(p) = popup(h) {
                match wp as u16 {
                    VK_ESCAPE => {
                        DestroyWindow(h);
                    }
                    VK_BACK => {
                        if p.query.pop().is_some() {
                            p.sel = 0;
                            p.scroll = 0;
                            p.refilter();
                            InvalidateRect(h, null_mut(), 0);
                        }
                    }
                    VK_UP => {
                        if p.sel > 0 {
                            p.sel -= 1;
                            p.ensure_visible();
                            InvalidateRect(h, null_mut(), 0);
                        }
                    }
                    VK_DOWN => {
                        if p.sel + 1 < p.filt.len() {
                            p.sel += 1;
                            p.ensure_visible();
                            InvalidateRect(h, null_mut(), 0);
                        }
                    }
                    VK_PRIOR => {
                        p.sel = p.sel.saturating_sub(p.visible().max(1));
                        p.ensure_visible();
                        InvalidateRect(h, null_mut(), 0);
                    }
                    VK_NEXT => {
                        p.sel = (p.sel + p.visible().max(1)).min(p.filt.len().saturating_sub(1));
                        p.ensure_visible();
                        InvalidateRect(h, null_mut(), 0);
                    }
                    VK_HOME => {
                        p.sel = 0;
                        p.scroll = 0;
                        InvalidateRect(h, null_mut(), 0);
                    }
                    VK_END => {
                        p.sel = p.filt.len().saturating_sub(1);
                        p.ensure_visible();
                        InvalidateRect(h, null_mut(), 0);
                    }
                    VK_RETURN => {
                        p.paste_selected();
                    }
                    VK_DELETE => p.delete_selected(),
                    _ => {}
                }
            }
            0
        }
        WM_ACTIVATE => {
            if let Some(p) = popup(h) {
                // A click on another window closes the popup. The guard skips the activation traffic of ShowWindow.
                if (wp as u32 & 0xffff) == WA_INACTIVE && p.ready && GetTickCount64().saturating_sub(p.opened) > 150 {
                    DestroyWindow(h);
                }
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(h);
            0
        }
        WM_DESTROY => {
            KillTimer(h, TIMER_CARET);
            if let Some(p) = popup(h) {
                for f in [p.f_text, p.f_small, p.f_title, p.f_icon, p.f_empty] {
                    if !f.is_null() {
                        DeleteObject(f as HGDIOBJ);
                    }
                }
                for (_, hb) in p.thumbs.drain() {
                    if !hb.is_null() {
                        DeleteObject(hb as HGDIOBJ);
                    }
                }
                SetWindowLongPtrW(h, GWLP_USERDATA, 0);
                drop(Box::from_raw(p as *mut Popup));
            }
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
