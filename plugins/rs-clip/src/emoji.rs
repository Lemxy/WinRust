//! Win+.: the emoji picker (420 x 520 logical, frost).
//!
//! Five groups of Segoe UI Emoji: faces, gestures, food, objects, symbols. Typing searches every group by the English
//! name below (the table is ours: no font let alone a network lookup). Click or Enter puts the glyph on the clipboard and
//! sends Ctrl+V to the window that was in front; Esc or a click outside closes.
use std::{
    cell::RefCell,
    ptr::null_mut,
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

use crate::{store, ui};

const CLASS: &str = "WinRustClipEmoji";
const TIMER_CARET: usize = 1;
const CELL: i32 = 40;

thread_local! {
    static PASTE_TEXT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// (glyph, English name for the search, group 0..4)
type Emoji = (&'static str, &'static str, u8);

const EMOJI: &[Emoji] = &[
    // faces
    ("😀", "grinning face", 0),
    ("😃", "grinning face big eyes", 0),
    ("😄", "grinning face smiling eyes", 0),
    ("😁", "beaming face", 0),
    ("😆", "laughing", 0),
    ("😅", "grinning face sweat", 0),
    ("🤣", "rolling on the floor laughing", 0),
    ("😂", "face with tears of joy", 0),
    ("🙂", "slightly smiling", 0),
    ("🙃", "upside down", 0),
    ("😉", "winking", 0),
    ("😊", "smiling eyes", 0),
    ("😇", "smiling halo", 0),
    ("🥰", "smiling hearts", 0),
    ("😍", "heart eyes", 0),
    ("🤩", "star struck", 0),
    ("😘", "kissing heart", 0),
    ("😗", "kissing", 0),
    ("😋", "savoring food", 0),
    ("😛", "tongue", 0),
    ("😜", "winking tongue", 0),
    ("🤪", "zany", 0),
    ("🤑", "money mouth", 0),
    ("🤗", "hugging", 0),
    ("🤭", "hand over mouth", 0),
    ("🤔", "thinking", 0),
    ("🤐", "zipper mouth", 0),
    ("😐", "neutral", 0),
    ("😑", "expressionless", 0),
    ("😶", "no mouth", 0),
    ("😏", "smirking", 0),
    ("🙄", "eye roll", 0),
    ("😬", "grimacing", 0),
    ("😴", "sleeping", 0),
    ("😪", "sleepy", 0),
    ("😷", "medical mask", 0),
    ("🤒", "thermometer", 0),
    ("🤕", "bandaged head", 0),
    ("🤢", "nauseated", 0),
    ("🤮", "vomiting", 0),
    ("🥵", "hot", 0),
    ("🥶", "cold", 0),
    ("😵", "dizzy", 0),
    ("🤯", "exploding head", 0),
    ("🥳", "partying", 0),
    ("😎", "smiling sunglasses", 0),
    ("🤓", "nerd", 0),
    ("🧐", "monocle", 0),
    ("😕", "confused", 0),
    ("😟", "worried", 0),
    ("🙁", "frowning", 0),
    ("😮", "open mouth", 0),
    ("😲", "astonished", 0),
    ("😳", "flushed", 0),
    ("🥺", "pleading", 0),
    ("😢", "crying", 0),
    ("😭", "sobbing", 0),
    ("😱", "screaming fear", 0),
    ("😖", "confounded", 0),
    ("😣", "persevering", 0),
    ("😞", "disappointed", 0),
    ("😩", "weary", 0),
    ("😫", "tired", 0),
    ("🥱", "yawning", 0),
    ("😤", "triumph", 0),
    ("😡", "angry", 0),
    ("😠", "angry face", 0),
    ("🤬", "cursing", 0),
    ("😈", "smiling devil", 0),
    ("👿", "angry devil", 0),
    ("💀", "skull", 0),
    ("💩", "pile of poo", 0),
    ("🤡", "clown", 0),
    ("👻", "ghost", 0),
    ("👽", "alien", 0),
    ("🤖", "robot", 0),
    // gestures
    ("👍", "thumbs up", 1),
    ("👎", "thumbs down", 1),
    ("👌", "ok hand", 1),
    ("🤌", "pinched fingers", 1),
    ("✌️", "victory hand", 1),
    ("🤞", "crossed fingers", 1),
    ("🤟", "love you gesture", 1),
    ("🤘", "horns", 1),
    ("🤙", "call me hand", 1),
    ("👈", "pointing left", 1),
    ("👉", "pointing right", 1),
    ("👆", "pointing up", 1),
    ("👇", "pointing down", 1),
    ("☝️", "index pointing up", 1),
    ("✋", "raised hand", 1),
    ("🤚", "back of hand", 1),
    ("👋", "waving hand", 1),
    ("🤝", "handshake", 1),
    ("🙏", "folded hands", 1),
    ("💪", "flexed biceps", 1),
    ("🖐️", "hand fingers splayed", 1),
    ("🫡", "salute", 1),
    ("👏", "clapping", 1),
    ("🙌", "raising hands", 1),
    ("🙋", "raising hand", 1),
    ("🤷", "shrug", 1),
    ("🤦", "facepalm", 1),
    ("💅", "nail polish", 1),
    ("👀", "eyes", 1),
    ("🧠", "brain", 1),
    ("🫶", "heart hands", 1),
    ("🤲", "palms up", 1),
    ("👊", "fist", 1),
    ("✊", "raised fist", 1),
    ("🤛", "left fist", 1),
    ("🤜", "right fist", 1),
    ("🖖", "vulcan salute", 1),
    // food
    ("🍎", "apple", 2),
    ("🍌", "banana", 2),
    ("🍇", "grapes", 2),
    ("🍓", "strawberry", 2),
    ("🍒", "cherries", 2),
    ("🍑", "peach", 2),
    ("🍍", "pineapple", 2),
    ("🥝", "kiwi", 2),
    ("🍅", "tomato", 2),
    ("🥑", "avocado", 2),
    ("🥕", "carrot", 2),
    ("🌽", "corn", 2),
    ("🥔", "potato", 2),
    ("🍞", "bread", 2),
    ("🥐", "croissant", 2),
    ("🥖", "baguette", 2),
    ("🧀", "cheese", 2),
    ("🍳", "cooking egg", 2),
    ("🥓", "bacon", 2),
    ("🍔", "hamburger", 2),
    ("🍟", "fries", 2),
    ("🍕", "pizza", 2),
    ("🌭", "hot dog", 2),
    ("🌮", "taco", 2),
    ("🌯", "burrito", 2),
    ("🍜", "noodles", 2),
    ("🍣", "sushi", 2),
    ("🍚", "rice", 2),
    ("🍰", "cake", 2),
    ("🍫", "chocolate", 2),
    ("🍪", "cookie", 2),
    ("🍩", "donut", 2),
    ("🧁", "cupcake", 2),
    ("☕", "coffee", 2),
    ("🍵", "tea", 2),
    ("🍺", "beer", 2),
    ("🍷", "wine", 2),
    ("🥤", "soda", 2),
    ("🍿", "popcorn", 2),
    ("🥗", "salad", 2),
    // objects
    ("💻", "laptop", 3),
    ("🖥️", "desktop computer", 3),
    ("⌨️", "keyboard", 3),
    ("🖱️", "mouse", 3),
    ("📱", "phone", 3),
    ("☎️", "telephone", 3),
    ("📷", "camera", 3),
    ("🎥", "video camera", 3),
    ("🎧", "headphones", 3),
    ("🎤", "microphone", 3),
    ("📺", "television", 3),
    ("⏰", "alarm clock", 3),
    ("⌚", "watch", 3),
    ("💡", "light bulb", 3),
    ("🔦", "flashlight", 3),
    ("🔑", "key", 3),
    ("🔒", "lock", 3),
    ("🔓", "unlock", 3),
    ("🔨", "hammer", 3),
    ("🛠️", "tools", 3),
    ("🧰", "toolbox", 3),
    ("📦", "box", 3),
    ("📁", "folder", 3),
    ("📄", "page", 3),
    ("📝", "memo", 3),
    ("✏️", "pencil", 3),
    ("📌", "pushpin", 3),
    ("📎", "paperclip", 3),
    ("✂️", "scissors", 3),
    ("📅", "calendar", 3),
    ("📈", "chart increasing", 3),
    ("💾", "floppy disk", 3),
    ("🗑️", "trash", 3),
    ("🚀", "rocket", 3),
    ("🚗", "car", 3),
    ("✈️", "airplane", 3),
    ("🏠", "house", 3),
    ("⚙️", "gear", 3),
    ("🔔", "bell", 3),
    ("🎁", "gift", 3),
    // symbols
    ("❤️", "red heart", 4),
    ("🧡", "orange heart", 4),
    ("💛", "yellow heart", 4),
    ("💚", "green heart", 4),
    ("💙", "blue heart", 4),
    ("💜", "purple heart", 4),
    ("🖤", "black heart", 4),
    ("🤍", "white heart", 4),
    ("🤎", "brown heart", 4),
    ("💔", "broken heart", 4),
    ("✨", "sparkles", 4),
    ("⭐", "star", 4),
    ("🌟", "glowing star", 4),
    ("🔥", "fire", 4),
    ("💧", "droplet", 4),
    ("☀️", "sun", 4),
    ("🌙", "crescent moon", 4),
    ("☁️", "cloud", 4),
    ("⚡", "bolt", 4),
    ("❄️", "snowflake", 4),
    ("🎵", "music note", 4),
    ("🎶", "music notes", 4),
    ("✅", "check mark button", 4),
    ("❌", "cross mark", 4),
    ("⚠️", "warning", 4),
    ("❓", "question mark", 4),
    ("❗", "exclamation mark", 4),
    ("➕", "plus", 4),
    ("➖", "minus", 4),
    ("✔️", "check mark", 4),
    ("✖️", "multiplication", 4),
    ("♻️", "recycle", 4),
    ("🔴", "red circle", 4),
    ("🟢", "green circle", 4),
    ("🔵", "blue circle", 4),
    ("💯", "hundred points", 4),
    ("🔕", "bell with slash", 4),
    ("🏁", "chequered flag", 4),
    ("🎉", "party popper", 4),
    ("💤", "zzz", 4),
];

struct Popup {
    hwnd: HWND,
    query: String,
    cat: u8,
    filt: Vec<usize>,
    sel: usize,
    scroll: usize,
    hover: i32,
    caret: bool,
    ready: bool,
    opened: u64,
    px: Vec<u32>,
    f_text: HFONT,
    f_small: HFONT,
    f_title: HFONT,
    f_icon: HFONT,
    f_emoji: HFONT,
    f_tab: HFONT,
    f_big: HFONT,
}

pub fn open(target: isize) {
    let target = target as HWND;
    unsafe {
        let inst = GetModuleHandleW(null_mut());
        let class = ui::wide(CLASS);
        let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: inst, lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassW(&wc);

        let (x, y, w, h) = ui::geometry(ui::POPUP_W, ui::POPUP_H);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            ui::wide("rs-clip emoji").as_ptr(),
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
            crate::log("emoji: the window could not be created");
            return;
        }

        let mut p = Box::new(Popup {
            hwnd,
            query: String::new(),
            cat: 0,
            filt: Vec::new(),
            sel: 0,
            scroll: 0,
            hover: -1,
            caret: true,
            ready: false,
            opened: GetTickCount64(),
            px: Vec::new(),
            f_text: ui::font(hwnd, 13, 400, winrust_common::style::ui_face()),
            f_small: ui::font(hwnd, 11, 400, winrust_common::style::ui_face()),
            f_title: ui::font(hwnd, 15, 600, winrust_common::style::ui_face()),
            f_icon: ui::font(hwnd, 16, 400, winrust_common::style::icon_face()),
            f_emoji: ui::font(hwnd, 24, 400, ui::EMOJI_FACE),
            f_tab: ui::font(hwnd, 11, 400, winrust_common::style::ui_face()),
            f_big: ui::font(hwnd, 26, 400, winrust_common::style::icon_face()),
        });
        p.refilter();
        p.px = ui::frost_bits(w, h, x, y);
        SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, ui::sc(hwnd, 18), ui::sc(hwnd, 18)), 1);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(p) as isize);
        crate::popup_created(hwnd);
        PASTE_TEXT.with(|c| *c.borrow_mut() = None);
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

    if let Some(s) = PASTE_TEXT.with(|c| c.borrow_mut().take()) {
        if store::put_text(&s) {
            ui::paste_into(target);
        } else {
            crate::log("emoji: the clipboard stayed busy");
        }
    }
}

unsafe fn popup(h: HWND) -> Option<&'static mut Popup> {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Popup;
    (!p.is_null()).then(|| &mut *p)
}

impl Popup {
    unsafe fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        self.filt = if q.is_empty() {
            (0..EMOJI.len()).filter(|i| EMOJI[*i].2 == self.cat).collect()
        } else {
            (0..EMOJI.len()).filter(|i| EMOJI[*i].1.contains(&q)).collect()
        };
        if self.sel >= self.filt.len() {
            self.sel = self.filt.len().saturating_sub(1);
        }
        self.hover = -1;
        self.clamp_scroll();
    }

    unsafe fn grid(&self, m: &ui::Metrics) -> (RECT, usize, usize) {
        let tabs = self.tabs(m);
        let cell = ui::sc(self.hwnd, CELL).max(1);
        let grid = RECT { left: m.body.left, top: tabs.bottom + ui::sc(self.hwnd, 4), right: m.body.right, bottom: m.footer.top - ui::sc(self.hwnd, 6) };
        let cols = ((grid.right - grid.left) / cell).max(1) as usize;
        let rows = ((grid.bottom - grid.top) / cell).max(1) as usize;
        (grid, cols, rows)
    }

    unsafe fn tabs(&self, m: &ui::Metrics) -> RECT {
        RECT { left: m.body.left, top: m.body.top, right: m.body.right, bottom: m.body.top + ui::sc(self.hwnd, 30) }
    }

    unsafe fn tab_rect(&self, m: &ui::Metrics, i: usize) -> RECT {
        let t = self.tabs(m);
        let w = (t.right - t.left) / 5;
        RECT { left: t.left + i as i32 * w, top: t.top, right: t.left + (i as i32 + 1) * w, bottom: t.bottom }
    }

    unsafe fn clamp_scroll(&mut self) {
        let m = ui::metrics(self.hwnd);
        let (_, cols, rows) = self.grid(&m);
        let max = self.filt.len().saturating_sub(cols * rows);
        self.scroll = (self.scroll.min(max) / cols.max(1)) * cols.max(1);
    }

    unsafe fn ensure_visible(&mut self) {
        let m = ui::metrics(self.hwnd);
        let (_, cols, rows) = self.grid(&m);
        let vis = cols * rows;
        if self.sel < self.scroll {
            self.scroll = self.sel / cols * cols;
        } else if self.sel >= self.scroll + vis {
            self.scroll = (self.sel / cols + 1).saturating_sub(rows) * cols;
        }
        self.clamp_scroll();
    }

    unsafe fn insert_selected(&mut self) {
        if let Some(&i) = self.filt.get(self.sel) {
            PASTE_TEXT.with(|c| *c.borrow_mut() = Some(EMOJI[i].0.to_string()));
        }
        DestroyWindow(self.hwnd);
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
        if mem.is_null() || bmp.is_null() {
            if !mem.is_null() {
                DeleteDC(mem);
            }
            if !bmp.is_null() {
                DeleteObject(bmp);
            }
            EndPaint(h, &ps);
            return;
        }
        let old = SelectObject(mem, bmp);
        let p = pal();
        ui::surface(mem, &self.px, cr);
        let m = ui::metrics(h);
        ui::title(mem, &m, t!("Эмодзи", "Emoji", ja: "絵文字", pt: "Emoji", zh: "表情符号"), self.f_title);
        ui::search(
            mem,
            h,
            m.search,
            &self.query,
            t!("Поиск по названию", "Search by name", ja: "名前で検索", pt: "Pesquisar por nome", zh: "按名称搜索"),
            self.f_text,
            self.f_icon,
            self.caret,
        );

        let tab_names = [
            t!("Лица", "Faces", ja: "顔", pt: "Rostos", zh: "表情"),
            t!("Жесты", "Gestures", ja: "ジェスチャー", pt: "Gestos", zh: "手势"),
            t!("Еда", "Food", ja: "食べ物", pt: "Comida", zh: "食物"),
            t!("Предметы", "Objects", ja: "もの", pt: "Objetos", zh: "物品"),
            t!("Символы", "Symbols", ja: "記号", pt: "Símbolos", zh: "符号"),
        ];
        for (i, name) in tab_names.iter().enumerate() {
            let r = self.tab_rect(&m, i);
            let on = self.query.is_empty() && i as u8 == self.cat;
            ui::text(mem, name, r, self.f_tab, if on { p.accent } else { p.dim }, DT_CENTER);
            if on {
                let y = r.bottom - ui::sc(h, 2);
                ui::line(mem, r.left + ui::sc(h, 14), y, r.right - ui::sc(h, 14), y, p.accent);
            }
        }

        let (grid, cols, rows) = self.grid(&m);
        let cell = ui::sc(h, CELL).max(1);
        let vis = cols * rows;
        self.scroll = self.scroll.min(self.filt.len().saturating_sub(vis));
        let x0 = grid.left + ((grid.right - grid.left) - cols as i32 * cell) / 2;
        if self.filt.is_empty() {
            let cy = grid.top + (grid.bottom - grid.top) / 2;
            ui::text(mem, "\u{E76E}", RECT { left: grid.left, top: cy - ui::sc(h, 46), right: grid.right, bottom: cy - ui::sc(h, 4) }, self.f_big, p.dim, DT_CENTER);
            ui::text(
                mem,
                &t!("Ничего не найдено", "No emoji found", ja: "絵文字が見つかりません", pt: "Nenhum emoji encontrado", zh: "未找到表情符号"),
                RECT { left: grid.left, top: cy, right: grid.right, bottom: cy + ui::sc(h, 26) },
                self.f_text,
                p.text,
                DT_CENTER,
            );
        } else {
            let last = (self.scroll + vis).min(self.filt.len());
            for i in self.scroll..last {
                let slot = i - self.scroll;
                let r = RECT {
                    left: x0 + (slot % cols) as i32 * cell,
                    top: grid.top + (slot / cols) as i32 * cell,
                    right: x0 + (slot % cols) as i32 * cell + cell,
                    bottom: grid.top + (slot / cols) as i32 * cell + cell,
                };
                let box_ = RECT { left: r.left + ui::sc(h, 3), top: r.top + ui::sc(h, 3), right: r.right - ui::sc(h, 3), bottom: r.bottom - ui::sc(h, 3) };
                if i == self.sel {
                    ui::rounded(mem, box_, mix(p.bg, p.accent, 0.22), ui::sc(h, 8));
                } else if self.hover == i as i32 {
                    ui::rounded(mem, box_, p.hover, ui::sc(h, 8));
                }
                ui::text(mem, EMOJI[self.filt[i]].0, r, self.f_emoji, p.text, DT_CENTER);
            }
            ui::scrollbar(mem, h, grid, self.filt.len(), vis, self.scroll);
        }

        ui::footer_sep(mem, &m, h);
        let hint = self.filt.get(self.sel).map_or_else(String::new, |i| EMOJI[*i].1.to_string());
        ui::text(mem, &hint, m.footer, self.f_small, p.dim, DT_END_ELLIPSIS);
        ui::text(mem, "Enter · Esc", m.footer, self.f_small, p.dim, DT_RIGHT);

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
                let (grid, cols, _) = p.grid(&m);
                let cell = ui::sc(h, CELL).max(1);
                let x0 = grid.left + ((grid.right - grid.left) - cols as i32 * cell) / 2;
                let mut hover = -1i32;
                if y >= grid.top && y < grid.bottom && x >= x0 && x < x0 + cols as i32 * cell {
                    let slot = ((y - grid.top) / cell) as usize * cols + ((x - x0) / cell) as usize;
                    let i = p.scroll + slot;
                    if i < p.filt.len() {
                        hover = i as i32;
                    }
                }
                if hover != p.hover {
                    p.hover = hover;
                    InvalidateRect(h, null_mut(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            if let Some(p) = popup(h) {
                let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
                let m = ui::metrics(h);
                let tabs = p.tabs(&m);
                if y >= tabs.top && y < tabs.bottom {
                    let w = ((tabs.right - tabs.left) / 5).max(1);
                    let i = ((x - tabs.left) / w).clamp(0, 4) as u8;
                    p.cat = i;
                    p.query.clear();
                    p.sel = 0;
                    p.scroll = 0;
                    p.refilter();
                    InvalidateRect(h, null_mut(), 0);
                    return 0;
                }
                let (grid, cols, _) = p.grid(&m);
                let cell = ui::sc(h, CELL).max(1);
                let x0 = grid.left + ((grid.right - grid.left) - cols as i32 * cell) / 2;
                if y >= grid.top && y < grid.bottom && x >= x0 && x < x0 + cols as i32 * cell {
                    let slot = ((y - grid.top) / cell) as usize * cols + ((x - x0) / cell) as usize;
                    let i = p.scroll + slot;
                    if i < p.filt.len() {
                        p.sel = i;
                        p.insert_selected();
                    }
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            if let Some(p) = popup(h) {
                let delta = ((wp >> 16) & 0xffff) as i16 as i32;
                let m = ui::metrics(h);
                let (_, cols, _) = p.grid(&m);
                let step = cols.max(1);
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
                if let Some(c) = char::from_u32(wp as u32) {
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
                let m = ui::metrics(h);
                let (_, cols, rows) = p.grid(&m);
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
                    VK_LEFT => {
                        if p.sel > 0 {
                            p.sel -= 1;
                            p.ensure_visible();
                            InvalidateRect(h, null_mut(), 0);
                        }
                    }
                    VK_RIGHT => {
                        if p.sel + 1 < p.filt.len() {
                            p.sel += 1;
                            p.ensure_visible();
                            InvalidateRect(h, null_mut(), 0);
                        }
                    }
                    VK_UP => {
                        p.sel = p.sel.saturating_sub(cols.max(1));
                        p.ensure_visible();
                        InvalidateRect(h, null_mut(), 0);
                    }
                    VK_DOWN => {
                        p.sel = (p.sel + cols.max(1)).min(p.filt.len().saturating_sub(1));
                        p.ensure_visible();
                        InvalidateRect(h, null_mut(), 0);
                    }
                    VK_PRIOR => {
                        p.sel = p.sel.saturating_sub(cols.max(1) * rows.max(1));
                        p.ensure_visible();
                        InvalidateRect(h, null_mut(), 0);
                    }
                    VK_NEXT => {
                        p.sel = (p.sel + cols.max(1) * rows.max(1)).min(p.filt.len().saturating_sub(1));
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
                        p.insert_selected();
                    }
                    _ => {}
                }
            }
            0
        }
        WM_ACTIVATE => {
            if let Some(p) = popup(h) {
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
                for f in [p.f_text, p.f_small, p.f_title, p.f_icon, p.f_emoji, p.f_tab, p.f_big] {
                    if !f.is_null() {
                        DeleteObject(f as HGDIOBJ);
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
