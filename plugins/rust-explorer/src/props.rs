//! Our own **Properties** window.
//!
//! Why it exists: without explorer.exe the shell's `properties` verb answers "No application is associated with this
//! file" and `SHObjectProperties` returns TRUE without showing anything, because the property sheet is hosted by
//! Explorer itself. So the window, its rows and the folder-size walk are ours.
//!
//! The look follows the file window: the palette and the painted surfaces of `ui.rs`, the frost capture of the menus
//! (`common::frost`) and a rounded `WS_POPUP` owned by the explorer window, like the "Open with" picker (`openwith.rs`).
//! It is a tool window, so it never shows in the taskbar, and the title row is a drag handle (`HTCAPTION`).
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    mem::{size_of, zeroed},
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::*,
    Storage::FileSystem::SetFileAttributesW,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{
            SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_RETURN,
        },
        Shell::{
            SHGetStockIconInfo, SHGSI_ICON, SHGSI_LARGEICON, SHSTOCKICONINFO, SIID_DRIVECD,
            SIID_DRIVEFIXED, SIID_DRIVENET, SIID_DRIVEREMOVE, SIID_DRIVEUNKNOWN,
        },
        WindowsAndMessaging::*,
    },
};
use winrust_common::{t, tf};

use crate::ui::{self, sc};

const CLASS: &str = "WinRustProps";
const WIN_W: i32 = 420;
const WIN_H: i32 = 480;
const PARA: i32 = 20;
const TITLE_H: i32 = 38;
const HEAD_ICON: i32 = 48;
const CARD_TOP: i32 = 110;
const ROW_H: i32 = 34;
const BTN_W: i32 = 92;
const BTN_H: i32 = 32;
const WM_MOUSELEAVE: u32 = 0x02A3; // not exported under the enabled features
/// The folder-size worker: wp = total bytes.
const WM_PROPS_SIZE: u32 = WM_APP + 14;
const ATTR_READONLY: u32 = 0x1;
const ATTR_HIDDEN: u32 = 0x2;
const ATTR_SYSTEM: u32 = 0x4;
const ATTR_ARCHIVE: u32 = 0x20;
const ATTR_REPARSE: u32 = 0x400;

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    None,
    Close,
    Ok,
    ReadOnly,
    Hidden,
}

struct DriveInfo {
    label: String,
    fs: String,
    total: u64,
    free: u64,
}

struct TrashInfo {
    original: PathBuf,
    deleted: u64,
    size: u64,
}

struct Dlg {
    path: PathBuf,
    name: String,
    kind: String,
    location: String,
    folder: bool,
    size: u64,
    size_known: bool,
    created: u64,
    modified: u64,
    accessed: u64,
    attrs: u32,
    can_attr: bool,
    ro: bool,
    hidden: bool,
    drive: Option<DriveInfo>,
    trash: Option<TrashInfo>,
    icon: HICON,
    own_icon: bool,
    done: bool,
    hover: Hit,
    tracking: bool,
    px: Vec<u32>,
    fonts: [HFONT; 4], // heading, body, small, glyph
    close_btn: RECT,
    ok_btn: RECT,
    ro_row: RECT,
    hid_row: RECT,
    scan: Option<PathBuf>,
    /// Set when the window closes: a big folder walk stops at the next entry instead of finishing for nobody.
    stop: Arc<AtomicBool>,
    error: Option<String>,
}

impl Drop for Dlg {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for f in self.fonts {
            if !f.is_null() {
                unsafe { DeleteObject(f) };
            }
        }
        // `best` icons belong to the shared cache; only the stock drive icon is ours.
        if self.own_icon && !self.icon.is_null() {
            unsafe { DestroyIcon(self.icon) };
        }
    }
}

/// A copy of what the list already knows about a row (the folder's rows are not borrowed while the window runs).
struct Snap {
    name: String,
    dir: bool,
    size: u64,
    time: u64,
    trash: Option<crate::Trash>,
}

fn snapshot(path: &Path) -> Option<Snap> {
    let want = path.to_string_lossy();
    crate::ST.with_borrow(|st| {
        st.rows
            .iter()
            .find(|r| !r.hdr && r.full(&st.dir).to_string_lossy().eq_ignore_ascii_case(&want))
            .map(|r| Snap {
                name: r.name.clone(),
                dir: r.dir,
                size: r.size,
                time: r.time,
                trash: r.trash.clone(),
            })
    })
}

/// `C:\` (not `C:`): the only paths that are drives, not folders.
fn drive_root(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    b.len() == 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

fn stock_icon(ty: u32) -> HICON {
    let id = match ty {
        2 => SIID_DRIVEREMOVE,
        3 => SIID_DRIVEFIXED,
        4 => SIID_DRIVENET,
        5 => SIID_DRIVECD,
        _ => SIID_DRIVEUNKNOWN,
    };
    let mut si: SHSTOCKICONINFO = unsafe { zeroed() };
    si.cbSize = size_of::<SHSTOCKICONINFO>() as u32;
    if unsafe { SHGetStockIconInfo(id, SHGSI_ICON | SHGSI_LARGEICON, &mut si) } != 0 {
        return ptr::null_mut();
    }
    si.hIcon
}

fn bytes_line(n: u64) -> String {
    if n == 0 {
        return t!("0 байт", "0 bytes", ja: "0 バイト", pt: "0 bytes", zh: "0 字节").to_string();
    }
    format!("{} ({})", crate::size_text(n), tf!("{} байт", "{} bytes", ja: "{} バイト", pt: "{} bytes", zh: "{} 字节", n))
}

fn dash(s: String) -> String {
    if s.is_empty() {
        "—".to_string()
    } else {
        s
    }
}

fn attrs_text(a: u32) -> String {
    let mut v = vec![];
    if a & ATTR_READONLY != 0 {
        v.push(t!("Только чтение", "Read-only", ja: "読み取り専用", pt: "Somente leitura", zh: "只读"));
    }
    if a & ATTR_HIDDEN != 0 {
        v.push(t!("Скрытый", "Hidden", ja: "隠し", pt: "Oculto", zh: "隐藏"));
    }
    if a & ATTR_SYSTEM != 0 {
        v.push(t!("Системный", "System", ja: "システム", pt: "Sistema", zh: "系统"));
    }
    if a & ATTR_ARCHIVE != 0 {
        v.push(t!("Архивный", "Archive", ja: "アーカイブ", pt: "Arquivo", zh: "存档"));
    }
    if v.is_empty() {
        "—".to_string()
    } else {
        v.join(", ")
    }
}

/// Iterative and bounded by `stop`: never follows junctions (a reparse loop would make the walk endless).
fn folder_size(root: &Path, stop: &AtomicBool) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    let mut seen = 0u32;
    while let Some(dir) = stack.pop() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if stop.load(Ordering::Relaxed) {
                return total;
            }
            seen = seen.wrapping_add(1);
            if seen & 0x1FF == 0 && stop.load(Ordering::Relaxed) {
                return total;
            }
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.is_dir() {
                if m.file_attributes() & ATTR_REPARSE == 0 {
                    stack.push(e.path());
                }
            } else {
                total = total.saturating_add(m.len());
            }
        }
    }
    total
}

unsafe fn build(path: &Path) -> Dlg {
    let snap = snapshot(path);
    let mut d = Dlg {
        path: path.to_path_buf(),
        name: String::new(),
        kind: String::new(),
        location: String::new(),
        folder: false,
        size: 0,
        size_known: true,
        created: 0,
        modified: 0,
        accessed: 0,
        attrs: 0,
        can_attr: false,
        ro: false,
        hidden: false,
        drive: None,
        trash: None,
        icon: ptr::null_mut(),
        own_icon: false,
        done: false,
        hover: Hit::None,
        tracking: false,
        px: Vec::new(),
        fonts: [
            ui::font(15, 600, ui::ui_face()) as HFONT,
            ui::font(13, 400, ui::ui_face()) as HFONT,
            ui::font(11, 400, ui::ui_face()) as HFONT,
            ui::font(14, 400, winrust_common::style::icon_face()) as HFONT,
        ],
        close_btn: zeroed(),
        ok_btn: zeroed(),
        ro_row: zeroed(),
        hid_row: zeroed(),
        scan: None,
        stop: Arc::new(AtomicBool::new(false)),
        error: None,
    };

    // a drive: only the cache is asked (a dead volume must not stall the window); no metadata / attr checks at all
    if drive_root(path) {
        let root = path.to_string_lossy();
        let cached = crate::drives::cached().into_iter().find(|x| x.root.eq_ignore_ascii_case(&root));
        let (ty, label, fs, total, free) = match &cached {
            Some(x) => (x.ty, x.label.clone(), x.fs.clone(), x.total, x.free),
            None => (0, String::new(), String::new(), 0, 0),
        };
        d.kind = crate::drive_kind(ty);
        d.name = match &cached {
            Some(x) => format!("{} ({}:)", if x.label.is_empty() { crate::drive_kind(x.ty) } else { x.label.clone() }, x.letter),
            None => root.clone().into_owned(),
        };
        d.location = root.clone().into_owned();
        d.icon = stock_icon(ty);
        d.own_icon = !d.icon.is_null();
        d.drive = Some(DriveInfo { label, fs, total, free });
        return d;
    }

    let meta = std::fs::metadata(path).ok();
    let is_trash = meta.is_none() && snap.as_ref().is_some_and(|s| s.trash.is_some());
    if is_trash {
        let s = snap.as_ref().unwrap();
        let t = s.trash.as_ref().unwrap(); // `is_trash` checked it
        d.name = s.name.clone();
        d.kind = crate::type_text(&d.name, s.dir);
        d.location = path.to_string_lossy().into_owned();
        d.size = s.size;
        d.icon = winrust_common::icon::best(&t.r, sc(HEAD_ICON));
        d.trash = Some(TrashInfo { original: path.to_path_buf(), deleted: s.time, size: s.size });
        return d;
    }

    d.name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    let dir = meta.as_ref().is_some_and(|m| m.is_dir());
    d.folder = dir;
    d.kind = crate::type_text(&d.name, dir);
    d.location = path
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    d.size = meta.as_ref().map_or_else(|| snap.as_ref().map_or(0, |s| s.size), |m| m.len());
    d.size_known = !dir || meta.is_none();
    d.created = meta.as_ref().map_or(0, |m| m.creation_time());
    d.modified = meta.as_ref().map_or_else(|| snap.as_ref().map_or(0, |s| s.time), |m| m.last_write_time());
    d.accessed = meta.as_ref().map_or(0, |m| m.last_access_time());
    d.attrs = meta.as_ref().map_or(0, |m| m.file_attributes());
    d.can_attr = meta.is_some();
    d.ro = d.attrs & ATTR_READONLY != 0;
    d.hidden = d.attrs & ATTR_HIDDEN != 0;
    if dir && meta.is_some() {
        d.scan = Some(path.to_path_buf()); // "Calculating…" until the worker posts the total
    }
    d.icon = winrust_common::icon::best(path, sc(HEAD_ICON));
    d
}

fn debug(s: &str) {
    use std::io::Write;
    let Some(tmp) = std::env::var_os("TEMP") else { return };
    let p = PathBuf::from(tmp).join("opencode").join("props-log.txt");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
        let _ = writeln!(f, "{s}");
    }
}

fn inside(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT { left: l, top: t, right: r, bottom: b }
}

fn reflow(d: &mut Dlg) {
    let w = sc(WIN_W);
    let by = sc(WIN_H) - sc(20) - sc(BTN_H);
    d.ok_btn = rect(w - sc(PARA) - sc(BTN_W), by, w - sc(PARA), by + sc(BTN_H));
    d.close_btn = rect(w - sc(PARA) - sc(30), sc(6), w - sc(PARA), sc(32));
    let cy = by - sc(42);
    let half = (w - 2 * sc(PARA)) / 2;
    d.ro_row = rect(sc(PARA), cy, sc(PARA) + half - sc(6), cy + sc(24));
    d.hid_row = rect(sc(PARA) + half + sc(6), cy, w - sc(PARA), cy + sc(24));
}

fn row_count(d: &Dlg) -> i32 {
    if d.drive.is_some() {
        4
    } else if d.trash.is_some() {
        5
    } else {
        7
    }
}

fn rows(d: &Dlg) -> Vec<(String, String)> {
    if let Some(dr) = &d.drive {
        return vec![
            (t!("Тип", "Type", ja: "種類", pt: "Tipo", zh: "类型").to_string(), d.kind.clone()),
            (t!("Метка", "Label", ja: "ラベル", pt: "Rótulo", zh: "标签").to_string(), dash(dr.label.clone())),
            (t!("Файловая система", "File system", ja: "ファイルシステム", pt: "Sistema de arquivos", zh: "文件系统").to_string(), dash(dr.fs.clone())),
            (t!("Расположение", "Location", ja: "場所", pt: "Local", zh: "位置").to_string(), d.location.clone()),
        ];
    }
    if let Some(tr) = &d.trash {
        return vec![
            (t!("Тип", "Type", ja: "種類", pt: "Tipo", zh: "类型").to_string(), d.kind.clone()),
            (t!("Исходное расположение", "Original location", ja: "元の場所", pt: "Local original", zh: "原始位置").to_string(), dash(tr.original.to_string_lossy().into_owned())),
            (t!("Дата удаления", "Date deleted", ja: "削除日時", pt: "Data de exclusão", zh: "删除日期").to_string(), dash(crate::date_text(tr.deleted))),
            (t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小").to_string(), bytes_line(tr.size)),
            (t!("Состояние", "Status", ja: "状態", pt: "Status", zh: "状态").to_string(), t!("В Корзине", "In the Recycle Bin", ja: "ごみ箱内", pt: "Na Lixeira", zh: "在回收站中").to_string()),
        ];
    }
    let size = if d.folder && !d.size_known {
        t!("Вычисление…", "Calculating…", ja: "計算中…", pt: "Calculando…", zh: "正在计算…").to_string()
    } else {
        bytes_line(d.size)
    };
    vec![
        (t!("Тип", "Type", ja: "種類", pt: "Tipo", zh: "类型").to_string(), d.kind.clone()),
        (t!("Расположение", "Location", ja: "場所", pt: "Local", zh: "位置").to_string(), d.location.clone()),
        (t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小").to_string(), size),
        (t!("Создан", "Created", ja: "作成日時", pt: "Criado", zh: "创建时间").to_string(), dash(crate::date_text(d.created))),
        (t!("Изменён", "Modified", ja: "更新日時", pt: "Modificado", zh: "修改时间").to_string(), dash(crate::date_text(d.modified))),
        (t!("Открыт", "Accessed", ja: "アクセス日時", pt: "Acessado", zh: "访问时间").to_string(), dash(crate::date_text(d.accessed))),
        (t!("Атрибуты", "Attributes", ja: "属性", pt: "Atributos", zh: "属性").to_string(), attrs_text(d.attrs)),
    ]
}

fn hit(d: &Dlg, x: i32, y: i32) -> Hit {
    if inside(d.close_btn, x, y) {
        return Hit::Close;
    }
    if inside(d.ok_btn, x, y) {
        return Hit::Ok;
    }
    if d.can_attr && inside(d.ro_row, x, y) {
        return Hit::ReadOnly;
    }
    if d.can_attr && inside(d.hid_row, x, y) {
        return Hit::Hidden;
    }
    Hit::None
}

unsafe fn draw(dc: HDC, _d: &Dlg, font: HFONT, s: &str, r: RECT, c: u32, flags: u32) {
    let old = SelectObject(dc, font);
    SetTextColor(dc, c);
    let mut rr = r;
    DrawTextW(dc, crate::w(s).as_ptr(), -1, &mut rr, flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    SelectObject(dc, old);
}

unsafe fn glyph(dc: HDC, d: &Dlg, g: &str, r: RECT, c: u32) {
    draw(dc, d, d.fonts[3], g, r, c, DT_CENTER);
}

/// Applies the two checkboxes; a failure keeps the window open with the reason, so nothing closes silently.
unsafe fn accept(h: HWND, d: &mut Dlg) {
    if d.can_attr {
        let mask = ATTR_READONLY | ATTR_HIDDEN;
        let mut a = d.attrs & !mask;
        if d.ro {
            a |= ATTR_READONLY;
        }
        if d.hidden {
            a |= ATTR_HIDDEN;
        }
        if a != d.attrs {
            let wide = crate::w(&d.path.to_string_lossy());
            if SetFileAttributesW(wide.as_ptr(), a) == 0 {
                let e = std::io::Error::last_os_error();
                d.error = Some(tf!(
                    "Не удалось изменить атрибуты: {}",
                    "Could not change the attributes: {}",
                    ja: "属性を変更できませんでした: {}",
                    pt: "Não foi possível alterar os atributos: {}",
                    zh: "无法更改属性：{}",
                    e
                ));
                InvalidateRect(h, ptr::null(), 0);
                return;
            }
        }
    }
    d.done = true;
}

unsafe fn paint(h: HWND, d: &Dlg) {
    let mut ps: PAINTSTRUCT = zeroed();
    let wdc = BeginPaint(h, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(h, &mut cr);
    let (w, ht) = (cr.right.max(1), cr.bottom.max(1));
    let mem = CreateCompatibleDC(wdc);
    let bmp = if mem.is_null() { ptr::null_mut() } else { CreateCompatibleBitmap(wdc, w, ht) };
    let mut old_bmp = ptr::null_mut();
    let mut buffered = false;
    let dc = if !mem.is_null() && !bmp.is_null() {
        old_bmp = SelectObject(mem, bmp);
        if old_bmp.is_null() {
            wdc
        } else {
            buffered = true;
            mem
        }
    } else {
        wdc
    };
    let p = ui::pal();

    // the same frost recipe as the menus: what was behind, blurred, with the theme over it
    // The property sheet is a dialog, not a menu: the menu tint (67 %) lets bright windows show through and the rows
    // become hard to read. Keep it frosted but dark enough to be legible over any background.
    let alpha = (255 * winrust_common::look::menu_alpha().max(87) as u32 / 100) as u8;
    let mut painted = false;
    if winrust_common::look::menu_blur() && !winrust_common::style::style().light() {
        painted = winrust_common::frost::paint_capture(dc, &d.px, cr, p.bg, alpha);
    }
    if !painted {
        let b = CreateSolidBrush(p.bg);
        FillRect(dc, &cr, b);
        DeleteObject(b);
        winrust_common::style::backdrop(dc, cr, sc(100) as f32 / 100.0);
    }
    SetBkMode(dc, TRANSPARENT as i32);

    // title row: the name of the dialog and the close button
    let title = t!("Свойства", "Properties", ja: "プロパティ", pt: "Propriedades", zh: "属性");
    draw(dc, d, d.fonts[2], title, rect(sc(PARA), 0, w - sc(PARA) - sc(36), sc(TITLE_H)), p.dim, DT_LEFT);
    if d.hover == Hit::Close {
        ui::fill(dc, d.close_btn, ui::mix(p.bg, 0xC42B1C, 0.55), sc(6));
    }
    glyph(dc, d, "\u{E711}", d.close_btn, if d.hover == Hit::Close { 0x00FF_FFFF } else { p.text });
    let line = CreateSolidBrush(p.line);
    FillRect(dc, &rect(0, sc(TITLE_H), w, sc(TITLE_H) + 1), line);
    DeleteObject(line);

    // header: the item's icon and its name
    let iy = sc(52);
    if !d.icon.is_null() {
        winrust_common::icon::draw(dc, d.icon, sc(PARA), iy, sc(HEAD_ICON));
    } else {
        let tile = rect(sc(PARA), iy, sc(PARA) + sc(HEAD_ICON), iy + sc(HEAD_ICON));
        ui::fill(dc, tile, ui::mix(p.bg, p.accent, 0.18), sc(10));
        glyph(dc, d, if d.drive.is_some() { "\u{EDA2}" } else { "\u{E8B7}" }, tile, p.text);
    }
    let name = rect(sc(PARA) + sc(HEAD_ICON) + sc(14), sc(46), w - sc(PARA), sc(46) + sc(52));
    let old = SelectObject(dc, d.fonts[0]);
    SetTextColor(dc, p.text);
    let mut nr = name;
    DrawTextW(dc, crate::w(&d.name).as_ptr(), -1, &mut nr, DT_LEFT | DT_WORDBREAK | DT_NOPREFIX | DT_END_ELLIPSIS);
    SelectObject(dc, old);

    // the information card
    let list = rows(d);
    let n = row_count(d);
    let card = rect(sc(PARA), sc(CARD_TOP) - sc(6), w - sc(PARA), sc(CARD_TOP) + n * sc(ROW_H) + sc(6));
    ui::fill(dc, card, ui::mix(p.bg, p.text, 0.05), sc(10));
    let label_x = sc(PARA) + sc(12);
    let value_x = sc(PARA) + sc(124);
    for (i, (label, value)) in list.iter().enumerate() {
        let top = sc(CARD_TOP) + i as i32 * sc(ROW_H);
        let lr = rect(label_x, top, value_x - sc(8), top + sc(ROW_H));
        let vr = rect(value_x, top, w - sc(PARA) - sc(12), top + sc(ROW_H));
        draw(dc, d, d.fonts[2], label, lr, p.dim, DT_LEFT | DT_END_ELLIPSIS);
        let path_like = value.contains('\\') || value.contains('/');
        draw(dc, d, d.fonts[1], value, vr, p.text, DT_LEFT | if path_like { DT_PATH_ELLIPSIS } else { DT_END_ELLIPSIS });
    }

    // a drive also says how full it is
    if let Some(dr) = &d.drive {
        let top = card.bottom + sc(16);
        let gap = sc(10);
        let cw = (card.right - card.left - 2 * gap) / 3;
        let used = dr.total.saturating_sub(dr.free);
        let stats = [
            (t!("Всего", "Total", ja: "合計", pt: "Total", zh: "总计"), dr.total),
            (t!("Свободно", "Free", ja: "空き", pt: "Livre", zh: "可用"), dr.free),
            (t!("Занято", "Used", ja: "使用中", pt: "Usado", zh: "已用"), used),
        ];
        for (i, (label, value)) in stats.iter().enumerate() {
            let left = card.left + i as i32 * (cw + gap);
            let r = rect(left, top, left + cw, top + sc(54));
            ui::fill(dc, r, ui::mix(p.bg, p.text, 0.05), sc(8));
            draw(dc, d, d.fonts[2], label, rect(left + sc(10), top + sc(4), left + cw - sc(6), top + sc(24)), p.dim, DT_LEFT | DT_END_ELLIPSIS);
            let v = if dr.total == 0 { "—".to_string() } else { crate::size_text(*value) };
            draw(dc, d, d.fonts[1], &v, rect(left + sc(10), top + sc(24), left + cw - sc(6), top + sc(50)), p.text, DT_LEFT | DT_END_ELLIPSIS);
        }
        if dr.total > 0 {
            let bar = rect(card.left, top + sc(66), card.right, top + sc(74));
            ui::fill(dc, bar, ui::mix(p.bg, p.text, 0.12), sc(4));
            let frac = (used as f32 / dr.total as f32).clamp(0.0, 1.0);
            let fw = ((bar.right - bar.left) as f32 * frac) as i32;
            if fw > 0 {
                ui::fill(dc, rect(bar.left, bar.top, bar.left + fw.max(sc(4)), bar.bottom), if frac > 0.9 { winrust_common::style::BAD } else { p.accent }, sc(4));
            }
        }
    }

    // read-only / hidden, only for a real file or folder (never a drive, never a recycled item)
    if d.can_attr {
        for (r, on, label) in [
            (d.ro_row, d.ro, t!("Только чтение", "Read-only", ja: "読み取り専用", pt: "Somente leitura", zh: "只读")),
            (d.hid_row, d.hidden, t!("Скрытый", "Hidden", ja: "隠し", pt: "Oculto", zh: "隐藏")),
        ] {
            let cb = sc(18);
            let by = (r.top + r.bottom - cb) / 2;
            let boxr = rect(r.left, by, r.left + cb, by + cb);
            ui::fill(dc, boxr, if on { p.accent } else { ui::mix(p.bg, p.text, 0.14) }, sc(4));
            if on {
                glyph(dc, d, "\u{E73E}", boxr, winrust_common::style::on_accent());
            }
            draw(dc, d, d.fonts[1], label, rect(boxr.right + sc(8), r.top, r.right, r.bottom), p.text, DT_LEFT | DT_END_ELLIPSIS);
        }
    }

    if let Some(err) = &d.error {
        draw(dc, d, d.fonts[2], err, rect(sc(PARA), d.ok_btn.top - sc(24), d.ok_btn.left - sc(12), d.ok_btn.top), winrust_common::style::BAD, DT_LEFT | DT_END_ELLIPSIS);
    }

    // "Aceptar", the only button: Esc and the close glyph are the cancel
    let ok_bg = if d.hover == Hit::Ok { ui::mix(p.accent, p.text, 0.12) } else { p.accent };
    ui::fill(dc, d.ok_btn, ok_bg, sc(6));
    draw(dc, d, d.fonts[1], t!("ОК", "OK", ja: "OK", pt: "OK", zh: "确定"), d.ok_btn, winrust_common::style::on_accent(), DT_CENTER);

    if buffered {
        BitBlt(wdc, 0, 0, w, ht, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
    }
    if !bmp.is_null() {
        DeleteObject(bmp);
    }
    if !mem.is_null() {
        DeleteDC(mem);
    }
    EndPaint(h, &ps);
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Dlg;
    if p.is_null() {
        return DefWindowProcW(h, msg, wp, lp);
    }
    let d = &mut *p;
    let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(h, d);
            0
        }
        WM_CLOSE => {
            debug("WM_CLOSE");
            d.done = true;
            0
        }
        WM_DESTROY => {
            debug("WM_DESTROY");
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_MOUSEMOVE => {
            if !d.tracking {
                d.tracking = true;
                let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                TrackMouseEvent(&mut t);
            }
            let hov = hit(d, x, y);
            if hov != d.hover {
                d.hover = hov;
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_MOUSELEAVE => {
            d.tracking = false;
            if d.hover != Hit::None {
                d.hover = Hit::None;
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            debug(&format!("WM_LBUTTONUP {x},{y}"));
            match hit(d, x, y) {
                Hit::Close => d.done = true,
                Hit::Ok => accept(h, d),
                Hit::ReadOnly => {
                    d.ro = !d.ro;
                    InvalidateRect(h, ptr::null(), 0);
                }
                Hit::Hidden => {
                    d.hidden = !d.hidden;
                    InvalidateRect(h, ptr::null(), 0);
                }
                Hit::None => {}
            }
            0
        }
        WM_KEYDOWN => {
            debug(&format!("WM_KEYDOWN {}", wp));
            match wp as u16 {
                VK_ESCAPE => d.done = true,
                VK_RETURN => accept(h, d),
                _ => {}
            }
            0
        }
        WM_NCHITTEST => {
            // the title row drags the window; the close button must keep its clicks
            let mut pt = POINT { x: (lp & 0xffff) as i16 as i32, y: ((lp >> 16) & 0xffff) as i16 as i32 };
            ScreenToClient(h, &mut pt);
            if pt.y < sc(TITLE_H) && !inside(d.close_btn, pt.x, pt.y) {
                return HTCAPTION as isize;
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_PROPS_SIZE => {
            debug(&format!("WM_PROPS_SIZE {wp}"));
            // the folder walk finished: show the total without ever having blocked this window
            if !d.done && d.scan.is_some() && d.folder {
                d.size = wp as u64;
                d.size_known = true;
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe fn ensure_class() {
    let name = crate::w(CLASS);
    let mut wc: WNDCLASSW = zeroed();
    if GetClassInfoW(GetModuleHandleW(ptr::null()), name.as_ptr(), &mut wc) != 0 {
        return;
    }
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(proc),
        hInstance: GetModuleHandleW(ptr::null()),
        hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
        lpszClassName: name.as_ptr(),
        ..zeroed()
    };
    RegisterClassW(&wc);
}

/// Shows the Properties window for `path`, owned by `owner`, and runs its message loop until Aceptar / Esc / ✕.
pub unsafe fn open(owner: HWND, path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }
    ensure_class();
    let mut d = Box::new(build(path));
    reflow(&mut d);
    let w = sc(WIN_W);
    let ht = sc(WIN_H);

    // centred on the monitor where the user is working, like the Open with picker
    let mut cur: POINT = zeroed();
    GetCursorPos(&mut cur);
    let mon = winrust_common::monitors::at(cur.x, cur.y);
    let area = if mon.work.right > mon.work.left && mon.work.bottom > mon.work.top { mon.work } else { mon.rc };
    let x = area.left + ((area.right - area.left) - w) / 2;
    let y = area.top + ((area.bottom - area.top) - ht) / 2;

    let title = format!("{}: {}", t!("Свойства", "Properties", ja: "プロパティ", pt: "Propriedades", zh: "属性"), d.name);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW, // owned + tool window: no taskbar button
        crate::w(CLASS).as_ptr(),
        crate::w(&title).as_ptr(),
        WS_POPUP,
        x,
        y,
        w,
        ht,
        owner,
        ptr::null_mut(),
        GetModuleHandleW(ptr::null()),
        ptr::null(),
    );
    if hwnd.is_null() {
        return; // the Box drops here and frees its fonts
    }
    let raw = Box::into_raw(d);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
    // still invisible: what is on screen there is what is behind the frost
    if winrust_common::look::menu_blur() && !winrust_common::style::style().light() {
        (*raw).px = winrust_common::frost::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + ht }, winrust_common::look::menu_radius());
    }
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, ht + 1, sc(16), sc(16)), 1);

    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    SetFocus(hwnd);
    InvalidateRect(hwnd, ptr::null(), 0);

    // the size of a folder is a walk: "Calculating…" at once, the total posted when it is done
    if let Some(folder) = (*raw).scan.clone() {
        let stop = (*raw).stop.clone();
        let target = hwnd as isize;
        std::thread::spawn(move || {
            let total = folder_size(&folder, &stop);
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let _ = unsafe { PostMessageW(target as HWND, WM_PROPS_SIZE, total as usize, 0) };
        });
    }

    debug(&format!("open {hwnd:p} {}", path.display()));
    let mut msg: MSG = zeroed();
    loop {
        let r = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
        if r <= 0 {
            debug(&format!("loop end r={r} msg=0x{:X}", msg.message));
            break;
        }
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
        if (*raw).done {
            debug(&format!("done msg=0x{:X}", msg.message));
            break;
        }
    }
    (*raw).stop.store(true, Ordering::Relaxed);
    DestroyWindow(hwnd);
    drop(Box::from_raw(raw));
    if !owner.is_null() && IsWindow(owner) != 0 {
        SetForegroundWindow(owner);
    }
}
