// rust-explorer [path]  — file manager window (Explorer replacement).
// Virtual list (LVS_OWNERDATA, only visible rows are asked for text/icon), system icons, tabs,
// IFileOperation (recycle bin / undo / elevation prompts), our own context menu with icons (its "More options…" row
// opens the real shell menu for the verbs it does not carry), OLE drag and drop.
// Keys: Enter open, Backspace up, F2 rename, Del recycle, Ctrl+C/X/V copy/cut/paste, Ctrl+N new folder,
// Ctrl+T/W/Tab tabs, Ctrl+M icon grid/details, Ctrl+L/F4 address bar, Ctrl+F or "? terms" in the bar = instant search through the rs service, right click / Menu key = our context menu.
// Look: topbar's palette, painted title row (tabs + window buttons) / toolbar / address pill and status line (src/ui.rs),
// dark frame and list.
// Clipboard is the system one (Ctrl+C/X/V interoperate with Explorer). Also: Alt+Left/Right back/forward, Alt+Up up,
// Alt+Enter properties, F5 refresh, Ctrl+H hidden files, click a column header to sort, right click on empty space = folder menu.
// shortcuts: std::fs listing (not FindFirstFileEx large fetch), icons are extension-based (exe/lnk/ico/url/scr
// per file) and synchronous (thumbnails: one background thread, 48px, no disk cache), internal copy/cut clipboard, silent rename errors,
// search blocks the UI while it waits for the service (no async), the service pipe is admin-only so run as admin,
// drops always land in the current folder (not on a folder row), tabs remember only the folder (no selection/scroll).
// upgrade: each of those one at a time; then register as the folder-open handler.
// rust-explorer --desktop  - the desktop for rs-shell: borderless bottom-most color-keyed window with the Desktop folders
//   (user + public) as big icons over the wallpaper it paints itself (without explorer nothing else does): reads the BMP
//   that rs-shell converts into %LOCALAPPDATA%\rs-shell\wallpaper.bmp, "fill" style, scaled once at start.
//   Exposes the window hierarchy wallpaper apps look for: Progman titled "Program Manager" (+ property desktop::MARKER),
//   SHELLDLL_DefView, the WorkerW that 0x052C spawns, and one wallpaper host per monitor. With look.txt `we=1` the WE
//   bridge (we.rs) opens WE's pop-out per monitor and reparents it into those hosts; off it just paints the picture.
//   shortcuts: wallpaper is loaded once (changing it needs a restart), icon labels are always white, no background menu /
//   drag-to-position / auto-arrange, items from the public desktop can't be renamed.
#![windows_subsystem = "windows"]
use std::{cell::RefCell, env, fs, os::windows::fs::MetadataExt, path::{Path, PathBuf}, ptr, sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed}};
use winrust_common::{t, tf};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::{ClientToScreen, GetStockObject, InvalidateRect, ScreenToClient}, System::LibraryLoader::GetModuleHandleW, UI::{Controls::*, Input::KeyboardAndMouse::{GetKeyState, ReleaseCapture, SetCapture, SetFocus, VK_CONTROL}, Shell::*, WindowsAndMessaging::*}};

mod zipx; // our own ZIP engine (deflate, inflate, container)
mod jobs; // our own copier, ZIP packer, ZIP/7z unpacker (background, progress in the status line)
mod shellwin; // registers the window as a shell window: "Show in folder" from other apps selects the file
mod ui; // the look: palette, painted tabs / toolbar / address pill / status line, dark theme
mod menu; // the context menu: rounded, blurred, with icons and shortcuts (the Win32 one has neither)
mod openwith; // our own "Open with" picker: the Windows one never renders without explorer.exe
mod we; // Wallpaper Engine behind the icons, through its official -control command line (look.txt we=1)
mod marquee; // the selection band ("lazo"): translucent white wash, for the desktop icons and the file list
mod drives; // the drive list, read on a worker and cached: querying a dead volume must never block a window

static DESKTOP: AtomicBool = AtomicBool::new(false);
fn desk() -> bool { DESKTOP.load(Relaxed) }
const CLR_NONE: isize = 0xFFFF_FFFF; // list background / text background: let the parent's wallpaper show through
thread_local! { static WALL: std::cell::Cell<(isize, i32, i32)> = const { std::cell::Cell::new((0, 0, 0)) }; } // (HBITMAP, w, h)
thread_local! {
    /// The desktop list's own icon list, as (ours, the shell's, px). The icons are the same size as the shell's, but each
    /// one is rendered from the sharpest asset the shell has (`winrust_common::icon`) instead of the single bitmap
    /// shell32 keeps for every list. px = 0: the shell's list is the one attached (small-icons mode).
    static DESK_IL: std::cell::Cell<(isize, isize, i32)> = const { std::cell::Cell::new((0, 0, 0)) };
    static DESK_ICON_AT: RefCell<std::collections::HashMap<String, i32>> = RefCell::new(std::collections::HashMap::new());
}
/// The virtual screen (every monitor): x, y, width, height. The desktop window covers it so each monitor gets its wallpaper.
fn virt() -> (i32, i32, i32, i32) {
    unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) }
}
/// The desktop's icon list stays on the primary monitor (at screen 0,0, i.e. -vx, -vy inside the desktop window), between the bar's strips.
fn place_desk_list(list: HWND) {
    let (vx, vy, ..) = virt();
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    unsafe { MoveWindow(list, -vx, -vy + desk_top(), sw, (sh - desk_top() - desk_bottom()).max(0), 1) };
}
/// Renders the wallpaper for the current monitor layout (at start and when monitors change), freeing the old one.
fn reload_wall() {
    let (vx, vy, cw, ch) = virt();
    let old = WALL.replace((wall::load(vx, vy, cw, ch), cw, ch)).0;
    if old != 0 { unsafe { windows_sys::Win32::Graphics::Gdi::DeleteObject(old as _) }; }
}
/// Where the file manager window was when it last closed: %APPDATA%\rust-explorer\window.txt, `max|normal left top right bottom`
/// (the restored rectangle, so un-maximizing goes back to the size it had).
fn place_file() -> PathBuf {
    PathBuf::from(env::var_os("APPDATA").unwrap_or_default()).join(r"rust-explorer\window.txt")
}
fn parse_place(s: &str) -> Option<(bool, RECT)> {
    let mut it = s.split_whitespace();
    let max = match it.next()? { "max" => true, "normal" => false, _ => return None };
    let n: Vec<i32> = it.map(str::parse).collect::<Result<_, _>>().ok()?;
    let [left, top, right, bottom] = n[..] else { return None };
    (right - left >= 200 && bottom - top >= 150).then_some((max, RECT { left, top, right, bottom })) // junk or a collapsed window: default size
}
fn save_place(h: HWND) {
    let mut wp: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    wp.length = size_of::<WINDOWPLACEMENT>() as u32;
    if unsafe { GetWindowPlacement(h, &mut wp) } == 0 { return; }
    let r = wp.rcNormalPosition;
    let f = place_file();
    let _ = fs::create_dir_all(f.parent().unwrap_or(&f));
    let _ = fs::write(f, format!("{} {} {} {} {}", if wp.showCmd == SW_SHOWMAXIMIZED as u32 { "max" } else { "normal" }, r.left, r.top, r.right, r.bottom));
}
/// The saved place, if it is still on a monitor (one may have been unplugged since).
fn saved_place() -> Option<(bool, RECT)> {
    use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect};
    let (max, r) = parse_place(&fs::read_to_string(place_file()).ok()?)?;
    (!unsafe { MonitorFromRect(&r, MONITOR_DEFAULTTONULL) }.is_null()).then_some((max, r))
}
/// What the bar reserves at the top and bottom of the screen: the work area topbar sets when there is no explorer.
fn work_strips() -> (i32, i32) {
    let mut r: RECT = unsafe { std::mem::zeroed() };
    if unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut _ as *mut _, 0) } == 0 { return (0, 0); }
    (r.top.max(0), (unsafe { GetSystemMetrics(SM_CYSCREEN) } - r.bottom).max(0))
}
/// Margins of the desktop's icon list, from the bar's real size: its reserved strip (any height, top or bottom).
/// The dock style reserves nothing but floats over the bottom edge, so it gets a fixed gap while topbar runs.
fn desk_top() -> i32 { work_strips().0 + ui::sc(8) }
fn desk_bottom() -> i32 {
    let reserved = work_strips().1;
    if reserved > 0 { return reserved + ui::sc(4); }
    let bar_up = unsafe { !FindWindowW(w("topbar").as_ptr(), ptr::null()).is_null() };
    if bar_up && winrust_common::style::style() == winrust_common::style::Style::Dock { ui::sc(52) } else { ui::sc(8) }
}
const THUMB: i32 = 48; // the icon grid's cells (half of the old 96: icons, spacing and labels shrink together)
const WM_THUMB: u32 = WM_APP + 2; // worker -> UI: wp = Box<(era, row, HBITMAP)>
static GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0); // bumped on every new listing; stale thumbnails are dropped
static TX: std::sync::OnceLock<std::sync::mpsc::Sender<(u32, usize, PathBuf)>> = std::sync::OnceLock::new();
const WM_GO: u32 = WM_APP + 1; // address bar: Enter (wp 13) / Esc (wp 27)
static EDIT_OLD: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static LABEL_OLD: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0); // the rename box (only one exists at a time)
static LIST_OLD: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0); // the file list (wheel: the nav pane)

struct Row {
    name: String, // what the list shows
    dir: bool,
    size: u64,
    time: u64, // FILETIME ticks
    icon: i32, // image-list index; -1 = ask the shell
    thumb: i32, // -2 not asked, -1 asked, >=0 image-list index
    hdr: bool,
    path: Option<PathBuf>, // absolute target when `name` is not a child of the folder shown (a drive in "This PC")
    cap: Option<(u64, u64)>, // (free, total) bytes of a drive: the size column shows both instead of a file size
    trash: Option<Trash>, // a row of the Recycle Bin: where its payload and metadata live
}

/// The two files Windows keeps for a deleted item: `$R...` is the payload, `$I...` the metadata (original path, size, date).
#[derive(Clone)]
struct Trash { r: PathBuf, i: PathBuf }

impl Row {
    /// Where the row really points: `dir.join(name)` in a folder, its own absolute path for a drive.
    fn full(&self, dir: &Path) -> PathBuf { self.path.clone().unwrap_or_else(|| dir.join(&self.name)) }
}

/// "This PC" is a view, not a folder: a marker no real path can be (the shell's own CLSID for My Computer).
const THIS_PC: &str = "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}";
/// The Recycle Bin is a view too (CLSID_RecycleBin): its rows come from the `$Recycle.Bin` folders of every drive.
const TRASH: &str = "::{645FF040-5081-101B-9F08-00AA002F954E}";

fn this_pc() -> PathBuf { PathBuf::from(THIS_PC) }
fn is_this_pc(p: &Path) -> bool { p.to_string_lossy().eq_ignore_ascii_case(THIS_PC) }
fn trash_dir() -> PathBuf { PathBuf::from(TRASH) }
fn is_trash(p: &Path) -> bool { p.to_string_lossy().eq_ignore_ascii_case(TRASH) }
/// A view that is not a folder: the folder verbs (new, paste, delete, rename) do not apply to it.
fn virtual_dir(p: &Path) -> bool { is_this_pc(p) || is_trash(p) }
/// The current view is "This PC": a drive list, so the folder verbs (new, paste, delete, rename) do not apply.
fn in_pc() -> bool { is_this_pc(&cur_dir()) }
fn in_virtual() -> bool { virtual_dir(&cur_dir()) }

/// What the address bar, the tab and the taskbar say for `dir`.
fn display_dir(dir: &Path) -> String {
    if is_this_pc(dir) { t!("Этот компьютер", "This PC", ja: "PC", pt: "Este Computador", zh: "此电脑").to_string() }
    else if is_trash(dir) { t!("Корзина", "Recycle Bin", ja: "ごみ箱", pt: "Lixeira", zh: "回收站").to_string() }
    else { dir.to_string_lossy().into_owned() }
}

/// The kind of drive behind `ty` (a GetDriveTypeW result), for drives without a volume label.
fn drive_kind(ty: u32) -> String {
    match ty {
        2 => t!("Съемный диск", "Removable Disk", ja: "リムーバブル ディスク", pt: "Disco removível", zh: "可移动磁盘").to_string(),
        3 => t!("Локальный диск", "Local Disk", ja: "ローカル ディスク", pt: "Disco local", zh: "本地磁盘").to_string(),
        4 => t!("Сетевой диск", "Network Drive", ja: "ネットワーク ドライブ", pt: "Unidade de rede", zh: "网络驱动器").to_string(),
        5 => t!("Компакт-диск", "CD Drive", ja: "CD ドライブ", pt: "Unidade de CD", zh: "光盘驱动器").to_string(),
        _ => t!("Диск", "Drive", ja: "ドライブ", pt: "Unidade", zh: "驱动器").to_string(),
    }
}

/// Every drive Windows knows, as the rows of "This PC": label, icon, free and total space, no date. The data comes from
/// `drives`' cache: this runs on the UI thread and asking a volume about itself can block on a dead device.
fn drives() -> Vec<Row> {
    drives::cached()
        .into_iter()
        .map(|d| {
            let base = if d.label.is_empty() { drive_kind(d.ty) } else { d.label };
            Row {
                name: format!("{base} ({}:)", d.letter),
                dir: true,
                size: 0,
                time: 0,
                icon: d.icon,
                thumb: -2,
                hdr: false,
                path: Some(PathBuf::from(&d.root)),
                cap: (d.total > 0).then_some((d.free, d.total)),
                trash: None,
            }
        })
        .collect()
}

/// The "Type" column of a row: a drive says what kind it is, everything else goes by its name.
fn row_type(r: &Row) -> String {
    match &r.path {
        Some(p) if r.dir => {
            let root = p.to_string_lossy();
            let ty = drives::cached().iter().find(|d| d.root.eq_ignore_ascii_case(&root)).map_or(3, |d| d.ty);
            drive_kind(ty)
        }
        _ => type_text(&r.name, r.dir),
    }
}

/// The "Size" column of a row: a drive shows how much is free of how much there is, a folder nothing.
fn row_size(r: &Row) -> String {
    match r.cap {
        Some((free, total)) => tf!("{} свободно из {}", "{} free of {}", ja: "{} 空き / {}", pt: "{} livres de {}", zh: "{} 可用，共 {}", size_text(free), size_text(total)),
        None if r.dir => String::new(),
        None => size_text(r.size),
    }
}

// ---------- Recycle Bin: a view of its own, filled from the $Recycle.Bin folders of every drive ----------

static TRASH_LOADING: AtomicBool = AtomicBool::new(false);
static TRASH_RESTORING: AtomicBool = AtomicBool::new(false); // one restore at a time: a dead device must not pile workers up

/// What a `$I` file says: (size at deletion, deletion time, original path). Version 2 stores the path length,
/// version 1 (Vista / 7) a fixed 520-byte buffer; both are UTF-16.
fn parse_trash_i(path: &Path) -> Option<(u64, u64, PathBuf)> {
    let d = fs::read(path).ok()?;
    let num = |at: usize| -> Option<u64> {
        let b: [u8; 8] = d.get(at..at + 8)?.try_into().ok()?;
        Some(u64::from_le_bytes(b))
    };
    let (ver, size, time) = (num(0)?, num(8)?, num(16)?);
    let mut w: Vec<u16> = vec![];
    if ver == 2 {
        let n = u32::from_le_bytes(d.get(24..28)?.try_into().ok()?) as usize;
        let bytes = d.get(28..28usize.checked_add(n.checked_mul(2)?)?)?; // n UTF-16 units
        w.extend(bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
    } else {
        for c in d.get(24..)?.chunks_exact(2) {
            let u = u16::from_le_bytes([c[0], c[1]]);
            if u == 0 { break; }
            w.push(u);
        }
    }
    let s = String::from_utf16_lossy(&w);
    // version 2 stores the path length *including its terminating NUL*: a PathBuf with that NUL makes every Win32 call
    // fail ("strings passed to WinAPI cannot contain NULs"), which is exactly why Restore never worked.
    let s = s.split('\0').next().unwrap_or("").to_string();
    if s.is_empty() { return None; }
    Some((size, time, PathBuf::from(s)))
}

/// Every deleted item found: `$R` is the payload, `$I` its metadata; the row shows the original path (absolute, so the
/// list shows its name and puts the folder it came from in the Type column, like a search hit).
fn trash_rows() -> Vec<Row> {
    let mut out = vec![];
    for bin in drives::trash_bins() {
        let Ok(sids) = fs::read_dir(&bin) else { continue }; // the bin of another user may be closed to us: skip it
        for sid in sids.flatten() {
            let Ok(entries) = fs::read_dir(sid.path()) else { continue };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some(id) = name.strip_prefix("$I") else { continue };
                let i = e.path();
                let r = i.with_file_name(format!("$R{id}"));
                let Some((size, time, orig)) = parse_trash_i(&i) else { continue };
                // the row shows the file name; the original path (Restore's destination) travels in `path`
                let shown = orig.file_name().map_or_else(|| orig.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned());
                out.push(Row { name: shown, dir: false, size, time, icon: -1, thumb: -2, hdr: false, path: Some(orig), cap: None, trash: Some(Trash { r, i }) });
                if out.len() >= 5000 { break; } // a bin with a million files must not fill the window
            }
        }
    }
    out.sort_by(|a, b| b.time.cmp(&a.time)); // most recently deleted first
    out
}

/// Reads the bins on a worker thread and hands the rows to the window (WM_TRASH): a big or slow drive never blocks the UI.
fn trash_scan(h: HWND) {
    if TRASH_LOADING.swap(true, Relaxed) { return; }
    let hwnd = h as isize;
    std::thread::spawn(move || {
        let rows = trash_rows();
        TRASH_LOADING.store(false, Relaxed);
        unsafe { PostMessageW(hwnd as HWND, WM_TRASH, Box::into_raw(Box::new(rows)) as usize, 0) };
    });
}

/// (original path, payload, metadata) of the selected rows of the Recycle Bin view.
fn selected_trash() -> Vec<(PathBuf, Trash)> {
    ST.with_borrow(|st| {
        let mut out = vec![];
        let mut i = -1isize;
        loop {
            i = put(st.list, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED as isize);
            let Some(r) = usize::try_from(i).ok().and_then(|i| st.rows.get(i)) else { break };
            if let Some(t) = &r.trash { out.push((r.path.clone().unwrap_or_else(|| PathBuf::from(&r.name)), t.clone())); }
        }
        out
    })
}

/// Outcome of a background Recycle Bin restore / empty / delete (WM_TRASH_DONE, lp): `what` picks the message.
#[derive(Clone, Copy, PartialEq)]
enum TrashWhat { Restored, Emptied, Deleted }

struct TrashDone { what: TrashWhat, ok: usize, bad: usize, why: Option<String> } // `why`: the last error, shown under the counts

/// Puts the selected items back where they came from on a worker thread; the folders are made again if needed. A name
/// that is taken by now gets the usual " (2)" suffix instead of overwriting anything. The result arrives as WM_TRASH_DONE.
fn trash_restore(h: HWND) {
    let picks = selected_trash();
    if picks.is_empty() {
        unsafe { ui::notice(h, t!("Выберите элементы для восстановления", "Select the items to restore", ja: "復元する項目を選択してください", pt: "Selecione os itens que deseja restaurar", zh: "请选择要还原的项目")) };
        return;
    }
    if TRASH_RESTORING.swap(true, Relaxed) {
        // A previous restore may be stuck on a device that vanished; piling up more workers would hide that even more.
        unsafe { ui::notice(h, t!("Восстановление уже выполняется", "A restore is already running", ja: "復元はすでに実行中です", pt: "Já existe uma restauração em andamento", zh: "已有还原正在进行")) };
        return;
    }
    let hwnd = h as isize;
    std::thread::spawn(move || {
        let (mut ok, mut bad, mut why) = (0usize, 0usize, None);
        for (orig, t) in picks {
            // A drive missing from the cache is gone: taking the file back would block inside the volume driver, so fail
            // fast and say which drive it is. (Before the first scan finishes the cache is empty: do not judge then.)
            if drives::ready() && !drives::holds(&orig) {
                bad += 1;
                why = Some(tf!("{}: диск недоступен", "{}: the drive is not available", ja: "{}: ドライブが利用できません", pt: "{}: a unidade não está disponível", zh: "{}：驱动器不可用", orig.display()).to_string());
                continue;
            }
            let parent = orig.parent().map_or_else(|| PathBuf::from("C:\\"), |p| p.to_path_buf());
            if let Err(e) = fs::create_dir_all(&parent) {
                bad += 1;
                why = Some(format!("{}: {e}", parent.display()));
                continue;
            }
            let name = orig.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let dest = jobs::free_name(&parent, &name, false);
            match rename_timeout(&t.r, &dest) {
                Ok(_) => { let _ = fs::remove_file(&t.i); ok += 1; }
                Err(e) => {
                    // A bin on another volume cannot be renamed across: copy + delete does it, for files only (a folder
                    // would need a recursive copy). ERROR_NOT_SAME_DEVICE = 17.
                    if e.raw_os_error() == Some(17) && t.r.is_file() {
                        match fs::copy(&t.r, &dest).and_then(|_| fs::remove_file(&t.r)) {
                            Ok(_) => { let _ = fs::remove_file(&t.i); ok += 1; }
                            Err(e2) => { bad += 1; why = Some(format!("{e2} — {}", orig.display())); }
                        }
                    } else {
                        bad += 1;
                        why = Some(format!("{e} — {}", orig.display()));
                    }
                }
            }
        }
        TRASH_RESTORING.store(false, Relaxed);
        let done = Box::new(TrashDone { what: TrashWhat::Restored, ok, bad, why });
        unsafe { PostMessageW(hwnd as HWND, WM_TRASH_DONE, 0, Box::into_raw(done) as isize) };
    });
}

/// `fs::rename`, but a device that stopped answering cannot hold the worker forever: after 20 s the blocked I/O of this
/// thread is cancelled (the same trick as the copy jobs) and the item counts as failed with the OS error.
fn rename_timeout(src: &Path, dst: &Path) -> std::io::Result<()> {
    use windows_sys::Win32::{Foundation::CloseHandle, System::{IO::CancelSynchronousIo, Threading::{GetCurrentThreadId, OpenThread, THREAD_TERMINATE}}};
    let id = unsafe { GetCurrentThreadId() };
    let done = std::sync::Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    let watch = std::thread::spawn(move || {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < end {
            if flag.load(Relaxed) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        unsafe {
            let t = OpenThread(THREAD_TERMINATE, 0, id);
            if !t.is_null() {
                CancelSynchronousIo(t);
                CloseHandle(t);
            }
        }
    });
    let r = fs::rename(src, dst);
    done.store(true, Relaxed);
    let _ = watch.join();
    r
}

/// Deletes the payload and the metadata of every item in every bin, after a confirmation. The walk runs on a worker (a
/// bin with 200 GB in it would freeze the window); the outcome comes back as WM_TRASH_DONE and goes to the status line.
fn trash_empty(h: HWND) {
    unsafe {
        let q = t!("Удалить все элементы корзины безвозвратно?", "Permanently delete all items in the Recycle Bin?", ja: "ごみ箱のすべての項目を完全に削除しますか?", pt: "Excluir permanentemente todos os itens da Lixeira?", zh: "要永久删除回收站中的所有项目吗？");
        let title = display_dir(&trash_dir());
        if MessageBoxW(h, w(q).as_ptr(), w(&title).as_ptr(), MB_YESNO | MB_ICONWARNING) != IDYES { return; }
    }
    let hwnd = h as isize;
    std::thread::spawn(move || {
        let (mut n, mut bad) = (0usize, 0usize);
        for bin in drives::trash_bins() {
            let Ok(sids) = fs::read_dir(&bin) else { bad += 1; continue };
            for sid in sids.flatten() {
                let Ok(entries) = fs::read_dir(sid.path()) else { bad += 1; continue };
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !(name.starts_with("$I") || name.starts_with("$R")) { continue; }
                    let p = e.path();
                    let done = if p.is_dir() { fs::remove_dir_all(&p).is_ok() } else { fs::remove_file(&p).is_ok() };
                    if done { n += 1 } else { bad += 1 }
                }
            }
        }
        let done = Box::new(TrashDone { what: TrashWhat::Emptied, ok: n, bad, why: None });
        unsafe { PostMessageW(hwnd as HWND, WM_TRASH_DONE, 0, Box::into_raw(done) as isize) };
    });
}

/// Permanently deletes the selected rows of the Recycle Bin (payload and metadata), after a confirmation. Same worker
/// and WM_TRASH_DONE path as Empty, so the list reloads and the status line reports what happened.
fn trash_delete(h: HWND) {
    let picks = selected_trash();
    if picks.is_empty() {
        unsafe { ui::notice(h, t!("Выберите элементы для удаления", "Select the items to delete", ja: "削除する項目を選択してください", pt: "Selecione os itens que deseja excluir", zh: "请选择要删除的项目")) };
        return;
    }
    unsafe {
        let q = tf!("Удалить выбранные элементы безвозвратно ({})?", "Permanently delete the selected items ({})?", ja: "選択した項目を完全に削除しますか ({} 件)?", pt: "Excluir permanentemente os itens selecionados ({})?", zh: "要永久删除所选项（{} 项）吗？", picks.len());
        let title = display_dir(&trash_dir());
        if MessageBoxW(h, w(&q).as_ptr(), w(&title).as_ptr(), MB_YESNO | MB_ICONWARNING) != IDYES {
            return;
        }
    }
    let hwnd = h as isize;
    std::thread::spawn(move || {
        let (mut ok, mut bad) = (0usize, 0usize);
        for (_, t) in picks {
            let gone = if t.r.is_dir() { fs::remove_dir_all(&t.r).is_ok() } else { fs::remove_file(&t.r).is_ok() };
            let meta = fs::remove_file(&t.i).is_ok();
            if gone && meta {
                ok += 1
            } else {
                bad += 1
            }
        }
        let done = Box::new(TrashDone { what: TrashWhat::Deleted, ok, bad, why: None });
        unsafe { PostMessageW(hwnd as HWND, WM_TRASH_DONE, 0, Box::into_raw(done) as isize) };
    });
}

/// Right click on the Recycle Bin in the left pane: open it, or empty it (Empty asks for confirmation itself).
fn nav_bin_menu(h: HWND) {
    use crate::menu::{Icon, Item};
    let items = vec![
        Item::new(1, t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开")).icon(Icon::Glyph("\u{E8E5}")),
        Item::new(2, t!("Очистить корзину", "Empty Recycle Bin", ja: "ごみ箱を空にする", pt: "Esvaziar Lixeira", zh: "清空回收站")).icon(Icon::Glyph("\u{E74D}")),
    ];
    let mut pt: POINT = unsafe { std::mem::zeroed() };
    unsafe { GetCursorPos(&mut pt) };
    match unsafe { crate::menu::show(pt.x, pt.y, items) } {
        1 => load(h, trash_dir()),
        2 => trash_empty(h),
        _ => {}
    }
}

/// The Recycle Bin menu: on a row it offers Restore, Delete, Empty and Refresh. Our own menu, like the other ones.
fn trash_menu(h: HWND, on_row: bool) {
    use crate::menu::{Icon, Item};
    let mut items = vec![];
    if on_row {
        items.push(Item::new(1, t!("Восстановить", "Restore", ja: "復元", pt: "Restaurar", zh: "还原")).icon(Icon::Glyph("\u{E777}")));
        items.push(Item::new(4, t!("Удалить безвозвратно", "Delete permanently", ja: "完全に削除", pt: "Excluir permanentemente", zh: "永久删除")).icon(Icon::Glyph("\u{E711}")));
    }
    items.push(Item::new(2, t!("Очистить корзину", "Empty Recycle Bin", ja: "ごみ箱を空にする", pt: "Esvaziar Lixeira", zh: "清空回收站")).icon(Icon::Glyph("\u{E74D}")));
    items.push(Item::new(3, t!("Обновить", "Refresh", ja: "最新の情報に更新", pt: "Atualizar", zh: "刷新")).icon(Icon::Glyph("\u{E72C}")).hint("F5"));
    let mut pt: POINT = unsafe { std::mem::zeroed() };
    unsafe { GetCursorPos(&mut pt) };
    let id = unsafe { crate::menu::show(pt.x, pt.y, items) };
    match id {
        1 => trash_restore(h),
        2 => trash_empty(h),
        3 => reload(h),
        4 => trash_delete(h),
        _ => {}
    }
}

/// Back / forward trail of visited folders (one for the whole window, not per tab).
#[derive(Default)]
struct Hist { v: Vec<PathBuf>, pos: usize }
impl Hist {
    fn visit(&mut self, p: PathBuf) {
        if self.v.get(self.pos) == Some(&p) { return; }
        self.v.truncate(self.pos + usize::from(!self.v.is_empty())); // a new visit drops the forward trail
        self.v.push(p);
        self.pos = self.v.len() - 1;
    }
    fn back(&mut self) -> Option<PathBuf> { if self.pos > 0 { self.pos -= 1; Some(self.v[self.pos].clone()) } else { None } }
    fn forward(&mut self) -> Option<PathBuf> { if self.pos + 1 < self.v.len() { self.pos += 1; Some(self.v[self.pos].clone()) } else { None } }
}

struct State { list: HWND, tab: HWND, edit: HWND, dir: PathBuf, rows: Vec<Row>, thumbs: bool, himl: isize, tabs: Vec<PathBuf>, active: usize, hist: Hist }
thread_local! {
    static ST: RefCell<State> = RefCell::new(State { list: 0 as _, tab: 0 as _, edit: 0 as _, dir: PathBuf::new(), rows: vec![], thumbs: false, himl: 0, tabs: vec![], active: 0, hist: Hist { v: vec![], pos: 0 } });
}
static SORT_COL: AtomicUsize = AtomicUsize::new(0); // 0 name, 1 size, 2 modified
static SORT_ASC: AtomicBool = AtomicBool::new(true);
static SHOW_HIDDEN: AtomicBool = AtomicBool::new(false); // Ctrl+H
static NAV: AtomicBool = AtomicBool::new(false); // loading from the back / forward trail: do not record it again
const WM_NAV: u32 = WM_APP + 3; // wp: 1 back, 2 forward, 3 properties, 4 up (sent from the message loop for Alt+key)
const WM_JOB: u32 = WM_APP + 5; // jobs.rs worker finished: wp = Box<Option<String>> (the error, if any)
const WM_TRASH: u32 = WM_APP + 7; // Recycle Bin scan finished: wp = Box<Vec<Row>>
const WM_DESK_CHANGED: u32 = WM_APP + 13; // a Desktop folder changed on disk: debounce, then reload the icons
const WM_DELETED: u32 = WM_APP + 9; // a shell file operation (delete / copy / move) ended on a worker: lp = Box<sh::FileOpDone>
const WM_TRASH_DONE: u32 = WM_APP + 10; // Recycle Bin restore / empty ended on a worker: lp = Box<TrashDone>

fn row(name: String, m: Option<&fs::Metadata>) -> Row {
    Row { name, dir: m.is_some_and(|m| m.is_dir()), size: m.map_or(0, |m| m.len()), time: m.map_or(0, |m| m.last_write_time()), icon: -1, thumb: -2, hdr: false, path: None, cap: None, trash: None }
}

/// Folders always first; inside each group by the column (0 name, 1 size, 2 modified), ties by name.
fn sort_rows(rows: &mut [Row], col: usize, asc: bool) {
    let key = |r: &Row| r.name.rsplit('\\').next().unwrap_or("").to_lowercase();
    rows.sort_by(|a, b| {
        let o = match col { 1 => a.time.cmp(&b.time), 2 => type_text(&a.name, a.dir).cmp(&type_text(&b.name, b.dir)), 3 => a.size.cmp(&b.size), _ => key(a).cmp(&key(b)) };
        (!a.dir).cmp(&!b.dir).then(if asc { o } else { o.reverse() }).then_with(|| key(a).cmp(&key(b)))
    });
    // grouped by date: the groups are contiguous (the stable sort keeps the order above inside each one)
    if group_on() { bucket_refresh(); rows.sort_by_cached_key(|r| bucket(r.time)); }
}

// ---------- grouping by date ("Group by date" in the folder's menu): Today, Yesterday, Earlier this week, ... ----------

const DAY: i64 = 864_000_000_000; // FILETIME ticks per day
/// The date headings of "Group by date", in bucket order: shown above the rows of each group.
fn bucket_name(i: usize) -> &'static str {
    [
        t!("Сегодня", "Today", ja: "今日", pt: "Hoje", zh: "今天"),
        t!("Вчера", "Yesterday", ja: "昨日", pt: "Ontem", zh: "昨天"),
        t!("Ранее на этой неделе", "Earlier this week", ja: "今週", pt: "No início desta semana", zh: "本周早些时候"),
        t!("На прошлой неделе", "Last week", ja: "先週", pt: "Semana passada", zh: "上周"),
        t!("Ранее в этом месяце", "Earlier this month", ja: "今月", pt: "No início deste mês", zh: "本月早些时候"),
        t!("В прошлом месяце", "Last month", ja: "先月", pt: "Mês passado", zh: "上个月"),
        t!("Ранее в этом году", "Earlier this year", ja: "今年", pt: "No início deste ano", zh: "今年早些时候"),
        t!("Давно", "A long time ago", ja: "ずっと前", pt: "Há muito tempo", zh: "很久以前"),
    ][i]
}

thread_local! {
    static BCTX: std::cell::Cell<(i64, i64)> = const { std::cell::Cell::new((0, 0)) }; // (today's local day number, local - UTC in ticks)
}

fn group_marker() -> PathBuf { PathBuf::from(env::var_os("APPDATA").unwrap_or_default()).join(r"rust-explorer\group-by-date") }
static THUMBS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false); // thumbnail view: no headings
fn group_on() -> bool { !desk() && !THUMBS.load(Relaxed) && group_marker().exists() }

/// Takes "now" and the time zone offset once, so bucket() is plain arithmetic while a folder is sorted and painted.
fn bucket_refresh() {
    use windows_sys::Win32::{Foundation::FILETIME, Storage::FileSystem::FileTimeToLocalFileTime};
    let now = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() / 100) as i64) + 116_444_736_000_000_000;
    let (utc, mut local) = (FILETIME { dwLowDateTime: now as u32, dwHighDateTime: (now >> 32) as u32 }, FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 });
    let off = if unsafe { FileTimeToLocalFileTime(&utc, &mut local) } != 0 { ((local.dwHighDateTime as i64) << 32 | local.dwLowDateTime as i64) - now } else { 0 };
    BCTX.set(((now + off) / DAY, off));
}

/// Index into BUCKETS of a FILETIME (UTC ticks).
fn bucket(time: u64) -> i32 {
    let (today, off) = BCTX.get();
    match today - (time as i64 + off) / DAY {
        i64::MIN..=0 => 0,
        1 => 1,
        2..=6 => 2,
        7..=13 => 3,
        14..=29 => 4,
        30..=59 => 5,
        60..=364 => 6,
        _ => 7,
    }
}

/// A heading row ("Today", ...) before the first row of each date group. Its name starts with '?', which no file can have.
fn add_headers(rows: &mut Vec<Row>) {
    if !group_on() { return; }
    bucket_refresh();
    let mut out = Vec::with_capacity(rows.len() + 8);
    let mut last = -1;
    for r in rows.drain(..) {
        let b = bucket(r.time);
        if b != last {
            last = b;
            out.push(Row { name: format!("?{}", bucket_name(b as usize)), dir: false, size: 0, time: 0, icon: -1, thumb: -1, hdr: true, path: None, cap: None, trash: None });
        }
        out.push(r);
    }
    *rows = out;
}

/// Explorer's "Type" column in short form: "File folder", "TXT file", "File".
fn type_text(name: &str, dir: bool) -> String {
    if dir { return t!("Папка с файлами", "File folder", ja: "ファイル フォルダー", pt: "Pasta de arquivos", zh: "文件夹").into(); }
    let file = name.rsplit('\\').next().unwrap_or(name);
    match file.rsplit_once('.') { Some((stem, e)) if !stem.is_empty() && !e.is_empty() => tf!("Файл {}", "{} file", ja: "{} ファイル", pt: "Arquivo {}", zh: "{} 文件", e.to_uppercase()), _ => t!("Файл", "File", ja: "ファイル", pt: "Arquivo", zh: "文件").into() }
}

/// Explorer shows file sizes in KB; the bigger ones read better in MB / GB.
fn size_text(b: u64) -> String {
    const MB: u64 = 1 << 20;
    if b < MB { format!("{} KB", b.div_ceil(1024)) } else if b < 1 << 30 { format!("{:.1} MB", b as f64 / MB as f64) } else { format!("{:.2} GB", b as f64 / (1u64 << 30) as f64) }
}

fn date_text(ft: u64) -> String {
    use windows_sys::Win32::{Foundation::{FILETIME, SYSTEMTIME}, System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime}};
    if ft == 0 { return String::new(); }
    unsafe {
        let f = FILETIME { dwLowDateTime: ft as u32, dwHighDateTime: (ft >> 32) as u32 };
        let (mut u, mut l): (SYSTEMTIME, SYSTEMTIME) = (std::mem::zeroed(), std::mem::zeroed());
        if FileTimeToSystemTime(&f, &mut u) == 0 || SystemTimeToTzSpecificLocalTime(ptr::null(), &u, &mut l) == 0 { return String::new(); }
        format!("{:04}-{:02}-{:02} {:02}:{:02}", l.wYear, l.wMonth, l.wDay, l.wHour, l.wMinute)
    }
}

/// Our own tools installed by the launcher (photoview, wiztree): the exe in Program Files, None when not installed.
fn tool(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env::var_os("ProgramFiles")?).join(name).join(format!("{name}.exe"));
    p.is_file().then_some(p)
}

/// Same list as photoview's EXTS.
fn is_pic(p: &std::path::Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["jpg", "jpeg", "jfif", "png", "bmp", "gif", "tif", "tiff", "webp", "heic", "avif"].contains(&e.to_ascii_lowercase().as_str()))
}

fn properties(path: &PathBuf) {
    let (f, v) = (w(&path.to_string_lossy()), w("properties"));
    let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    sei.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = 0x0C; // SEE_MASK_INVOKEIDLIST
    sei.lpVerb = v.as_ptr(); sei.lpFile = f.as_ptr(); sei.nShow = SW_SHOW;
    unsafe { ShellExecuteExW(&mut sei) };
}

/// New folder / new empty file with a free name ("New folder", "New folder (2)", ...).
fn new_entry(dir: &PathBuf, base: &str, folder: bool) {
    if zipx::split_zip(dir).is_some() { return; }
    let (stem, ext) = match base.rsplit_once('.') { Some((s, e)) if !folder => (s.to_string(), format!(".{e}")), _ => (base.to_string(), String::new()) };
    let path = (1..).map(|n| dir.join(if n == 1 { base.to_string() } else { format!("{stem} ({n}){ext}") })).find(|p| !p.exists());
    if let Some(p) = path { let _ = if folder { fs::create_dir(p) } else { fs::File::create(p).map(drop) }; }
}

/// Pastes what is on the system clipboard (files copied or cut here or in any other file manager).
/// Copies and moves to another drive run in our own copier (jobs.rs: parallel on SSDs, unbuffered for big files); moves on the
/// same drive (a rename), shell items, and folders that need elevation go through the shell.
fn paste(h: HWND, dir: &PathBuf) {
    if zipx::split_zip(dir).is_some() { return; } // an archive is read-only here
    let Some((paths, cut)) = sh::clip_get() else { return };
    // A drive that is gone must not be probed: can_fast_copy / writable below touch the source and the destination, and
    // on a removed device those calls block inside the volume driver with no job to cancel yet (the window froze on Ctrl+V).
    if drives::ready() {
        let missing = std::iter::once(dir.as_path())
            .chain(paths.iter().map(|p| p.as_path()))
            .find(|p| !drives::holds(p));
        if let Some(p) = missing {
            unsafe { ui::notice(h, tf!("{}: диск недоступен", "{}: the drive is not available", ja: "{}: ドライブが利用できません", pt: "{}: a unidade não está disponível", zh: "{}：驱动器不可用", p.display())) };
            return;
        }
    }
    let fast = jobs::can_fast_copy(&paths, dir) && !(cut && paths.iter().all(|p| jobs::same_volume(p, dir))) && jobs::writable(dir);
    if fast && !jobs::busy() {
        jobs::copy(h as isize, paths, dir.clone(), cut);
    } else {
        // the shell's engine (elevation prompts, shell items): on a worker too, a big copy must not freeze the window
        sh::fileop_async(h, if cut { sh::Op::Move } else { sh::Op::Copy }, &paths, dir);
    }
    if cut { sh::clip_clear(); }
}

/// Ctrl+C / Ctrl+X: the selection goes on the system clipboard as real files (entries of a .zip are copied out into the
/// temp folder first). Nothing can be cut out of an archive: there it is always a copy.
fn clip_selected(h: HWND, cut: bool) {
    let (_, _, paths) = selected();
    if paths.is_empty() || in_virtual() { return; }
    match real_paths(&paths) {
        Ok(p) => { let _ = sh::clip_set(h, &p, cut && !in_zip()); }
        Err(e) => warn(h, &e),
    }
}

/// F2: edits the name of the selected row in place (the list answers with LVN_ENDLABELEDITW).
fn rename_selected() {
    let (list, _, paths) = selected();
    if paths.is_empty() || in_virtual() { return; }
    let i = put(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
    if i >= 0 {
        unsafe { SetFocus(list) };
        put(list, LVM_EDITLABELW, i as usize, 0);
    }
}

// The file menu's rows. Any nonzero id is ours: the popup is drawn by menu.rs, not by the shell.
const MENU_OPEN: u32 = 0x9000;
const MENU_ZIP: u32 = 0x9001;
const MENU_EXTRACT: u32 = 0x9002;
const MENU_PIN: u32 = 0x9003;
const MENU_RENAME: u32 = 0x9004;
const MENU_OPENWITH: u32 = 0x9005;
const MENU_COPY: u32 = 0x9006;
const MENU_CUT: u32 = 0x9007;
const MENU_PASTE: u32 = 0x9008;
const MENU_DELETE: u32 = 0x9009;
const MENU_PROPS: u32 = 0x900A;
const MENU_MORE: u32 = 0x900B;

/// True while the window shows the inside of a .zip: read-only, its rows are no files on disk.
fn in_zip() -> bool { zipx::split_zip(&cur_dir()).is_some() }

fn warn(h: HWND, text: &str) {
    unsafe { MessageBoxW(h, w(text).as_ptr(), w("WinRust Explorer").as_ptr(), MB_ICONWARNING) };
}

/// (archive, archive paths) when every one of `paths` is something inside one and the same .zip.
fn zip_names(paths: &[PathBuf]) -> Option<(PathBuf, Vec<String>)> {
    let mut arc = None;
    let mut names = vec![];
    for p in paths {
        let (a, inner) = zipx::split_zip(p).filter(|(_, i)| !i.is_empty())?;
        if arc.get_or_insert_with(|| a.clone()) != &a { return None; }
        names.push(inner);
    }
    Some((arc?, names))
}

/// Things from an archive as real files in a temp folder: the clipboard, drag and "open with" need files on disk. One path per name.
/// Folders of earlier days are cleaned up here.
fn materialize(arc: &Path, names: &[String]) -> Result<Vec<PathBuf>, String> {
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = env::temp_dir().join("WinRust-zip");
    for e in fs::read_dir(&root).into_iter().flatten().flatten() {
        if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > std::time::Duration::from_secs(86_400)) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
    let out = root.join(format!("{}-{}", std::process::id(), N.fetch_add(1, Relaxed)));
    zipx::extract_some(arc, names, &out)?;
    Ok(names.iter().map(|n| out.join(n.rsplit('/').next().unwrap_or(n))).collect())
}

/// `paths` as files on disk: unchanged, or taken out of the archive they are in first.
fn real_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    match zip_names(paths) {
        Some((arc, names)) => materialize(&arc, &names),
        None => Ok(paths.to_vec()),
    }
}

/// Enter / double click on a row.
unsafe fn activate(h: HWND, p: PathBuf, is_dir: bool) {
    if is_trash(&cur_dir()) { return; } // bin rows: Restore from the menu, a double click does nothing
    // a file inside a zip: a real copy in the temp folder first
    let p = match zip_names(std::slice::from_ref(&p)) {
        Some((arc, names)) if !is_dir => match materialize(&arc, &names) {
            Ok(mut v) => v.remove(0),
            Err(e) => return warn(h, &e),
        },
        _ => p,
    };
    if is_dir && desk() { let _ = std::process::Command::new(env::current_exe().unwrap_or_default()).arg(p).spawn(); }
    else if is_dir { load(h, p) }
    else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")) && p.is_file() { load(h, p) } // a zip opens like a folder; "Extract here" is in its menu
    else if let Some(exe) = tool("photoview").filter(|_| is_pic(&p)) { let _ = std::process::Command::new(exe).arg(&p).spawn(); }
    else if !openwith::associated(&p) {
        // No association: Windows' modern picker cannot render without explorer.exe (it starts at (-32000,-32000)
        // with a 6x28 window and never shows), so ask with our own dialog instead.
        unsafe { openwith::open(h, &p) };
    }
    else {
        // The shell is not asked to open anything: with a broken association ShellExecuteW ends in OpenWith.exe, the
        // picker that cannot render without explorer. openwith starts the handler itself, or asks with our dialog.
        unsafe { openwith::open_file(h, &p) };
    }
}

/// The menu for rows inside a zip. They are no shell items, so this is a small menu of our own.
fn zip_menu(h: HWND, paths: &[PathBuf]) {
    let id = unsafe {
        let m = CreatePopupMenu();
        let items = [
            (1usize, t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开")),
            (2, t!("Копировать", "Copy", ja: "コピー", pt: "Copiar", zh: "复制")),
            (3, t!("Извлечь сюда", "Extract here", ja: "ここに展開", pt: "Extrair aqui", zh: "解压到此处")),
        ];
        for (i, lbl) in items { AppendMenuW(m, MF_STRING, i, w(lbl).as_ptr()); }
        let mut pt: POINT = std::mem::zeroed();
        GetCursorPos(&mut pt);
        let id = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, ptr::null());
        DestroyMenu(m);
        id
    };
    let Some((arc, names)) = zip_names(paths) else { return };
    match id {
        1 => if let [p] = paths {
            let is_dir = ST.with_borrow(|st| st.rows.iter().any(|r| r.dir && st.dir.join(&r.name) == *p));
            unsafe { activate(h, p.clone(), is_dir) };
        },
        2 => match materialize(&arc, &names) {
            Ok(v) => { let _ = sh::clip_set(h, &v, false); }
            Err(e) => warn(h, &e),
        },
        3 => {
            // next to the archive, in a new folder named after it; no progress yet (shortcut: fire and forget)
            let (Some(parent), Some(stem)) = (arc.parent(), arc.file_stem()) else { return };
            let out = jobs::free_name(parent, &stem.to_string_lossy(), true);
            std::thread::spawn(move || _ = zipx::extract_some(&arc, &names, &out));
        }
        _ => {}
    }
}

/// Pins `p` to Quick access, or takes it out when it is already there.
fn toggle_quick(p: &Path) {
    if ui::quick_has(p) { ui::quick_unpin(p); } else { ui::quick_pin(p); }
}

/// The menu for the selected rows: our own popup (rounded, blurred, icons) instead of the shell's. The shell menu is
/// still one click away under "More options…", for the verbs this one does not cover (Send to, Share, …).
fn item_menu(h: HWND, dir: &Path, paths: &[PathBuf]) {
    use crate::menu::{Icon, Item};
    if is_trash(dir) { return trash_menu(h, true); } // Restore / Empty instead of these verbs
    if in_pc() { return; } // a drive is not a file: no shell verbs (delete, rename, properties) on it
    if zipx::split_zip(dir).is_some() { return zip_menu(h, paths); }
    if paths.is_empty() { return; }
    let busy = jobs::busy();
    let mut items = vec![];
    // Open / Open with: only one row can be opened at a time, so these two need a single selection
    if let [p] = paths {
        items.push(Item::new(MENU_OPEN, t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开")).icon(Icon::Glyph("\u{E8E5}")).hint("Enter"));
        if p.is_file() {
            // our own picker: the shell's "Open with" goes to the Windows surface that cannot render without explorer.exe
            items.push(Item::new(MENU_OPENWITH, t!("Открыть с помощью…", "Open with…", ja: "プログラムから開く…", pt: "Abrir com…", zh: "打开方式…")).icon(Icon::Glyph("\u{E8AB}")));
        }
    }
    items.push(Item::new(MENU_ZIP, t!("Сжать в ZIP", "Compress to ZIP", ja: "ZIP に圧縮", pt: "Compactar em ZIP", zh: "压缩为 ZIP")).icon(Icon::Glyph("\u{E8B7}")).dim(busy));
    if paths.iter().all(|p| p.is_file() && jobs::is_archive(p)) {
        items.push(Item::new(MENU_EXTRACT, t!("Извлечь сюда", "Extract here", ja: "ここに展開", pt: "Extrair aqui", zh: "解压到此处")).icon(Icon::Glyph("\u{E896}")).dim(busy));
    }
    if let [p] = paths {
        // the shell menu has no Rename here (it needs the shell's own folder view to answer it): the same edit box as F2
        items.push(Item::new(MENU_RENAME, t!("Переименовать", "Rename", ja: "名前の変更", pt: "Renomear", zh: "重命名")).icon(Icon::Glyph("\u{E8AC}")).hint("F2").dim(busy));
        if p.is_dir() {
            let lbl = if ui::quick_has(p) { t!("Открепить от быстрого доступа", "Unpin from Quick access", ja: "クイック アクセスからピン留めを解除", pt: "Desafixar do Acesso rápido", zh: "从快速访问取消固定") } else { t!("Закрепить в быстром доступе", "Pin to Quick access", ja: "クイック アクセスにピン留め", pt: "Fixar no Acesso rápido", zh: "固定到快速访问") };
            items.push(Item::new(MENU_PIN, lbl).icon(Icon::Glyph("\u{E718}")));
        }
    }
    items.push(Item::new(MENU_COPY, t!("Копировать", "Copy", ja: "コピー", pt: "Copiar", zh: "复制")).icon(Icon::Glyph("\u{E8C8}")).hint("Ctrl+C"));
    items.push(Item::new(MENU_CUT, t!("Вырезать", "Cut", ja: "切り取り", pt: "Recortar", zh: "剪切")).icon(Icon::Glyph("\u{E8C6}")).hint("Ctrl+X"));
    if !in_virtual() && sh::clip_get().is_some() {
        items.push(Item::new(MENU_PASTE, t!("Вставить", "Paste", ja: "貼り付け", pt: "Colar", zh: "粘贴")).icon(Icon::Glyph("\u{E77F}")).hint("Ctrl+V").dim(busy));
    }
    if !in_virtual() {
        items.push(Item::new(MENU_DELETE, t!("Удалить", "Delete", ja: "削除", pt: "Excluir", zh: "删除")).icon(Icon::Glyph("\u{E74D}")).hint("Del").dim(busy));
    }
    items.push(Item::new(MENU_PROPS, t!("Свойства", "Properties", ja: "プロパティ", pt: "Propriedades", zh: "属性")).icon(Icon::Glyph("\u{E713}")).hint("Alt+Enter"));
    // .sep() puts the divider above this row: the shell menu keeps every verb this one does not carry
    items.push(Item::new(MENU_MORE, t!("Дополнительные параметры…", "More options…", ja: "その他のオプション…", pt: "Mais opções…", zh: "更多选项…")).icon(Icon::Glyph("\u{E712}")).hint("Shift+F10").sep());
    let mut pt: POINT = unsafe { std::mem::zeroed() };
    unsafe { GetCursorPos(&mut pt) };
    let id = unsafe { crate::menu::show(pt.x, pt.y, items) };
    let dest = dir.to_path_buf();
    match id {
        MENU_OPEN => unsafe { activate(h, paths[0].clone(), paths[0].is_dir()) },
        MENU_OPENWITH => unsafe { openwith::open(h, &paths[0]) },
        MENU_PIN => toggle_quick(&paths[0]),
        MENU_RENAME => rename_selected(),
        MENU_ZIP => {
            let base = if paths.len() == 1 { paths[0].file_stem().unwrap_or_default().to_string_lossy().into_owned() } else { dir.file_name().map_or("Archive".into(), |n| n.to_string_lossy().into_owned()) };
            let dest = jobs::free_name(dir, &format!("{base}.zip"), false);
            jobs::zip(h as isize, paths.to_vec(), dest);
        }
        MENU_EXTRACT => _ = jobs::extract(h as isize, paths.to_vec(), dest.clone()),
        MENU_COPY => clip_selected(h, false),
        MENU_CUT => clip_selected(h, true),
        MENU_PASTE => { paste(h, &dest); reload(h); }
        MENU_DELETE => { sh::delete(h, paths); } // WM_DELETED reloads when the worker is done
        MENU_PROPS => properties(&paths[0]),
        // the native menu, for everything we do not cover ourselves; reload like the old path did, the shell may have
        // deleted or renamed something
        MENU_MORE => { unsafe { SetForegroundWindow(h) }; let _ = sh::show(h, paths, &[]); reload(h); }
        _ => {}
    }
}

/// "Hide desktop icons" is a marker file, so it survives restarts.
fn icons_marker() -> PathBuf { PathBuf::from(env::var_os("APPDATA").unwrap_or_default()).join(r"WinRust\desktop-icons-hidden.txt") }
fn small_marker() -> PathBuf { icons_marker().with_file_name("small-icons") }
/// Desktop icon size: the system's large (32 px, default) or small (16 px) icons with a tighter grid. Same icon indexes in both.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn apply_icon_size(list: HWND) {
    let small = small_marker().exists();
    let mut fi: SHFILEINFOW = std::mem::zeroed();
    let shell = SHGetFileInfoW(w("C:\\").as_ptr(), 0, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | if small { SHGFI_SMALLICON } else { SHGFI_LARGEICON });
    // Ours holds the big icons (same size, sharper source); with small icons the shell's own list is used.
    let (ours, _, px) = DESK_IL.with(|c| c.get());
    let use_ours = !small && ours != 0;
    put(list, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, if use_ours { ours } else { shell as isize });
    DESK_IL.set((ours, shell as isize, if use_ours { px } else { 0 }));
    // the indexes point into the list that was just attached: every row asks for its icon again
    ST.with_borrow_mut(|st| st.rows.iter_mut().for_each(|r| r.icon = -1));
    InvalidateRect(list, ptr::null(), 0);
    let (cx, cy) = if small { (76, 64) } else { (88, 96) };
    put(list, LVM_SETICONSPACING, 0, ((ui::sc(cy) as isize) << 16) | ui::sc(cx) as isize);
}

/// The icon index of a desktop item: our own list when it is attached, the shell's index otherwise. `icon::best` gives
/// the same size the list wants, so the control never stretches anything; a path the shell has no icon for still gets
/// the shell's (a document type, a drive), copied over from its list.
fn desk_icon(path: &PathBuf, dir: bool) -> i32 {
    let (ours, shell, px) = DESK_IL.with(|c| c.get());
    if ours == 0 || px <= 0 { return icon_of(path, dir); }
    let key = path.to_string_lossy().to_lowercase();
    if let Some(i) = DESK_ICON_AT.with_borrow(|m| m.get(&key).copied()) { return i; }
    let mut idx = unsafe {
        let ic = winrust_common::icon::best(path, px);
        if !ic.is_null() {
            ImageList_ReplaceIcon(ours as _, -1, ic) // i = -1: append
        } else {
            let shell_ic = ImageList_GetIcon(shell as _, icon_of(path, dir), ILD_TRANSPARENT);
            if shell_ic.is_null() {
                -1
            } else {
                let i = ImageList_ReplaceIcon(ours as _, -1, shell_ic);
                DestroyIcon(shell_ic); // the list keeps its own copy
                i
            }
        }
    };
    // 0 = the first slot: a row with iImage -1 (I_IMAGECALLBACK) would be asked for on every paint, and the shell gives
    // up on the same path each time
    if idx < 0 { idx = 0; }
    DESK_ICON_AT.with_borrow_mut(|m| { m.insert(key, idx); });
    idx
}

/// The rows are gone: their indexes into the desktop's icon list go with them.
fn reset_desk_icons() {
    let ours = DESK_IL.with(|c| c.get()).0;
    if ours != 0 { unsafe { ImageList_Remove(ours as _, -1) }; }
    DESK_ICON_AT.with_borrow_mut(|m| m.clear());
}
fn icons_hidden() -> bool { icons_marker().exists() }
fn set_icons_hidden(hide: bool) {
    let m = icons_marker();
    if hide { if let Some(d) = m.parent() { let _ = fs::create_dir_all(d); } let _ = fs::write(&m, "1"); } else { let _ = fs::remove_file(&m); }
    let list = ST.with_borrow(|st| st.list);
    unsafe { ShowWindow(list, if hide { SW_HIDE } else { SW_SHOW }) };
}

/// Right click on empty space: the folder's own menu.
#[allow(unsafe_op_in_unsafe_fn)]
/// Right click on empty space: the folder's own menu.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn bg_menu(h: HWND) {
    use crate::menu::{Icon, Item};
    if is_trash(&cur_dir()) { return trash_menu(h, false); } // empty space in the bin: Empty / Refresh only
    const DIR: u32 = 0x10; // FILE_ATTRIBUTE_DIRECTORY
    const FILE: u32 = 0x80; // FILE_ATTRIBUTE_NORMAL
    let disk = tool("wiztree");
    let mut items = vec![
        Item::new(1, t!("Новая папка", "New folder", ja: "新しいフォルダー", pt: "Nova pasta", zh: "新建文件夹")).icon(Icon::Shell("folder", DIR)).hint("Ctrl+N"),
        Item::new(2, t!("Новый текстовый файл", "New text file", ja: "新しいテキスト ドキュメント", pt: "Novo arquivo de texto", zh: "新建文本文件")).icon(Icon::Shell("a.txt", FILE)),
        Item::new(3, t!("Вставить", "Paste", ja: "貼り付け", pt: "Colar", zh: "粘贴")).icon(Icon::Glyph("\u{E77F}")).hint("Ctrl+V"),
        Item::new(4, t!("Обновить", "Refresh", ja: "最新の情報に更新", pt: "Atualizar", zh: "刷新")).icon(Icon::Glyph("\u{E72C}")).hint("F5"),
    ];
    if disk.is_some() {
        items.push(Item::new(6, t!("Занятое место (WinRust Disk)", "Disk usage (WinRust Disk)", ja: "ディスク使用量 (WinRust Disk)", pt: "Uso do disco (WinRust Disk)", zh: "磁盘占用（WinRust Disk）")).icon(Icon::Shell("C:\\", FILE)));
    }
    if desk() {
        let sort = vec![
            Item::new(8, t!("Упорядочить значки по имени", "Sort icons by name", ja: "名前順にアイコンを並べ替え", pt: "Classificar ícones por nome", zh: "按名称排列图标")),
            Item::new(9, t!("Упорядочить значки по дате", "Sort icons by date", ja: "日付順にアイコンを並べ替え", pt: "Classificar ícones por data", zh: "按日期排列图标")),
            Item::new(10, t!("Упорядочить значки по размеру", "Sort icons by size", ja: "サイズ順にアイコンを並べ替え", pt: "Classificar ícones por tamanho", zh: "按大小排列图标")),
        ];
        items.push(Item::new(0, t!("Сортировать по", "Sort icons by", ja: "並べ替え", pt: "Classificar por", zh: "排序方式")).icon(Icon::Glyph("\u{E8CB}")).sub(sort));
        items.push(Item::new(11, t!("Выровнять значки", "Arrange icons", ja: "アイコンの整列", pt: "Alinhar ícones", zh: "排列图标")).icon(Icon::Glyph("\u{E80A}")));
    }
    if !desk() {
        let lbl = if group_on() { t!("Без группировки", "No grouping", ja: "グループ化しない", pt: "Sem agrupamento", zh: "不分组") } else { t!("Группировать по дате", "Group by date", ja: "日付でグループ化", pt: "Agrupar por data", zh: "按日期分组") };
        items.push(Item::new(13, lbl.to_string()).icon(Icon::Glyph("\u{E8CB}")));
    }
    if desk() {
        let small = small_marker().exists();
        let lbl = if small { t!("Крупные значки", "Large icons", ja: "大きいアイコン", pt: "Ícones grandes", zh: "大图标") } else { t!("Мелкие значки", "Small icons", ja: "小さいアイコン", pt: "Ícones pequenos", zh: "小图标") };
        items.push(Item::new(12, lbl.to_string()).icon(Icon::Glyph("\u{E71D}")).check(small));
        let hidden = icons_hidden();
        let lbl = if hidden { t!("Показать значки рабочего стола", "Show desktop icons", ja: "デスクトップ アイコンを表示", pt: "Mostrar ícones da área de trabalho", zh: "显示桌面图标") } else { t!("Скрыть значки рабочего стола", "Hide desktop icons", ja: "デスクトップ アイコンを非表示", pt: "Ocultar ícones da área de trabalho", zh: "隐藏桌面图标") };
        items.push(Item::new(7, lbl.to_string()).icon(Icon::Glyph("\u{E7B3}")).check(hidden));
    }
    // Windows keeps Properties at the very bottom, on its own group
    if !desk() && !in_virtual() {
        // pin the folder we are in (or take it out again)
        let lbl = if ui::quick_has(&cur_dir()) { t!("Открепить от быстрого доступа", "Unpin from Quick access", ja: "クイック アクセスからピン留めを解除", pt: "Desafixar do Acesso rápido", zh: "从快速访问取消固定") } else { t!("Закрепить в быстром доступе", "Pin to Quick access", ja: "クイック アクセスにピン留め", pt: "Fixar no Acesso rápido", zh: "固定到快速访问") };
        items.push(Item::new(14, lbl).icon(Icon::Glyph("\u{E718}")));
    }
    items.push(Item::new(5, t!("Свойства", "Properties", ja: "プロパティ", pt: "Propriedades", zh: "属性")).icon(Icon::Glyph("\u{E713}")));
    if in_pc() {
        // the drive list is not a folder: only a refresh makes sense here (no new / paste / properties)
        items.retain(|it| it.id == 4);
    }
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    let id = crate::menu::show(pt.x, pt.y, items);
    let dir = cur_dir();
    match id {
        7 => set_icons_hidden(!icons_hidden()),
        13 => {
            let k = group_marker();
            if group_on() { let _ = fs::remove_file(&k); } else { if let Some(d) = k.parent() { let _ = fs::create_dir_all(d); } let _ = fs::write(&k, "1"); }
        }
        12 => { // the grid changes with the size: icons are laid out again
            let k = small_marker();
            if k.exists() { let _ = fs::remove_file(&k); } else { let _ = fs::write(&k, "1"); }
            apply_icon_size(ST.with_borrow(|st| st.list));
            let _ = fs::remove_file(desk::pos_file());
        }
        8..=11 => { // the saved cells are dropped: reload lays every icon out again, down the columns in sort order
            if id != 11 { SORT_COL.store([0, 1, 3][id as usize - 8], Relaxed); SORT_ASC.store(true, Relaxed); }
            let _ = fs::remove_file(desk::pos_file());
        }
        6 => { // wiztree takes the drive letter and asks for admin rights itself
            let drive: String = dir.to_string_lossy().chars().take(2).collect();
            if let Some(exe) = disk { let _ = std::process::Command::new(exe).arg(drive).spawn(); }
        }
        1 => new_entry(&dir, t!("Новая папка", "New folder", ja: "新しいフォルダー", pt: "Nova pasta", zh: "新建文件夹"), true),
        2 => new_entry(&dir, t!("Новый текстовый документ.txt", "New text document.txt", ja: "新しいテキスト ドキュメント.txt", pt: "Novo documento de texto.txt", zh: "新建文本文档.txt"), false),
        3 => paste(h, &dir),
        5 => properties(&dir),
        14 => toggle_quick(&dir),
        _ => {}
    }
    if id != 0 { reload(h); }
}

/// Command line -> (folder to open, file to select there). Understands `/select,"C:\a\b.txt"` (what browsers and other
/// apps pass for "show in folder"), a plain folder, or a plain file path (its folder, file selected). No argument at all
/// opens "This PC" (the drive list), like Explorer.
fn start_target() -> (PathBuf, Option<String>) {
    let mut a = env::args().skip(1);
    let mut p = a.next().unwrap_or_default();
    if p.to_lowercase().starts_with("/select,") || p.to_lowercase().starts_with("--select") {
        p = p.get(8..).unwrap_or("").trim_start_matches(',').to_string();
        if p.is_empty() { p = a.next().unwrap_or_default(); }
    }
    let p = PathBuf::from(p.trim_matches('"'));
    if p.as_os_str().is_empty() { return (this_pc(), None); } // no argument: the drive list, like Explorer
    match (p.is_file(), p.parent()) {
        (true, Some(d)) => (d.to_path_buf(), p.file_name().map(|n| n.to_string_lossy().into_owned())),
        _ => (p, None),
    }
}

/// Another app asked the shell to show `p` in a folder window (SHOpenFolderAndSelectItems): open its folder here, select it, come to front.
fn shell_select(h: isize, p: PathBuf) {
    let h = h as HWND;
    let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { return };
    if !cur_dir().as_os_str().eq_ignore_ascii_case(dir.as_os_str()) { load(h, dir.to_path_buf()); }
    select_row(&name.to_string_lossy());
    unsafe {
        if IsIconic(h) != 0 { ShowWindow(h, SW_RESTORE); }
        SetForegroundWindow(h);
    }
}

/// Selects and scrolls to the row called `name` in the current folder.
fn select_row(name: &str) {
    let (list, i) = ST.with_borrow(|st| (st.list, st.rows.iter().position(|r| r.name.eq_ignore_ascii_case(name))));
    let Some(i) = i else { return };
    unsafe {
        let mut it: LVITEMW = std::mem::zeroed();
        it.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
        put(list, LVM_SETITEMSTATE, usize::MAX, &it as *const _ as isize); // the shell selects it alone
        it.state = LVIS_SELECTED | LVIS_FOCUSED;
        put(list, LVM_SETITEMSTATE, i, &it as *const _ as isize);
        put(list, LVM_ENSUREVISIBLE, i, 0);
    }
}

/// Back / forward along the visited-folder trail.
fn nav(h: HWND, back: bool) {
    let to = ST.with_borrow_mut(|st| if back { st.hist.back() } else { st.hist.forward() });
    if let Some(p) = to {
        NAV.store(true, Relaxed);
        load(h, p);
        NAV.store(false, Relaxed);
    }
}

fn w(s: &str) -> Vec<u16> { s.encode_utf16().chain([0]).collect() }

fn put(h: HWND, msg: u32, wp: usize, lp: isize) -> isize { unsafe { SendMessageW(h, msg, wp, lp) } }

fn wstr(p: *const u16) -> String {
    let mut n = 0;
    while unsafe { *p.add(n) } != 0 { n += 1; }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) })
}

pub fn cur_dir() -> PathBuf { ST.with_borrow(|st| st.dir.clone()) }

/// System image-list index. Extension-based (no disk access) except for files that carry their own icon.
fn icon_of(p: &PathBuf, dir: bool) -> i32 {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let own = !dir && matches!(ext.as_str(), "exe" | "lnk" | "ico" | "url" | "scr");
    let mut fi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let attr = if dir { 0x10 } else { 0x80 }; // DIRECTORY / NORMAL
    let flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON | if own { 0 } else { SHGFI_USEFILEATTRIBUTES };
    unsafe { SHGetFileInfoW(w(&p.to_string_lossy()).as_ptr(), attr, &mut fi, size_of::<SHFILEINFOW>() as u32, flags) };
    fi.iIcon
}

/// The tabs are painted by the chrome window from State.tabs: "setting a tab's text" is just a repaint.
fn tab_text(tab: HWND, _i: usize, _text: &str, _insert: bool) {
    if !tab.is_null() { unsafe { InvalidateRect(tab, ptr::null(), 0) }; }
}

/// The text of a desktop row. Rows from the public desktop carry absolute paths: only the file name is shown, and shortcuts
/// lose their ".lnk" / ".url" like in Explorer. Other windows show the row as it is.
fn shown_name(name: &str, desktop: bool) -> String {
    if !desktop { return name.to_string(); }
    let file = std::path::Path::new(name).file_name().map_or_else(|| name.to_string(), |n| n.to_string_lossy().into_owned());
    let lower = file.to_lowercase();
    if lower.ends_with(".lnk") || lower.ends_with(".url") { file[..file.len() - 4].to_string() } else { file }
}

fn label(dir: &PathBuf) -> String {
    // the views are not folders: "This PC" and the Recycle Bin have no file name ("::{645FF...}")
    if is_this_pc(dir) || is_trash(dir) { return display_dir(dir); }
    dir.file_name().map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}

/// Rows of `dir`, folders first. Desktop mode merges the public desktop (absolute names) and hides hidden files.
/// None = `dir` itself is unreadable.
fn rows_of(dir: &PathBuf) -> Option<Vec<Row>> {
    if is_this_pc(dir) { // not a folder: every drive, sorted by its label
        let mut rows = drives();
        sort_rows(&mut rows, SORT_COL.load(Relaxed), SORT_ASC.load(Relaxed));
        return Some(rows); // no date headings: drives have no date
    }
    if is_trash(dir) { return Some(vec![]); } // filled by the background scan (trash_scan / WM_TRASH)
    if !desk() {
        if let Some((arc, inner)) = zipx::split_zip(dir) { // inside a .zip: its entries are the rows
            let mut rows: Vec<Row> = zipx::list_dir(&arc, &inner).ok()?.into_iter().map(|i| Row { name: i.name, dir: i.dir, size: i.size, time: zipx::dos_to_filetime(i.time), icon: -1, thumb: -2, hdr: false, path: None, cap: None, trash: None }).collect();
            sort_rows(&mut rows, SORT_COL.load(Relaxed), SORT_ASC.load(Relaxed));
            add_headers(&mut rows);
            return Some(rows);
        }
    }
    let mut dirs = vec![dir.clone()];
    if desk() { dirs.extend(sh::desktop_dirs().into_iter().skip(1)); }
    let mut rows = vec![];
    for (n, d) in dirs.iter().enumerate() {
        let Ok(rd) = fs::read_dir(d) else { if n == 0 { return None } else { continue } };
        for e in rd.flatten() {
            let m = e.metadata().ok();
            if (desk() || !SHOW_HIDDEN.load(Relaxed)) && m.as_ref().is_some_and(|m| m.file_attributes() & 2 != 0) { continue; } // FILE_ATTRIBUTE_HIDDEN
            let name = if n == 0 { e.file_name().to_string_lossy().into_owned() } else { e.path().to_string_lossy().into_owned() };
            rows.push(row(name, m.as_ref()));
        }
    }
    sort_rows(&mut rows, SORT_COL.load(Relaxed), SORT_ASC.load(Relaxed));
    add_headers(&mut rows);
    Some(rows)
}

/// Loads `dir` into the virtual list and the active tab. Unreadable dir (access denied, gone) leaves the view unchanged.
/// Never call with ST borrowed: the list sends GETDISPINFO back into the window proc.
fn load(h: HWND, dir: PathBuf) {
    // A removed drive keeps its letter for a while and reading it blocks inside the volume driver: do not touch it.
    if drives::ready() && !drives::holds(&dir) {
        unsafe { ui::notice(h, tf!("{}: диск недоступен", "{}: the drive is not available", ja: "{}: ドライブが利用できません", pt: "{}: a unidade não está disponível", zh: "{}：驱动器不可用", dir.display())) };
        return;
    }
    let Some(rows) = rows_of(&dir) else { return };
    let shown = display_dir(&dir); // "This PC" / "Recycle Bin" for the views, the path otherwise
    let (pc, trash) = (is_this_pc(&dir), is_trash(&dir));
    if !desk() { unsafe { SetWindowTextW(h, w(&shown).as_ptr()) }; }
    let (list, tab, edit, active, n) = ST.with_borrow_mut(|st| {
        if let Some(t) = st.tabs.get_mut(st.active) { *t = dir.clone(); }
        st.dir = dir.clone(); st.rows = rows;
        if !NAV.load(Relaxed) { st.hist.visit(dir.clone()); }
        (st.list, st.tab, st.edit, st.active, st.rows.len())
    });
    // the grid's cells: wider in This PC, where each drive carries a second line of text under its name
    if !desk() && THUMBS.load(Relaxed) { ui::grid_spacing(list, pc); }
    unsafe { SetWindowTextW(edit, w(&shown).as_ptr()) };
    if !pc && !trash { shellwin::navigated(&dir); } // a shell window shows a folder, not a view
    ui::set_search(None); // a folder again: the pill shows its path
    ui::clear_notice(); // a new place: the old status message goes
    tab_text(tab, active, &label(&dir), false);
    ui::nav_dirty(); // the pane highlights where we are
    // "free of total" needs more room than a file size; a folder listing keeps its familiar width
    put(list, LVM_SETCOLUMNWIDTH, 3, ui::sc(if pc { 190 } else { 96 }) as isize);
    reset_thumbs();
    reset_desk_icons(); // the rows are replaced: their indexes into the desktop's icon list are stale
    put(list, LVM_DELETEALLITEMS, 0, 0);
    if desk() { desk::fill(list, n); } else { put(list, LVM_SETITEMCOUNT, n, 0); }
    if trash { trash_scan(h); } // the bins are read on a worker: the list fills when WM_TRASH arrives
}

/// Asks the rust-search service of drive `d` (see plugins/rust-search): terms in, one path per line out.
/// None = no service for that drive or no access (its pipe is admin-only unless started with --allow).
fn ask(d: char, q: &str) -> Option<String> {
    use std::{fs::OpenOptions, io::{Read, Write}};
    let mut p = OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\rs-search-{d}")).ok()?;
    p.write_all(q.as_bytes()).ok()?;
    let (mut out, mut chunk) = (Vec::new(), [0u8; 1 << 16]);
    // the server ends its reply by hanging up, which surfaces as a read error: treat any error as EOF
    while let Ok(n @ 1..) = p.read(&mut chunk) { out.extend_from_slice(&chunk[..n]); }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Fills the list with search hits from every drive's service. Rows hold absolute paths, so joining them
/// onto the current dir still yields the right file. Blocks the UI for the duration of the query (fast).
/// Never call with ST borrowed.
fn search(h: HWND, q: &str) {
    let mut rows = vec![];
    let mut more = String::new();
    let mut indexed = false; // some drive had a rust-search service
    for d in 'A'..='Z' {
        let Some(reply) = ask(d, q) else { continue };
        indexed = true;
        for line in reply.lines() {
            if line.starts_with("... ") { more = format!(", {}", line.trim_start_matches("... ")); continue; }
            let m = fs::metadata(line).ok();
            rows.push(row(line.to_string(), m.as_ref()));
        }
    }
    if !indexed {
        // no rust-search installed: look through the current folder and below instead
        let (local, cut, flipped) = local_search(&cur_dir(), q);
        if flipped { more = tf!(" для «{}»", " for \u{201C}{}\u{201D}", ja: "「{}」として", pt: " para \u{201C}{}\u{201D}", zh: "（按「{}」）", flip_layout(q)); } // typed in the wrong layout: shown what was meant
        rows = local;
        if cut { more += t!(", остановлено: уточните запрос", ", stopped early: narrow the search", ja: "、中断: 検索範囲を狭めてください", pt: ", interrompido: refine a pesquisa", zh: "，已提前停止：请缩小搜索范围"); }
    }
    let n = rows.len();
    let where_ = if indexed { "" } else { t!(" в этой папке", " in this folder", ja: "（このフォルダー内）", pt: " nesta pasta", zh: "（仅此文件夹）") };
    let title = if n == 0 {
        tf!("поиск: {} - ничего не найдено{}{}", "search: {} - nothing{}{}", ja: "検索: {} - 見つかりません{}{}", pt: "pesquisa: {} - nada encontrado{}{}", zh: "搜索：{} - 无结果{}{}", q, where_, "")
    } else {
        tf!("поиск: {} - совпадений: {}{}{}", "search: {} - {} hits{}{}", ja: "検索: {} - {} 件{}{}", pt: "pesquisa: {} - {} resultados{}{}", zh: "搜索：{} - {} 个结果{}{}", q, n, where_, more)
    };
    ui::set_search(Some(q.to_string()));
    unsafe { SetWindowTextW(h, w(&title).as_ptr()) };
    let list = ST.with_borrow_mut(|st| { st.rows = rows; st.list });
    reset_thumbs();
    put(list, LVM_DELETEALLITEMS, 0, 0);
    put(list, LVM_SETITEMCOUNT, n, 0);
}

/// The same keys typed in the other layout (QWERTY <-> ЙЦУКЕН): "сфкпщ" -> "cargo", "ljrevtyns" -> "документы".
fn flip_layout(q: &str) -> String {
    let en: Vec<char> = "qwertyuiop[]asdfghjkl;'zxcvbnm,.`QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>~".chars().collect();
    let ru: Vec<char> = "йцукенгшщзхъфывапролджэячсмитьбюёЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮЁ".chars().collect();
    q.chars().map(|c| en.iter().position(|&x| x == c).map(|i| ru[i]).or_else(|| ru.iter().position(|&x| x == c).map(|i| en[i])).unwrap_or(c)).collect()
}

/// Files and folders under `dir` whose name contains `q` (any case), as rows with absolute names. Walks breadth-first and
/// stops after LOCAL_MAX hits or LOCAL_MS so a search from C:\ cannot hang the window. The same walk also matches `q` typed
/// in the other keyboard layout; those hits are used only when `q` itself found nothing.
/// Returns (rows, stopped early, rows are for the other layout).
/// shortcut: blocks the UI while it walks (up to LOCAL_MS); move to a thread if deeper searches are wanted.
fn local_search(dir: &Path, q: &str) -> (Vec<Row>, bool, bool) {
    const LOCAL_MAX: usize = 500;
    const LOCAL_MS: u128 = 1500;
    let (q, t0) = (q.to_lowercase(), std::time::Instant::now());
    let alt = flip_layout(&q);
    let (mut out, mut other, mut queue) = (vec![], vec![], std::collections::VecDeque::from([dir.to_path_buf()]));
    let mut cut = false;
    'walk: while let Some(d) = queue.pop_front() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            if out.len() >= LOCAL_MAX || t0.elapsed().as_millis() > LOCAL_MS { cut = true; break 'walk; }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() && !ft.is_symlink() { queue.push_back(e.path()); } // no junction loops
            let name = e.file_name().to_string_lossy().to_lowercase();
            let hit = |v: &mut Vec<Row>| v.push(row(e.path().to_string_lossy().into_owned(), e.metadata().ok().as_ref()));
            if name.contains(&q) { hit(&mut out); } else if alt != q && other.len() < LOCAL_MAX && name.contains(&alt) { hit(&mut other); }
        }
    }
    if out.is_empty() && !other.is_empty() { (other, cut, true) } else { (out, cut, false) }
}

/// Drops all thumbnails and invalidates in-flight ones; call whenever the rows are replaced.
fn reset_thumbs() {
    GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let himl = ST.with_borrow(|st| st.himl);
    unsafe { ImageList_Remove(himl as _, -1) };
}

fn reload(h: HWND) { load(h, cur_dir()); }

/// (list, current dir, full paths of the selected rows)
fn selected() -> (HWND, PathBuf, Vec<PathBuf>) {
    ST.with_borrow(|st| {
        let (mut sel, mut i) = (vec![], -1isize);
        loop {
            i = put(st.list, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED as isize);
            let Some(r) = usize::try_from(i).ok().and_then(|i| st.rows.get(i)) else { break };
            if !r.hdr { sel.push(r.full(&st.dir)); }
        }
        (st.list, st.dir.clone(), sel)
    })
}

fn col(list: HWND, i: i32, text: &str, width: i32, right: bool) {
    let mut t = w(text);
    let mut c: LVCOLUMNW = unsafe { std::mem::zeroed() };
    c.mask = LVCF_TEXT | LVCF_WIDTH | LVCF_FMT; c.pszText = t.as_mut_ptr(); c.cx = width;
    c.fmt = if right { LVCFMT_RIGHT } else { LVCFMT_LEFT };
    put(list, LVM_INSERTCOLUMNW, i as usize, &c as *const _ as isize);
}

/// Opens a new tab on This PC (the drive list), the way Explorer's "+" and Ctrl+T do.
fn tab_new(h: HWND) {
    let dir = this_pc();
    let tab = ST.with_borrow_mut(|st| { st.tabs.push(dir.clone()); st.active = st.tabs.len() - 1; st.tab });
    tab_text(tab, 0, "", true);
    load(h, dir);
}

/// Opens `dir` in a new tab: the network group and "Open in a new tab" in the left pane use it.
fn tab_open(h: HWND, dir: PathBuf) {
    let tab = ST.with_borrow_mut(|st| { st.tabs.push(dir.clone()); st.active = st.tabs.len() - 1; st.tab });
    tab_text(tab, 0, "", true);
    load(h, dir);
}

fn tab_select(h: HWND, i: usize) {
    let dir = ST.with_borrow_mut(|st| { st.active = i; st.tabs[i].clone() });
    load(h, dir);
}

fn tab_close(h: HWND) { let a = ST.with_borrow(|st| st.active); tab_close_at(h, a); }

/// Closes tab `i` (the last one stays). The listing changes only when the active tab was the one closed.
fn tab_close_at(h: HWND, i: usize) {
    let (tab, was_active) = ST.with_borrow_mut(|st| {
        if st.tabs.len() < 2 || i >= st.tabs.len() { return (st.tab, None); }
        st.tabs.remove(i);
        let before = st.active;
        st.active = if i < before { before - 1 } else { before.min(st.tabs.len() - 1) };
        (st.tab, Some(i == before))
    });
    match was_active {
        Some(true) => { let a = ST.with_borrow(|st| st.active); tab_select(h, a); }
        Some(false) => tab_text(tab, 0, "", false),
        None => {}
    }
}

fn toggle_view(h: HWND) {
    set_view(h, !THUMBS.load(Relaxed));
}

/// The two views: the icon grid (the default: pictures, tight cells, small labels) and the details list.
fn set_view(h: HWND, icons: bool) {
    let list = ST.with_borrow(|st| st.list);
    ST.with_borrow_mut(|st| st.thumbs = icons);
    THUMBS.store(icons, Relaxed);
    unsafe {
        ui::row_height(list, !icons);
        put(list, LVM_SETVIEW, if icons { LV_VIEW_ICON } else { LV_VIEW_DETAILS } as usize, 0);
        if icons { ui::grid_spacing(list, ST.with_borrow(|st| is_this_pc(&st.dir))); }
        // labels under an icon are smaller than the rows of the details list
        put(list, WM_SETFONT, ui::font(if icons { 11 } else { 14 }, 400, ui::ui_face()) as usize, 1);
    }
    reload(h);
}

/// Address box in search mode ("? terms").
fn search_focus() {
    let edit = ST.with_borrow(|st| st.edit);
    unsafe {
        SetWindowTextW(edit, w("? ").as_ptr());
        SetFocus(edit);
    }
    put(edit, EM_SETSEL, 2, 2);
}

/// Wallpaper for the desktop window.
mod wall {
    use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::*, UI::WindowsAndMessaging::{IMAGE_BITMAP, LR_LOADFROMFILE, LoadImageW}};

    /// The part of a bw x bh picture that "fill" shows on a cw x ch screen: centered, cropped to the screen's aspect.
    /// (x, y, w, h) in the picture.
    pub fn fill_src(bw: i32, bh: i32, cw: i32, ch: i32) -> (i32, i32, i32, i32) {
        let (sw, sh) = if bw as i64 * ch as i64 > bh as i64 * cw as i64 { (bh * cw / ch.max(1), bh) } else { (bw, bw * ch / cw.max(1)) };
        ((bw - sw) / 2, (bh - sh) / 2, sw, sh)
    }

    /// Every monitor's rectangle, relative to the virtual screen's top-left (vx, vy).
    fn monitors(vx: i32, vy: i32) -> Vec<RECT> {
        unsafe extern "system" fn each(_: HMONITOR, _: HDC, r: *mut RECT, lp: isize) -> i32 {
            unsafe { (*(lp as *mut Vec<RECT>)).push(*r) };
            1
        }
        let mut v: Vec<RECT> = Vec::new();
        unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut v as *mut _ as isize) };
        v.iter().map(|r| RECT { left: r.left - vx, top: r.top - vy, right: r.right - vx, bottom: r.bottom - vy }).collect()
    }

    /// The BMP for the whole virtual screen (vx, vy, cw x ch): scaled with "fill" onto each monitor on its own, so a second
    /// monitor of another size or aspect gets a whole picture too. 0 = no wallpaper file.
    pub fn load(vx: i32, vy: i32, cw: i32, ch: i32) -> isize {
        let Ok(base) = std::env::var("LOCALAPPDATA") else { return 0 };
        let path: Vec<u16> = format!(r"{base}\rs-shell\wallpaper.bmp").encode_utf16().chain([0]).collect();
        unsafe {
            let src = LoadImageW(std::ptr::null_mut(), path.as_ptr(), IMAGE_BITMAP, 0, 0, LR_LOADFROMFILE);
            if src.is_null() { return 0; }
            let mut bm: BITMAP = std::mem::zeroed();
            GetObjectW(src, size_of::<BITMAP>() as i32, &mut bm as *mut _ as _);
            let screen = GetDC(std::ptr::null_mut());
            let dst = CreateCompatibleBitmap(screen, cw, ch);
            let (sdc, ddc) = (CreateCompatibleDC(screen), CreateCompatibleDC(screen));
            let (o1, o2) = (SelectObject(sdc, src), SelectObject(ddc, dst));
            PatBlt(ddc, 0, 0, cw, ch, BLACKNESS); // the gaps between monitors of different heights
            SetStretchBltMode(ddc, HALFTONE);
            let mut mons = monitors(vx, vy);
            if mons.is_empty() { mons.push(RECT { left: 0, top: 0, right: cw, bottom: ch }); }
            for m in mons {
                let (mw, mh) = (m.right - m.left, m.bottom - m.top);
                let (sx, sy, sw, sh) = fill_src(bm.bmWidth, bm.bmHeight, mw, mh);
                StretchBlt(ddc, m.left, m.top, mw, mh, sdc, sx, sy, sw, sh, SRCCOPY);
            }
            SelectObject(sdc, o1); SelectObject(ddc, o2);
            DeleteDC(sdc); DeleteDC(ddc); ReleaseDC(std::ptr::null_mut(), screen); DeleteObject(src);
            dst as isize
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn fill_crops_to_the_screen_aspect() {
            assert_eq!(super::fill_src(1920, 1080, 1920, 1080), (0, 0, 1920, 1080));
            assert_eq!(super::fill_src(1920, 1080, 1080, 1080), (420, 0, 1080, 1080)); // square screen: the sides go
            assert_eq!(super::fill_src(1920, 1080, 2560, 1080), (0, 135, 1920, 810)); // ultrawide: top and bottom go
            assert_eq!(super::fill_src(100, 100, 0, 0).2, 100); // no division by zero
        }
    }

    pub fn paint(dc: HDC, bmp: isize, cw: i32, ch: i32) {
        paint_at(dc, bmp, 0, 0, cw, ch);
    }

    /// One rectangle of the virtual-screen bitmap, at the DC's origin (a per-monitor wallpaper host).
    pub fn paint_at(dc: HDC, bmp: isize, x: i32, y: i32, w: i32, h: i32) {
        unsafe {
            let m = CreateCompatibleDC(dc);
            let old = SelectObject(m, bmp as _);
            BitBlt(dc, 0, 0, w, h, m, x, y, SRCCOPY);
            SelectObject(m, old);
            DeleteDC(m);
        }
    }
}

/// Shell integration through COM (windows crate): file operations, context menu, drag and drop.
mod sh {
    use std::{cell::RefCell, ffi::c_void, os::windows::ffi::OsStrExt, path::PathBuf};
    use windows::{core::*, Win32::{Foundation::*, System::{Com::*, DataExchange::RegisterClipboardFormatW, Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock}, Ole::*, SystemServices::MODIFIERKEYS_FLAGS}, UI::{Shell::{Common::ITEMIDLIST, *}, WindowsAndMessaging::*}}};

    #[derive(Clone, Copy)]
    pub enum Op { Copy, Move, Delete }

    /// [user Desktop, public Desktop] (the user one may be redirected, e.g. to OneDrive).
    pub fn desktop_dirs() -> Vec<PathBuf> {
        [&FOLDERID_Desktop, &FOLDERID_PublicDesktop].iter().filter_map(|id| unsafe {
            let p = SHGetKnownFolderPath(*id, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const c_void));
            s.map(PathBuf::from)
        }).collect()
    }

    /// A known folder where Windows says it is (it may be moved to another drive).
    pub fn known_dir(id: &GUID) -> Option<PathBuf> {
        unsafe {
            let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const c_void));
            s.map(PathBuf::from)
        }
    }

    pub fn init() { unsafe { let _ = OleInitialize(None); } }

    fn wide(p: &PathBuf) -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() }

    /// Copy / move into `to`, or recycle, on the calling thread. Must not be called while ST is borrowed (the shell pumps
    /// messages). The window uses `fileop_async` / `delete`: a long operation here would freeze it.
    pub fn fileop(h: *mut c_void, op: Op, from: &[PathBuf], to: &PathBuf) -> Result<()> {
        fileop_ex(h, op, from, to, true)
    }

    fn fileop_ex(h: *mut c_void, op: Op, from: &[PathBuf], to: &PathBuf, undo: bool) -> Result<()> {
        unsafe {
            let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
            fo.SetOwnerWindow(HWND(h))?;
            fo.SetOperationFlags(match op {
                Op::Delete if undo => FOF_ALLOWUNDO,
                Op::Delete => FILEOPERATION_FLAGS(0),
                _ => FOF_RENAMEONCOLLISION,
            })?;
            let item = |p: &PathBuf| SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(wide(p).as_ptr()), None);
            let dest = if matches!(op, Op::Delete) { None } else { Some(item(to)?) };
            for p in from {
                let it = item(p)?;
                match (op, &dest) {
                    (Op::Copy, Some(d)) => fo.CopyItem(&it, d, PCWSTR::null(), None)?,
                    (Op::Move, Some(d)) => fo.MoveItem(&it, d, PCWSTR::null(), None)?,
                    _ => fo.DeleteItem(&it, None)?,
                }
            }
            fo.PerformOperations()
        }
    }

    /// Delete through the Recycle Bin, on a worker thread: `IFileOperation` is synchronous and pumps its own messages, so
    /// a 200 GB folder would freeze our window (and the shell's progress dialog, whose owner is that window) for hours.
    /// The outcome comes back as WM_DELETED; a recycle that fails is retried permanently only after the UI asks.
    pub fn delete(h: *mut c_void, from: &[PathBuf]) {
        spawn_fileop(h, Op::Delete, from.to_vec(), None, true);
    }

    /// The permanent delete the UI offers after a failed recycle: the same worker path, without the undo flag.
    pub fn delete_permanent(h: *mut c_void, from: Vec<PathBuf>) {
        spawn_fileop(h, Op::Delete, from, None, false);
    }

    /// Copy / move into `to` on a worker thread; the window reloads when a WM_DELETED arrives.
    pub fn fileop_async(h: *mut c_void, op: Op, from: &[PathBuf], to: &PathBuf) {
        spawn_fileop(h, op, from.to_vec(), Some(to.clone()), true);
    }

    /// What a finished background shell operation sends to the window (WM_DELETED, lp).
    pub struct FileOpDone {
        pub err: Option<String>, // HRESULT text; a failed delete also goes to %TEMP%\winrust-delete.log
        pub retry: Vec<PathBuf>, // the recycle-bin delete failed here: the UI offers a permanent delete
    }

    /// Runs `fileop_ex` on a thread of its own with an STA of its own. `h` only reaches `SetOwnerWindow` (a handle is a
    /// number), so the shell's progress dialog stays modal to the window while our message loop keeps running.
    fn spawn_fileop(h: *mut c_void, op: Op, from: Vec<PathBuf>, to: Option<PathBuf>, undo: bool) {
        let hwnd = h as isize; // HWND is not Send: cross the thread boundary as a plain number
        std::thread::spawn(move || {
            let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            let initialized = init.is_ok();
            let mut err = if initialized { None } else { Some(format!("CoInitializeEx: {init:?}")) };
            let mut failed_delete = false;
            if initialized {
                let none = PathBuf::new();
                if let Err(e) = fileop_ex(hwnd as *mut c_void, op, &from, to.as_ref().unwrap_or(&none), undo) {
                    err = Some(format!("{e:?}"));
                    failed_delete = matches!(op, Op::Delete);
                }
                unsafe { CoUninitialize() }; // on the thread that initialized COM, even when the operation failed
            }
            if failed_delete {
                if let Some(e) = &err { log_delete(e, &from); }
            }
            // only a failed recycle is worth a retry: a permanent delete already is the last word
            let retry = if failed_delete && undo { from.clone() } else { vec![] };
            let done = Box::new(FileOpDone { err, retry });
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(hwnd as _, super::WM_DELETED, 0, Box::into_raw(done) as isize) };
        });
    }

    /// Keeps the evidence of a failed delete: which paths and which HRESULT (Btrfs, locked files, long paths).
    fn log_delete(err: &str, from: &[PathBuf]) {
        use std::io::Write as _;
        let Some(p) = std::env::var_os("TEMP").map(|t| PathBuf::from(t).join("winrust-delete.log")) else { return };
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "delete failed: {err}");
            for x in from {
                let _ = writeln!(f, "  {}", x.display());
            }
        }
    }

    /// Background thumbnail loader: GetImage for each (era, row, path) asked for, result posted to the main window.
    pub fn start_thumbs(main: isize) -> std::sync::mpsc::Sender<(u32, usize, PathBuf)> {
        let (tx, rx) = std::sync::mpsc::channel::<(u32, usize, PathBuf)>();
        std::thread::spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            for (era, row, path) in rx {
                if era != super::GEN.load(std::sync::atomic::Ordering::Relaxed) { continue; }
                let Ok(f) = SHCreateItemFromParsingName::<_, _, IShellItemImageFactory>(PCWSTR(wide(&path).as_ptr()), None) else { continue };
                let Ok(bmp) = f.GetImage(SIZE { cx: super::THUMB, cy: super::THUMB }, SIIGBF_BIGGERSIZEOK) else { continue };
                let padded = pad(bmp.0);
                windows_sys::Win32::Graphics::Gdi::DeleteObject(bmp.0 as _);
                if padded.is_null() { continue; }
                let b = Box::new((era, row, padded as isize));
                let _ = PostMessageW(Some(HWND(main as *mut c_void)), super::WM_THUMB, WPARAM(Box::into_raw(b) as usize), LPARAM(0));
            }
        });
        tx
    }

    /// Centers `src` (any size up to THUMB, 32-bit with alpha) on a transparent THUMB x THUMB bitmap: ImageList_Add needs exact sizes.
    unsafe fn pad(src: *mut c_void) -> *mut c_void {
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe {
            let t = super::THUMB;
            let mut bm: BITMAP = std::mem::zeroed();
            GetObjectW(src as _, size_of::<BITMAP>() as i32, &mut bm as *mut _ as _);
            let mut bi: BITMAPINFO = std::mem::zeroed();
            bi.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: t, biHeight: -t, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
            let mut bits = std::ptr::null_mut();
            let dst = CreateDIBSection(std::ptr::null_mut(), &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if dst.is_null() { return std::ptr::null_mut(); }
            let (dc, sdc) = (CreateCompatibleDC(std::ptr::null_mut()), CreateCompatibleDC(std::ptr::null_mut()));
            let (o1, o2) = (SelectObject(dc, dst), SelectObject(sdc, src as _));
            BitBlt(dc, (t - bm.bmWidth) / 2, (t - bm.bmHeight) / 2, bm.bmWidth, bm.bmHeight, sdc, 0, 0, SRCCOPY);
            SelectObject(dc, o1); SelectObject(sdc, o2);
            DeleteDC(dc); DeleteDC(sdc);
            dst
        }
    }

    /// Puts `paths` on the system clipboard like Explorer: the shell's own data object plus the "Preferred DropEffect"
    /// marker (copy or cut), so Ctrl+V works in a real Explorer window and in other file managers.
    pub fn clip_set(h: *mut c_void, paths: &[PathBuf], cut: bool) -> Result<()> {
        unsafe {
            let data: IDataObject = ui(HWND(h), paths)?;
            let fmt = FORMATETC { cfFormat: RegisterClipboardFormatW(w!("Preferred DropEffect")) as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0 as u32, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
            let hg = GlobalAlloc(GMEM_MOVEABLE, 4)?;
            *(GlobalLock(hg) as *mut u32) = if cut { DROPEFFECT_MOVE.0 } else { DROPEFFECT_COPY.0 };
            let _ = GlobalUnlock(hg);
            let med = STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: hg }, pUnkForRelease: std::mem::ManuallyDrop::new(None) };
            data.SetData(&fmt, &med, true)?; // the data object owns the memory from here
            OleSetClipboard(&data)
        }
    }

    /// What is on the system clipboard as files: (paths, cut?). None = no files there.
    pub fn clip_get() -> Option<(Vec<PathBuf>, bool)> {
        unsafe {
            let o = OleGetClipboard().ok()?;
            let paths = hdrop_paths(&o).ok().filter(|p| !p.is_empty())?;
            let fmt = FORMATETC { cfFormat: RegisterClipboardFormatW(w!("Preferred DropEffect")) as u16, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0 as u32, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
            let cut = o.GetData(&fmt).ok().is_some_and(|mut m| {
                let p = GlobalLock(m.u.hGlobal) as *const u32;
                let v = if p.is_null() { 0 } else { *p };
                let _ = GlobalUnlock(m.u.hGlobal);
                ReleaseStgMedium(&mut m);
                v & DROPEFFECT_MOVE.0 != 0
            });
            Some((paths, cut))
        }
    }

    pub fn clip_clear() { unsafe { let _ = OleSetClipboard(None); } }

    struct Pidls(Vec<*mut ITEMIDLIST>);
    impl Drop for Pidls { fn drop(&mut self) { for p in &self.0 { unsafe { CoTaskMemFree(Some(*p as *const c_void)) }; } } }

    /// A shell UI object (IContextMenu, IDataObject...) for items that all live in one folder.
    fn ui<T: Interface>(h: HWND, paths: &[PathBuf]) -> Result<T> {
        unsafe {
            let mut abs = Pidls(vec![]);
            for p in paths {
                let mut pidl = std::ptr::null_mut();
                SHParseDisplayName(PCWSTR(wide(p).as_ptr()), None, &mut pidl, 0, None)?;
                abs.0.push(pidl);
            }
            let parent: IShellFolder = SHBindToParent(abs.0[0], None)?;
            let kids: Vec<*const ITEMIDLIST> = abs.0.iter().map(|&p| ILFindLastID(p) as *const _).collect();
            parent.GetUIObjectOf(h, &kids, None)
        }
    }

    thread_local! { static CM2: RefCell<Option<IContextMenu2>> = const { RefCell::new(None) }; }

    /// Forwards owner-draw / init-popup menu messages so submenus like "Send to" get filled in.
    pub fn menu_msg(m: u32, wp: usize, lp: isize) -> bool {
        CM2.with_borrow(|c| c.as_ref().is_some_and(|c| unsafe { c.HandleMenuMsg(m, WPARAM(wp), LPARAM(lp)).is_ok() }))
    }

    /// The real shell context menu for `paths`, so 7-Zip/Git/etc. entries show up. `extra` = our own items on top
    /// (ids above the shell's range); returns the one picked, None when the shell handled the pick or nothing was picked.
    /// The name Windows shows for its own "Send to > Compressed (zipped) folder": the stem of the .ZFSendToTarget file in SendTo.
    pub(crate) fn system_zip_name() -> Option<String> {
        let dir = PathBuf::from(std::env::var_os("APPDATA")?).join(r"Microsoft\Windows\SendTo");
        let f = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ZFSendToTarget")))?;
        f.file_stem().map(|s| s.to_string_lossy().into_owned())
    }

    /// Takes Windows' own zip entry out of the menu: our "Compress to ZIP" is the one. (Menus of other zip tools, 7-Zip etc., stay.)
    /// The shell's own "Open with" runs OpenWith.exe, which cannot render without Explorer. Our menu already inserts its own
/// row at the top, so the shell one is removed; its verb is asked with GetCommandString (GCS_VERBW) rather than guessing
/// the localized text.
unsafe fn hide_system_openwith(cm: &IContextMenu, menu: HMENU) {
    let n = GetMenuItemCount(Some(menu));
    let mut i = n - 1;
    while i >= 0 {
        let id = GetMenuItemID(menu, i);
        if id != u32::MAX {
            let mut buf = [0u16; 64];
            if cm.GetCommandString(id as usize, GCS_VERBW, None, PSTR(buf.as_mut_ptr() as *mut u8), buf.len() as u32).is_ok() {
                let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                if String::from_utf16_lossy(&buf[..end]).eq_ignore_ascii_case("openwith") {
                    let _ = RemoveMenu(menu, i as u32, MF_BYPOSITION);
                }
            }
        }
        i -= 1;
    }
}

unsafe fn hide_system_zip(cm: &IContextMenu, menu: HMENU) {        use windows::Win32::UI::WindowsAndMessaging::{DeleteMenu, GetMenuItemCount, GetMenuStringW, GetSubMenu, MF_BYPOSITION};
        let Some(name) = system_zip_name() else { return };
        // WM_INITMENUPOPUP: the Send to submenu fills itself only when it is opened
        if let Ok(c2) = cm.cast::<IContextMenu2>() { let _ = unsafe { c2.HandleMenuMsg(0x0117, WPARAM(menu.0 as usize), LPARAM(0)) }; }
        unsafe fn strip(m: HMENU, name: &str) { unsafe {
            for i in (0..GetMenuItemCount(Some(m)).max(0)).rev() {
                let mut b = [0u16; 256];
                let n = GetMenuStringW(m, i as u32, Some(&mut b), MF_BYPOSITION);
                if String::from_utf16_lossy(&b[..n.max(0) as usize]).replace('&', "").eq_ignore_ascii_case(name) {
                    let _ = DeleteMenu(m, i as u32, MF_BYPOSITION);
                    continue;
                }
                let sub = GetSubMenu(m, i);
                if !sub.is_invalid() { strip(sub, name); }
            }
        } }
        unsafe { strip(menu, &name) };
    }

    unsafe extern "system" fn collect(h: HWND, lp: LPARAM) -> BOOL {
        unsafe { (*(lp.0 as *mut Vec<isize>)).push(h.0 as isize); }
        TRUE
    }

    fn top_windows() -> Vec<isize> {
        let mut v: Vec<isize> = vec![];
        unsafe { let _ = EnumWindows(Some(collect), LPARAM(&mut v as *mut _ as isize)); }
        v
    }

    /// Dialogs the shell opens for a command ("Open with" > choose another app, Properties) can land partly off screen on some
    /// setups. For a few seconds after the command, any new window that sticks out of its monitor is moved back inside.
    fn keep_on_screen(before: Vec<isize>) {
        std::thread::spawn(move || unsafe {
            for _ in 0..40 {
                std::thread::sleep(std::time::Duration::from_millis(150));
                for h in top_windows().into_iter().filter(|h| !before.contains(h)) {
                    let h = HWND(h as *mut c_void);
                    let mut r = RECT::default();
                    if !IsWindowVisible(h).as_bool() || IsZoomed(h).as_bool() || GetWindowRect(h, &mut r).is_err() || r.right <= r.left { continue; }
                    let mut mi = windows::Win32::Graphics::Gdi::MONITORINFO { cbSize: size_of::<windows::Win32::Graphics::Gdi::MONITORINFO>() as u32, ..Default::default() };
                    let mon = windows::Win32::Graphics::Gdi::MonitorFromWindow(h, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST);
                    if !windows::Win32::Graphics::Gdi::GetMonitorInfoW(mon, &mut mi).as_bool() { continue; }
                    let a = mi.rcWork;
                    if r.right - r.left > a.right - a.left || r.bottom - r.top > a.bottom - a.top { continue; } // maximized-like: not a stray dialog
                    let x = r.left.min(a.right - (r.right - r.left)).max(a.left);
                    let y = r.top.min(a.bottom - (r.bottom - r.top)).max(a.top);
                    if (x, y) != (r.left, r.top) {
                        let _ = SetWindowPos(h, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                }
            }
        });
    }

    pub fn show(h: *mut c_void, paths: &[PathBuf], extra: &[(u32, &str)]) -> Result<Option<u32>> {
        unsafe {
            let hwnd = HWND(h);
            let cm: IContextMenu = ui(hwnd, paths)?;
            let menu = CreatePopupMenu()?;
            cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL).ok()?;
            hide_system_zip(&cm, menu);
            hide_system_openwith(&cm, menu);
            if !extra.is_empty() {
                let _ = InsertMenuW(menu, 0, MF_BYPOSITION | MF_SEPARATOR, 0, PCWSTR::null());
                for (id, text) in extra.iter().rev() {
                    let t: Vec<u16> = text.encode_utf16().chain([0]).collect();
                    let _ = InsertMenuW(menu, 0, MF_BYPOSITION | MF_STRING, *id as usize, PCWSTR(t.as_ptr()));
                }
            }
            CM2.set(cm.cast::<IContextMenu2>().ok());
            let mut pt = POINT::default();
            GetCursorPos(&mut pt)?;
            let id = TrackPopupMenuEx(menu, TPM_RETURNCMD.0 | TPM_RIGHTBUTTON.0, pt.x, pt.y, hwnd, None).0;
            CM2.set(None);
            if extra.iter().any(|e| e.0 as i32 == id) {
                DestroyMenu(menu)?;
                return Ok(Some(id as u32));
            }
            if id > 0 {
                // ptInvoke: without it dialogs the shell opens ("Open with" > choose another app) land off screen
                let ci = CMINVOKECOMMANDINFOEX {
                    cbSize: size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                    fMask: CMIC_MASK_PTINVOKE,
                    hwnd,
                    lpVerb: PCSTR((id - 1) as usize as *const u8),
                    nShow: SW_SHOWNORMAL.0,
                    ptInvoke: pt,
                    ..Default::default()
                };
                let before = top_windows();
                cm.InvokeCommand(&ci as *const _ as *const CMINVOKECOMMANDINFO)?;
                keep_on_screen(before);
            }
            DestroyMenu(menu)?;
            Ok(None)
        }
    }

    #[cfg(test)]
    pub fn menu_dump(paths: &[PathBuf]) -> Vec<String> {
        use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuItemID, GetMenuStringW, GetSubMenu, MF_BYPOSITION};
        fn walk(m: HMENU, depth: usize, out: &mut Vec<String>) {
            unsafe {
                for i in 0..GetMenuItemCount(Some(m)).max(0) as u32 {
                    let mut b = [0u16; 256];
                    let n = GetMenuStringW(m, i, Some(&mut b), MF_BYPOSITION);
                    out.push(format!("{}{} (id {})", "  ".repeat(depth), String::from_utf16_lossy(&b[..n.max(0) as usize]), GetMenuItemID(m, i as i32)));
                    let sub = GetSubMenu(m, i as i32);
                    if !sub.is_invalid() { walk(sub, depth + 1, out); }
                }
            }
        }
        let mut out = vec![];
        unsafe {
            let cm: IContextMenu = ui(HWND::default(), paths).unwrap();
            let menu = CreatePopupMenu().unwrap();
            cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL).ok().unwrap();
            hide_system_zip(&cm, menu);
            walk(menu, 0, &mut out);
        }
        out
    }

    /// Starts an OLE drag of `paths` (blocks until dropped).
    pub fn drag(h: *mut c_void, paths: &[PathBuf]) -> Result<()> {
        unsafe {
            let hwnd = HWND(h);
            let data: IDataObject = ui(hwnd, paths)?;
            SHDoDragDrop(Some(hwnd), &data, None, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK)?;
            Ok(())
        }
    }

    fn hdrop_paths(o: &IDataObject) -> Result<Vec<PathBuf>> {
        unsafe {
            let fmt = FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0 as u32, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 };
            let mut med = o.GetData(&fmt)?;
            let hd = HDROP(med.u.hGlobal.0);
            let mut out = vec![];
            for i in 0..DragQueryFileW(hd, u32::MAX, None) {
                let mut buf = vec![0u16; DragQueryFileW(hd, i, None) as usize + 1];
                let n = DragQueryFileW(hd, i, Some(&mut buf)) as usize;
                out.push(PathBuf::from(String::from_utf16_lossy(&buf[..n])));
            }
            ReleaseStgMedium(&mut med);
            Ok(out)
        }
    }

    #[implement(IDropTarget)]
    struct DropT { main: isize }

    fn effect(k: MODIFIERKEYS_FLAGS) -> DROPEFFECT { if k.0 & 8 != 0 { DROPEFFECT_COPY } else { DROPEFFECT_MOVE } } // 8 = MK_CONTROL

    impl IDropTarget_Impl for DropT_Impl {
        fn DragEnter(&self, _o: Ref<'_, IDataObject>, k: MODIFIERKEYS_FLAGS, _pt: &POINTL, e: *mut DROPEFFECT) -> Result<()> { unsafe { *e = if super::in_zip() { DROPEFFECT_NONE } else { effect(k) } }; Ok(()) }
        fn DragOver(&self, k: MODIFIERKEYS_FLAGS, _pt: &POINTL, e: *mut DROPEFFECT) -> Result<()> { unsafe { *e = if super::in_zip() { DROPEFFECT_NONE } else { effect(k) } }; Ok(()) }
        fn DragLeave(&self) -> Result<()> { Ok(()) }
        fn Drop(&self, o: Ref<'_, IDataObject>, k: MODIFIERKEYS_FLAGS, pt: &POINTL, e: *mut DROPEFFECT) -> Result<()> {
            if super::desk::dragging() {
                // desktop icons dropped back on the desktop: a move of the icons, not of the files (the dragger finishes it)
                super::desk::dropped_here((pt.x, pt.y));
                unsafe { *e = DROPEFFECT_NONE };
                return Ok(());
            }
            if super::in_zip() { // nothing can be dropped into an archive here
                unsafe { *e = DROPEFFECT_NONE };
                return Ok(());
            }
            let paths = hdrop_paths(o.ok()?)?;
            let to = super::cur_dir();
            // like Explorer: same drive moves, other drive copies, Ctrl forces copy
            let copy = k.0 & 8 != 0 || paths.first().and_then(|p| p.components().next()) != to.components().next();
            unsafe { *e = if copy { DROPEFFECT_COPY } else { DROPEFFECT_MOVE } };
            // synchronous on purpose: for a DROPEFFECT_MOVE the OLE source (Explorer) deletes its originals as soon as
            // Drop returns, so the move must be finished by then. A cross-drive copy here still blocks this thread; going
            // async needs the "Performed DropEffect" format set first so the source does not delete its originals.
            let _ = fileop(self.main as *mut c_void, if copy { Op::Copy } else { Op::Move }, &paths, &to);
            unsafe { let _ = PostMessageW(Some(HWND(self.main as *mut c_void)), WM_APP, WPARAM(0), LPARAM(0)); }
            Ok(())
        }
    }

    pub fn register_drop(list: *mut c_void, main: isize) -> Result<()> {
        let t: IDropTarget = DropT { main }.into();
        unsafe { RegisterDragDrop(HWND(list), &t) }
    }
}


/// The desktop's own logic: icons stay where the user puts them (positions saved per name), dropping one onto a folder
/// icon moves it into the folder, and a host window for Wallpaper Engine (it draws behind the icons).
mod desk {
    #![allow(unsafe_op_in_unsafe_fn)]
    use super::*;
    use std::{cell::{Cell, RefCell}, collections::{HashMap, HashSet}};
    use windows_sys::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, DeleteDC, DeleteObject, DIB_RGB_COLORS, EndPaint, FillRect, GetDC, HDC, PAINTSTRUCT, ReleaseDC, ScreenToClient, SelectObject};

    pub const SPAWN_WORKERW: u32 = 0x052C; // what wallpaper apps send to "Progman" to get a window behind the icons
    const KEY: u32 = 0x0001_0001; // rgb(1, 0, 1): the list's background colour, made see-through while a live wallpaper runs

    thread_local! {
        static DRAG_FROM: Cell<Option<(i32, i32)>> = const { Cell::new(None) }; // our own icon drag: where it started (list coords)
        static DROP_AT: Cell<Option<(i32, i32)>> = const { Cell::new(None) }; // ...and where it was dropped (screen coords)
        static WORKERW: Cell<isize> = const { Cell::new(0) };
        /// The invisible click catcher over a live wallpaper (see `catcher_sync`).
        static CATCHER: Cell<isize> = const { Cell::new(0) };
        static LIVE: Cell<bool> = const { Cell::new(false) }; // a live wallpaper is running in WORKERW or in a host
        static WW_LIVE: Cell<bool> = const { Cell::new(false) }; // ...and it is in WORKERW, so the hosts must step aside
        /// One wallpaper surface per monitor (below the icons, above WorkerW). Rebuilt when the monitor list changes.
        static HOSTS: RefCell<Vec<super::we::Host>> = const { RefCell::new(Vec::new()) };
    }

    /// The per-monitor host windows, in `common::monitors::all()` order (screen rectangles).
    pub fn hosts() -> Vec<super::we::Host> {
        HOSTS.with_borrow(|v| v.clone())
    }

    /// Creates, moves and destroys the wallpaper host of every monitor. The desktop window covers the virtual screen, so a
    /// host is the monitor rectangle in its client coordinates; a monitor that changed size gets a fresh host (the WE
    /// pop-out inside it dies and the WE bridge opens a new one, sized to the new rectangle).
    pub unsafe fn hosts_sync(pm: HWND) {
        let (vx, vy, ..) = super::virt();
        let mons = winrust_common::monitors::all();
        let old = HOSTS.with_borrow(|v| v.clone());
        let mut used = vec![false; old.len()];
        let mut keep: Vec<super::we::Host> = Vec::with_capacity(mons.len());
        let inst = GetModuleHandleW(ptr::null());
        for m in &mons {
            let sr = (m.rc.left, m.rc.top, m.rc.right - m.rc.left, m.rc.bottom - m.rc.top);
            match old.iter().enumerate().position(|(i, o)| !used[i] && o.screen == sr) {
                Some(i) => {
                    used[i] = true;
                    keep.push(old[i]);
                }
                None => {
                    let hwnd = CreateWindowExW(
                        0,
                        w(we::HOST_CLASS).as_ptr(),
                        ptr::null(),
                        WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                        sr.0 - vx, sr.1 - vy, sr.2, sr.3,
                        pm,
                        0 as _,
                        inst,
                        ptr::null(),
                    );
                    if !hwnd.is_null() {
                        keep.push(super::we::Host { hwnd, screen: sr });
                    }
                }
            }
        }
        for (i, o) in old.iter().enumerate() {
            if !used[i] {
                DestroyWindow(o.hwnd);
            }
        }
        HOSTS.with_borrow_mut(|v| *v = keep);
        hosts_place();
    }

    /// Puts the hosts at the bottom of Progman's children (just above WorkerW, below the icon list and DefView).
    unsafe fn hosts_place() {
        let (vx, vy, ..) = super::virt();
        let list = HOSTS.with_borrow(|v| v.clone());
        for h in &list {
            SetWindowPos(h.hwnd, HWND_BOTTOM, h.screen.0 - vx, h.screen.1 - vy, h.screen.2, h.screen.3, SWP_NOACTIVATE);
            InvalidateRect(h.hwnd, ptr::null(), 1);
        }
        let ww = WORKERW.get() as HWND;
        if !ww.is_null() && IsWindow(ww) != 0 {
            SetWindowPos(ww, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        // A host or WorkerW is created at the top of the z-order and paints its wallpaper before it is sent to the
        // bottom: that paint stays over the icon list until the list repaints. Invalidate it, or 0x052C leaves the
        // desktop without icons (they are there, the list's surface is just covered by a stale wallpaper bitmap).
        let list = super::ST.with_borrow(|st| st.list);
        if !list.is_null() && IsWindow(list) != 0 {
            InvalidateRect(list, ptr::null(), 1);
        }
    }

    /// The wallpaper slice of one host: the virtual-screen bitmap already has each monitor's picture in place, so this is
    /// a straight BitBlt of the host's part of it (no second scaling, no change of the picture on this machine).
    unsafe fn paint_host(h: HWND, dc: HDC) {
        let (bmp, cw, ch) = super::WALL.get();
        let rect = HOSTS.with_borrow(|v| v.iter().find(|x| x.hwnd == h).map(|x| x.screen));
        let Some((sx, sy, sw, sh)) = rect else { return };
        let (vx, vy, ..) = super::virt();
        if bmp != 0 {
            let (x, y) = ((sx - vx).clamp(0, cw), (sy - vy).clamp(0, ch));
            let (w, hh) = (sw.min(cw - x).max(0), sh.min(ch - y).max(0));
            if w > 0 && hh > 0 {
                super::wall::paint_at(dc, bmp, x, y, w, hh);
            }
        } else {
            let b = windows_sys::Win32::Graphics::Gdi::GetSysColorBrush(1);
            FillRect(dc, &RECT { left: 0, top: 0, right: sw, bottom: sh }, b as _);
        }
    }

    pub unsafe extern "system" fn host_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match m {
            WM_ERASEBKGND => {
                paint_host(h, wp as HDC);
                1
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(h, &mut ps);
                paint_host(h, dc);
                EndPaint(h, &ps);
                0
            }
            // the hosts cover the secondary monitors: the desktop's empty-space menu must still open there
            WM_CONTEXTMENU => {
                super::bg_menu(GetParent(h));
                0
            }
            _ => DefWindowProcW(h, m, wp, lp),
        }
    }

    /// An invisible (alpha 1/255) window between the live wallpaper and the icons. While a live wallpaper runs the icon
    /// list is click-through (its background is the colour key), so a drag or a right click on the empty desktop landed in
    /// the wallpaper window: this one is in the way by design and runs the marquee itself (the list is not asked: its own
    /// marquee is drawn opaque and only inside its window, which does not reach a second monitor). Hidden when there is no
    /// live wallpaper, where the list takes the mouse itself.
    pub unsafe fn catcher_sync(pm: HWND) {
        let mut c = CATCHER.get() as HWND;
        if c.is_null() || IsWindow(c) == 0 {
            let (_, _, vw, vh) = super::virt();
            c = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_NOACTIVATE,
                w("WinRustCatcher").as_ptr(),
                ptr::null(),
                WS_CHILD,
                0,
                0,
                vw,
                vh,
                pm,
                0 as _,
                GetModuleHandleW(ptr::null()),
                ptr::null(),
            );
            if c.is_null() {
                return;
            }
            // alpha 1: invisible to the eye but still hit-tested (a fully transparent window would be click-through)
            SetLayeredWindowAttributes(c, 0, 1, LWA_ALPHA);
            CATCHER.set(c as isize);
        }
        if LIVE.get() {
            let list = super::ST.with_borrow(|st| st.list);
            // right under the icon list and over the wallpaper (our hosts or WE's own window)
            SetWindowPos(c, list, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            ShowWindow(c, SW_SHOWNOACTIVATE);
        } else {
            ShowWindow(c, SW_HIDE);
        }
        super::marquee::hide();
    }

    /// Refreshes the icons when either Desktop folder changes (an app saving, renaming or deleting a file there): one
    /// blocked `ReadDirectoryChangesW` per folder, no polling and no extra timers. The burst a file operation produces
    /// is coalesced by the 250 ms timer in the window procedure.
    pub unsafe fn watch(h: HWND) {
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW};
        for dir in super::sh::desktop_dirs() {
            let hwnd = h as isize;
            std::thread::spawn(move || unsafe {
                let path = w(&dir.to_string_lossy());
                let f = CreateFileW(path.as_ptr(), FILE_LIST_DIRECTORY, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut());
                if f == INVALID_HANDLE_VALUE {
                    return;
                }
                let mut buf = [0u8; 4096];
                loop {
                    let mut got = 0u32;
                    let ok = ReadDirectoryChangesW(f, buf.as_mut_ptr() as _, buf.len() as u32, 0, FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE, &mut got, std::ptr::null_mut(), None);
                    if ok == 0 {
                        break;
                    }
                    PostMessageW(hwnd as _, super::WM_DESK_CHANGED, 0, 0);
                }
                CloseHandle(f);
            });
        }
    }

    /// The click catcher only exists while the icon list is click-through (live wallpaper): presses on empty desktop
    /// start our marquee (`marquee`), which draws over any monitor and selects the icons it covers on release.
    pub unsafe extern "system" fn catcher_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let screen = |h: HWND, lp: LPARAM| {
            let mut r: RECT = std::mem::zeroed();
            GetWindowRect(h, &mut r);
            POINT { x: r.left + (lp & 0xffff) as i16 as i32, y: r.top + ((lp >> 16) & 0xffff) as i16 as i32 }
        };
        match m {
            WM_ERASEBKGND => 1,
            WM_LBUTTONDOWN => {
                let list = super::ST.with_borrow(|st| st.list);
                let pt = screen(h, lp);
                if !list.is_null() && IsWindow(list) != 0 {
                    super::marquee::begin(list, pt.x, pt.y, false);
                    SetCapture(h);
                }
                0
            }
            WM_MOUSEMOVE => {
                if super::marquee::dragging() {
                    let pt = screen(h, lp);
                    super::marquee::track(pt.x, pt.y);
                }
                0
            }
            WM_LBUTTONUP => {
                let pt = screen(h, lp);
                let done = super::marquee::finish(pt.x, pt.y); // before ReleaseCapture: that sends WM_CAPTURECHANGED
                ReleaseCapture();
                if done {
                    let list = super::ST.with_borrow(|st| st.list);
                    if !list.is_null() && IsWindow(list) != 0 {
                        InvalidateRect(list, ptr::null(), 1);
                    }
                }
                0
            }
            WM_CAPTURECHANGED => {
                super::marquee::cancel();
                0
            }            WM_RBUTTONUP => {
                super::bg_menu(GetParent(h));
                0
            }
            _ => DefWindowProcW(h, m, wp, lp),
        }
    }

    /// The live-wallpaper window follows the desktop's size (monitors added, removed or resized).
    pub unsafe fn size_workerw(cw: i32, ch: i32) {
        let ww = WORKERW.get() as HWND;
        if !ww.is_null() && IsWindow(ww) != 0 {
            MoveWindow(ww, 0, 0, cw, ch, 1);
        }
    }

    pub fn pos_file() -> PathBuf {
        PathBuf::from(env::var_os("APPDATA").unwrap_or_default()).join(r"rust-explorer\desktop-icons.txt")
    }

    /// Saved icon positions: name -> (x, y) in list pixels.
    fn saved() -> HashMap<String, (i32, i32)> {
        fs::read_to_string(pos_file()).unwrap_or_default().lines().filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.to_string()))
        }).map(|(x, y, n): (i32, i32, String)| (n, (x, y))).collect()
    }

    fn names() -> Vec<String> { ST.with_borrow(|st| st.rows.iter().map(|r| r.name.clone()).collect()) }

    fn pos(list: HWND, i: usize) -> (i32, i32) {
        let mut p = POINT { x: 0, y: 0 };
        put(list, LVM_GETITEMPOSITION, i, &mut p as *mut _ as isize);
        (p.x, p.y)
    }

    fn set_pos(list: HWND, i: usize, (x, y): (i32, i32)) {
        let p = POINT { x, y };
        put(list, LVM_SETITEMPOSITION32, i, &p as *const _ as isize);
    }

    /// Writes every icon's position (after a move or a fresh layout).
    pub fn save(list: HWND) {
        let lines: String = names().iter().enumerate().map(|(i, n)| { let (x, y) = pos(list, i); format!("{x}\t{y}\t{n}\n") }).collect();
        let f = pos_file();
        let _ = fs::create_dir_all(f.parent().unwrap_or(&f));
        let _ = fs::write(f, lines);
    }

    /// The icon grid: cell size, and how many columns / rows fit.
    struct Grid { cx: i32, cy: i32, cols: i32, rows: i32 }

    impl Grid {
        fn of(list: HWND) -> Grid {
            let s = put(list, LVM_GETITEMSPACING, 0, 0) as u32;
            let (cx, cy) = (((s & 0xFFFF) as i32).max(1), ((s >> 16) as i32).max(1));
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(list, &mut r) };
            if r.bottom <= 0 {
                // the first fill runs in WM_CREATE, before WM_SIZE gives the list its size: the size it is about to get
                r = unsafe { RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) - desk_top() - desk_bottom() } };
            }
            Grid { cx, cy, cols: (r.right / cx).max(1), rows: (r.bottom / cy).max(1) }
        }
        /// Nearest cell to a position; None if it is off the grid (screen got smaller).
        fn cell(&self, (x, y): (i32, i32)) -> Option<(i32, i32)> {
            let c = ((x + self.cx / 2).div_euclid(self.cx), (y + self.cy / 2).div_euclid(self.cy));
            (c.0 >= 0 && c.1 >= 0 && c.0 < self.cols && c.1 < self.rows).then_some(c)
        }
        fn at(&self, (c, r): (i32, i32)) -> (i32, i32) { (c * self.cx, r * self.cy) }
        /// Free cell nearest to `want`; with no wish the first free one going down the columns, like Windows.
        fn free(&self, taken: &HashSet<(i32, i32)>, want: Option<(i32, i32)>) -> (i32, i32) {
            let all = (0..self.cols).flat_map(|c| (0..self.rows).map(move |r| (c, r))).filter(|c| !taken.contains(c));
            match want {
                Some((wc, wr)) => all.min_by_key(|&(c, r)| (c - wc).pow(2) + (r - wr).pow(2)),
                None => all.min(),
            }
            .unwrap_or((0, 0))
        }
    }

    /// Fills the (non-virtual) desktop list with `n` rows and puts every icon on its saved cell, new ones on free cells.
    pub fn fill(list: HWND, n: usize) {
        for i in 0..n {
            let mut it: LVITEMW = unsafe { std::mem::zeroed() };
            it.mask = LVIF_TEXT | LVIF_IMAGE;
            it.iItem = i as i32;
            it.pszText = usize::MAX as *mut u16; // LPSTR_TEXTCALLBACKW: text and icon come from LVN_GETDISPINFO
            it.iImage = -1; // I_IMAGECALLBACK
            put(list, LVM_INSERTITEMW, 0, &it as *const _ as isize);
        }
        let (g, old, names) = (Grid::of(list), saved(), names());
        let mut taken = HashSet::new();
        let mut later = vec![];
        for (i, name) in names.iter().enumerate() {
            match old.get(name).and_then(|&p| g.cell(p)).filter(|c| !taken.contains(c)) {
                Some(c) => { taken.insert(c); set_pos(list, i, g.at(c)); }
                None => later.push(i),
            }
        }
        for i in later {
            let c = g.free(&taken, None);
            taken.insert(c);
            set_pos(list, i, g.at(c));
        }
        save(list);
    }

    // ---------- moving icons ----------

    /// Our own drag of desktop icons starts (`from` in list coordinates).
    pub fn drag_start(from: (i32, i32)) { DRAG_FROM.set(Some(from)); DROP_AT.set(None); }
    pub fn dragging() -> bool { DRAG_FROM.get().is_some() }
    /// Called by the drop target: the drag ended on the desktop itself at screen point `at`.
    pub fn dropped_here(at: (i32, i32)) { DROP_AT.set(Some(at)); }

    /// After the drag: on a folder icon (not one of the dragged) the selection moves into that folder; anywhere else the
    /// icons move by the drag distance, snapped to the grid, each to the nearest free cell. true = handled here.
    pub fn drag_end(h: HWND, list: HWND) -> bool {
        let Some(from) = DRAG_FROM.take() else { return false };
        // normally our drop target recorded where it ended; over a live wallpaper OLE never reaches it (the wallpaper window of
        // another process is under the pointer), so then: the pointer is over our desktop window = the drop was on the desktop
        let at = DROP_AT.take().or_else(|| unsafe {
            let mut c = POINT { x: 0, y: 0 };
            GetCursorPos(&mut c);
            (GetAncestor(WindowFromPoint(c), GA_ROOT) == h).then_some((c.x, c.y))
        });
        let Some(at) = at else { return false };
        let mut pt = POINT { x: at.0, y: at.1 };
        unsafe { ScreenToClient(list, &mut pt) };
        let (_, dir, paths) = selected();
        let mut sel = vec![];
        let mut i = -1isize;
        loop {
            i = put(list, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED as isize);
            if i < 0 { break; }
            sel.push(i as usize);
        }
        // onto a folder icon
        let mut ht: LVHITTESTINFO = unsafe { std::mem::zeroed() };
        ht.pt = pt;
        let tgt = put(list, LVM_HITTEST, 0, &mut ht as *mut _ as isize);
        if tgt >= 0 && !sel.contains(&(tgt as usize)) {
            let target = ST.with_borrow(|st| st.rows.get(tgt as usize).filter(|r| r.dir).map(|r| st.dir.join(&r.name)));
            if let Some(t) = target {
                let _ = sh::fileop(h as _, sh::Op::Move, &paths, &t); // a rename: instant, no need for a worker
                let _ = dir;
                unsafe { PostMessageW(h, WM_APP, 0, 0) }; // reload: the moved icons are gone from here
                return true;
            }
        }
        let g = Grid::of(list);
        let n = put(list, LVM_GETITEMCOUNT, 0, 0).max(0) as usize;
        let mut taken: HashSet<(i32, i32)> = (0..n).filter(|i| !sel.contains(i)).filter_map(|i| g.cell(pos(list, i))).collect();
        let (dx, dy) = (pt.x - from.0, pt.y - from.1);
        for &i in &sel {
            let (x, y) = pos(list, i);
            let want = g.cell((x + dx, y + dy)).unwrap_or_else(|| {
                // dragged off the grid: the nearest edge cell
                (((x + dx) / g.cx).clamp(0, g.cols - 1), ((y + dy) / g.cy).clamp(0, g.rows - 1))
            });
            let c = if taken.contains(&want) { g.free(&taken, Some(want)) } else { want };
            taken.insert(c);
            set_pos(list, i, g.at(c));
        }
        save(list);
        true
    }

    // ---------- Wallpaper Engine (and other live wallpapers) ----------
    //
    // They look for the "Progman" window (that is our desktop's class), send it 0x052C and put their window into the
    // "WorkerW" that appears: Windows 11 24H2 has it as a child of Progman, under the icons. Ours is the same, and while it
    // holds a wallpaper the icon list turns see-through (colour key) so the wallpaper shows between the icons.

    pub unsafe fn spawn_workerw(h: HWND) {
        if WORKERW.get() != 0 && IsWindow(WORKERW.get() as HWND) != 0 {
            hosts_place();
            return;
        }
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(h, &mut r);
        let ww = CreateWindowExW(0, w("WorkerW").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS, 0, 0, r.right, r.bottom, h, 0 as _, GetModuleHandleW(ptr::null()), ptr::null());
        WORKERW.set(ww as isize);
        hosts_place(); // WorkerW at the very bottom, under every wallpaper host and the icon list
    }

    /// True when WE (or another wallpaper app) put its window in the WorkerW we expose: its own native hook, which
    /// means no pop-outs are needed.
    pub unsafe fn workerw_live() -> bool {
        let ww = WORKERW.get() as HWND;
        !ww.is_null() && IsWindow(ww) != 0 && !GetWindow(ww, GW_CHILD).is_null()
    }

    /// Every couple of seconds: a wallpaper window inside WorkerW = WE attached by itself (raised topology); a child in
    /// one of our hosts = the WE bridge attached a pop-out there. Both mean the list must become see-through.
    pub unsafe fn check_live(list: HWND) {
        let ww_live = workerw_live();
        if ww_live != WW_LIVE.replace(ww_live) {
            // a wallpaper in WorkerW is *below* the hosts: they have to get out of the way (hidden, not destroyed, so
            // the picture is back at once when WE closes it)
            let show = if ww_live { SW_HIDE } else { SW_SHOWNOACTIVATE };
            HOSTS.with_borrow(|v| for h in v {
                ShowWindow(h.hwnd, show);
            });
            if !ww_live {
                hosts_place();
            }
        }
        let host_live = HOSTS.with_borrow(|v| v.iter().any(|h| !GetWindow(h.hwnd, GW_CHILD).is_null()));
        let live = ww_live || host_live;
        if live == LIVE.replace(live) {
            return;
        }
        // the list stays "transparent" (its background is what the desktop paints: the key colour while live, see erase()),
        // so it keeps drawing the label shadows; the colour key then cuts the background out
        let ex = GetWindowLongPtrW(list, GWL_EXSTYLE);
        if live {
            SetWindowLongPtrW(list, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
            SetLayeredWindowAttributes(list, KEY, 0, LWA_COLORKEY);
        } else {
            SetWindowLongPtrW(list, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
        }
        InvalidateRect(list, ptr::null(), 1);
        InvalidateRect(GetParent(list), ptr::null(), 1);
        catcher_sync(GetParent(list)); // visible only while the list is click-through
    }

    /// The desktop's background: the key colour while a live wallpaper shows through, else the picture. false = none to paint.
    pub unsafe fn erase(dc: HDC, h: HWND) -> bool {
        let (bmp, cw, ch) = WALL.get();
        if LIVE.get() {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(h, &mut r);
            let b = CreateSolidBrush(KEY);
            FillRect(dc, &r, b);
            DeleteObject(b);
        } else if bmp != 0 {
            wall::paint(dc, bmp, cw, ch);
        } else {
            return false;
        }
        true
    }

    /// WorkerW without a wallpaper in it shows the picture wallpaper, like the desktop behind it.
    pub unsafe extern "system" fn workerw_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if m == WM_ERASEBKGND {
            let (bmp, cw, ch) = WALL.get();
            if bmp != 0 { wall::paint(wp as _, bmp, cw, ch); } else { let mut r: RECT = std::mem::zeroed(); GetClientRect(h, &mut r); FillRect(wp as _, &r, windows_sys::Win32::Graphics::Gdi::GetSysColorBrush(1) as _); }
            return 1;
        }
        DefWindowProcW(h, m, wp, lp)
    }

    #[cfg(test)]
    mod tests {
        use super::Grid;
        use std::collections::HashSet;

        #[test]
        fn grid_cells() {
            let g = Grid { cx: 100, cy: 90, cols: 5, rows: 4 };
            assert_eq!(g.cell((0, 0)), Some((0, 0)));
            assert_eq!(g.cell((149, 44)), Some((1, 0))); // rounds to the nearest cell
            assert_eq!(g.cell((900, 0)), None); // off the grid
            let mut taken: HashSet<_> = [(0, 0), (0, 1)].into();
            assert_eq!(g.free(&taken, None), (0, 2)); // down the first column first
            taken.insert((2, 2));
            assert_eq!(g.free(&taken, Some((2, 2))), (1, 2)); // nearest free to the wish (ties: lowest)
        }
    }
}

/// Trace of the icon drags (%TEMP%\rx-drag.log), to find out where one stops working. Cheap: a few lines per drag.
fn drag_log(s: &str) {
    use std::io::Write;
    let p = std::env::temp_dir().join("rx-drag.log");
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(p) { let _ = writeln!(f, "{s}"); }
}

/// Ctrl+Backspace in an edit box (it arrives as WM_CHAR 0x7F): deletes back to the start of the previous word.
unsafe fn delete_word(h: HWND) {
    unsafe {
        let (mut a, mut b) = (0u32, 0u32);
        SendMessageW(h, EM_GETSEL, &mut a as *mut u32 as usize, &mut b as *mut u32 as isize);
        if a == b {
            let n = GetWindowTextLengthW(h) as usize;
            let mut buf = vec![0u16; n + 1];
            GetWindowTextW(h, buf.as_mut_ptr(), n as i32 + 1);
            let mut i = a as usize;
            while i > 0 && buf[i - 1] == b' ' as u16 { i -= 1; }
            while i > 0 && buf[i - 1] != b' ' as u16 { i -= 1; }
            SendMessageW(h, EM_SETSEL, i, a as isize);
        }
        SendMessageW(h, EM_REPLACESEL, 1, w("").as_ptr() as isize);
    }
}

/// Rename box subclass: only Ctrl+Backspace is ours.
/// The list keeps the wheel for itself. While the pointer is over the nav pane the pane should scroll instead.
/// Pressing on empty space runs our own marquee (`marquee`): the same translucent white band as the desktop, instead of
/// the control's opaque blue rectangle.
unsafe extern "system" fn list_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if m == WM_MOUSEWHEEL && ui::wheel_to_nav(wp, lp) {
        return 0;
    }
    let point = || POINT { x: (lp & 0xffff) as i16 as i32, y: ((lp >> 16) & 0xffff) as i16 as i32 };
    match m {
        WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
            let pt = point();
            let mut ht: LVHITTESTINFO = std::mem::zeroed();
            ht.pt = pt;
            if SendMessageW(h, LVM_HITTEST, 0, &mut ht as *mut _ as isize) < 0 {
                // empty space: our band, the list never sees the press (so its blue rectangle never starts)
                let mut sc = pt;
                ClientToScreen(h, &mut sc);
                SetFocus(h);
                marquee::begin(h, sc.x, sc.y, true);
                SetCapture(h);
                return 0;
            }
        }
        WM_MOUSEMOVE if marquee::dragging() => {
            let mut sc = point();
            ClientToScreen(h, &mut sc);
            marquee::track(sc.x, sc.y);
            return 0;
        }
        WM_LBUTTONUP if marquee::dragging() => {
            let mut sc = point();
            ClientToScreen(h, &mut sc);
            marquee::finish(sc.x, sc.y);
            ReleaseCapture();
            InvalidateRect(h, ptr::null(), 0);
            return 0;
        }
        WM_CAPTURECHANGED if marquee::dragging() => {
            marquee::cancel();
            return 0;
        }
        _ => {}
    }
    CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(LIST_OLD.load(std::sync::atomic::Ordering::Relaxed)), h, m, wp, lp)
}

unsafe extern "system" fn label_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if m == WM_CHAR && wp == 0x7F { delete_word(h); return 0; }
        CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(LABEL_OLD.load(std::sync::atomic::Ordering::Relaxed)), h, m, wp, lp)
    }
}

/// Address bar subclass: Enter / Esc go to the main window, everything else is the normal edit control.
unsafe extern "system" fn edit_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if matches!(wp, 13 | 27) && matches!(m, WM_KEYDOWN | WM_CHAR) {
            if m == WM_KEYDOWN { PostMessageW(GetAncestor(h, GA_ROOT), WM_GO, wp, 0); }
            return 0; // WM_CHAR swallowed so there is no beep
        }
        if m == WM_CHAR && wp == 0x7F { delete_word(h); return 0; }
        CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(EDIT_OLD.load(std::sync::atomic::Ordering::Relaxed)), h, m, wp, lp)
    }
}

unsafe extern "system" fn proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if m == winrust_common::style::msg() {
            // the topbar switched the WinRust style: take it now (the desktop's list keeps its see-through colours)
            winrust_common::style::reload();
            if desk() { InvalidateRect(h, ptr::null(), 1); } else { ui::restyle(h); }
            return 0;
        }
        match m {
            WM_CREATE => {
                let inst = GetModuleHandleW(ptr::null());
                // desktop mode has no tab bar / address bar: the null handles are harmless to every SendMessage below
                let (mut tab, mut edit) = (0 as HWND, 0 as HWND);
                if !desk() {
                    ui::dark_title(h);
                    // the chrome (tabs, toolbar, address pill) is one painted window; the address box is a real edit inside its pill
                    tab = CreateWindowExW(0, w("RxChrome").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN, 0, 0, 0, 0, h, 0 as _, inst, ptr::null());
                    ui::nav_refresh(CreateWindowExW(0, w("RxNav").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE, 0, 0, 0, 0, h, 0 as _, inst, ptr::null()));
                    edit = CreateWindowExW(0, w("EDIT").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32, 0, 0, 0, 0, tab, 0 as _, inst, ptr::null());
                    put(edit, WM_SETFONT, ui::font(14, 400, ui::ui_face()) as usize, 1);
                    EDIT_OLD.store(SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize), std::sync::atomic::Ordering::Relaxed);
                }
                let view = if desk() { LVS_ICON } else { LVS_REPORT };
                let list = CreateWindowExW(0, w("SysListView32").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE | view | if desk() { 0 } else { LVS_OWNERDATA } | LVS_EDITLABELS | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS, 0, 0, 0, 0, h, 0 as _, inst, ptr::null());
                LIST_OLD.store(SetWindowLongPtrW(list, GWLP_WNDPROC, list_proc as *const () as isize), std::sync::atomic::Ordering::Relaxed); // wheel over the nav pane scrolls it
                put(list, LVM_SETEXTENDEDLISTVIEWSTYLE, 0, (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize);
                let mut fi: SHFILEINFOW = std::mem::zeroed();
                let il = SHGetFileInfoW(w("C:\\").as_ptr(), 0, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | SHGFI_SMALLICON);
                put(list, LVM_SETIMAGELIST, LVSIL_SMALL as usize, il as isize);
                if desk() {
                    // system large icons; indexes are shared with the small list, so icon_of() works unchanged
                    let big = SHGetFileInfoW(w("C:\\").as_ptr(), 0, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | SHGFI_LARGEICON);
                    put(list, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, big as isize);
                    // Our own list of the same size: the shell's one bitmap per icon is replaced by the sharpest asset of
                    // each file (`icon::best`). It also stays the fallback for the paths that have no icon of their own.
                    let (mut icx, mut icy) = (0i32, 0i32);
                    let mut ours: HIMAGELIST = 0;
                    if ImageList_GetIconSize(big as _, &mut icx, &mut icy) != 0 && icx > 0 {
                        ours = ImageList_Create(icx, icy, ILC_COLOR32, 32, 32);
                    }
                    if ours != 0 {
                        put(list, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, ours as isize);
                        DESK_IL.set((ours as isize, big as isize, icx));
                    }
                    apply_icon_size(list);
                    // transparent background (the desktop paints it) and shadowed labels, readable on light wallpapers too
                    let ex = LVS_EX_TRANSPARENTBKGND; // the labels are painted by hand (ui::desk_draw): no themed light glow
                    put(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
                    for msg in [LVM_SETBKCOLOR, LVM_SETTEXTBKCOLOR] { put(list, msg, 0, CLR_NONE); }
                    put(list, LVM_SETTEXTCOLOR, 0, 0x00FF_FFFF);
                    reload_wall();
                    // grey (not ClearType) smoothing: on a live wallpaper the labels sit on a colour key, where ClearType leaves coloured fringes
                    put(list, WM_SETFONT, windows_sys::Win32::Graphics::Gdi::CreateFontW(-ui::sc(13), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 4, 0, w(ui::ui_face()).as_ptr()) as usize, 1);
                    // like explorer's desktop: wallpaper apps look for a SHELLDLL_DefView under Progman
                    CreateWindowExW(0, w("SHELLDLL_DefView").as_ptr(), ptr::null(), WS_CHILD, 0, 0, 0, 0, h, 0 as _, inst, ptr::null());
                    desk::hosts_sync(h); // one wallpaper surface per monitor, under the icon list
                    desk::catcher_sync(h); // invisible click catcher over the live wallpaper
                    desk::watch(h); // a file saved / renamed / deleted on the Desktop reloads the icons
                    SetTimer(h, 7, 2000, None); // live wallpaper on / off (desk::check_live), WE bridge (we::tick)
                } else {
                    let himl = ImageList_Create(THUMB, THUMB, ILC_COLOR32, 64, 64);
                    put(list, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, himl as isize);
                    let _ = TX.set(sh::start_thumbs(h as isize));
                    ST.with_borrow_mut(|st| st.himl = himl as isize);
                    // the same columns, in the same order, as Explorer's details view
                    col(list, 0, t!("Имя", "Name", ja: "名前", pt: "Nome", zh: "名称"), ui::sc(360), false);
                    col(list, 1, t!("Дата изменения", "Date modified", ja: "更新日時", pt: "Data de modificação", zh: "修改日期"), ui::sc(160), false);
                    col(list, 2, t!("Тип", "Type", ja: "種類", pt: "Tipo", zh: "类型"), ui::sc(140), false);
                    col(list, 3, t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小"), ui::sc(96), true);
                    ui::style_list(list);
                }
                ST.with_borrow_mut(|st| { st.list = list; st.tab = tab; st.edit = edit; st.tabs = vec![PathBuf::new()]; });
                if !desk() { ui::place_edit(tab); }
                let _ = sh::register_drop(list as _, h as isize);
                if desk() && icons_hidden() { ShowWindow(list, SW_HIDE); }
                // live wallpaper: the list is see-through (colour key), so over empty desktop OLE finds the wallpaper window under it,
                // and walks up from there to the desktop window: it must accept the drop too
                if desk() { let _ = sh::register_drop(h as _, h as isize); }
                let (start, pick) = if desk() { (sh::desktop_dirs().into_iter().next().unwrap_or_default(), None) } else { start_target() };
                if !desk() {
                    // the icon grid is the default look; the toolbar button and Ctrl+M switch to the details list
                    ST.with_borrow_mut(|st| st.thumbs = true);
                    THUMBS.store(true, Relaxed);
                    ui::row_height(list, false);
                    put(list, LVM_SETVIEW, LV_VIEW_ICON as usize, 0);
                    put(list, WM_SETFONT, ui::font(11, 400, ui::ui_face()) as usize, 1);
                }
                load(h, start);
                if let Some(name) = pick { select_row(&name); }
                if desk() {
                    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos};
                    let mut dirs: Vec<PathBuf> = [&FOLDERID_Desktop, &FOLDERID_Downloads, &FOLDERID_Documents, &FOLDERID_Pictures, &FOLDERID_Music, &FOLDERID_Videos].iter().filter_map(|id| sh::known_dir(id)).collect();
                    dirs.extend(ui::quick_items());
                    shellwin::register_resident(&dirs);
                } else {
                    if !in_pc() { shellwin::navigated(&cur_dir()); }
                    shellwin::register(h as isize);
                }
                0
            }
            // the bar came, went or changed size (it sets the work area), or the screen changed: the icon list follows
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE if desk() => {
                if m == WM_DISPLAYCHANGE {
                    reload_wall(); // monitors came, went or changed resolution: each one gets its picture again
                    let (vx, vy, vw, vh) = virt();
                    SetWindowPos(h, HWND_BOTTOM, vx, vy, vw, vh, SWP_NOACTIVATE); // WM_SIZE lays out
                    desk::hosts_sync(h); // hosts follow the new monitor rectangles (recreated when one changed)
                    InvalidateRect(h, ptr::null(), 1);
                } else if wp == SPI_SETWORKAREA as usize {
                    place_desk_list(ST.with_borrow(|st| st.list));
                }
                0
            }
            // per-monitor DPI in a system-DPI-aware process is rare, but a scaling change must not leave the hosts on the
            // old rectangles (and the picture composed for the old monitor sizes)
            WM_DPICHANGED if desk() => {
                reload_wall();
                desk::hosts_sync(h);
                InvalidateRect(h, ptr::null(), 1);
                0
            }
            // mouse buttons 4 / 5 (and keyboard media keys) arrive as APPCOMMAND_BROWSER_BACKWARD / _FORWARD
            0x0319 if !desk() && matches!((lp >> 16) & 0x7FF, 1 | 2) => { nav(h, (lp >> 16) & 0x7FF == 1); 1 }
            // the frosted background under the window is a capture of the desktop: retake it where the window is now
            WM_MOVE if !desk() => { ui::frost_moved(); 0 }
            WM_SIZE => {
                let list = ST.with_borrow(|st| st.list);
                let (cw, ch) = ((lp & 0xFFFF) as i32, ((lp >> 16) & 0xFFFF) as i32);
                if desk() {
                    place_desk_list(list);
                    desk::size_workerw(cw, ch);
                    desk::hosts_sync(h); // a host follows the desktop's size even before WM_DISPLAYCHANGE arrives
                } else {
                    ui::layout_children(cw, ch);
                    InvalidateRect(h, ptr::null(), 0);
                }
                0
            }
            WM_PAINT if !desk() => { ui::paint_status(h); 0 }
            ui::WM_UI => {
                match wp {
                    1 => { let n = ST.with_borrow(|st| st.tabs.len()); if (lp as usize) < n { tab_select(h, lp as usize); } }
                    2 => tab_close_at(h, lp as usize),
                    3 => tab_new(h),
                    4 => nav(h, true),
                    5 => nav(h, false),
                    6 => { if let Some(p) = cur_dir().parent() { load(h, p.to_path_buf()) } }
                    7 => reload(h),
                    8 => toggle_view(h),
                    9 => search_focus(),
                    10..=14 => {
                        let (_, dir, paths) = selected();
                        match wp {
                            10 | 11 if !paths.is_empty() && !in_virtual() => clip_selected(h, wp == 10),
                            12 if !in_virtual() => { paste(h, &dir); reload(h); }
                            13 if !in_zip() && !in_virtual() => rename_selected(),
                            14 if !paths.is_empty() && !in_zip() && !in_virtual() => { sh::delete(h, &paths); } // WM_DELETED reloads
                            _ => {}
                        }
                    }
                    20 => { if let Some(p) = ui::nav_path(lp as usize) { load(h, p); } } // a folder in the left pane
                    25 => { if ui::nav_path(lp as usize).is_some_and(|p| is_trash(&p)) { nav_bin_menu(h); } }
                    21 => { if let Some(p) = ui::crumb_path(lp as usize) { load(h, p); } } // a part of the path in the address pill
                    // the window buttons at the right end of the title row
                    22 => { PostMessageW(h, WM_SYSCOMMAND, SC_MINIMIZE as usize, 0); }
                    23 => { let cmd = if IsZoomed(h) != 0 { SC_RESTORE } else { SC_MAXIMIZE }; PostMessageW(h, WM_SYSCOMMAND, cmd as usize, 0); }
                    24 => { PostMessageW(h, WM_SYSCOMMAND, SC_CLOSE as usize, 0); }
                    // "open in a new tab": lp = Box<PathBuf> (left pane, network shares, Quick access)
                    25 => { let p = *Box::from_raw(lp as *mut PathBuf); tab_open(h, p); }
                    _ => {}
                }
                0
            }
            // the native caption is gone: only the resize frame stays around the client area, so the title row (tabs,
            // window buttons) is ordinary client content painted by the chrome window
            WM_NCCALCSIZE if !desk() => {
                let bx = GetSystemMetrics(SM_CXSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                let by = GetSystemMetrics(SM_CYSIZEFRAME) + GetSystemMetrics(SM_CXPADDEDBORDER);
                if wp == 0 {
                    let r = &mut *(lp as *mut RECT);
                    r.left += bx; r.right -= bx; r.top += by; r.bottom -= by;
                } else {
                    let p = &mut *(lp as *mut NCCALCSIZE_PARAMS);
                    p.rgrc[0].left += bx; p.rgrc[0].right -= bx;
                    p.rgrc[0].top += by; p.rgrc[0].bottom -= by;
                }
                0
            }
            // inside our client area there is no caption any more (DefWindowProc would still call the top strip one);
            // the frame ring is left to DefWindowProc, so the native resize borders keep working
            WM_NCHITTEST if !desk() => {
                let mut p = POINT { x: (lp & 0xFFFF) as i16 as i32, y: ((lp >> 16) & 0xFFFF) as i16 as i32 };
                ScreenToClient(h, &mut p);
                let mut cr: RECT = std::mem::zeroed();
                GetClientRect(h, &mut cr);
                if p.x >= 0 && p.y >= 0 && p.x < cr.right && p.y < cr.bottom {
                    if ui::caption_at(p.x, p.y) { return HTCAPTION as LRESULT; }
                    return HTCLIENT as LRESULT;
                }
                DefWindowProcW(h, m, wp, lp)
            }
            WM_ERASEBKGND if desk() && desk::erase(wp as _, h) => 1,
            WM_WINDOWPOSCHANGING if desk() => { // the desktop never rises above other windows
                let wp = &mut *(lp as *mut WINDOWPOS);
                wp.hwndInsertAfter = HWND_BOTTOM;
                wp.flags |= SWP_NOACTIVATE;
                0
            }
            WM_INITMENUPOPUP if sh::menu_msg(m, wp, lp) => 0,
            WM_DRAWITEM | WM_MEASUREITEM if sh::menu_msg(m, wp, lp) => 1,
            WM_NOTIFY => {
                let nm = &*(lp as *const NMHDR);
                let list = ST.with_borrow(|st| st.list);
                match nm.code {
                    NM_CUSTOMDRAW if nm.hwndFrom == list => return if desk() { ui::desk_draw(lp) } else { ui::list_draw(lp) }, // painted by hand
                    LVN_ITEMCHANGED | LVN_ODSTATECHANGED => ui::status_dirty(h), // selection count in the status line
                    // typing a letter jumps to the next row that starts with it (#81): a virtual list asks us to search
                    LVN_ODFINDITEMW if nm.hwndFrom == list => {
                        let f = &*(lp as *const NMLVFINDITEMW);
                        if f.lvfi.flags & (LVFI_STRING | LVFI_PARTIAL) == 0 || f.lvfi.psz.is_null() { return -1; }
                        let mut n = 0;
                        while *f.lvfi.psz.add(n) != 0 { n += 1; }
                        let q = String::from_utf16_lossy(std::slice::from_raw_parts(f.lvfi.psz, n)).to_lowercase();
                        return ST.with_borrow(|st| {
                            let len = st.rows.len();
                            let start = usize::try_from(f.iStart).unwrap_or(0);
                            (0..len).map(|k| (start + k) % len).find(|&i| !st.rows[i].hdr && st.rows[i].name.to_lowercase().starts_with(&q)).map_or(-1, |i| i as isize)
                        });
                    }
                    LVN_GETDISPINFOW => {
                        let it = &mut (*(lp as *mut NMLVDISPINFOW)).item;
                        // a request that arrives while ST is borrowed is skipped; the list repaints later
                        ST.with(|c| {
                            let Ok(mut st) = c.try_borrow_mut() else { return };
                            let State { dir, rows, thumbs, .. } = &mut *st;
                            let Some(i) = usize::try_from(it.iItem).ok() else { return };
                            let Some(r) = rows.get_mut(i) else { return };
                            if it.mask & LVIF_TEXT != 0 && it.cchTextMax > 0 {
                                let s = if r.hdr { if it.iSubItem == 0 { r.name[1..].to_string() } else { String::new() } } else { match it.iSubItem { 0 => shown_name(&r.name, desk()), 1 => date_text(r.time), 2 => row_type(r), _ => row_size(r) } };
                                let u: Vec<u16> = s.encode_utf16().take(it.cchTextMax as usize - 1).collect();
                                ptr::copy_nonoverlapping(u.as_ptr(), it.pszText, u.len());
                                *it.pszText.add(u.len()) = 0;
                            }
                            if r.hdr { if it.mask & LVIF_IMAGE != 0 { it.iImage = I_IMAGENONE; } return; }
                            if it.mask & LVIF_IMAGE != 0 && *thumbs {
                                if r.thumb == -2 {
                                    r.thumb = -1;
                                    if let Some(tx) = TX.get() { let _ = tx.send((GEN.load(std::sync::atomic::Ordering::Relaxed), i, r.full(dir))); }
                                }
                                it.iImage = if r.thumb >= 0 { r.thumb } else { I_IMAGENONE };
                            } else if it.mask & LVIF_IMAGE != 0 {
                                if r.icon < 0 {
                                    let p = r.full(dir);
                                    // the desktop's list holds our own copies: render the icon at its size, not at 16 px
                                    r.icon = if desk() { desk_icon(&p, r.dir) } else { icon_of(&p, r.dir) };
                                }
                                it.iImage = r.icon;
                            }
                        });
                    }
                    LVN_ITEMACTIVATE => {
                        let i = (*(lp as *const NMITEMACTIVATE)).iItem;
                        let hit = ST.with_borrow(|st| usize::try_from(i).ok().and_then(|i| st.rows.get(i)).filter(|r| !r.hdr).map(|r| (r.full(&st.dir), r.dir)));
                        if let Some((p, is_dir)) = hit { activate(h, p, is_dir); }
                    }
                    NM_RCLICK if nm.hwndFrom == list => {
                        let (_, dir, paths) = selected();
                        if !paths.is_empty() { item_menu(h, &dir, &paths); } else { bg_menu(h); }
                    }
                    LVN_COLUMNCLICK => {
                        let c = (*(lp as *const NMLISTVIEW)).iSubItem as usize;
                        let asc = if SORT_COL.load(Relaxed) == c { !SORT_ASC.load(Relaxed) } else { true };
                        SORT_COL.store(c, Relaxed);
                        SORT_ASC.store(asc, Relaxed);
                        let pc = in_pc(); // drives have no date: no date headings in the "This PC" list
                        let n = ST.with_borrow_mut(|st| {
                            st.rows.retain(|r| !r.hdr);
                            sort_rows(&mut st.rows, c, asc);
                            if !pc { add_headers(&mut st.rows); }
                            st.rows.iter_mut().for_each(|r| r.thumb = -2); // rows moved: thumbnails are asked for again
                            st.rows.len()
                        });
                        reset_thumbs();
                        put(list, LVM_REDRAWITEMS, 0, n as isize - 1);
                    }
                    LVN_BEGINDRAG => {
                        if in_virtual() { return 0; } // a drive or a bin row is not dragged around
                        let (_, _, paths) = selected();
                        let paths = match real_paths(&paths) { Ok(p) => p, Err(e) => { warn(h, &e); return 0; } }; // out of a zip: real files first
                        drag_log(&format!("BEGINDRAG desk={} selected={}", desk(), paths.len()));
                        if !paths.is_empty() {
                            if desk() { let p = (*(lp as *const NMLISTVIEW)).ptAction; desk::drag_start((p.x, p.y)); }
                            let r = sh::drag(h, &paths);
                            drag_log(&format!("drag finished: {r:?}"));
                            // dropped on the desktop itself: the icons were moved (or went into a folder icon), no reload needed
                            let handled = desk() && desk::drag_end(h, list);
                            drag_log(&format!("moved on the desktop: {handled}"));
                            if !handled { reload(h); }
                        }
                    }
                    LVN_KEYDOWN => {
                        let vk = (*(lp as *const NMLVKEYDOWN)).wVKey as u32;
                        let ctrl = GetKeyState(VK_CONTROL as i32) < 0;
                        if desk() && matches!(vk, 0x08 | 0x46 | 0x4D | 0x54 | 0x57 | 0x09 | 0x4C | 0x73) { return 0; } // browsing keys make no sense on the desktop
                        let (list, dir, paths) = selected();
                        match vk {
                            0x08 => if let Some(p) = dir.parent() { load(h, p.to_path_buf()) },
                            0x1B if jobs::busy() => { jobs::cancel(); return 0; } // Esc stops packing / unpacking
                            0x2E if !paths.is_empty() && !in_zip() && !in_virtual() => { sh::delete(h, &paths); }
                            // Space: a quick look. Pictures open in photoview, videos / text / anything else in its default app
                            0x20 if !desk() => if let Some(p) = paths.first().filter(|p| p.is_file()) { activate(h, p.clone(), false); },
                            0x71 if !in_zip() && !in_virtual() => rename_selected(),
                            0x46 if ctrl => { search_focus(); return 0; } // Ctrl+F: address bar in search mode
                            0x73 | 0x4C if vk == 0x73 || ctrl => { // F4 / Ctrl+L: address bar
                                let edit = ST.with_borrow(|st| st.edit);
                                SetFocus(edit);
                                put(edit, EM_SETSEL, 0, -1);
                                return 0;
                            }
                            0x5D if !paths.is_empty() => item_menu(h, &dir, &paths),
                            0x43 | 0x58 if ctrl && !paths.is_empty() => { clip_selected(h, vk == 0x58); return 0; }
                            0x41 if ctrl => { // Ctrl+A: select all (state mask LVIS_SELECTED, item -1 = every row)
                                let mut it: LVITEMW = std::mem::zeroed();
                                it.stateMask = LVIS_SELECTED;
                                it.state = LVIS_SELECTED;
                                put(list, LVM_SETITEMSTATE, usize::MAX, &it as *const _ as isize);
                                return 0;
                            }
                            0x56 if ctrl && !in_virtual() => paste(h, &dir),
                            0x4E if ctrl && !in_virtual() => new_entry(&dir, t!("Новая папка", "New folder", ja: "新しいフォルダー", pt: "Nova pasta", zh: "新建文件夹"), true),
                            0x48 if ctrl => { SHOW_HIDDEN.fetch_xor(true, Relaxed); } // Ctrl+H: hidden files
                            0x74 => { ui::net_rescan(); ui::nav_refresh(ui::nav_window()); } // F5: the pane reads the drives and the network again
                            0x4D if ctrl => { toggle_view(h); return 0; } // Ctrl+M: details <-> thumbnails
                            0x54 if ctrl => tab_new(h),
                            0x57 if ctrl => { tab_close(h); return 0; }
                            0x09 if ctrl => {
                                let next = ST.with_borrow(|st| (st.active + 1) % st.tabs.len());
                                tab_select(h, next);
                                return 0;
                            }
                            _ => return 0,
                        }
                        reload(h);
                    }
                    LVN_BEGINLABELEDITW => {
                        let hdr = ST.with_borrow(|st| usize::try_from((*(lp as *const NMLVDISPINFOW)).item.iItem).ok().and_then(|i| st.rows.get(i)).is_some_and(|r| r.hdr));
                        if hdr || in_virtual() { return 1; } // a heading or a drive cannot be renamed
                        let e = SendMessageW(ST.with_borrow(|st| st.list), LVM_GETEDITCONTROL, 0, 0) as HWND;
                        if e != 0 as HWND { LABEL_OLD.store(SetWindowLongPtrW(e, GWLP_WNDPROC, label_proc as *const () as isize), std::sync::atomic::Ordering::Relaxed); }
                    }
                    LVN_ENDLABELEDITW => {
                        let it = &(*(lp as *const NMLVDISPINFOW)).item;
                        if !it.pszText.is_null() {
                            let new = wstr(it.pszText);
                            // reject separators so a rename can never move the item
                            if !new.is_empty() && !new.contains(['\\', '/', ':']) {
                                ST.with_borrow(|st| if let Some(r) = usize::try_from(it.iItem).ok().and_then(|i| st.rows.get(i)) { let _ = fs::rename(r.full(&st.dir), st.dir.join(&new)); });
                                PostMessageW(h, WM_APP, 0, 0); // reload after the edit control is gone
                            }
                        }
                    }
                    _ => {}
                }
                0
            }
            WM_SETFOCUS => { SetFocus(ST.with_borrow(|st| st.list)); 0 }
            WM_APP => { reload(h); 0 }
            shellwin::WM_SHELL_SELECT => { shell_select(h as isize, *Box::from_raw(lp as *mut PathBuf)); 0 }
            WM_JOB => {
                let err = *Box::from_raw(wp as *mut Option<String>);
                reload(h);
                ui::status_dirty(h);
                if let Some(e) = err { MessageBoxW(h, w(&e).as_ptr(), w("WinRust Explorer").as_ptr(), MB_ICONWARNING); }
                0
            }
            // a shell file operation (delete / copy / move) ended on a worker thread
            WM_DELETED => {
                let sh::FileOpDone { err, retry } = *Box::from_raw(lp as *mut sh::FileOpDone);
                reload(h); // the deleted / moved rows are gone
                ui::status_dirty(h);
                if retry.is_empty() {
                    if let Some(e) = err { ui::notice(h, e); } // the worker also wrote it to %TEMP%\winrust-delete.log
                } else {
                    // the Recycle Bin refused it (an elevated shell cannot write the user's bin): ask here, on the UI
                    // thread; the worker never opens a dialog of its own
                    let msg = tf!(
                        "Не удалось переместить в корзину.\n\n{}\n\nУдалить навсегда?",
                        "Could not move to the Recycle Bin.\n\n{}\n\nDelete permanently?",
                        ja: "ごみ箱に移動できませんでした。\n\n{}\n\n完全に削除しますか?",
                        pt: "Não foi possível mover para a Lixeira.\n\n{}\n\nExcluir permanentemente?",
                        zh: "无法移到回收站。\n\n{}\n\n永久删除？",
                        err.unwrap_or_default()
                    );
                    let r = MessageBoxW(h, w(&msg).as_ptr(), w("WinRust").as_ptr(), MB_YESNO | MB_ICONWARNING);
                    if r == IDYES { sh::delete_permanent(h, retry); }
                }
                0
            }
            // the Recycle Bin restore / empty / delete ended on a worker thread
            WM_TRASH_DONE => {
                let TrashDone { what, ok, bad, why } = *Box::from_raw(lp as *mut TrashDone);
                reload(h); // what was restored / deleted is gone from the bin; the message must not be wiped by that load
                let mut msg = match (what, bad > 0) {
                    (TrashWhat::Emptied, true) => tf!("корзина очищена: {}, ошибок: {}", "emptied: {}, failed: {}", ja: "削除: {} 件、失敗: {} 件", pt: "esvaziados: {}, falhas: {}", zh: "已清空 {} 项，失败 {} 项", ok, bad),
                    (TrashWhat::Emptied, false) => tf!("корзина очищена: {}", "emptied: {}", ja: "削除: {} 件", pt: "esvaziados: {}", zh: "已清空 {} 项", ok),
                    (TrashWhat::Deleted, true) => tf!("удалено безвозвратно: {}, ошибок: {}", "deleted: {}, failed: {}", ja: "完全に削除: {} 件、失敗: {} 件", pt: "excluídos: {}, falhas: {}", zh: "已永久删除 {} 项，失败 {} 项", ok, bad),
                    (TrashWhat::Deleted, false) => tf!("удалено безвозвратно: {}", "deleted: {}", ja: "完全に削除: {} 件", pt: "excluídos: {}", zh: "已永久删除 {} 项", ok),
                    (TrashWhat::Restored, true) => tf!("восстановлено: {}, не удалось: {}", "restored: {}, failed: {}", ja: "復元: {} 件、失敗: {} 件", pt: "restaurados: {}, falhas: {}", zh: "已还原 {} 项，失败 {} 项", ok, bad),
                    (TrashWhat::Restored, false) => tf!("восстановлено: {}", "restored: {}", ja: "復元: {} 件", pt: "restaurados: {}", zh: "已还原 {} 项", ok),
                };
                if let Some(w) = why {
                    msg = format!("{msg} — {w}"); // the failure reason inline: the status line does not show a second line
                }
                ui::notice(h, msg);
                0
            }
            // a USB stick came or went, a volume changed: the pane lists the drives again, the drive list follows
            WM_DEVICECHANGE if !desk() => {
                ui::nav_refresh(ui::nav_window());
                if is_this_pc(&cur_dir()) { reload(h); }
                0
            }
            // the drive scan finished on its worker: rebuild the pane from the cache, and reload a view that lists drives
            ui::WM_DRIVES => {
                ui::nav_rebuild(ui::nav_window());
                if is_this_pc(&cur_dir()) || is_trash(&cur_dir()) {
                    reload(h);
                }
                0
            }
            // the network scan finished (on a worker thread): the pane lists what it found
            ui::WM_NET => {
                ui::net_take(lp);
                ui::nav_refresh(ui::nav_window());
                0
            }
            // the Recycle Bin scan finished (on a worker thread)
            WM_TRASH => {
                let rows = *Box::from_raw(wp as *mut Vec<Row>);
                if is_trash(&cur_dir()) {
                    let (list, n) = ST.with_borrow_mut(|st| { st.rows = rows; (st.list, st.rows.len()) });
                    reset_thumbs();
                    put(list, LVM_DELETEALLITEMS, 0, 0);
                    put(list, LVM_SETITEMCOUNT, n, 0);
                    ui::status_dirty(h);
                }
                0
            }
            WM_NAV => {
                match wp {
                    1 => nav(h, true),
                    2 => nav(h, false),
                    3 => { if !is_trash(&cur_dir()) { let (_, dir, sel) = selected(); properties(&sel.first().cloned().unwrap_or(dir)); } }
                    _ => if let Some(p) = cur_dir().parent() { load(h, p.to_path_buf()) },
                }
                0
            }
            WM_THUMB => {
                let (era, row, bmp) = *Box::from_raw(wp as *mut (u32, usize, isize));
                let (list, himl) = ST.with_borrow(|st| (st.list, st.himl));
                if era == GEN.load(std::sync::atomic::Ordering::Relaxed) {
                    let idx = ImageList_Add(himl as _, bmp as _, ptr::null_mut());
                    let ok = idx >= 0 && ST.with_borrow_mut(|st| st.rows.get_mut(row).map(|r| r.thumb = idx).is_some());
                    if ok { put(list, LVM_REDRAWITEMS, row, row as isize); }
                }
                windows_sys::Win32::Graphics::Gdi::DeleteObject(bmp as _);
                0
            }
            WM_GO => {
                let (edit, list) = ST.with_borrow(|st| (st.edit, st.list));
                if wp == 13 {
                    let mut buf = vec![0u16; GetWindowTextLengthW(edit) as usize + 1];
                    let n = GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32) as usize;
                    let p = PathBuf::from(String::from_utf16_lossy(&buf[..n]).trim().trim_matches('"'));
                    let text = String::from_utf16_lossy(&buf[..n]);
                    if let Some(q) = text.trim().strip_prefix('?') {
                        if !q.trim().is_empty() { search(h, q.trim()); SetFocus(list); return 0; } // keep the query in the bar
                    } else if is_this_pc(&p) || text.trim().eq_ignore_ascii_case(&display_dir(&this_pc())) {
                        load(h, this_pc()); // "This PC" typed (or the marker): the drive list
                    } else if is_trash(&p) || text.trim().eq_ignore_ascii_case(&display_dir(&trash_dir())) {
                        load(h, trash_dir()); // "Recycle Bin" typed (or the marker)
                    } else if p.is_dir() || zipx::split_zip(&p).is_some() { load(h, p); } // also a path inside a .zip
                }
                SetWindowTextW(edit, w(&display_dir(&cur_dir())).as_ptr()); // also undoes a bad path / Esc
                SetFocus(list);
                0
            }
            // a click on the desktop itself, not on the icon list (which has its own NM_RCLICK menu): icons hidden, or
            // a secondary monitor, where the list does not reach (#125)
            WM_CONTEXTMENU if desk() && (icons_hidden() || wp as HWND == h) => { bg_menu(h); 0 }
            desk::SPAWN_WORKERW if desk() => { desk::spawn_workerw(h); 0 }
            WM_TIMER if desk() && wp == 7 => {
                let list = ST.with_borrow(|st| st.list);
                let native = desk::workerw_live(); // WE's own hook first: then no pop-outs on top of it
                we::tick(h, list, &desk::hosts(), native);
                desk::check_live(list);
                0
            }
            // the Desktop changed: coalesce the burst a file operation produces, then load the icons again
            WM_DESK_CHANGED if desk() => {
                SetTimer(h, 9, 250, None);
                0
            }
            WM_TIMER if desk() && wp == 9 => {
                KillTimer(h, 9);
                reload(h);
                0
            }
            // logoff / restart: WE windows are children of ours and must not outlive the hosts they were put in
            WM_ENDSESSION if desk() => {
                if wp != 0 {
                    we::shutdown();
                }
                0
            }
            WM_DESTROY => {
                if desk() {
                    we::shutdown();
                } else {
                    save_place(h);
                }
                shellwin::revoke();
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, m, wp, lp),
        }
    }
}

/// Control Panel, Recycle Bin, "This PC" arrive as shell addresses (`::{GUID}`, `shell:...`), not folders: not ours to show (#85, #80, #70).
fn namespace_target() -> Option<String> {
    env::args().skip(1).find_map(|a| {
        let t = a.trim_matches('"').rsplit(',').next().unwrap_or("").to_string(); // `/e,::{...}`
        let l = t.to_lowercase();
        (l.starts_with("::{") || l.starts_with("shell:")).then_some(t)
    })
}

fn main() {
    winrust_common::appid::set("WinRust"); // group every WinRust process under one Task Manager entry
    if let Some(t) = namespace_target() {
        // control.exe opens its own window; explorer.exe given a namespace address opens it directly (no folder verb involved)
        // Only the "open" verb of folders is ours: control.exe and explorer.exe <address> both come back here through it
        // (an endless control.exe <-> rust-explorer loop, flickering cursor). The "explore" verb still has Explorer's own
        // handler, so it shows the panel / Recycle Bin in a real Explorer window. The hop mark stops any loop that remains.
        if env::var_os("RX_NS_HOP").is_none() {
            let addr = if t.starts_with("::") { format!("shell:{t}") } else { t };
            unsafe { env::set_var("RX_NS_HOP", "1") }; // inherited by whatever the shell starts from here
            let (f, v) = (w(&addr), w("explore"));
            let mut sei: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
            sei.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
            sei.fMask = 0x0000_0100 | 0x0000_0400; // SEE_MASK_NOASYNC (we exit right after) | SEE_MASK_FLAG_NO_UI
            sei.lpVerb = v.as_ptr();
            sei.lpFile = f.as_ptr();
            sei.nShow = SW_SHOWNORMAL;
            unsafe { ShellExecuteExW(&mut sei) };
        }
        return;
    }
    sh::init();
    unsafe {
        SetProcessDPIAware(); // sharp text at any scaling; every size below goes through ui::sc
        ui::init_scale();
        ui::gdiplus_start();
        ui::dark_mode();
        InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES });
        let inst = GetModuleHandleW(ptr::null());
        let cls = w("RustExplorer");
        let desktop = env::args().nth(1).as_deref() == Some("--desktop");
        // One desktop only. An rs-shell built before the "Program Manager" title change still looks for a window titled
        // "rs-desktop"; it sees this one as gone and keeps starting `--desktop`. Those extra processes must not create a
        // second Progman: the marker is already up, so they leave. (The new rs-shell finds the marker itself.)
        if desktop && winrust_common::desktop::is_up() {
            return;
        }
        // desktop without a wallpaper file shows black, not window white
        // desktop: COLOR_BACKGROUND, the user's solid colour when there is no picture, never black (#108, #111)
        let bg = if desktop { windows_sys::Win32::Graphics::Gdi::GetSysColorBrush(1) as _ } else { windows_sys::Win32::Graphics::Gdi::CreateSolidBrush(ui::pal().bg) };
        RegisterClassW(&WNDCLASSW { style: CS_DBLCLKS, lpfnWndProc: Some(ui::proc), hInstance: inst, lpszClassName: w("RxChrome").as_ptr(), hCursor: LoadCursorW(0 as _, IDC_ARROW), ..std::mem::zeroed() });
        RegisterClassW(&WNDCLASSW { style: CS_DBLCLKS, lpfnWndProc: Some(ui::nav_proc), hInstance: inst, lpszClassName: w("RxNav").as_ptr(), hCursor: LoadCursorW(0 as _, IDC_ARROW), ..std::mem::zeroed() });
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: cls.as_ptr(), hCursor: LoadCursorW(0 as _, IDC_ARROW), hIcon: LoadIconW(inst, 1 as _), hbrBackground: bg, ..std::mem::zeroed() });
        // the marquee is needed by the desktop *and* by the file list: register it in both modes
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(marquee::proc), hInstance: inst, lpszClassName: w("WinRustBand").as_ptr(), ..std::mem::zeroed() });
        if desktop {
            DESKTOP.store(true, Relaxed);
            // the whole screen: it paints the wallpaper too; the icon list sits above the bar (desk_top / desk_bottom)
            // class "Progman" and title "Program Manager" like explorer's desktop: that is what Wallpaper Engine looks
            // for with FindWindowW(L"Progman", L"Program Manager"). Our own programs find it by the property below, so
            // the canonical title does not make it indistinguishable from Explorer's.
            let pm = w("Progman");
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: pm.as_ptr(), hCursor: LoadCursorW(0 as _, IDC_ARROW), hIcon: LoadIconW(inst, 1 as _), hbrBackground: bg, ..std::mem::zeroed() });
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(desk::workerw_proc), hInstance: inst, lpszClassName: w("WorkerW").as_ptr(), ..std::mem::zeroed() });
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), hInstance: inst, lpszClassName: w("SHELLDLL_DefView").as_ptr(), ..std::mem::zeroed() });
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(desk::host_proc), hInstance: inst, lpszClassName: w(we::HOST_CLASS).as_ptr(), ..std::mem::zeroed() });
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(desk::catcher_proc), hInstance: inst, lpszClassName: w("WinRustCatcher").as_ptr(), ..std::mem::zeroed() });
            let (vx, vy, vw, vh) = virt(); // every monitor: the wallpaper reaches the secondary ones too (#67)
            let d = CreateWindowExW(WS_EX_TOOLWINDOW, pm.as_ptr(), w("Program Manager").as_ptr(), WS_POPUP | WS_CLIPCHILDREN, vx, vy, vw, vh, 0 as _, 0 as _, inst, ptr::null());
            if !d.is_null() && SetPropW(d, w(winrust_common::desktop::MARKER).as_ptr(), 1 as *mut _) == 0 {
                we::log("could not mark the desktop window: rs-shell will not see it");
            }
            SetWindowPos(d, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        } else {
            // where it was last closed, maximized again if it was (#71); a first start opens on the monitor of the cursor
            let (cw, ch2) = (ui::sc(1000), ui::sc(660));
            let (fx, fy, fw, fh) = winrust_common::place::centered(cw, ch2);
            let win = CreateWindowExW(0, cls.as_ptr(), ptr::null(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN, fx, fy, fw, fh, 0 as _, 0 as _, inst, ptr::null());
            match saved_place() {
                // the same API that saved it: rcNormalPosition is in work-area coordinates, which CreateWindow does not use
                Some((max, r)) => {
                    let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
                    wp.length = size_of::<WINDOWPLACEMENT>() as u32;
                    wp.showCmd = if max { SW_SHOWMAXIMIZED } else { SW_SHOWNORMAL } as u32;
                    wp.rcNormalPosition = r;
                    SetWindowPlacement(win, &wp);
                }
                None => { ShowWindow(win, SW_SHOWNORMAL); }
            }
            // 87 % opaque by default (look.txt `window-opacity`): the whole window fades, children included
            winrust_common::winfx::translucent(win, winrust_common::look::window_opacity());
        }
        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
            // Alt+Left / Right / Up / Enter arrive as WM_SYSKEYDOWN at the list, which never reports them: handle them here
            if msg.message == WM_SYSKEYDOWN && !desk() {
                let act = match msg.wParam { 0x25 => 1, 0x27 => 2, 0x0D => 3, 0x26 => 4, _ => 0 };
                if act != 0 { PostMessageW(GetAncestor(msg.hwnd, GA_ROOT), WM_NAV, act, 0); continue; }
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}


#[cfg(test)]
mod tests {
    #[test]
    fn window_place_parses_and_rejects_junk() {
        let (max, r) = super::parse_place("max 10 20 1010 680").unwrap();
        assert!(max);
        assert_eq!((r.left, r.top, r.right, r.bottom), (10, 20, 1010, 680));
        assert!(!super::parse_place("normal -1900 5 -900 700").unwrap().0); // a monitor left of the primary
        assert!(super::parse_place("max 0 0 50 50").is_none()); // collapsed
        assert!(super::parse_place("max 0 0 800").is_none());
        assert!(super::parse_place("big 0 0 800 600").is_none());
        assert!(super::parse_place("").is_none());
    }

    #[test]
    fn a_zip_opens_like_a_folder_and_gives_real_files() {
        let d = std::env::temp_dir().join(format!("rx-zipview-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("f.txt"), "hello").unwrap();
        let z = d.join("a.zip");
        let mut w = zipx::Writer::new(fs::File::create(&z).unwrap());
        w.add_prepared(zipx::prepare("inner/f.txt".into(), &d.join("f.txt")).unwrap()).unwrap();
        w.add_prepared(zipx::prepare("top.txt".into(), &d.join("f.txt")).unwrap()).unwrap();
        w.finish().unwrap();
        let names = |dir: &PathBuf| rows_of(dir).unwrap().into_iter().map(|r| format!("{}{}", r.name, if r.dir { "/" } else { "" })).collect::<Vec<_>>();
        assert_eq!(names(&z), ["inner/", "top.txt"]); // folders first
        assert_eq!(names(&z.join("inner")), ["f.txt"]);
        assert!(rows_of(&d.join("nothing.zip")).is_none());
        let paths = [z.join("inner").join("f.txt"), z.join("top.txt")];
        let real = real_paths(&paths).unwrap();
        assert!(real.iter().all(|p| p.is_file() && fs::read_to_string(p).unwrap() == "hello"), "{real:?}");
        assert_eq!(real_paths(&[d.join("f.txt")]).unwrap(), [d.join("f.txt")], "ordinary files stay as they are");
        assert!(zip_names(&[z.join("top.txt"), d.join("f.txt")]).is_none(), "mixed selections are not archive selections");
        let _ = fs::remove_dir_all(&d);
        let _ = fs::remove_dir_all(real[0].parent().unwrap());
    }

    /// By hand (needs the real shell): `cargo test -p rust-explorer system_zip -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn system_zip_is_not_in_the_menu() {
        sh::init();
        let f = std::env::temp_dir().join("rx-menu-dump.txt");
        std::fs::write(&f, "x").unwrap();
        let menu = sh::menu_dump(&[f.clone()]);
        let _ = std::fs::remove_file(f);
        println!("{}", menu.join("
"));
        assert!(menu.iter().any(|l| l.contains("Se&nd to") || l.contains("Send to") || l.contains("&Delete") || l.contains("Delete")), "the menu is there");
        if let Some(n) = sh::system_zip_name() { assert!(!menu.iter().any(|l| l.trim().starts_with(&n)), "{n} should be gone"); }
    }

    use super::*;

    fn r(name: &str, dir: bool, size: u64, time: u64) -> Row { Row { name: name.into(), dir, size, time, icon: -1, thumb: -2 , hdr: false, path: None, cap: None, trash: None } }
    fn names(v: &[Row]) -> Vec<&str> { v.iter().map(|r| r.name.as_str()).collect() }

    #[test]
    fn desktop_rows_show_file_names_without_shortcut_extensions() {
        assert_eq!(shown_name(r"C:\Users\Public\Desktop\Steam.lnk", true), "Steam");
        assert_eq!(shown_name("Notes.txt", true), "Notes.txt");
        assert_eq!(shown_name(r"C:\x\Game.URL", true), "Game");
        assert_eq!(shown_name(r"C:\x\Steam.lnk", false), r"C:\x\Steam.lnk");
    }

    #[test]
    fn folders_first_then_by_the_chosen_column() {
        let mut v = vec![r("b.txt", false, 5, 1), r("A", true, 0, 2), r("a.txt", false, 9, 3)];
        sort_rows(&mut v, 0, true);
        assert_eq!(names(&v), ["A", "a.txt", "b.txt"]);
        sort_rows(&mut v, 3, false); // biggest first, the folder still on top
        assert_eq!(names(&v), ["A", "a.txt", "b.txt"]);
        sort_rows(&mut v, 1, true); // oldest first
        assert_eq!(names(&v), ["A", "b.txt", "a.txt"]);
    }

    #[test]
    fn search_hit_paths_sort_by_file_name() {
        let mut v = vec![r("C:\\z\\b.txt", false, 1, 1), r("D:\\a\\a.txt", false, 1, 1)];
        sort_rows(&mut v, 0, true);
        assert_eq!(names(&v), ["D:\\a\\a.txt", "C:\\z\\b.txt"]);
    }

    #[test]
    fn history_goes_back_and_forward_and_a_new_visit_drops_the_forward_trail() {
        let mut h = Hist::default();
        for p in ["a", "b", "c"] { h.visit(p.into()); }
        assert_eq!(h.back(), Some("b".into()));
        assert_eq!(h.back(), Some("a".into()));
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some("b".into()));
        h.visit("x".into()); // from b: c is no longer ahead
        assert_eq!(h.forward(), None);
        assert_eq!(h.back(), Some("b".into()));
        h.visit("x".into()); h.visit("x".into()); // same folder twice is one step
        assert_eq!(h.v.len(), 3);
    }

    /// The `$I` path length counts the terminating NUL; keeping it in the PathBuf made every restore fail inside Win32.
    #[test]
    fn trash_i_path_has_no_nul() {
        let path = "C:\\Users\\x\\a.txt";
        let units: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let mut d = vec![];
        d.extend_from_slice(&2u64.to_le_bytes());
        d.extend_from_slice(&1u64.to_le_bytes());
        d.extend_from_slice(&0u64.to_le_bytes());
        d.extend_from_slice(&(units.len() as u32).to_le_bytes());
        for u in &units {
            d.extend_from_slice(&u.to_le_bytes());
        }
        let dir = std::env::temp_dir().join(format!("winrust-trashi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("$Itest.txt");
        std::fs::write(&f, &d).unwrap();
        let (_, _, p) = parse_trash_i(&f).unwrap();
        assert_eq!(p, PathBuf::from(path));
        assert!(!p.to_string_lossy().contains('\0'));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Recycle Bin restore renames through a watchdog: it must still be a plain rename when nothing is wrong.
    #[test]
    fn rename_timeout_moves_and_reports_errors() {
        let d = std::env::temp_dir().join(format!("winrust-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (a, b) = (d.join("a.txt"), d.join("b.txt"));
        std::fs::write(&a, b"x").unwrap();
        rename_timeout(&a, &b).unwrap();
        assert!(b.exists() && !a.exists());
        assert!(rename_timeout(&a, &b).is_err(), "the source is gone");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn type_and_size_read_like_explorers() {
        // the tests run in Russian (lang!() in a test build), like Explorer's own Type column
        assert_eq!(type_text("x", true), "Папка с файлами");
        assert_eq!(type_text("a.txt", false), "Файл TXT");
        assert_eq!(type_text("C:\\d\\b.tar.gz", false), "Файл GZ");
        assert_eq!(type_text(".gitignore", false), "Файл"); // a dot file has no extension
        assert_eq!(type_text("Makefile", false), "Файл");
        assert_eq!(size_text(1), "1 KB");
        assert_eq!(size_text(2048), "2 KB");
        assert_eq!(size_text(5 << 20), "5.0 MB");
        assert_eq!(size_text(3 << 30), "3.00 GB");
    }

    #[test]
    fn dates_are_formatted_and_zero_is_blank() {
        assert_eq!(date_text(0), "");
        let t = date_text(132_000_000_000_000_000); // some day in 2019, local time zone
        assert!(t.len() == 16 && t.starts_with("20"), "{t}");
    }
}

#[cfg(test)]
mod search_tests {
    #[test]
    fn local_search_finds_names_below_the_folder() {
        let d = std::env::temp_dir().join(format!("rx-search-test-{}", std::process::id()));
        std::fs::create_dir_all(d.join(r"sub\deep")).unwrap();
        for f in [r"Report.txt", r"sub\notes.md", r"sub\deep\REPORT-2.pdf"] { std::fs::write(d.join(f), "x").unwrap(); }
        let (rows, cut, flipped) = super::local_search(&d, "report");
        let (wrong, _, was_flipped) = super::local_search(&d, "кузщке"); // "report" typed in the Russian layout
        let mut names: Vec<String> = rows.iter().map(|r| std::path::Path::new(&r.name).file_name().unwrap().to_string_lossy().into_owned()).collect();
        names.sort();
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(names, ["REPORT-2.pdf", "Report.txt"]); // any case, any depth, absolute paths
        assert!(!cut && !flipped);
        assert!(was_flipped && wrong.len() == 2);
    }

    #[test]
    fn layout_flip_both_ways() {
        assert_eq!(super::flip_layout("сфкпщ"), "cargo");
        assert_eq!(super::flip_layout("ljrevtyns"), "документы");
        assert_eq!(super::flip_layout("Ыфму"), "Save");
    }
}
