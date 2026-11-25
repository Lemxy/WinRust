#![windows_subsystem = "windows"]
// rust-notepad [file] - the text editor of the WinRust image (see tools/iso-replacement.md).
//
// A Notepad that does not need Explorer: its own window chrome (title, menu bar, status bar, rounded
// cards), a multiline EDIT control as the editor, and the file work done by hand.
//
// Covered: New / Open / Save / Save as / Exit with the "save changes?" question, UTF-8 (with or without
// BOM), UTF-16 LE/BE and ANSI detection, CRLF / LF / CR kept on save, find with F3 and a highlighted
// selection, drag and drop, Ctrl+A/C/V/X/Z/Y (Z/Y are a multi-level undo of our own), the caret line and
// column, the length and the encoding in the status bar, and `rust-notepad [file]`.
// Not covered: printing, automatic line wrap, word wrap, replace, go to line, a font chooser, encodings
// other than the ones above, and a different undo granularity than one step per typing pause.
// Saving always writes UTF-8 (plus a BOM only when the file had a UTF-8 one) and the file's own line
// ending, so a UTF-16 or ANSI file is converted on its first save. See README.md.
//
// Safety: only documented Win32 calls, and only the file the user picked or dropped is written, after
// the save question. Nothing is started and no string is run as a command.

#[macro_use]
extern crate winrust_common;

mod text;
mod ui;

use std::{
    cell::RefCell,
    ffi::c_void,
    mem::size_of,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::atomic::{AtomicIsize, Ordering},
};
use ui::{F_BODY, F_ICON, F_ICON_S, F_MONO, F_SMALL, F_TITLE, FONT_COUNT};
use winrust_common::style::{mix, on_accent, rgb};

use windows_sys::Win32::{
    Foundation::*,
    Graphics::{
        Dwm::DwmSetWindowAttribute,
        Gdi::*,
        GdiPlus::{GdiplusShutdown, GdiplusStartup, GdiplusStartupInput},
    },
    System::{LibraryLoader::GetModuleHandleW, SystemInformation::GetTickCount64},
    UI::{
        Controls::SetWindowTheme,
        Controls::Dialogs::{
            GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST,
        },
        HiDpi::{GetDpiForSystem, GetDpiForWindow},
        Input::KeyboardAndMouse::*,
        Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP},
        WindowsAndMessaging::*,
    },
};

const APP_NAME: &str = "WinRust Notepad";
const TITLE_H: i32 = 44;
const MENU_H: i32 = 32;
const STATUS_H: i32 = 28;
const FIND_H: i32 = 46;
const PAD: i32 = 12;
const BTN_W: i32 = 46;
const WM_MOUSELEAVE: u32 = 0x02A3;
/// A key from the editor or the find field: wParam = VK_*, lParam = ctrl | shift << 1.
const WM_CHILD_KEY: u32 = WM_APP + 1;
/// The caret may have moved: refresh the status bar.
const WM_CARET: u32 = WM_APP + 2;
const ID_EDIT: usize = 100;
const ID_FIND: usize = 101;
const EN_CHANGE_CODE: u16 = 0x0300;
// The EM_* messages are not in windows-sys' bindings; the values are the ones from winuser.h.
const EM_GETSEL: u32 = 0x00B0;
const EM_SETSEL: u32 = 0x00B1;
const EM_SCROLLCARET: u32 = 0x00B7;
const EM_LINEINDEX: u32 = 0x00BB;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const EM_LINEFROMCHAR: u32 = 0x00C9;
const EM_SETMARGINS: u32 = 0x00D3;
const EM_SETCUEBANNER: u32 = 0x1501;
const TIMER_NOTE: usize = 1;
const NOTE_MS: u64 = 4000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmd {
    New,
    Open,
    Save,
    SaveAs,
    Exit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Find,
    FindNext,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hot {
    Title(u8),
    MenuBar(usize),
    FindBtn,
    FindClose,
}

struct App {
    dpi: i32,
    fonts: [isize; FONT_COUNT],
    font_dpi: i32,
    font_style: u8,
    edit: isize,
    find: isize,
    brush: isize,
    find_open: bool,
    hover: Option<Hot>,
    path: Option<PathBuf>,
    enc: text::Enc,
    eol: text::Eol,
    /// Write a UTF-8 BOM on save (only files that already had one).
    bom: bool,
    saved: String,
    /// The text as of the last undo snapshot.
    last_text: String,
    undo: Vec<String>,
    redo: Vec<String>,
    last_push: u64,
    /// A programmatic WM_SETTEXT is in progress: its EN_CHANGE is ours, not the user's.
    applying: bool,
    dirty: bool,
    chars: usize,
    /// A short message in the status bar: text, when it appeared, whether it is a warning.
    note: Option<(String, u64, bool)>,
}

impl Default for App {
    fn default() -> Self {
        App {
            dpi: 96,
            fonts: [0; FONT_COUNT],
            font_dpi: 0,
            font_style: 255,
            edit: 0,
            find: 0,
            brush: 0,
            find_open: false,
            hover: None,
            path: None,
            enc: text::Enc::Utf8,
            eol: text::Eol::Crlf,
            bom: false,
            saved: String::new(),
            last_text: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_push: 0,
            applying: false,
            dirty: false,
            chars: 0,
            note: None,
        }
    }
}

thread_local! { static APP: RefCell<App> = RefCell::new(App::default()) }
static EDIT_PROC: AtomicIsize = AtomicIsize::new(0);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn p(dpi: i32, v: i32) -> i32 {
    v * dpi / 96
}

// ---------------------------------------------------------------- geometry

fn title_buttons(cw: i32, dpi: i32) -> [RECT; 3] {
    let w = p(dpi, BTN_W);
    let h = p(dpi, TITLE_H);
    [
        RECT { left: cw - 3 * w, top: 0, right: cw - 2 * w, bottom: h },
        RECT { left: cw - 2 * w, top: 0, right: cw - w, bottom: h },
        RECT { left: cw - w, top: 0, right: cw, bottom: h },
    ]
}

fn menu_tab(i: usize, dpi: i32) -> RECT {
    let w = p(dpi, 92);
    let x = p(dpi, 10) + w * i as i32;
    RECT { left: x, top: p(dpi, TITLE_H), right: x + w, bottom: p(dpi, TITLE_H + MENU_H) }
}

fn editor_card(cw: i32, ch: i32, dpi: i32, find_open: bool) -> RECT {
    let bottom = ch - p(dpi, STATUS_H) - p(dpi, PAD) - if find_open { p(dpi, FIND_H) } else { 0 };
    let top = p(dpi, TITLE_H + MENU_H) + p(dpi, 8);
    RECT { left: p(dpi, PAD), top, right: cw - p(dpi, PAD), bottom: bottom.max(top + p(dpi, 40)) }
}

fn editor_inner(cw: i32, ch: i32, dpi: i32, find_open: bool) -> RECT {
    ui::inset(editor_card(cw, ch, dpi, find_open), p(dpi, 8))
}

fn find_bar(cw: i32, ch: i32, dpi: i32) -> RECT {
    RECT { left: p(dpi, PAD), top: ch - p(dpi, STATUS_H + FIND_H), right: cw - p(dpi, PAD), bottom: ch - p(dpi, STATUS_H) }
}

/// The find field, its button and its close glyph.
fn find_widgets(cw: i32, ch: i32, dpi: i32) -> (RECT, RECT, RECT) {
    let bar = find_bar(cw, ch, dpi);
    let cy = (bar.top + bar.bottom) / 2;
    let close = RECT { left: bar.right - p(dpi, 30), top: cy - p(dpi, 14), right: bar.right, bottom: cy + p(dpi, 14) };
    let bw = p(dpi, 152);
    let btn = RECT { left: close.left - p(dpi, 8) - bw, top: cy - p(dpi, 14), right: close.left - p(dpi, 8), bottom: cy + p(dpi, 14) };
    let field = RECT { left: bar.left + p(dpi, 32), top: cy - p(dpi, 15), right: btn.left - p(dpi, 12), bottom: cy + p(dpi, 15) };
    (field, btn, close)
}

fn find_edit_rect(cw: i32, ch: i32, dpi: i32) -> RECT {
    let (field, ..) = find_widgets(cw, ch, dpi);
    RECT { left: field.left + p(dpi, 8), top: field.top + p(dpi, 5), right: field.right - p(dpi, 8), bottom: field.bottom - p(dpi, 5) }
}

// ---------------------------------------------------------------- text in and out of the EDIT control

fn get_text(h: HWND) -> String {
    unsafe {
        let n = SendMessageW(h, WM_GETTEXTLENGTH, 0, 0) as usize;
        let mut b = vec![0u16; n + 1];
        let got = SendMessageW(h, WM_GETTEXT, b.len(), b.as_mut_ptr() as isize) as usize;
        String::from_utf16_lossy(&b[..got.min(b.len())])
    }
}

fn set_text(h: HWND, s: &str) {
    let w = wide(s);
    unsafe { SendMessageW(h, WM_SETTEXT, 0, w.as_ptr() as isize) };
}

fn selection(h: HWND) -> (usize, usize) {
    let (mut s, mut e) = (0u32, 0u32);
    unsafe { SendMessageW(h, EM_GETSEL, &mut s as *mut u32 as usize, &mut e as *mut u32 as isize) };
    (s as usize, e as usize)
}

fn line_col(h: HWND) -> (usize, usize) {
    unsafe {
        let (s, _) = selection(h);
        let line = SendMessageW(h, EM_LINEFROMCHAR, s, 0) as usize;
        let start = SendMessageW(h, EM_LINEINDEX, line, 0) as usize;
        (line + 1, s.saturating_sub(start) + 1)
    }
}

fn doc_name(a: &App) -> String {
    a.path
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| t!("Без имени", "Untitled", ja: "無題", pt: "Sem título", zh: "无标题").to_string())
}

fn menu_name(i: usize) -> &'static str {
    if i == 0 { t!("Файл", "File", ja: "ファイル", pt: "Arquivo", zh: "文件") } else { t!("Правка", "Edit", ja: "編集", pt: "Editar", zh: "编辑") }
}

fn update(hwnd: HWND) {
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

fn set_title(hwnd: HWND) {
    let (name, dirty) = APP.with_borrow(|a| (doc_name(a), a.dirty));
    let t = format!("{name}{} - {APP_NAME}", if dirty { " *" } else { "" });
    let w = wide(&t);
    unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
    update(hwnd);
}

fn note(hwnd: HWND, s: String, warn: bool) {
    APP.with_borrow_mut(|a| a.note = Some((s, unsafe { GetTickCount64() }, warn)));
    unsafe { SetTimer(hwnd, TIMER_NOTE, 400, None) };
    update(hwnd);
}

fn error_box(hwnd: HWND, msg: &str) {
    unsafe { MessageBoxW(hwnd, wide(msg).as_ptr(), wide(APP_NAME).as_ptr(), MB_OK | MB_ICONERROR) };
}

// ---------------------------------------------------------------- documents

/// Puts `text` in the editor without recording a change and forgets the undo history.
fn set_document(hwnd: HWND, text_in: String, path: Option<PathBuf>, enc: text::Enc, eol: text::Eol, bom: bool) {
    let edit = APP.with_borrow(|a| a.edit as HWND);
    APP.with_borrow_mut(|a| a.applying = true);
    if !edit.is_null() {
        set_text(edit, &text_in);
    }
    let chars = text_in.chars().count();
    APP.with_borrow_mut(|a| {
        a.applying = false;
        a.saved = text_in.clone();
        a.last_text = text_in;
        a.undo.clear();
        a.redo.clear();
        a.last_push = 0;
        a.dirty = false;
        a.chars = chars;
        a.path = path;
        a.enc = enc;
        a.eol = eol;
        a.bom = bom;
        a.note = None;
    });
    if !edit.is_null() {
        unsafe {
            SendMessageW(edit, EM_SETSEL, 0, 0);
            SendMessageW(edit, EM_SCROLLCARET, 0, 0);
            SetFocus(edit);
        }
    }
    set_title(hwnd);
    update(hwnd);
}

fn open_path(hwnd: HWND, path: &Path) {
    match std::fs::read(path) {
        Ok(bytes) => {
            let l = text::decode(&bytes);
            let bom = l.enc == text::Enc::Utf8Bom;
            set_document(hwnd, l.text, Some(path.to_path_buf()), l.enc, l.eol, bom);
        }
        Err(e) => error_box(
            hwnd,
            &tf!(
                "Не удалось открыть файл: {}",
                "Cannot open the file: {}",
                ja: "ファイルを開けません: {}",
                pt: "Não foi possível abrir o arquivo: {}",
                zh: "无法打开文件: {}",
                format!("{} ({e})", path.display())
            ),
        ),
    }
}

fn open_initial(hwnd: HWND, path: &Path) {
    if path.is_dir() {
        error_box(hwnd, t!("Нельзя открыть папку.", "Cannot open a folder.", ja: "フォルダーは開けません。", pt: "Não é possível abrir uma pasta.", zh: "无法打开文件夹。"));
        return;
    }
    if path.exists() {
        open_path(hwnd, path);
    } else {
        // A path that does not exist yet: the name is shown, the file appears on the first save.
        set_document(hwnd, String::new(), Some(path.to_path_buf()), text::Enc::Utf8, text::Eol::Crlf, false);
    }
}

fn save(hwnd: HWND, as_new: bool) -> bool {
    let (edit, current, eol, bom) = APP.with_borrow(|a| (a.edit as HWND, if as_new { None } else { a.path.clone() }, a.eol, a.bom));
    let path = match current.or_else(|| pick_file(hwnd, true)) {
        Some(p) => p,
        None => return false,
    };
    let body = get_text(edit);
    let bytes = text::encode(&body, eol, bom);
    match std::fs::write(&path, &bytes) {
        Ok(()) => {
            let chars = body.chars().count();
            APP.with_borrow_mut(|a| {
                a.path = Some(path);
                a.saved = body;
                a.dirty = false;
                a.chars = chars;
                a.enc = if bom { text::Enc::Utf8Bom } else { text::Enc::Utf8 };
                a.note = None;
            });
            set_title(hwnd);
            note(hwnd, t!("Сохранено", "Saved", ja: "保存しました", pt: "Salvo", zh: "已保存").to_string(), false);
            true
        }
        Err(e) => {
            error_box(
                hwnd,
                &tf!(
                    "Не удалось сохранить файл: {}",
                    "Cannot save the file: {}",
                    ja: "ファイルを保存できません: {}",
                    pt: "Não foi possível salvar o arquivo: {}",
                    zh: "无法保存文件: {}",
                    format!("{} ({e})", path.display())
                ),
            );
            false
        }
    }
}

fn confirm_discard(hwnd: HWND) -> bool {
    let (dirty, name) = APP.with_borrow(|a| (a.dirty, doc_name(a)));
    if !dirty {
        return true;
    }
    let msg = tf!(
        "Сохранить изменения в файле \"{}\"?",
        "Save changes to \"{}\"?",
        ja: "「{}」への変更を保存しますか?",
        pt: "Salvar alterações em \"{}\"?",
        zh: "是否保存对“{}”的更改?",
        name
    );
    let r = unsafe { MessageBoxW(hwnd, wide(&msg).as_ptr(), wide(APP_NAME).as_ptr(), MB_YESNOCANCEL | MB_ICONQUESTION) };
    match r {
        IDYES => save(hwnd, false),
        IDNO => true,
        _ => false,
    }
}

fn new_doc(hwnd: HWND) {
    if confirm_discard(hwnd) {
        set_document(hwnd, String::new(), None, text::Enc::Utf8, text::Eol::Crlf, false);
    }
}

fn pick_file(hwnd: HWND, save_as: bool) -> Option<PathBuf> {
    let mut buf = vec![0u16; 4096];
    if let Some(p) = APP.with_borrow(|a| a.path.clone())
        && let Some(s) = p.to_str()
    {
        let w: Vec<u16> = s.encode_utf16().take(buf.len() - 1).collect();
        buf[..w.len()].copy_from_slice(&w);
    }
    let filter: Vec<u16> = format!(
        "{}\0*.txt;*.md;*.log;*.ini;*.json;*.csv\0{}\0*.*\0\0",
        t!("Текстовые файлы", "Text files", ja: "テキスト ファイル", pt: "Arquivos de texto", zh: "文本文件"),
        t!("Все файлы", "All files", ja: "すべてのファイル", pt: "Todos os arquivos", zh: "所有文件")
    )
    .encode_utf16()
    .collect();
    let title = if save_as {
        t!("Сохранить как", "Save as", ja: "名前を付けて保存", pt: "Salvar como", zh: "另存为")
    } else {
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开")
    };
    let title_w = wide(title);
    let ext = wide("txt");
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = hwnd;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = buf.as_mut_ptr();
    ofn.nMaxFile = buf.len() as u32;
    ofn.lpstrTitle = title_w.as_ptr();
    ofn.Flags = if save_as {
        OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR
    } else {
        OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR
    };
    if save_as {
        ofn.lpstrDefExt = ext.as_ptr();
    }
    let ok = unsafe { if save_as { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) } };
    if ok == 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let s = String::from_utf16_lossy(&buf[..len]);
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// Replaces the editor text programmatically and puts the caret back (undo, redo).
fn apply_text(hwnd: HWND, body: &str, caret: Option<usize>) {
    let edit = APP.with_borrow(|a| a.edit as HWND);
    if edit.is_null() {
        return;
    }
    APP.with_borrow_mut(|a| a.applying = true);
    set_text(edit, body);
    let chars = body.chars().count();
    APP.with_borrow_mut(|a| {
        a.applying = false;
        a.last_text = body.to_string();
        a.last_push = 0; // the next keystroke starts a new undo step
        a.chars = chars;
        a.dirty = body != a.saved.as_str();
    });
    let at = caret.unwrap_or(0).min(text::utf16_len(body));
    unsafe {
        SendMessageW(edit, EM_SETSEL, at, at as isize);
        SendMessageW(edit, EM_SCROLLCARET, 0, 0);
    }
    set_title(hwnd);
    update(hwnd);
}

fn undo(hwnd: HWND) {
    let edit = APP.with_borrow(|a| a.edit as HWND);
    if edit.is_null() {
        return;
    }
    let caret = selection(edit).0;
    let prev = APP.with_borrow_mut(|a| {
        if a.undo.is_empty() {
            return None;
        }
        let cur = get_text(edit);
        let prev = a.undo.pop()?;
        a.redo.push(cur);
        Some(prev)
    });
    if let Some(body) = prev {
        apply_text(hwnd, &body, Some(caret));
    }
}

fn redo(hwnd: HWND) {
    let edit = APP.with_borrow(|a| a.edit as HWND);
    if edit.is_null() {
        return;
    }
    let caret = selection(edit).0;
    let next = APP.with_borrow_mut(|a| {
        if a.redo.is_empty() {
            return None;
        }
        let cur = get_text(edit);
        let next = a.redo.pop()?;
        a.undo.push(cur);
        Some(next)
    });
    if let Some(body) = next {
        apply_text(hwnd, &body, Some(caret));
    }
}

fn on_change(hwnd: HWND) {
    let (applying, edit) = APP.with_borrow(|a| (a.applying, a.edit as HWND));
    if applying || edit.is_null() {
        return;
    }
    let body = get_text(edit);
    let now = unsafe { GetTickCount64() };
    let chars = body.chars().count();
    let was_dirty = APP.with_borrow_mut(|a| {
        let old = a.dirty;
        a.dirty = body != a.saved;
        a.chars = chars;
        a.redo.clear(); // typing after an undo drops the redo stack, even without a new snapshot
        // One snapshot per typing pause: undo goes back to the text before the pause, which is what a
        // person expects from "typing" as one step. A long paste is a step of its own.
        if now.saturating_sub(a.last_push) > 400 || a.undo.is_empty() {
            let prev = std::mem::replace(&mut a.last_text, body.clone());
            a.undo.push(prev);
            if a.undo.len() > 80 {
                a.undo.remove(0);
            }
            a.last_push = now;
        }
        old != a.dirty
    });
    if was_dirty {
        set_title(hwnd);
    }
    update(hwnd);
}

// ---------------------------------------------------------------- find

/// Lower-cased UTF-16 of `s` plus, for every original char, (its UTF-16 length, its lower-cased one).
fn fold(s: &str) -> (Vec<u16>, Vec<(usize, usize)>) {
    let mut low = Vec::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len());
    for c in s.chars() {
        let mut n = 0;
        for l in c.to_lowercase() {
            n += l.len_utf16();
            let mut b = [0u16; 2];
            low.extend_from_slice(l.encode_utf16(&mut b));
        }
        map.push((c.len_utf16(), n));
    }
    (low, map)
}

fn find_in(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    let first = from.min(hay.len() - needle.len());
    (first..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

/// A folded index back to the UTF-16 index of the original text (rounded down to a char start).
fn orig_index(map: &[(usize, usize)], lower: usize) -> usize {
    let (mut o, mut l) = (0, 0);
    for &(ol, ll) in map {
        if l + ll > lower {
            break;
        }
        o += ol;
        l += ll;
    }
    o
}

fn open_find(hwnd: HWND) {
    let (open, edit, find) = APP.with_borrow(|a| (a.find_open, a.edit as HWND, a.find as HWND));
    if !open {
        APP.with_borrow_mut(|a| a.find_open = true);
        layout(hwnd);
    }
    // Prefill with the selection when it is one short line: the usual "find this".
    let (s, e) = selection(edit);
    if e > s && e - s < 200 {
        let body = get_text(edit);
        let units: Vec<u16> = body.encode_utf16().collect();
        if e <= units.len() {
            let sel = String::from_utf16_lossy(&units[s..e]);
            if !sel.contains('\r') && !sel.contains('\n') {
                set_text(find, &sel);
            }
        }
    }
    unsafe {
        SetFocus(find);
        SendMessageW(find, EM_SETSEL, 0, -1);
    }
}

fn close_find(hwnd: HWND) {
    let (open, edit) = APP.with_borrow(|a| (a.find_open, a.edit as HWND));
    if !open {
        return;
    }
    APP.with_borrow_mut(|a| a.find_open = false);
    layout(hwnd);
    unsafe { SetFocus(edit) };
}

fn find_next(hwnd: HWND) {
    let (edit, find) = APP.with_borrow(|a| (a.edit as HWND, a.find as HWND));
    if edit.is_null() {
        return;
    }
    let needle = get_text(find);
    if needle.is_empty() {
        open_find(hwnd);
        return;
    }
    let body = get_text(edit);
    let (_, end) = selection(edit);
    let (hay, map) = fold(&body);
    let (need, _) = fold(&needle);
    let at = find_in(&hay, &need, end).or_else(|| find_in(&hay, &need, 0));
    match at {
        Some(i) => {
            let a = orig_index(&map, i);
            let b = orig_index(&map, i + need.len()).max(a);
            unsafe {
                SendMessageW(edit, EM_SETSEL, a, b as isize);
                SendMessageW(edit, EM_SCROLLCARET, 0, 0);
            }
            update(hwnd);
        }
        None => note(hwnd, t!("Не найдено", "Not found", ja: "見つかりません", pt: "Não encontrado", zh: "未找到").to_string(), true),
    }
}

// ---------------------------------------------------------------- menus

/// The drop-downs are native popup menus: they are drawn above the editor's child window, take the
/// keyboard and close on a click outside by themselves (rust-control uses the same for its menus).
fn menu_items(which: usize) -> Vec<(u32, String, &'static str)> {
    if which == 0 {
        vec![
            (1, t!("Создать", "New", ja: "新規作成", pt: "Novo", zh: "新建").to_string(), "Ctrl+N"),
            (2, t!("Открыть...", "Open...", ja: "開く...", pt: "Abrir...", zh: "打开...").to_string(), "Ctrl+O"),
            (3, t!("Сохранить", "Save", ja: "保存", pt: "Salvar", zh: "保存").to_string(), "Ctrl+S"),
            (4, t!("Сохранить как...", "Save as...", ja: "名前を付けて保存...", pt: "Salvar como...", zh: "另存为...").to_string(), "Ctrl+Shift+S"),
            (0, String::new(), ""),
            (5, t!("Выход", "Exit", ja: "終了", pt: "Sair", zh: "退出").to_string(), "Alt+F4"),
        ]
    } else {
        vec![
            (11, t!("Отменить", "Undo", ja: "元に戻す", pt: "Desfazer", zh: "撤销").to_string(), "Ctrl+Z"),
            (12, t!("Повторить", "Redo", ja: "やり直し", pt: "Refazer", zh: "重做").to_string(), "Ctrl+Y"),
            (0, String::new(), ""),
            (13, t!("Вырезать", "Cut", ja: "切り取り", pt: "Recortar", zh: "剪切").to_string(), "Ctrl+X"),
            (14, t!("Копировать", "Copy", ja: "コピー", pt: "Copiar", zh: "复制").to_string(), "Ctrl+C"),
            (15, t!("Вставить", "Paste", ja: "貼り付け", pt: "Colar", zh: "粘贴").to_string(), "Ctrl+V"),
            (16, t!("Выделить всё", "Select all", ja: "すべて選択", pt: "Selecionar tudo", zh: "全选").to_string(), "Ctrl+A"),
            (0, String::new(), ""),
            (17, t!("Найти...", "Find...", ja: "検索...", pt: "Localizar...", zh: "查找...").to_string(), "Ctrl+F"),
            (18, t!("Найти далее", "Find next", ja: "次を検索", pt: "Localizar próxima", zh: "查找下一个").to_string(), "F3"),
        ]
    }
}

fn cmd_from_id(id: u32) -> Option<Cmd> {
    Some(match id {
        1 => Cmd::New,
        2 => Cmd::Open,
        3 => Cmd::Save,
        4 => Cmd::SaveAs,
        5 => Cmd::Exit,
        11 => Cmd::Undo,
        12 => Cmd::Redo,
        13 => Cmd::Cut,
        14 => Cmd::Copy,
        15 => Cmd::Paste,
        16 => Cmd::SelectAll,
        17 => Cmd::Find,
        18 => Cmd::FindNext,
        _ => return None,
    })
}

fn show_menu(hwnd: HWND, which: usize) {
    let dpi = APP.with_borrow(|a| a.dpi);
    let items = menu_items(which);
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return;
        }
        for (id, label, key) in &items {
            if *id == 0 {
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
            } else {
                let text = if key.is_empty() { label.clone() } else { format!("{label}\t{key}") };
                let w = wide(&text);
                AppendMenuW(menu, MF_STRING, *id as usize, w.as_ptr());
            }
        }
        let anchor = menu_tab(which, dpi);
        let mut pt = POINT { x: anchor.left, y: anchor.bottom };
        ClientToScreen(hwnd, &mut pt);
        SetForegroundWindow(hwnd);
        let id = TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_TOPALIGN | TPM_RETURNCMD, pt.x, pt.y, 0, hwnd, null());
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0); // the documented "close cleanly" ping after TPM_RETURNCMD
        APP.with_borrow_mut(|a| a.hover = None);
        update(hwnd);
        if let Some(cmd) = cmd_from_id(id as u32) {
            apply(hwnd, cmd);
        }
    }
}

// ---------------------------------------------------------------- commands

fn apply(hwnd: HWND, cmd: Cmd) {
    match cmd {
        Cmd::New => new_doc(hwnd),
        Cmd::Open => {
            if confirm_discard(hwnd)
                && let Some(path) = pick_file(hwnd, false)
            {
                open_path(hwnd, &path);
            }
        }
        Cmd::Save => {
            save(hwnd, false);
        }
        Cmd::SaveAs => {
            save(hwnd, true);
        }
        Cmd::Exit => {
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        }
        Cmd::Undo => undo(hwnd),
        Cmd::Redo => redo(hwnd),
        Cmd::Cut | Cmd::Copy | Cmd::Paste => {
            let (edit, msg) = APP.with_borrow(|a| {
                (a.edit as HWND, match cmd {
                    Cmd::Cut => WM_CUT,
                    Cmd::Copy => WM_COPY,
                    _ => WM_PASTE,
                })
            });
            unsafe {
                SetFocus(edit);
                SendMessageW(edit, msg, 0, 0);
            }
        }
        Cmd::SelectAll => {
            let edit = APP.with_borrow(|a| a.edit as HWND);
            unsafe {
                SetFocus(edit);
                SendMessageW(edit, EM_SETSEL, 0, -1);
            }
        }
        Cmd::Find => open_find(hwnd),
        Cmd::FindNext => find_next(hwnd),
    }
}

fn drop_files(hwnd: HWND, drop: HDROP) {
    unsafe {
        let n = DragQueryFileW(drop, 0, null_mut(), 0);
        if n == 0 {
            DragFinish(drop);
            return;
        }
        let mut buf = vec![0u16; n as usize + 1];
        DragQueryFileW(drop, 0, buf.as_mut_ptr(), buf.len() as u32);
        DragFinish(drop);
        let path = PathBuf::from(String::from_utf16_lossy(&buf[..n as usize]));
        if path.is_dir() {
            error_box(hwnd, t!("Нельзя открыть папку.", "Cannot open a folder.", ja: "フォルダーは開けません。", pt: "Não é possível abrir uma pasta.", zh: "无法打开文件夹。"));
            return;
        }
        if confirm_discard(hwnd) {
            open_path(hwnd, &path);
        }
    }
}

fn on_key(hwnd: HWND, key: u16) {
    let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) < 0 };
    let shift = unsafe { GetKeyState(VK_SHIFT as i32) < 0 };
    match key {
        k if k == VK_N && ctrl => apply(hwnd, Cmd::New),
        k if k == VK_O && ctrl => apply(hwnd, Cmd::Open),
        k if k == VK_S && ctrl => apply(hwnd, if shift { Cmd::SaveAs } else { Cmd::Save }),
        k if k == VK_F && ctrl => open_find(hwnd),
        k if k == VK_F3 => find_next(hwnd),
        k if k == VK_ESCAPE => close_find(hwnd),
        _ => {}
    }
}

// ---------------------------------------------------------------- hit testing

fn hot_at(hwnd: HWND, x: i32, y: i32) -> Option<Hot> {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (dpi, find_open) = APP.with_borrow(|a| (a.dpi, a.find_open));
        for (i, r) in title_buttons(rc.right, dpi).iter().enumerate() {
            if ui::contains(r, x, y) {
                return Some(Hot::Title(i as u8));
            }
        }
        if y >= p(dpi, TITLE_H) && y < p(dpi, TITLE_H + MENU_H) {
            for i in 0..2 {
                if ui::contains(&menu_tab(i, dpi), x, y) {
                    return Some(Hot::MenuBar(i));
                }
            }
        }
        if find_open {
            let (_, btn, close) = find_widgets(rc.right, rc.bottom, dpi);
            if ui::contains(&btn, x, y) {
                return Some(Hot::FindBtn);
            }
            if ui::contains(&close, x, y) {
                return Some(Hot::FindClose);
            }
        }
        None
    }
}

fn on_move(hwnd: HWND, x: i32, y: i32) {
    unsafe {
        let mut tme = TRACKMOUSEEVENT {
            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: TME_LEAVE,
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        TrackMouseEvent(&mut tme);
    }
    let hot = hot_at(hwnd, x, y);
    let changed = APP.with_borrow_mut(|a| {
        let old = a.hover;
        a.hover = hot;
        old != hot
    });
    if changed {
        update(hwnd);
    }
}

fn on_click(hwnd: HWND, x: i32, y: i32) {
    let dpi = APP.with_borrow(|a| a.dpi);
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut rc) };
    for (i, r) in title_buttons(rc.right, dpi).iter().enumerate() {
        if ui::contains(r, x, y) {
            unsafe {
                match i {
                    0 => {
                        ShowWindow(hwnd, SW_MINIMIZE);
                    }
                    1 => {
                        if IsZoomed(hwnd) != 0 {
                            ShowWindow(hwnd, SW_RESTORE);
                        } else {
                            ShowWindow(hwnd, SW_MAXIMIZE);
                        }
                    }
                    _ => {
                        PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    }
                }
            }
            return;
        }
    }
    for i in 0..2 {
        if ui::contains(&menu_tab(i, dpi), x, y) {
            show_menu(hwnd, i);
            return;
        }
    }
    if APP.with_borrow(|a| a.find_open) {
        let (_, btn, close) = find_widgets(rc.right, rc.bottom, dpi);
        if ui::contains(&btn, x, y) {
            find_next(hwnd);
        } else if ui::contains(&close, x, y) {
            close_find(hwnd);
        }
    }
}

// ---------------------------------------------------------------- painting

unsafe fn refresh_fonts() {
    let style_id = winrust_common::style::style() as u8;
    let (edit, find, font, small) = APP.with_borrow_mut(|a| {
        if a.fonts[0] != 0 && a.font_dpi == a.dpi && a.font_style == style_id {
            return (0, 0, 0, 0);
        }
        let new = ui::create_fonts(a.dpi);
        let old = std::mem::replace(&mut a.fonts, new);
        unsafe { ui::delete_fonts(&old) };
        a.font_dpi = a.dpi;
        a.font_style = style_id;
        (a.edit, a.find, a.fonts[F_MONO], a.fonts[F_BODY])
    });
    unsafe {
        if edit != 0 {
            SendMessageW(edit as HWND, WM_SETFONT, font as usize, 1);
        }
        if find != 0 {
            SendMessageW(find as HWND, WM_SETFONT, small as usize, 1);
        }
    }
}

unsafe fn frame(hwnd: HWND) {
    let dark = 1u32;
    unsafe { DwmSetWindowAttribute(hwnd, 20, &dark as *const u32 as *const c_void, 4) };
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        refresh_fonts();
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (cw, ch) = (rc.right, rc.bottom);
        if cw <= 0 || ch <= 0 {
            EndPaint(hwnd, &ps);
            return;
        }
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, cw, ch);
        if mem.is_null() || bmp.is_null() {
            if !bmp.is_null() {
                DeleteObject(bmp as _);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            EndPaint(hwnd, &ps);
            return;
        }
        let old = SelectObject(mem, bmp);
        let t = ui::theme();
        APP.with_borrow(|a| {
            ui::shape(mem, rc, t.bg, 0);
            paint_title(hwnd, mem, a, &t, cw);
            paint_menu_bar(mem, a, &t, cw);
            let card = editor_card(cw, ch, a.dpi, a.find_open);
            ui::shape(mem, card, t.card, p(a.dpi, 10));
            let inner = editor_inner(cw, ch, a.dpi, a.find_open);
            let edge = mix(t.card, t.text, 0.10);
            ui::hline(mem, inner.left, inner.right, inner.top - 1, edge);
            ui::hline(mem, inner.left + 8, inner.right - 8, inner.bottom, edge);
            if a.find_open {
                paint_find(mem, a, &t, cw, ch);
            }
            paint_status(mem, a, &t, cw, ch);
        });
        BitBlt(dc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp as _);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

fn paint_title(hwnd: HWND, dc: HDC, a: &App, t: &ui::Theme, cw: i32) {
    unsafe {
        let d = a.dpi;
        let bar = RECT { left: 0, top: 0, right: cw, bottom: p(d, TITLE_H) };
        ui::shape(dc, bar, t.side, 0);
        ui::hline(dc, 0, cw, p(d, TITLE_H) - 1, t.sep);
        let s = p(d, 24);
        let ir = RECT { left: p(d, 14), top: (p(d, TITLE_H) - s) / 2, right: p(d, 14) + s, bottom: (p(d, TITLE_H) + s) / 2 };
        ui::shape(dc, ir, t.accent, p(d, 6));
        ui::text(dc, a.fonts[F_ICON], "\u{E70F}", ir, on_accent(), DT_CENTER);
        let mut name = doc_name(a);
        if a.dirty {
            name.push_str(" *");
        }
        let r = RECT { left: ir.right + p(d, 10), right: cw - p(d, 3 * BTN_W) - p(d, 8), bottom: p(d, TITLE_H), ..bar };
        ui::text(dc, a.fonts[F_TITLE], &name, r, t.text, DT_LEFT);
        let maximized = IsZoomed(hwnd) != 0;
        for (i, r) in title_buttons(cw, d).iter().enumerate() {
            let hot = a.hover == Some(Hot::Title(i as u8));
            if hot {
                let fill = if i == 2 { mix(t.side, t.bad, 0.62) } else { mix(t.side, t.text, 0.09) };
                ui::shape(dc, *r, fill, 0);
            }
            let glyph = match i {
                0 => "\u{E921}",
                1 if maximized => "\u{E923}",
                1 => "\u{E922}",
                _ => "\u{E8BB}",
            };
            let ink = if hot && i == 2 { rgb(255, 255, 255) } else { t.dim };
            ui::text(dc, a.fonts[F_ICON_S], glyph, *r, ink, DT_CENTER);
        }
    }
}

fn paint_menu_bar(dc: HDC, a: &App, t: &ui::Theme, cw: i32) {
    unsafe {
        let d = a.dpi;
        let bar = RECT { left: 0, top: p(d, TITLE_H), right: cw, bottom: p(d, TITLE_H + MENU_H) };
        ui::shape(dc, bar, t.bg, 0);
        ui::hline(dc, 0, cw, bar.bottom - 1, t.sep);
        for i in 0..2 {
            let r = menu_tab(i, d);
            if a.hover == Some(Hot::MenuBar(i)) {
                ui::shape(dc, ui::inset(r, p(d, 4)), t.hover, p(d, 7));
            }
            ui::text(dc, a.fonts[F_BODY], menu_name(i), r, t.text, DT_CENTER);
        }
        if let Some(path) = &a.path {
            let r = RECT { left: menu_tab(1, d).right + p(d, 16), right: cw - p(d, 14), bottom: bar.bottom, ..bar };
            ui::text(dc, a.fonts[F_SMALL], &path.display().to_string(), r, t.dim, DT_RIGHT);
        }
    }
}

fn paint_find(dc: HDC, a: &App, t: &ui::Theme, cw: i32, ch: i32) {
    unsafe {
        let d = a.dpi;
        let (field, btn, close) = find_widgets(cw, ch, d);
        ui::shape(dc, field, mix(t.card, t.text, 0.06), p(d, 9));
        let mag = RECT { left: field.left - p(d, 26), top: field.top, right: field.left - p(d, 2), bottom: field.bottom };
        ui::text(dc, a.fonts[F_ICON_S], "\u{E721}", mag, t.dim, DT_CENTER);
        let hot = a.hover == Some(Hot::FindBtn);
        ui::shape(dc, btn, if hot { mix(t.card, t.accent, 0.30) } else { mix(t.card, t.accent, 0.16) }, p(d, 9));
        ui::text(
            dc,
            a.fonts[F_BODY],
            t!("Найти далее", "Find next", ja: "次を検索", pt: "Localizar próxima", zh: "查找下一个"),
            btn,
            t.accent,
            DT_CENTER,
        );
        if a.hover == Some(Hot::FindClose) {
            ui::shape(dc, close, t.hover, p(d, 7));
        }
        ui::text(dc, a.fonts[F_ICON_S], "\u{E8BB}", close, t.dim, DT_CENTER);
    }
}

fn paint_status(dc: HDC, a: &App, t: &ui::Theme, cw: i32, ch: i32) {
    unsafe {
        let d = a.dpi;
        let r = RECT { left: 0, top: ch - p(d, STATUS_H), right: cw, bottom: ch };
        ui::shape(dc, r, t.side, 0);
        ui::hline(dc, 0, cw, r.top, t.sep);
        let (line, col) = if a.edit != 0 { line_col(a.edit as HWND) } else { (1, 1) };
        let lc = tf!(
            "Строка {}, столбец {}",
            "Line {}, Column {}",
            ja: "{} 行、{} 列",
            pt: "Linha {}, Coluna {}",
            zh: "第 {} 行，第 {} 列",
            line,
            col
        );
        ui::text(dc, a.fonts[F_SMALL], &lc, RECT { left: p(d, 14), right: cw / 2, ..r }, t.dim, DT_LEFT);
        let (mid, ink) = match &a.note {
            Some((s, _, warn)) => (s.clone(), if *warn { t.warn } else { t.text }),
            None => (
                tf!("Длина: {}", "Length: {}", ja: "長さ: {}", pt: "Comprimento: {}", zh: "长度: {}", a.chars),
                t.dim,
            ),
        };
        ui::text(dc, a.fonts[F_SMALL], &mid, RECT { left: cw / 2 - p(d, 160), right: cw / 2 + p(d, 160), ..r }, ink, DT_CENTER);
        let right = format!("{} · {}", a.enc.label(), a.eol.name());
        ui::text(dc, a.fonts[F_SMALL], &right, RECT { left: cw - p(d, 220), right: cw - p(d, 14), ..r }, t.dim, DT_RIGHT);
    }
}

// ---------------------------------------------------------------- layout, children, window procedure

fn layout(hwnd: HWND) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (cw, ch, dpi, edit, find, open) = APP.with_borrow(|a| (rc.right, rc.bottom, a.dpi, a.edit, a.find, a.find_open));
        if edit != 0 {
            let r = editor_inner(cw, ch, dpi, open);
            MoveWindow(edit as HWND, r.left, r.top, (r.right - r.left).max(16), (r.bottom - r.top).max(16), 1);
        }
        if find != 0 {
            let r = find_edit_rect(cw, ch, dpi);
            MoveWindow(find as HWND, r.left, r.top, (r.right - r.left).max(16), (r.bottom - r.top).max(12), 1);
            ShowWindow(find as HWND, if open { SW_SHOW } else { SW_HIDE });
        }
        update(hwnd);
    }
}

unsafe extern "system" fn edit_proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe {
        let (is_editor, is_find) = (h == APP.with_borrow(|a| a.edit as HWND), h == APP.with_borrow(|a| a.find as HWND));
        if msg == WM_KEYDOWN {
            let key = wp as u16;
            let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
            let shift = GetKeyState(VK_SHIFT as i32) < 0;
            let flags = ctrl as isize | (shift as isize) << 1;
            if is_editor {
                if ctrl {
                    match key {
                        VK_Z => {
                            if shift {
                                redo(GetParent(h));
                            } else {
                                undo(GetParent(h));
                            }
                            return 0;
                        }
                        VK_Y => {
                            redo(GetParent(h));
                            return 0;
                        }
                        VK_A => {
                            SendMessageW(h, EM_SETSEL, 0, -1);
                            return 0;
                        }
                        VK_N | VK_O | VK_S | VK_F => {
                            PostMessageW(GetParent(h), WM_CHILD_KEY, wp, flags);
                            return 0;
                        }
                        _ => {}
                    }
                }
                if key == VK_F3 || key == VK_ESCAPE {
                    PostMessageW(GetParent(h), WM_CHILD_KEY, wp, flags);
                    return 0;
                }
            } else if is_find && (key == VK_RETURN || key == VK_ESCAPE || key == VK_F3 || (ctrl && matches!(key, VK_N | VK_O | VK_S | VK_F))) {
                PostMessageW(GetParent(h), WM_CHILD_KEY, wp, flags);
                return 0;
            }
        }
        if msg == WM_SYSKEYDOWN && is_editor && (lp as usize) & (1 << 29) != 0 && (wp as u16 == VK_F || wp as u16 == VK_E) {
            // Alt+F / Alt+E open the menus from the editor too; other Alt combos stay with the EDIT (Alt+Numpad)
            PostMessageW(GetParent(h), WM_SYSKEYDOWN, wp, lp);
            return 0;
        }
        if msg == WM_CHAR {
            let c = wp as u16;
            // Control characters whose key we handled ourselves; a single-line field also ignores CR and Esc.
            if (is_editor && matches!(c, 1 | 6 | 14 | 15 | 19 | 25 | 26)) || (is_find && (c == 13 || c == 27)) {
                return 0;
            }
        }
        if msg == WM_DROPFILES {
            PostMessageW(GetParent(h), WM_DROPFILES, wp, lp);
            return 0;
        }
        if (msg == WM_KEYUP || msg == WM_LBUTTONUP || msg == WM_LBUTTONDBLCLK) && is_editor {
            PostMessageW(GetParent(h), WM_CARET, 0, 0);
        }
        let prev = EDIT_PROC.load(Ordering::Relaxed);
        CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(prev), h, msg, wp, lp)
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe {
        if msg == winrust_common::style::msg() {
            winrust_common::style::reload();
            refresh_fonts();
            frame(hwnd);
            layout(hwnd);
            return 0;
        }
        match msg {
            WM_CREATE => {
                let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
                APP.with_borrow_mut(|a| a.dpi = dpi);
                refresh_fonts();
                DragAcceptFiles(hwnd, 1);
                create_children(hwnd);
                layout(hwnd);
                0
            }
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_ERASEBKGND => 1,
            WM_SIZE => {
                layout(hwnd);
                0
            }
            WM_DPICHANGED => {
                let dpi = ((wp >> 16) & 0xFFFF) as i32;
                if dpi > 0 {
                    APP.with_borrow_mut(|a| a.dpi = dpi);
                }
                refresh_fonts();
                let r = &*(lp as *const RECT);
                SetWindowPos(hwnd, null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                layout(hwnd);
                0
            }
            WM_SETTINGCHANGE => {
                refresh_fonts();
                layout(hwnd);
                0
            }
            WM_GETMINMAXINFO => {
                let mm = &mut *(lp as *mut MINMAXINFO);
                let d = APP.with_borrow(|a| a.dpi);
                mm.ptMinTrackSize = POINT { x: p(d, 520), y: p(d, 360) };
                let mon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
                let mut mi: MONITORINFO = std::mem::zeroed();
                mi.cbSize = size_of::<MONITORINFO>() as u32;
                if !mon.is_null() && GetMonitorInfoW(mon, &mut mi) != 0 {
                    mm.ptMaxPosition = POINT { x: mi.rcWork.left - mi.rcMonitor.left, y: mi.rcWork.top - mi.rcMonitor.top };
                    mm.ptMaxSize = POINT { x: mi.rcWork.right - mi.rcWork.left, y: mi.rcWork.bottom - mi.rcWork.top };
                }
                0
            }
            WM_NCHITTEST => {
                let hit = DefWindowProcW(hwnd, msg, wp, lp);
                if hit == HTCLIENT as isize {
                    let mut pt = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
                    ScreenToClient(hwnd, &mut pt);
                    let d = APP.with_borrow(|a| a.dpi);
                    let mut rc: RECT = std::mem::zeroed();
                    GetClientRect(hwnd, &mut rc);
                    if pt.y >= 0 && pt.y < p(d, TITLE_H) && !title_buttons(rc.right, d).iter().any(|b| ui::contains(b, pt.x, pt.y)) {
                        return HTCAPTION as isize;
                    }
                }
                hit
            }
            WM_NCMOUSEMOVE => {
                let had = APP.with_borrow_mut(|a| a.hover.take().is_some());
                if had {
                    update(hwnd);
                }
                0
            }
            WM_MOUSEMOVE => {
                on_move(hwnd, (lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                0
            }
            WM_MOUSELEAVE => {
                let had = APP.with_borrow_mut(|a| a.hover.take().is_some());
                if had {
                    update(hwnd);
                }
                0
            }
            WM_LBUTTONDOWN => {
                on_click(hwnd, (lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
                0
            }
            WM_KEYDOWN => {
                on_key(hwnd, wp as u16);
                0
            }
            WM_SYSKEYDOWN => {
                if (lp as usize) & (1 << 29) != 0 && wp as u16 == VK_F {
                    show_menu(hwnd, 0);
                    return 0;
                }
                if (lp as usize) & (1 << 29) != 0 && wp as u16 == VK_E {
                    show_menu(hwnd, 1);
                    return 0;
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_CHILD_KEY => {
                let key = wp as u16;
                let ctrl = lp & 1 != 0;
                let shift = lp & 2 != 0;
                match key {
                    VK_RETURN if !ctrl => find_next(hwnd),
                    VK_ESCAPE => close_find(hwnd),
                    VK_N if ctrl => apply(hwnd, Cmd::New),
                    VK_O if ctrl => apply(hwnd, Cmd::Open),
                    VK_S if ctrl => apply(hwnd, if shift { Cmd::SaveAs } else { Cmd::Save }),
                    VK_F if ctrl => open_find(hwnd),
                    VK_F3 => find_next(hwnd),
                    _ => {}
                }
                0
            }
            WM_CARET => {
                update(hwnd);
                0
            }
            WM_COMMAND => {
                let id = wp & 0xFFFF;
                let code = ((wp >> 16) & 0xFFFF) as u16;
                if id == ID_EDIT && code == EN_CHANGE_CODE {
                    on_change(hwnd);
                }
                0
            }
            WM_CTLCOLOREDIT => {
                let t = ui::theme();
                let is_find = lp as HWND == APP.with_borrow(|a| a.find as HWND);
                let bg = if is_find { mix(t.card, t.text, 0.06) } else { t.card };
                SetTextColor(wp as HDC, t.text);
                SetBkColor(wp as HDC, bg);
                let b = CreateSolidBrush(bg);
                let old = APP.with_borrow_mut(|a| std::mem::replace(&mut a.brush, b as isize));
                if old != 0 {
                    DeleteObject(old as _);
                }
                b as isize
            }
            WM_TIMER if wp == TIMER_NOTE => {
                let now = GetTickCount64();
                let expired = APP.with_borrow(|a| a.note.as_ref().is_none_or(|n| now.saturating_sub(n.1) > NOTE_MS));
                if expired {
                    APP.with_borrow_mut(|a| a.note = None);
                    KillTimer(hwnd, TIMER_NOTE);
                    update(hwnd);
                }
                0
            }
            WM_DROPFILES => {
                drop_files(hwnd, wp as HDROP);
                0
            }
            WM_CLOSE => {
                if confirm_discard(hwnd) {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_DESTROY => {
                let (brush, fonts) = APP.with_borrow_mut(|a| {
                    let b = std::mem::replace(&mut a.brush, 0);
                    let f = a.fonts;
                    a.fonts = [0; FONT_COUNT];
                    (b, f)
                });
                if brush != 0 {
                    DeleteObject(brush as _);
                }
                ui::delete_fonts(&fonts);
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe fn create_children(hwnd: HWND) {
    unsafe {
        let (dpi, mono, body) = APP.with_borrow(|a| (a.dpi, a.fonts[F_MONO], a.fonts[F_BODY]));
        let inst = GetModuleHandleW(null());
        let edit_style = WS_CHILD
            | WS_VISIBLE
            | WS_VSCROLL
            | WS_HSCROLL
            | (ES_MULTILINE as u32)
            | (ES_AUTOVSCROLL as u32)
            | (ES_AUTOHSCROLL as u32)
            | (ES_NOHIDESEL as u32)
            | (ES_LEFT as u32);
        let edit = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            null(),
            edit_style,
            0,
            0,
            16,
            16,
            hwnd,
            ID_EDIT as *mut c_void,
            inst,
            null(),
        );
        if !edit.is_null() {
            SendMessageW(edit, WM_SETFONT, mono as usize, 1);
            SendMessageW(edit, EM_SETLIMITTEXT, 0x7FFF_FFFE, 0);
            SendMessageW(edit, EM_SETMARGINS, 3, ((p(dpi, 8) << 16) | p(dpi, 8)) as isize);
            SetWindowTheme(edit, wide("DarkMode_Explorer").as_ptr(), null());
            EDIT_PROC.store(SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize), Ordering::Relaxed);
            DragAcceptFiles(edit, 1);
            APP.with_borrow_mut(|a| a.edit = edit as isize);
        }
        let find_style = WS_CHILD | (ES_AUTOHSCROLL as u32) | (ES_LEFT as u32);
        let find = CreateWindowExW(0, wide("EDIT").as_ptr(), null(), find_style, 0, 0, 16, 16, hwnd, ID_FIND as *mut c_void, inst, null());
        if !find.is_null() {
            SendMessageW(find, WM_SETFONT, body as usize, 1);
            SendMessageW(find, EM_SETLIMITTEXT, 400, 0);
            SendMessageW(find, EM_SETMARGINS, 3, ((p(dpi, 6) << 16) | p(dpi, 6)) as isize);
            let cue = wide(t!("Поиск", "Search", ja: "検索", pt: "Localizar", zh: "搜索"));
            SendMessageW(find, EM_SETCUEBANNER, 1, cue.as_ptr() as isize);
            SetWindowTheme(find, wide("DarkMode_Explorer").as_ptr(), null());
            SetWindowLongPtrW(find, GWLP_WNDPROC, edit_proc as *const () as isize);
            DragAcceptFiles(find, 1);
            APP.with_borrow_mut(|a| a.find = find as isize);
        }
    }
}

// ---------------------------------------------------------------- entry point

/// The file to open: the first path argument, flags skipped.
///
/// IFEO starts a replacement as `<image> <victim> [file]`, so `C:\Windows\System32\notepad.exe` arrives as
/// argv[1] and the user's file as argv[2]. That victim path (and a first argument that does not exist
/// while another one does) is dropped; a lone non-existing path is a new file, created on the first save.
/// Quotes are trimmed because a hand-written `Debugger` value can leave a literal `"` in the argument.
fn cli_file() -> Option<PathBuf> {
    path_from_args(std::env::args_os().skip(1))
}

fn path_from_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<PathBuf> {
    let victim = |p: &Path| p.file_name().is_some_and(|f| f.to_string_lossy().eq_ignore_ascii_case("notepad.exe"));
    let mut paths: Vec<PathBuf> = Vec::new();
    for a in args {
        let s = a.to_string_lossy();
        let s = s.trim().trim_matches('"').trim();
        // `/?`, `--help`, `-h`, `-`: nothing to open, and not a file name either
        if s.is_empty() || s.starts_with('/') || s.starts_with("--") || s == "-" || s == "-h" || s == "-?" {
            continue;
        }
        paths.push(PathBuf::from(s));
    }
    if paths.len() > 1 && (victim(&paths[0]) || !paths[0].exists()) {
        paths.remove(0);
    }
    let first = paths.into_iter().next()?;
    // A `notepad.exe` path is IFEO's victim, never a file to edit: with nothing behind it, an empty document
    if victim(&first) { None } else { Some(first) }
}

fn main() {
    winrust_common::appid::set("WinRust"); // group every WinRust process under one Task Manager entry
    unsafe {
        let mut token = 0usize;
        let mut input: GdiplusStartupInput = std::mem::zeroed();
        input.GdiplusVersion = 1;
        let _ = GdiplusStartup(&mut token, &input, null_mut());
        SetProcessDPIAware();
        let dpi = (GetDpiForSystem() as i32).max(96);
        APP.with_borrow_mut(|a| a.dpi = dpi);
        refresh_fonts();
        let inst = GetModuleHandleW(null());
        let icon = LoadIconW(inst, 1 as *const u16);
        let class = wide("WinRustNotepad");
        let wc = WNDCLASSW {
            style: CS_DBLCLKS,
            lpfnWndProc: Some(proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: inst,
            hIcon: icon,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let (want_w, want_h) = (p(dpi, 1000), p(dpi, 700));
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let (w, h) = (want_w.min(sw - p(dpi, 40)).max(p(dpi, 520)), want_h.min(sh - p(dpi, 60)).max(p(dpi, 360)));
        // the monitor under the cursor, not the primary one: two screens and the cursor is on the right
        let (x, y, cw, ch) = winrust_common::place::centered(w, h);
        let hwnd = CreateWindowExW(
            WS_EX_APPWINDOW,
            class.as_ptr(),
            wide(APP_NAME).as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU | WS_CLIPCHILDREN | WS_VISIBLE,
            x,
            y,
            cw,
            ch,
            null_mut(),
            null_mut(),
            inst,
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, icon as isize);
        SendMessageW(hwnd, WM_SETICON, ICON_SMALL as usize, icon as isize);
        frame(hwnd);
        if let Some(path) = cli_file() {
            open_initial(hwnd, &path);
        }
        ShowWindow(hwnd, SW_SHOW);
        let edit = APP.with_borrow(|a| a.edit as HWND);
        SetFocus(edit);
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        GdiplusShutdown(token);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn args(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rust-notepad-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn ifeo_victim_comes_before_the_file() {
        let f = temp("ifeo.txt");
        std::fs::write(&f, b"x").unwrap();
        let victim = r"C:\Windows\System32\notepad.exe";
        assert_eq!(path_from_args(args(&[victim, &f.to_string_lossy()])), Some(f.clone()));
        // the victim alone (notepad started without a file) is an empty document, not the binary
        assert_eq!(path_from_args(args(&[victim])), None);
        let _ = std::fs::remove_file(f);
    }

    #[test]
    fn a_missing_first_argument_gives_way_to_an_existing_second() {
        let existing = temp("second.txt");
        std::fs::write(&existing, b"x").unwrap();
        let missing = temp("missing.txt");
        let _ = std::fs::remove_file(&missing);
        assert_eq!(path_from_args(args(&[&missing.to_string_lossy(), &existing.to_string_lossy()])), Some(existing.clone()));
        // with nothing behind it, the missing path is the new file (created on save)
        assert_eq!(path_from_args(args(&[&missing.to_string_lossy()])), Some(missing.clone()));
        let _ = std::fs::remove_file(existing);
    }

    #[test]
    fn quotes_spaces_and_help() {
        assert_eq!(path_from_args(args(&["\"C:\\Program Files\\a b.txt\""])), Some(PathBuf::from(r"C:\Program Files\a b.txt")));
        assert_eq!(path_from_args(args(&["/?", "--help", "-h", "-?"])), None);
        assert_eq!(path_from_args(args(&["--help", r"C:\Temp\a.txt"])), Some(PathBuf::from(r"C:\Temp\a.txt")));
    }
}