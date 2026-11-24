#![windows_subsystem = "windows"]
// wiztree [X:] [--clean]  Disk usage treemap on the NTFS MFT (the same reader as rs-du, from winrust-common), and a cleaner
// for every drive (clean.rs; the button at the top right switches, --clean opens on it).
// Click a folder: go in. Right click / Backspace: up. Click a file: show it in Explorer. Tab: next drive. F5: rescan. Esc: quit.
// Needs admin (raw volume); starts itself elevated (UAC) when it is not.
// shortcuts: one level of the tree at a time, the 300 biggest items; a folder's own index size is not counted (see common/src/mft.rs).
use winrust_common::{t, tf};
use winrust_common::mft::*;
mod clean;
use std::{cell::RefCell, collections::HashMap, ptr::{null, null_mut}};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives},
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::GetDpiForSystem, Input::KeyboardAndMouse::*, Shell::{IsUserAnAdmin, ShellExecuteW}, WindowsAndMessaging::*},
};

const WM_DONE: u32 = WM_APP + 1; // lparam = Box<Result<Tree, String>>
const DRIVE_FIXED: u32 = 3;
const MAX_CELLS: usize = 300;
const BG: u32 = rgb(22, 22, 24);
const DIM: u32 = rgb(142, 142, 147);
const ACCENT: u32 = rgb(10, 132, 255);

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

struct Tree {
    recs: HashMap<u32, Rec>,
    tot: HashMap<u32, u64>,
    kids: HashMap<u32, Vec<u32>>,
}

fn build(recs: HashMap<u32, Rec>) -> Tree {
    let tot = totals(&recs, false);
    let mut kids: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&id, r) in &recs {
        if id != ROOT && tot.contains_key(&id) {
            kids.entry(r.parent).or_default().push(id);
        }
    }
    for v in kids.values_mut() {
        v.sort_by_key(|id| std::cmp::Reverse(tot[id]));
    }
    Tree { recs, tot, kids }
}

#[derive(Default)]
struct App {
    tree: Option<Tree>,
    drive: char,
    cur: u32,
    status: String,
    cells: Vec<(RECT, u32)>,
    hot: Option<usize>,
    dpi: i32,
    font: isize,
    bold: isize,
    small: isize,
    cleaning: bool, // the cleaner page is up instead of the map
    switch: RECT,   // the title bar button between the two
    switch_hot: bool,
    clean: clean::Clean,
}
thread_local! { static APP: RefCell<App> = RefCell::new(App { drive: 'C', cur: ROOT, dpi: 96, ..Default::default() }) }

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Squarified treemap: `items` (id, size) biggest first, laid out in the rectangle (x, y, w, h).
fn squarify(items: &[(u32, u64)], (mut x, mut y, mut w, mut h): (f64, f64, f64, f64)) -> Vec<(u32, [f64; 4])> {
    let total: f64 = items.iter().map(|i| i.1 as f64).sum();
    let mut out = vec![];
    if total <= 0.0 || w <= 0.0 || h <= 0.0 {
        return out;
    }
    let scale = w * h / total;
    let area = |k: usize| items[k].1 as f64 * scale;
    let mut i = 0;
    while i < items.len() {
        let side = w.min(h);
        let worst = |s: f64, mx: f64, mn: f64| (side * side * mx / (s * s)).max(s * s / (side * side * mn));
        let (mut j, mut s, mut mx, mut mn) = (i + 1, area(i), area(i), area(i));
        let mut cur = worst(s, mx, mn);
        while j < items.len() {
            let a = area(j);
            let next = worst(s + a, mx.max(a), mn.min(a));
            if next > cur {
                break;
            }
            (s, mx, mn, cur, j) = (s + a, mx.max(a), mn.min(a), next, j + 1);
        }
        if w >= h {
            // a column on the left
            let (cw, mut yy) = (s / h, y);
            for k in i..j {
                let ch = area(k) / cw;
                out.push((items[k].0, [x, yy, x + cw, yy + ch]));
                yy += ch;
            }
            x += cw;
            w -= cw;
        } else {
            // a row on top
            let (rh, mut xx) = (s / w, x);
            for k in i..j {
                let cw = area(k) / rh;
                out.push((items[k].0, [xx, y, xx + cw, y + rh]));
                xx += cw;
            }
            y += rh;
            h -= rh;
        }
        i = j;
    }
    out
}

/// Stable colour per extension / folder name (folders darker than files).
fn color(name: &str, dir: bool) -> u32 {
    let key = if dir { name } else { name.rsplit_once('.').map_or("", |e| e.1) };
    let hue = (key.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b.to_ascii_lowercase() as u32)) % 360) as f64;
    let (s, v) = if dir { (0.28, 0.36) } else { (0.42, 0.66) }; // muted: the map reads calm, the labels stay legible
    let f = |n: f64| {
        let k = (n + hue / 60.0) % 6.0;
        ((v - v * s * k.min(4.0 - k).clamp(0.0, 1.0)) * 255.0) as u8
    };
    rgb(f(5.0), f(3.0), f(1.0))
}

fn fixed_drives() -> Vec<char> {
    let mask = unsafe { GetLogicalDrives() };
    ('A'..='Z').filter(|c| mask & (1 << (*c as u8 - b'A')) != 0 && unsafe { GetDriveTypeW(wide(&format!("{c}:\\")).as_ptr()) } == DRIVE_FIXED).collect()
}

fn rescan(hwnd: HWND, drive: char) {
    APP.with_borrow_mut(|a| {
        (a.tree, a.drive, a.cur, a.hot, a.cells) = (None, drive, ROOT, None, vec![]);
        a.status = tf!("Чтение MFT диска {drive}: ...", "Reading the MFT of {drive}: ...", ja: "ドライブ {drive}: の MFT を読み込んでいます...", pt: "Lendo a MFT de {drive}: ...", zh: "正在读取 {drive}: 的 MFT…", es: "Leyendo la MFT de {drive}: ...");
    });
    unsafe { InvalidateRect(hwnd, null(), 0) };
    let h = hwnd as isize;
    std::thread::spawn(move || {
        let r: Result<Tree, String> = scan(drive).map(build);
        unsafe { PostMessageW(h as HWND, WM_DONE, 0, Box::into_raw(Box::new(r)) as isize) };
    });
}

/// The window draws its own caption: the path line on top is the title bar, the buttons sit at its right end.
const CAP: winrust_common::caption::Caption = winrust_common::caption::Caption { height: 32, drag: 44, buttons: winrust_common::caption::MIN | winrust_common::caption::MAX | winrust_common::caption::CLOSE };

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, w, h);
        let old = SelectObject(mem, bmp);
        APP.with_borrow_mut(|a| {
            let k = a.dpi as f64 / 96.0;
            let (top, bot) = ((44.0 * k) as i32, (30.0 * k) as i32);
            let fill = |r: &RECT, c: u32| {
                let b = CreateSolidBrush(c);
                FillRect(mem, r, b);
                DeleteObject(b as _);
            };
            fill(&rc, BG);
            SelectObject(mem, a.font as _);
            SetBkMode(mem, TRANSPARENT as _);
            let text = |s: &str, mut r: RECT, c: u32, fmt: u32| {
                SetTextColor(mem, c);
                let t = wide(s);
                DrawTextW(mem, t.as_ptr(), -1, &mut r, fmt);
            };
            let one = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX;
            a.cells.clear();
            let mut head = format!("{}:", a.drive);
            let mut foot = a.status.clone();
            if a.cleaning {
                head = t!("Освободить место на всех дисках", "Free up space on every drive", ja: "すべてのドライブの空き領域を増やす", pt: "Liberar espaço em todas as unidades", zh: "释放所有磁盘的空间", es: "Liberar espacio en todos los discos").into();
                foot.clear(); // the page has its own footer
                clean::paint(mem, a, w, h, top);
                SelectObject(mem, a.font as _);
            } else if let Some(t) = &a.tree {
                head = path_of(&t.recs, a.cur, a.drive).0;
                let items: Vec<(u32, u64)> = t.kids.get(&a.cur).map_or(vec![], |v| v.iter().take(MAX_CELLS).map(|&id| (id, t.tot[&id])).collect());
                let pad = 8.0 * k;
                for (id, r) in squarify(&items, (pad, top as f64, w as f64 - 2.0 * pad, (h - top - bot) as f64 - pad)) {
                    a.cells.push((RECT { left: r[0] as i32, top: r[1] as i32, right: r[2] as i32, bottom: r[3] as i32 }, id));
                }
                for (n, (c, id)) in a.cells.iter().enumerate() {
                    let rec = &t.recs[id];
                    let inner = RECT { left: c.left + 1, top: c.top + 1, right: c.right - 1, bottom: c.bottom - 1 };
                    fill(&inner, color(&rec.name, rec.dir));
                    if a.hot == Some(n) {
                        let b = CreateSolidBrush(rgb(255, 255, 255));
                        FrameRect(mem, &inner, b);
                        DeleteObject(b as _);
                    }
                    if inner.right - inner.left > (70.0 * k) as i32 && inner.bottom - inner.top > (34.0 * k) as i32 {
                        let r = RECT { left: inner.left + 6, top: inner.top + 4, right: inner.right - 4, bottom: inner.bottom - 2 };
                        text(&format!("{}\n{}", rec.name, human(t.tot[id])), r, rgb(255, 255, 255), DT_LEFT | DT_TOP | DT_WORDBREAK | DT_END_ELLIPSIS | DT_NOPREFIX);
                    }
                }
                let size = if a.cur == ROOT { t.tot.values().max().copied().unwrap_or(0) } else { t.tot.get(&a.cur).copied().unwrap_or(0) };
                head = format!("{head}    {}", human(size));
                foot = match a.hot.and_then(|n| a.cells.get(n)) {
                    Some((_, id)) => format!("{}    {}", path_of(&t.recs, *id, a.drive).0, human(t.tot[id])),
                    None => t!("Щелчок: открыть папку / показать файл    ПКМ, Backspace: вверх    Tab: следующий диск    F5: обновить", "Click: open folder / show file    Right click, Backspace: up    Tab: next drive    F5: rescan", ja: "クリック: フォルダーを開く / ファイルを表示    右クリック、Backspace: 上へ    Tab: 次のドライブ    F5: 再スキャン", pt: "Clique: abrir pasta / mostrar arquivo    Botão direito, Backspace: subir    Tab: próxima unidade    F5: examinar de novo", zh: "单击：打开文件夹 / 定位文件    右键、Backspace：上一层    Tab：下一个磁盘    F5：重新扫描", es: "Clic: abrir carpeta / mostrar archivo    Clic derecho, Retroceso: subir    Tab: siguiente unidad    F5: volver a analizar").into(),
                };
            }
            // the switch between the map and the cleaner, at the right end of the title bar
            let label = if a.cleaning { t!("Карта диска", "Disk map", ja: "ディスク マップ", pt: "Mapa do disco", zh: "磁盘地图", es: "Mapa del disco") } else { t!("Освободить место", "Free up space", ja: "空き領域を増やす", pt: "Liberar espaço", zh: "释放空间", es: "Liberar espacio") };
            let lw: Vec<u16> = label.encode_utf16().collect();
            let mut sz: SIZE = std::mem::zeroed();
            GetTextExtentPoint32W(mem, lw.as_ptr(), lw.len() as i32, &mut sz);
            let (bw, bh, right) = (sz.cx + (28.0 * k) as i32, (28.0 * k) as i32, w - winrust_common::caption::buttons_width(hwnd, CAP) - (10.0 * k) as i32);
            a.switch = RECT { left: right - bw, top: (top - bh) / 2, right, bottom: (top - bh) / 2 + bh };
            let b = CreateSolidBrush(if a.switch_hot { rgb(62, 62, 68) } else { rgb(44, 44, 48) });
            let (ob, op) = (SelectObject(mem, b as _), SelectObject(mem, GetStockObject(NULL_PEN)));
            RoundRect(mem, a.switch.left, a.switch.top, a.switch.right, a.switch.bottom, (16.0 * k) as i32, (16.0 * k) as i32);
            SelectObject(mem, ob);
            SelectObject(mem, op);
            DeleteObject(b as _);
            text(label, a.switch, rgb(235, 235, 235), DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            text(&head, RECT { left: (12.0 * k) as i32, top: 0, right: a.switch.left - 12, bottom: top }, ACCENT, one);
            if !foot.is_empty() {
                text(&foot, RECT { left: (12.0 * k) as i32, top: h - bot, right: w - 12, bottom: h }, DIM, one);
            }
        });
        winrust_common::caption::paint_buttons(mem, hwnd, CAP, BG, rgb(235, 235, 235));
        BitBlt(dc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp as _);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

/// Map <-> cleaner. Each page reads the disks the first time it is shown.
fn switch(hwnd: HWND) {
    let (cleaning, map_ready, cleaner_ready) = APP.with_borrow_mut(|a| {
        (a.cleaning, a.hot, a.switch_hot) = (!a.cleaning, None, false);
        (a.cleaning, a.tree.is_some() || !a.status.is_empty(), a.clean.started())
    });
    if cleaning && !cleaner_ready {
        clean::start(hwnd);
    } else if !cleaning && !map_ready {
        rescan(hwnd, APP.with_borrow(|a| a.drive));
    }
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

fn up(hwnd: HWND) {
    APP.with_borrow_mut(|a| {
        if let (Some(t), true) = (&a.tree, a.cur != ROOT) {
            a.cur = t.recs.get(&a.cur).map_or(ROOT, |r| r.parent);
            a.hot = None;
        }
    });
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if let Some(r) = winrust_common::caption::handle(hwnd, msg, wp, lp, CAP, |_, _| false) {
            return r;
        }
        match msg {
            WM_CREATE => {
                let dpi = GetDpiForSystem() as i32;
                let face = wide(winrust_common::style::sans_face());
                let font = |px: i32, weight: i32| CreateFontW(-(px * dpi / 96), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr()) as isize;
                APP.with_borrow_mut(|a| (a.dpi, a.font, a.bold, a.small) = (dpi, font(15, 400), font(15, 600), font(13, 400)));
                0
            }
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_ERASEBKGND => 1,
            clean::WM_STEP | clean::WM_PLAN | clean::WM_CLEANED => {
                clean::on_msg(hwnd, msg, lp);
                0
            }
            WM_DONE => {
                match *Box::from_raw(lp as *mut Result<Tree, String>) {
                    Ok(t) => APP.with_borrow_mut(|a| {
                        a.status = String::new();
                        a.tree = Some(t);
                    }),
                    Err(e) => APP.with_borrow_mut(|a| a.status = e),
                }
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_MOUSEMOVE => {
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, (lp >> 16) as i16 as i32);
                let on_switch = APP.with_borrow_mut(|a| {
                    let s = a.switch;
                    let on = x >= s.left && x < s.right && y >= s.top && y < s.bottom;
                    std::mem::replace(&mut a.switch_hot, on) != on
                });
                let page = if APP.with_borrow(|a| a.cleaning) {
                    clean::hover(x, y)
                } else {
                    APP.with_borrow_mut(|a| {
                        let n = a.cells.iter().position(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom);
                        std::mem::replace(&mut a.hot, n) != n
                    })
                };
                let changed = on_switch || page;
                if changed {
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_LBUTTONDOWN => {
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, (lp >> 16) as i16 as i32);
                let (on_switch, cleaning) = APP.with_borrow(|a| (a.switch_hot, a.cleaning));
                if on_switch {
                    switch(hwnd);
                    return 0;
                }
                if cleaning {
                    clean::click(hwnd, x, y);
                    return 0;
                }
                let act = APP.with_borrow_mut(|a| {
                    let id = a.hot.and_then(|n| a.cells.get(n))?.1;
                    let t = a.tree.as_ref()?;
                    if t.recs[&id].dir {
                        let p = None;
                        (a.cur, a.hot) = (id, None);
                        p
                    } else {
                        Some(path_of(&t.recs, id, a.drive).0)
                    }
                });
                if let Some(p) = act {
                    // show the file in Explorer (delete it from there)
                    winrust_common::reveal(&p);
                }
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_RBUTTONDOWN => {
                if APP.with_borrow(|a| a.cleaning) {
                    clean::right_click((lp & 0xFFFF) as i16 as i32, (lp >> 16) as i16 as i32);
                } else {
                    up(hwnd);
                }
                0
            }
            WM_MOUSEWHEEL => {
                if APP.with_borrow(|a| a.cleaning) {
                    clean::wheel(hwnd, wp);
                }
                0
            }
            WM_KEYDOWN => {
                let cleaning = APP.with_borrow(|a| a.cleaning);
                match wp as u16 {
                    VK_ESCAPE => {
                        DestroyWindow(hwnd);
                    }
                    VK_F5 if cleaning => {
                        if !APP.with_borrow(|a| a.clean.busy()) {
                            clean::start(hwnd);
                        }
                    }
                    _ if cleaning => {}
                    VK_BACK => up(hwnd),
                    VK_F5 => rescan(hwnd, APP.with_borrow(|a| a.drive)),
                    VK_TAB => {
                        let (d, all) = (APP.with_borrow(|a| a.drive), fixed_drives());
                        if let Some(n) = all.iter().position(|&c| c == d) {
                            rescan(hwnd, all[(n + 1) % all.len()]);
                        }
                    }
                    _ => {}
                }
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// The token's elevation type: 3 is the split token Explorer gives, which a UAC request raises. Anything else without admin
/// rights cannot be raised from here: a standard user (1), or the "basic user" token `runas /trustlevel` derives from an
/// elevated one (WinRust starts the bar, the search and what they open with it), which still says 2 (full). From that one
/// a UAC request quietly starts another copy without rights.
fn elevation() -> i32 {
    use windows_sys::Win32::{Security::{GetTokenInformation, TOKEN_QUERY, TokenElevationType}, System::Threading::{GetCurrentProcess, OpenProcessToken}};
    unsafe {
        let (mut tok, mut kind, mut n) = (null_mut(), 0i32, 0u32);
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
            return 1;
        }
        let ok = GetTokenInformation(tok, TokenElevationType, (&mut kind as *mut i32).cast(), 4, &mut n) != 0;
        CloseHandle(tok);
        if ok { kind } else { 1 }
    }
}

/// Explorer is the shell (not WinRust's file manager): it can start a program with its own token.
fn explorer_is_shell() -> bool {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(GetShellWindow(), &mut pid);
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if p.is_null() {
            return false;
        }
        let (mut b, mut len) = ([0u16; 520], 520u32);
        let ok = QueryFullProcessImageNameW(p, 0, b.as_mut_ptr(), &mut len) != 0;
        CloseHandle(p);
        ok && String::from_utf16_lossy(&b[..len as usize]).to_lowercase().ends_with("\\explorer.exe")
    }
}

/// Explorer starts programs without arguments: the copy it starts finds them here (written a moment before).
fn note_path() -> Option<std::path::PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|l| std::path::PathBuf::from(l).join(r"WinRust\disk-start.txt"))
}

fn take_note() -> Option<Vec<String>> {
    let p = note_path()?;
    let fresh = p.metadata().ok()?.modified().ok()?.elapsed().is_ok_and(|e| e.as_secs() < 30);
    let text = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    fresh.then(|| text.split_whitespace().map(String::from).collect())
}

fn main() {
    unsafe {
        let mut args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() {
            args = take_note().unwrap_or_default();
        }
        let has = |f: &str| args.iter().any(|a| a == f);
        let drive = args.iter().find(|a| a.len() == 2 && a.ends_with(':')).and_then(|a| a.chars().next()).filter(char::is_ascii_alphabetic).map_or('C', |c| c.to_ascii_uppercase());
        let cleaning = has("--clean");
        if IsUserAnAdmin() == 0 {
            // raw volume access needs admin. Asked for once at most: on a PC that elevates without a prompt, a copy that
            // could not be raised started the next one forever (the cursor spun, nothing opened)
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let mode = format!("{drive}:{}", if cleaning { " --clean" } else { "" });
            if has("--elevated") {
                let msg = t!("WinRust Disk нужны права администратора, чтобы читать диски, а Windows их не дала. Откройте его из меню «Пуск».", "WinRust Disk needs administrator rights to read the drives, and Windows did not give them. Open it from the Start menu.", ja: "WinRust Disk はドライブを読み取るために管理者権限が必要ですが、Windows から付与されませんでした。スタート メニューから開いてください。", pt: "O WinRust Disk precisa de direitos de administrador para ler as unidades, e o Windows não os concedeu. Abra-o pelo menu Iniciar.", zh: "WinRust Disk 需要管理员权限才能读取磁盘，但 Windows 没有授予。请从开始菜单打开它。", es: "WinRust Disk necesita permisos de administrador para leer los discos y Windows no se los dio. Ábrelo desde el menú Inicio.");
                MessageBoxW(null_mut(), wide(msg).as_ptr(), wide("WinRust Disk").as_ptr(), MB_ICONWARNING);
                return;
            }
            if elevation() != 3 && !has("--via-shell") && explorer_is_shell() && let Some(note) = note_path() {
                // a token UAC cannot raise: Explorer starts a copy with its own, and that one asks
                let _ = note.parent().map(std::fs::create_dir_all);
                if std::fs::write(&note, format!("{mode} --via-shell")).is_ok() && std::process::Command::new("explorer.exe").arg(&exe).spawn().is_ok() {
                    return;
                }
            }
            ShellExecuteW(null_mut(), wide("runas").as_ptr(), wide(&exe).as_ptr(), wide(&format!("{mode} --elevated")).as_ptr(), null(), SW_SHOWNORMAL);
            return;
        }
        SetProcessDPIAware();
        let (inst, class) = (GetModuleHandleW(null()), wide("WinRustDisk"));
        let wc = WNDCLASSW { style: 0, lpfnWndProc: Some(proc), cbClsExtra: 0, cbWndExtra: 0, hInstance: inst, hIcon: null_mut(), hCursor: LoadCursorW(null_mut(), IDC_ARROW), hbrBackground: null_mut(), lpszMenuName: null(), lpszClassName: class.as_ptr() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(0, class.as_ptr(), wide("WinRust Disk").as_ptr(), WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 1280, 800, null_mut(), null_mut(), inst, null());
        winrust_common::caption::install(hwnd);
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(hwnd, 20, &1i32 as *const i32 as *const _, 4); // dark frame and window menu
        APP.with_borrow_mut(|a| (a.drive, a.cleaning) = (drive, cleaning));
        if cleaning {
            clean::start(hwnd);
        } else {
            rescan(hwnd, drive);
        }
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treemap_fills_the_rect_and_keeps_size_ratios() {
        let items = [(1, 600), (2, 300), (3, 200), (4, 100), (5, 100)];
        let cells = squarify(&items, (0.0, 0.0, 400.0, 300.0));
        assert_eq!(cells.len(), 5);
        let a = |id: u32| {
            let r = cells.iter().find(|c| c.0 == id).unwrap().1;
            (r[2] - r[0]) * (r[3] - r[1])
        };
        let total: f64 = (1..=5).map(a).sum();
        assert!((total - 120_000.0).abs() < 1.0, "area {total}");
        assert!((a(1) / a(2) - 2.0).abs() < 1e-6);
    }
}
