// rust-explorer [path]  — file manager window (Explorer replacement).
// Virtual list (LVS_OWNERDATA, only visible rows are asked for text/icon), system icons, tabs,
// IFileOperation (recycle bin / undo / elevation prompts), real shell context menu with submenus, OLE drag and drop.
// Keys: Enter open, Backspace up, F2 rename, Del recycle, Ctrl+C/X/V copy/cut/paste, Ctrl+N new folder,
// Ctrl+T/W/Tab tabs, Ctrl+M thumbnails, Ctrl+L/F4 address bar, Ctrl+F or "? terms" in the bar = instant search through the rs service, right click / Menu key = shell context menu.
// Look: topbar's palette, painted tabs / toolbar / address pill and status line (src/ui.rs), dark title bar and list.
// Clipboard is the system one (Ctrl+C/X/V interoperate with Explorer). Also: Alt+Left/Right back/forward, Alt+Up up,
// Alt+Enter properties, F5 refresh, Ctrl+H hidden files, click a column header to sort, right click on empty space = folder menu.
// shortcuts: std::fs listing (not FindFirstFileEx large fetch), icons are extension-based (exe/lnk/ico/url/scr
// per file) and synchronous (thumbnails: one background thread, 96px, no disk cache), internal copy/cut clipboard, silent rename errors,
// search blocks the UI while it waits for the service (no async), the service pipe is admin-only so run as admin,
// drops always land in the current folder (not on a folder row), tabs remember only the folder (no selection/scroll).
// upgrade: each of those one at a time; then register as the folder-open handler.
// rust-explorer --desktop  - the desktop for rs-shell: borderless bottom-most color-keyed window with the Desktop folders
//   (user + public) as big icons over the wallpaper it paints itself (without explorer nothing else does): reads the BMP
//   that rs-shell converts into %LOCALAPPDATA%\rs-shell\wallpaper.bmp, "fill" style, scaled once at start.
//   shortcuts: wallpaper is loaded once (changing it needs a restart), icon labels are always white, no background menu /
//   drag-to-position / auto-arrange, items from the public desktop can't be renamed.
#![windows_subsystem = "windows"]
use std::{cell::RefCell, env, fs, os::windows::fs::MetadataExt, path::{Path, PathBuf}, ptr, sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed}};
use windows_sys::Win32::{Foundation::*, Graphics::Gdi::{BLACK_BRUSH, GetStockObject, InvalidateRect}, System::LibraryLoader::GetModuleHandleW, UI::{Controls::*, Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_CONTROL}, Shell::*, WindowsAndMessaging::*}};

mod zipx; // our own ZIP engine (deflate, inflate, container)
mod jobs; // our own copier, ZIP packer, ZIP/7z unpacker (background, progress in the status line)
mod shellwin; // registers the window as a shell window: "Show in folder" from other apps selects the file
mod ui; // the look: palette, painted tabs / toolbar / address pill / status line, dark theme
mod background;
mod recycle;
mod desktop_grid;

static DESKTOP: AtomicBool = AtomicBool::new(false);
fn desk() -> bool { DESKTOP.load(Relaxed) }
const CLR_NONE: isize = 0xFFFF_FFFF; // list background / text background: let the parent's wallpaper show through
thread_local! { static WALL: std::cell::Cell<(isize, i32, i32)> = const { std::cell::Cell::new((0, 0, 0)) }; } // (HBITMAP, w, h)
// The desktop list spans the virtual screen; individual monitor work areas constrain icon placement.
fn desk_top() -> i32 { ui::sc(8) }
fn desk_bottom() -> i32 { ui::sc(8) }

fn desktop_bounds()->winrust_common::screens::Bounds{winrust_common::screens::virtual_bounds()}
unsafe fn resize_desktop(h:HWND){unsafe{
    let b=desktop_bounds();
    SetWindowPos(h,HWND_BOTTOM,b.left,b.top,b.width(),b.height(),SWP_NOACTIVATE);
    let old=WALL.replace((wall::load_desktop(),b.width(),b.height())).0;
    if old!=0{windows_sys::Win32::Graphics::Gdi::DeleteObject(old as _);}
    desk::update_screens();desk::resize_worker();InvalidateRect(h,ptr::null(),1);
}}
const THUMB: i32 = 96;
static THUMB_PX:std::sync::atomic::AtomicI32=std::sync::atomic::AtomicI32::new(THUMB);
fn thumb_px()->i32{THUMB_PX.load(Relaxed)}
const WM_THUMB: u32 = WM_APP + 2; // worker -> UI: wp = Box<(era, row, HBITMAP)>
static GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0); // bumped on every new listing; stale thumbnails are dropped
static TX: std::sync::OnceLock<std::sync::mpsc::Sender<(u32, usize, PathBuf)>> = std::sync::OnceLock::new();
const WM_GO: u32 = WM_APP + 1; // address bar: Enter (wp 13) / Esc (wp 27)
static EDIT_OLD: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static LABEL_OLD: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0); // the rename box (only one exists at a time)

struct Row { name: String, dir: bool, size: u64, time: u64, icon: i32, thumb: i32, recycled: Option<recycle::Entry>, hdr: bool }
impl Row { fn display_name(&self) -> &str { self.recycled.as_ref().map_or(&self.name, |e| &e.name) } }
thread_local! {
    static WORK: RefCell<Option<background::Worker>> = const { RefCell::new(None) };
    static ICONS:RefCell<Option<background::Icons>>=const{RefCell::new(None)};
    static WATCH: RefCell<Option<background::Watcher>> = const { RefCell::new(None) };
    static WATCH_GEN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PENDING_PICK: RefCell<Option<String>> = const { RefCell::new(None) };
    static SEARCH_QUERY: RefCell<Option<String>> = const { RefCell::new(None) };
    static REFRESH_NEEDED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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
const WM_JOB: u32 = WM_APP + 5; // jobs.rs worker finished: wp = Box<jobs::Completion>

fn row(name: String, m: Option<&fs::Metadata>) -> Row {
    Row { name, dir: m.is_some_and(|m| m.is_dir()), size: m.map_or(0, |m| m.len()), time: m.map_or(0, |m| m.last_write_time()), icon: -1, thumb: -2, recycled: None, hdr: false }
}

/// Folders always first; inside each group by column (name, date, type, size), ties by name.
fn sort_rows(rows: &mut [Row], col: usize, asc: bool) { sort_rows_mode(rows, col, asc, group_on()); }
fn sort_rows_mode(rows: &mut [Row], col: usize, asc: bool, grouped: bool) {
    use std::cmp::Reverse;
    fn name(r: &Row) -> String { r.display_name().rsplit('\\').next().unwrap_or("").to_lowercase() }
    fn cached<K: Ord>(rows: &mut [Row], asc: bool, primary: impl Fn(&Row) -> K) {
        rows.sort_by_cached_key(|r| {
            let p = primary(r);
            let direction = if asc { (Some(p), None) } else { (None, Some(Reverse(p))) };
            (!r.dir, direction, name(r))
        });
    }
    // Computing keys inside the comparator allocated strings O(n log n) times
    // on the UI thread. Cache once per row, preserving stable ties and folders first.
    match col {
        1 => cached(rows, asc, |r| r.time),
        2 => cached(rows, asc, |r| r.recycled.as_ref().map_or_else(||type_text(r.display_name(),r.dir),|e|e.original_dir.to_lowercase())),
        3 => cached(rows, asc, |r| r.size),
        _ if asc => rows.sort_by_cached_key(|r| (!r.dir, name(r))),
        _ => rows.sort_by_cached_key(|r| (!r.dir, Reverse(name(r)))),
    }
    if grouped { bucket_refresh(); rows.sort_by_cached_key(|r| bucket(r.time)); }
}

// ---------- grouping by date ("Group by date" in the folder's menu): Today, Yesterday, Earlier this week, ... ----------

const DAY: i64 = 864_000_000_000; // FILETIME ticks per day
const BUCKETS: [&str; 8] = ["Today", "Yesterday", "Earlier this week", "Last week", "Earlier this month", "Last month", "Earlier this year", "A long time ago"];

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
fn add_headers(rows: &mut Vec<Row>) { add_headers_mode(rows, group_on()); }
fn add_headers_mode(rows: &mut Vec<Row>, grouped: bool) {
    if !grouped { return; }
    bucket_refresh();
    let mut out = Vec::with_capacity(rows.len() + 8);
    let mut last = -1;
    for r in rows.drain(..) {
        let b = bucket(r.time);
        if b != last {
            last = b;
            out.push(Row { name: format!("?{}", BUCKETS[b as usize]), dir: false, size: 0, time: 0, icon: -1, thumb: -1, hdr: true, recycled: None });
        }
        out.push(r);
    }
    *rows = out;
}

/// Explorer's "Type" column in short form: "File folder", "TXT file", "File".
fn type_text(name: &str, dir: bool) -> String {
    if dir { return "File folder".into(); }
    let file = name.rsplit('\\').next().unwrap_or(name);
    match file.rsplit_once('.') { Some((stem, e)) if !stem.is_empty() && !e.is_empty() => format!("{} file", e.to_uppercase()), _ => "File".into() }
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
    let fast = jobs::can_fast_copy(&paths, dir) && !(cut && paths.iter().all(|p| jobs::same_volume(p, dir))) && jobs::writable(dir);
    if fast && !jobs::busy() {
        jobs::copy(h as isize, paths, dir.clone(), cut);
    } else {
        let sequence = clipboard_sequence();
        match sh::fileop(h, if cut { sh::Op::Move } else { sh::Op::Copy }, &paths, dir) {
            Ok(()) if cut && clipboard_sequence() == sequence => clear_cut_if_unchanged(&paths),
            Err(e) => warn(h, &e.to_string()),
            _ => {}
        }
    }
}

fn clipboard_sequence() -> u32 { unsafe { windows::Win32::System::DataExchange::GetClipboardSequenceNumber() } }

fn clear_cut_if_unchanged(paths: &[PathBuf]) {
    if sh::clip_get().is_some_and(|(current, cut)| cut && current == paths) { sh::clip_clear(); }
}

/// Rename a single entry without overwriting a different existing file.
fn rename_entry(source: &Path, new: &str) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};
    if new.is_empty() || new == "." || new == ".." || new.ends_with([' ', '.'])
        || new.chars().any(|c| c < ' ' || "<>:\"/\\|?*".contains(c)) {
        return Err(Error::new(ErrorKind::InvalidInput, "Invalid file name"));
    }
    let dest = source.with_file_name(new);
    let same_name = source.file_name().is_some_and(|old| old.to_string_lossy().eq_ignore_ascii_case(new));
    if !same_name && dest.try_exists()? {
        return Err(Error::new(ErrorKind::AlreadyExists, "A file or folder with this name already exists"));
    }
    use std::os::windows::ffi::OsStrExt;
    let wide_path = |p: &Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
    // No REPLACE_EXISTING flag: a file created after the existence check is
    // protected too, while Windows still permits changing only the case.
    if unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(wide_path(source).as_ptr(), wide_path(&dest).as_ptr(), 0) } == 0 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

const MENU_ZIP: u32 = 0x9001;
const MENU_EXTRACT: u32 = 0x9002;
const MENU_PIN: u32 = 0x9003;
const MENU_RENAME: u32 = 0x9004;

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
    else { unsafe { ShellExecuteW(0 as _, ptr::null(), w(&p.to_string_lossy()).as_ptr(), ptr::null(), ptr::null(), SW_SHOWNORMAL) }; }
}

/// The menu for rows inside a zip. They are no shell items, so this is a small menu of our own.
fn zip_menu(h: HWND, paths: &[PathBuf]) {
    let id = unsafe {
        let m = CreatePopupMenu();
        for (i, t) in [(1usize, "Open"), (2, "Copy"), (3, "Extract here")] { AppendMenuW(m, MF_STRING, i, w(t).as_ptr()); }
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

/// The shell menu for the selection, with our "Compress to ZIP" / "Extract here" on top.
fn item_menu(h: HWND, dir: &Path, paths: &[PathBuf]) {
    if zipx::split_zip(dir).is_some() { return zip_menu(h, paths); }
    let mut extra = vec![(MENU_ZIP, "Compress to ZIP")];
    if paths.iter().all(|p| p.is_file() && jobs::is_archive(p)) {
        extra.insert(0, (MENU_EXTRACT, "Extract here"));
    }
    // the shell menu has no Rename here (it needs the shell's own folder view to answer it): the same edit box as F2
    if paths.len() == 1 { extra.push((MENU_RENAME, "Rename")); }
    if let [p] = paths { if p.is_dir() { extra.push((MENU_PIN, if ui::pins().contains(p) { "Unpin from Quick access" } else { "Pin to Quick access" })); } }
    let picked = sh::show(h, paths, if jobs::busy() { &[] } else { &extra }).ok().flatten();
    match picked {
        Some(MENU_PIN) => ui::toggle_pin(&paths[0]),
        Some(MENU_RENAME) => unsafe {
            let (list, ..) = selected();
            let i = put(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
            if i >= 0 {
                SetFocus(list);
                put(list, LVM_EDITLABELW, i as usize, 0);
            }
        },
        Some(MENU_ZIP) => {
            let base = if paths.len() == 1 { paths[0].file_stem().unwrap_or_default().to_string_lossy().into_owned() } else { dir.file_name().map_or("Archive".into(), |n| n.to_string_lossy().into_owned()) };
            let dest = jobs::free_name(dir, &format!("{base}.zip"), false);
            jobs::zip(h as isize, paths.to_vec(), dest);
        }
        Some(MENU_EXTRACT) => _ = jobs::extract(h as isize, paths.to_vec(), dir.to_path_buf()),
        _ => reload(h),
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
    let il = SHGetFileInfoW(w("C:\\").as_ptr(), 0, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | if small { SHGFI_SMALLICON } else { SHGFI_LARGEICON });
    put(list, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, il as isize);
    let (cx, cy) = if small { (76, 64) } else { (88, 96) };
    put(list, LVM_SETICONSPACING, 0, ((ui::sc(cy) as isize) << 16) | ui::sc(cx) as isize);
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
unsafe fn bg_menu(h: HWND) {
    let m = CreatePopupMenu();
    for (id, t) in [(1usize, "New folder"), (2, "New text file"), (3, "Paste"), (4, "Refresh"), (5, "Properties")] { AppendMenuW(m, MF_STRING, id, w(t).as_ptr()); }
    let disk = tool("wiztree");
    if disk.is_some() { AppendMenuW(m, MF_STRING, 6, w("Disk usage (WinRust Disk)").as_ptr()); }
    if desk() { for (id, l) in [(8usize, "Sort icons by name"), (9, "Sort icons by date"), (10, "Sort icons by size"), (11, "Arrange icons")] { AppendMenuW(m, MF_STRING, id, w(l).as_ptr()); } }
    if !desk() { AppendMenuW(m, MF_STRING, 13, w(if group_on() { "No grouping" } else { "Group by date" }).as_ptr()); }
    if desk() { AppendMenuW(m, MF_STRING, 12, w(if small_marker().exists() { "Large icons" } else { "Small icons" }).as_ptr()); }
    if desk() { AppendMenuW(m, MF_STRING, 7, w(if icons_hidden() { "Show desktop icons" } else { "Hide desktop icons" }).as_ptr()); }
    let mut pt: POINT = std::mem::zeroed();
    GetCursorPos(&mut pt);
    let id = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON, pt.x, pt.y, 0, h, ptr::null());
    DestroyMenu(m);
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
        1 => new_entry(&dir, "New folder", true),
        2 => new_entry(&dir, "New text document.txt", false),
        3 => paste(h, &dir),
        5 => properties(&dir),
        _ => {}
    }
    if id != 0 { reload(h); }
}

/// Command line -> (folder to open, file to select there). Understands `/select,"C:\a\b.txt"` (what browsers and other
/// apps pass for "show in folder"), a plain folder, or a plain file path (its folder, file selected).
fn start_target() -> (PathBuf, Option<String>) {
    if env::args().any(|a|a=="--recycle-bin"){return(PathBuf::from(recycle::PATH),None);}
    let mut a = env::args().skip(1);
    let mut p = a.next().unwrap_or_default();
    let select=p.to_lowercase().starts_with("/select,")||p.to_lowercase().starts_with("--select");
    if p.to_lowercase().starts_with("/select,") || p.to_lowercase().starts_with("--select") {
        p = p.get(8..).unwrap_or("").trim_start_matches(',').to_string();
        if p.is_empty() { p = a.next().unwrap_or_default(); }
    }
    let p = PathBuf::from(p.trim_matches('"'));
    if p.as_os_str().is_empty() { return (PathBuf::from("C:\\"), None); }
    match (select, p.parent()) {
        (true, Some(d)) => (d.to_path_buf(), p.file_name().map(|n| n.to_string_lossy().into_owned())),
        _ => (p, None),
    }
}

/// Another app asked the shell to show `p` in a folder window (SHOpenFolderAndSelectItems): open its folder here, select it, come to front.
fn shell_select(h: isize, p: PathBuf) {
    let h = h as HWND;
    let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { return };
    if !cur_dir().as_os_str().eq_ignore_ascii_case(dir.as_os_str()) { PENDING_PICK.set(Some(name.to_string_lossy().into_owned()));load(h, dir.to_path_buf()); }
    else{select_row(&name.to_string_lossy());}
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
fn queue_icon(list:HWND,index:usize,dir:&Path,r:&mut Row){
    if r.hdr { return; }
    if r.icon != -1{return;}
    let path=if r.recycled.is_some(){PathBuf::from(r.display_name())}else{dir.join(&r.name)};
    let window=unsafe{GetAncestor(list,GA_ROOT)} as isize;
    if ICONS.with_borrow_mut(|i|i.get_or_insert_with(||background::Icons::new(window)).request(index,path,r.dir)){r.icon=-2;}
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
    if recycle::is(dir) { return "Корзина".into(); }
    dir.file_name().map_or_else(|| dir.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}

/// Rows of `dir`, folders first. Desktop mode merges the public desktop (absolute names) and hides hidden files.
/// None = `dir` itself is unreadable.
#[cfg(test)]
fn rows_of(dir: &PathBuf) -> Option<Vec<Row>> {
    rows_for(dir, desk(), &if desk() { sh::desktop_dirs().into_iter().skip(1).collect() } else {vec![]}, SHOW_HIDDEN.load(Relaxed), SORT_COL.load(Relaxed), SORT_ASC.load(Relaxed), &|| false)
}

fn rows_for(dir: &PathBuf, desktop: bool, extras: &[PathBuf], hidden: bool, col: usize, asc: bool, cancelled: &impl Fn()->bool) -> Option<Vec<Row>> {
    if recycle::is(dir) { let mut rows = recycle::list(cancelled).ok()?; sort_rows(&mut rows,col,asc); return Some(rows); }
    if !desktop {
        if let Some((arc, inner)) = zipx::split_zip(dir) { // inside a .zip: its entries are the rows
            let mut rows: Vec<Row> = zipx::list_dir(&arc, &inner).ok()?.into_iter().map(|i| Row { name: i.name, dir: i.dir, size: i.size, time: zipx::dos_to_filetime(i.time), icon: -1, thumb: -2, recycled: None, hdr: false }).collect();
            if cancelled() { return None; }
            sort_rows(&mut rows, col, asc);
            add_headers(&mut rows);
            return Some(rows);
        }
    }
    let mut dirs = vec![dir.clone()];
    dirs.extend_from_slice(extras);
    let mut rows = vec![];
    for (n, d) in dirs.iter().enumerate() {
        let Ok(rd) = fs::read_dir(d) else { if n == 0 { return None } else { continue } };
        for e in rd.flatten() {
            if cancelled() { return None; }
            let m = e.metadata().ok();
            if (desktop || !hidden) && m.as_ref().is_some_and(|m| m.file_attributes() & 2 != 0) { continue; } // FILE_ATTRIBUTE_HIDDEN
            let name = if n == 0 { e.file_name().to_string_lossy().into_owned() } else { e.path().to_string_lossy().into_owned() };
            rows.push(row(name, m.as_ref()));
        }
    }
    if cancelled() { return None; }
    sort_rows(&mut rows, col, asc);
    add_headers(&mut rows);
    Some(rows)
}

/// Loads `dir` into the virtual list and the active tab. Unreadable dir (access denied, gone) leaves the view unchanged.
/// Never call with ST borrowed: the list sends GETDISPINFO back into the window proc.
fn load(h: HWND, dir: PathBuf) {
    submit_load(h, dir, None, false);
}

fn submit_load(h: HWND, dir: PathBuf, query: Option<String>, refresh: bool) {
    let request = background::Request { id:0, dir, query, desktop:desk(), extras:if desk(){sh::desktop_dirs().into_iter().skip(1).collect()}else{vec![]}, hidden:SHOW_HIDDEN.load(Relaxed),col:SORT_COL.load(Relaxed),asc:SORT_ASC.load(Relaxed),history:!NAV.load(Relaxed),refresh,pick:PENDING_PICK.take() };
    WORK.with_borrow_mut(|w| { w.get_or_insert_with(|| background::Worker::new(h as isize)).submit(request); });
    if !refresh { ui::activity(Some("Загрузка…  Esc — отменить".into())); unsafe{ui::status_dirty(h);} }
}

fn apply_folder(h: HWND, dir: PathBuf, rows: Vec<Row>, history: bool) {
    if !desk() { unsafe { SetWindowTextW(h, w(&if recycle::is(&dir){"Корзина".into()}else{dir.to_string_lossy().into_owned()}).as_ptr()) }; }
    let (list, tab, edit, active, n) = ST.with_borrow_mut(|st| {
        if let Some(t) = st.tabs.get_mut(st.active) { *t = dir.clone(); }
        st.dir = dir.clone(); st.rows = rows;
        if history { st.hist.visit(dir.clone()); }
        (st.list, st.tab, st.edit, st.active, st.rows.len())
    });
    unsafe { SetWindowTextW(edit, w(&dir.to_string_lossy()).as_ptr()) };
    shellwin::navigated(&dir);
    ui::set_search(None); // a folder again: the pill shows its path
    tab_text(tab, active, &label(&dir), false);
    ui::nav_dirty(); // the pane highlights where we are
    if !desk(){ui::headers(list,recycle::is(&dir));}
    reset_thumbs();
    put(list, LVM_DELETEALLITEMS, 0, 0);
    if desk() { desk::fill(list, n); } else { put(list, LVM_SETITEMCOUNT, n, 0); }
}

/// Asks the rust-search service of drive `d` (see plugins/rust-search): terms in, one path per line out.
/// None = no service for that drive or no access (its pipe is admin-only unless started with --allow).
fn ask(d: char, q: &str, cancelled: &impl Fn()->bool, deadline: std::time::Instant) -> Option<String> {
    use std::{fs::OpenOptions, io::{Read, Write}, os::windows::io::AsRawHandle};
    let mut p = OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\rs-search-{d}")).ok()?;
    let mode = windows_sys::Win32::System::Pipes::PIPE_NOWAIT;
    if unsafe { windows_sys::Win32::System::Pipes::SetNamedPipeHandleState(p.as_raw_handle(), &mode, ptr::null(), ptr::null()) } == 0 { return None; }
    p.write_all(q.as_bytes()).ok()?;
    let (mut out, mut chunk) = (Vec::new(), [0u8; 1 << 16]);
    loop {
        if cancelled() || std::time::Instant::now() >= deadline { return None; }
        match p.read(&mut chunk) {
            Ok(0) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Ok(n) => { out.extend_from_slice(&chunk[..n]); if out.len() > 16*1024*1024 { return None; } }
            Err(e) if e.raw_os_error() == Some(232) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) if e.raw_os_error() == Some(109) => break,
            Err(_) => return None,
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Fills the list with search hits from every drive's service. Rows hold absolute paths, so joining them
/// onto the current dir still yields the right file. Blocks the UI for the duration of the query (fast).
/// Never call with ST borrowed.
fn search(h: HWND, q: &str) {
    submit_load(h, cur_dir(), Some(q.into()), false);
}

fn search_rows(dir: &Path, q: &str, cancelled: &impl Fn()->bool) -> (Result<Vec<Row>,String>, Option<String>) {
    if recycle::is(dir){return(recycle::list(cancelled).map(|rows|rows.into_iter().filter(|r|r.display_name().to_lowercase().contains(&q.to_lowercase())).collect()),Some(format!("Корзина — {q}")));}
    let mut rows = vec![];
    let mut more = String::new();
    let mut indexed = false; // some drive had a rust-search service
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(3);
    for d in 'A'..='Z' {
        if cancelled() { return (Err("Отменено".into()),None); }
        if std::time::Instant::now()>=deadline { break; }
        let Some(reply) = ask(d, q, cancelled, deadline) else { continue };
        indexed = true;
        for line in reply.lines() {
            if cancelled() { return (Err("Отменено".into()),None); }
            if line.starts_with("... ") { more = format!(", {}", line.trim_start_matches("... ")); continue; }
            let m = fs::metadata(line).ok();
            rows.push(row(line.to_string(), m.as_ref()));
        }
    }
    if !indexed {
        // no rust-search installed: look through the current folder and below instead
        let (local, cut, flipped) = local_search_cancel(dir, q, cancelled);
        if flipped { more = format!(" for \u{201C}{}\u{201D}", flip_layout(q)); } // typed in the wrong layout: shown what was meant
        rows = local;
        if cut { more += ", stopped early: narrow the search"; }
    }
    let n = rows.len();
    let where_ = if indexed { "" } else { " in this folder" };
    let title = if n == 0 { format!("search: {q} - nothing{where_}") } else { format!("search: {q} - {n} hits{where_}{more}") };
    (Ok(rows), Some(title))
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
#[cfg(test)]
fn local_search(dir: &Path, q: &str) -> (Vec<Row>, bool, bool) {
    local_search_cancel(dir, q, &|| false)
}
fn local_search_cancel(dir: &Path, q: &str, cancelled: &impl Fn()->bool) -> (Vec<Row>, bool, bool) {
    const LOCAL_MAX: usize = 500;
    const LOCAL_MS: u128 = 1500;
    let (q, t0) = (q.to_lowercase(), std::time::Instant::now());
    let alt = flip_layout(&q);
    let (mut out, mut other, mut queue) = (vec![], vec![], std::collections::VecDeque::from([dir.to_path_buf()]));
    let mut cut = false;
    'walk: while let Some(d) = queue.pop_front() {
        if cancelled() { return (vec![],true,false); }
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            if cancelled() || out.len() >= LOCAL_MAX || t0.elapsed().as_millis() > LOCAL_MS { cut = true; break 'walk; }
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
    ST.with_borrow_mut(|s|{for r in &mut s.rows{if r.icon==-2{r.icon=-1;}}});
    let himl = ST.with_borrow(|st| st.himl);
    unsafe { ImageList_Remove(himl as _, -1) };
}
unsafe fn resize_thumbnails(){unsafe{
    THUMB_PX.store(ui::sc(THUMB),Relaxed);GEN.fetch_add(1,Relaxed);
    let(list,old)=ST.with_borrow(|s|(s.list,s.himl));
    let images=ImageList_Create(thumb_px(),thumb_px(),ILC_COLOR32,64,64);
    if images==0{return;}
    put(list,LVM_SETIMAGELIST,LVSIL_NORMAL as usize,images as isize);
    ST.with_borrow_mut(|s|{s.himl=images as isize;for r in &mut s.rows{r.thumb=-2;if r.icon==-2{r.icon=-1;}}});
    if old!=0{ImageList_Destroy(old as _);}
    put(list,LVM_SETICONSPACING,0,((ui::sc(THUMB+34)as isize)<<16)|ui::sc(THUMB+24)as isize);
}}

fn reload(h: HWND) { submit_load(h,cur_dir(),SEARCH_QUERY.with_borrow(Clone::clone),true); }
fn up(h:HWND){if recycle::current(){if let Some(dir)=sh::desktop_dirs().into_iter().next(){load(h,dir);}}else if let Some(dir)=cur_dir().parent(){load(h,dir.to_path_buf());}}

fn cancel_loading(h: HWND) {
    WORK.with_borrow_mut(|w| { if let Some(w)=w { w.cancel(); } });
    ui::activity(None); unsafe{ui::status_dirty(h);}
}

fn apply_ready(h: HWND) {
    let Some(result)=WORK.with_borrow_mut(|w| w.as_mut().and_then(background::Worker::take)) else {return};
    let mut request=result.request;
    if request.col!=SORT_COL.load(Relaxed)||request.asc!=SORT_ASC.load(Relaxed){request.col=SORT_COL.load(Relaxed);request.asc=SORT_ASC.load(Relaxed);WORK.with_borrow_mut(|w|{if let Some(w)=w{w.submit(request);}});return;}
    let rows=match result.rows { Ok(rows)=>rows, Err(e)=>{REFRESH_NEEDED.set(false);ui::activity(Some(e));unsafe{ui::status_dirty(h);}return;} };
    let same=request.refresh && cur_dir()==request.dir;
    let saved=if same {Some(view_state())}else{None};
    if desk() && same {desk::save(ST.with_borrow(|s|s.list));}
    if request.query.is_some() {
        let n=rows.len();
        let list=ST.with_borrow_mut(|s|{s.rows=rows;s.list});
        reset_thumbs(); put(list,LVM_DELETEALLITEMS,0,0);put(list,LVM_SETITEMCOUNT,n,0);
        if let Some(title)=result.title {unsafe{SetWindowTextW(h,w(&title).as_ptr());}}
        ui::set_search(request.query.clone());
    }else { apply_folder(h,request.dir.clone(),rows,request.history && !same); }
    if let Some(saved)=saved {restore_view(saved);}
    if let Some(name)=request.pick {select_row(&name);}
    SEARCH_QUERY.set(request.query.clone());
    if !recycle::busy(){ui::activity(None);}unsafe{ui::status_dirty(h);}
    if !same || WATCH.with_borrow(|w|w.is_none()) {
        WATCH.set(None);
        let generation=WATCH_GEN.get().wrapping_add(1);WATCH_GEN.set(generation);
        let dirs=if recycle::is(&request.dir){recycle::watch_dirs()}else if zipx::split_zip(&request.dir).is_some(){vec![]}else{std::iter::once(request.dir).chain(request.extras).collect()};
        if !dirs.is_empty(){WATCH.set(Some(background::Watcher::start(h as isize,dirs,generation,request.query.is_some()||recycle::current())));}
    }
    if REFRESH_NEEDED.replace(false){unsafe{SetTimer(h,background::REFRESH_TIMER,150,None);}}
}

struct ViewState { names:std::collections::HashSet<String>, focus:Option<String>, anchor:Option<String>, top:Option<String> }
fn view_state()->ViewState {
    ST.with_borrow(|s|{
        let mut names=std::collections::HashSet::new(); let mut i=-1;
        loop {i=put(s.list,LVM_GETNEXTITEM,i as usize,LVNI_SELECTED as isize);let Some(r)=usize::try_from(i).ok().and_then(|i|s.rows.get(i))else{break};names.insert(r.name.clone());}
        let name=|i:isize|usize::try_from(i).ok().and_then(|i|s.rows.get(i)).map(|r|r.name.clone());
        ViewState{names,focus:name(put(s.list,LVM_GETNEXTITEM,usize::MAX,LVNI_FOCUSED as isize)),anchor:name(put(s.list,LVM_GETSELECTIONMARK,0,0)),top:name(put(s.list,LVM_GETTOPINDEX,0,0))}
    })
}
fn restore_view(view:ViewState){
    let (list,states,anchor,top)=ST.with_borrow(|s|{
        let states=s.rows.iter().enumerate().filter_map(|(i,r)|{let state=if view.names.contains(&r.name){LVIS_SELECTED}else{0}|if view.focus.as_ref()==Some(&r.name){LVIS_FOCUSED}else{0};(state!=0).then_some((i,state))}).collect::<Vec<_>>();
        let index=|n:&Option<String>|n.as_ref().and_then(|n|s.rows.iter().position(|r|&r.name==n));(s.list,states,index(&view.anchor),index(&view.top))
    });
    let mut item:LVITEMW=unsafe{std::mem::zeroed()};item.stateMask=LVIS_SELECTED|LVIS_FOCUSED;
    put(list,LVM_SETITEMSTATE,usize::MAX,&item as *const _ as isize);
    for(i,state)in states{item.state=state;put(list,LVM_SETITEMSTATE,i,&item as *const _ as isize);}
    put(list,LVM_SETSELECTIONMARK,0,anchor.map_or(-1,|i|i as isize));
    if let Some(i)=top {put(list,LVM_ENSUREVISIBLE,i,0);let current=put(list,LVM_GETTOPINDEX,0,0);if current>=0 {put(list,LVM_SCROLL,0,(i as isize-current)*ui::sc(ui::ROW_H) as isize);}}
}

/// Virtual list selection belongs to indices. Move it with the files when
/// sorting, or a subsequent Delete/Cut could operate on a different file.
fn sort_visible_rows(list: HWND, col: usize, asc: bool) {
    let (_, _, paths) = selected();
    let paths: std::collections::HashSet<PathBuf> = paths.into_iter().collect();
    let focus = put(list, LVM_GETNEXTITEM, usize::MAX, LVNI_FOCUSED as isize);
    let focus_name = ST.with_borrow(|s| usize::try_from(focus).ok().and_then(|i| s.rows.get(i)).map(|r| r.name.clone()));
    let (selected_indices, focused_index, n) = ST.with_borrow_mut(|s| {
        s.rows.retain(|r| !r.hdr);
        sort_rows(&mut s.rows, col, asc);
        if !recycle::is(&s.dir) { add_headers(&mut s.rows); }
        s.rows.iter_mut().for_each(|r| r.thumb = -2);
        let selected = s.rows.iter().enumerate().filter_map(|(i, r)| paths.contains(&s.dir.join(&r.name)).then_some(i)).collect::<Vec<_>>();
        let focused = focus_name.and_then(|name| s.rows.iter().position(|r| r.name == name));
        (selected, focused, s.rows.len())
    });
    reset_thumbs();
    put(list, LVM_SETITEMCOUNT, n, (LVSICF_NOINVALIDATEALL | LVSICF_NOSCROLL) as isize);
    let mut item: LVITEMW = unsafe { std::mem::zeroed() };
    item.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
    put(list, LVM_SETITEMSTATE, usize::MAX, &item as *const _ as isize);
    item.stateMask = LVIS_SELECTED;
    item.state = LVIS_SELECTED;
    for i in selected_indices { put(list, LVM_SETITEMSTATE, i, &item as *const _ as isize); }
    if let Some(i) = focused_index {
        item.stateMask = LVIS_FOCUSED;
        item.state = LVIS_FOCUSED;
        put(list, LVM_SETITEMSTATE, i, &item as *const _ as isize);
        put(list, LVM_SETSELECTIONMARK, 0, i as isize);
    } else { put(list, LVM_SETSELECTIONMARK, 0, -1); }
    if n > 0 { put(list, LVM_REDRAWITEMS, 0, n as isize - 1); }
}

/// (list, current dir, full paths of the selected rows)
fn selected() -> (HWND, PathBuf, Vec<PathBuf>) {
    ST.with_borrow(|st| {
        let (mut sel, mut i) = (vec![], -1isize);
        loop {
            i = put(st.list, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED as isize);
            let Some(r) = usize::try_from(i).ok().and_then(|i| st.rows.get(i)) else { break };
            if !r.hdr { sel.push(st.dir.join(&r.name)); }
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

/// Opens a new tab on the current folder.
fn tab_new() {
    let h=unsafe{GetAncestor(ST.with_borrow(|s|s.list),GA_ROOT)};cancel_loading(h);
    let tab = ST.with_borrow_mut(|st| { st.tabs.push(st.dir.clone()); st.active = st.tabs.len() - 1; st.tab });
    tab_text(tab, 0, "", true);
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
    let (list, on) = ST.with_borrow_mut(|st| { st.thumbs = !st.thumbs; (st.list, st.thumbs) });
    THUMBS.store(on, Relaxed);
    ui::row_height(list, !on);
    put(list, LVM_SETVIEW, if on { LV_VIEW_ICON } else { LV_VIEW_DETAILS } as usize, 0);
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
    use windows_sys::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::{IMAGE_BITMAP, LR_LOADFROMFILE, LoadImageW}};

    /// The BMP scaled ("fill": cropped to the screen's aspect) to cw x ch. 0 = no wallpaper file.
    pub fn load(cw: i32, ch: i32) -> isize {
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
            SetStretchBltMode(ddc, HALFTONE);
            let (sw, sh) = if bm.bmWidth * ch > bm.bmHeight * cw { (bm.bmHeight * cw / ch, bm.bmHeight) } else { (bm.bmWidth, bm.bmWidth * ch / cw) };
            StretchBlt(ddc, 0, 0, cw, ch, sdc, (bm.bmWidth - sw) / 2, (bm.bmHeight - sh) / 2, sw, sh, SRCCOPY);
            SelectObject(sdc, o1); SelectObject(ddc, o2);
            DeleteDC(sdc); DeleteDC(ddc); ReleaseDC(std::ptr::null_mut(), screen); DeleteObject(src);
            dst as isize
        }
    }

    pub fn paint(dc: HDC, bmp: isize, cw: i32, ch: i32) {
        unsafe {
            let m = CreateCompatibleDC(dc);
            let old = SelectObject(m, bmp as _);
            BitBlt(dc, 0, 0, cw, ch, m, 0, 0, SRCCOPY);
            SelectObject(m, old);
            DeleteDC(m);
        }
    }

    pub fn load_desktop()->isize{unsafe{
        let screens=winrust_common::screens::all();let b=winrust_common::screens::union(&screens);
        let screen=GetDC(std::ptr::null_mut());let dc=CreateCompatibleDC(screen);let bitmap=CreateCompatibleBitmap(screen,b.width(),b.height());
        if bitmap.is_null()||dc.is_null(){if !bitmap.is_null(){DeleteObject(bitmap);}if !dc.is_null(){DeleteDC(dc);}ReleaseDC(std::ptr::null_mut(),screen);return 0;}
        let old=SelectObject(dc,bitmap);let rect=windows_sys::Win32::Foundation::RECT{left:0,top:0,right:b.width(),bottom:b.height()};FillRect(dc,&rect,GetStockObject(BLACK_BRUSH)as _);
        for monitor in screens{let r=monitor.bounds;let image=load(r.width(),r.height());if image!=0{let source=CreateCompatibleDC(screen);let previous=SelectObject(source,image as _);BitBlt(dc,r.left-b.left,r.top-b.top,r.width(),r.height(),source,0,0,SRCCOPY);SelectObject(source,previous);DeleteDC(source);DeleteObject(image as _);}}
        SelectObject(dc,old);DeleteDC(dc);ReleaseDC(std::ptr::null_mut(),screen);bitmap as isize
    }}
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

    /// Copy / move into `to`, or recycle. Must not be called while ST is borrowed (the shell pumps messages).
    pub fn fileop(h: *mut c_void, op: Op, from: &[PathBuf], to: &PathBuf) -> Result<()> {
        unsafe {
            let fo: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
            fo.SetOwnerWindow(HWND(h))?;
            fo.SetOperationFlags(if matches!(op, Op::Delete) { FOF_ALLOWUNDO } else { FOF_RENAMEONCOLLISION })?;
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
            fo.PerformOperations()?;
            if fo.GetAnyOperationsAborted()?.as_bool() {
                return Err(windows::core::Error::from_hresult(windows::core::HRESULT(0x800704C7u32 as i32)));
            }
            Ok(())
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
                let size=super::thumb_px();
                let Ok(bmp) = f.GetImage(SIZE { cx: size, cy: size }, SIIGBF_BIGGERSIZEOK) else { continue };
                let padded = pad(bmp.0,size);
                windows_sys::Win32::Graphics::Gdi::DeleteObject(bmp.0 as _);
                if padded.is_null() { continue; }
                let b = Box::new((era, row, padded as isize));
                let payload=Box::into_raw(b);
                if PostMessageW(Some(HWND(main as *mut c_void)), super::WM_THUMB, WPARAM(payload as usize), LPARAM(0)).is_err(){let(_,_,bitmap)=*Box::from_raw(payload);windows_sys::Win32::Graphics::Gdi::DeleteObject(bitmap as _);}
            }
        });
        tx
    }

    /// Centers `src` (any size up to THUMB, 32-bit with alpha) on a transparent THUMB x THUMB bitmap: ImageList_Add needs exact sizes.
    unsafe fn pad(src: *mut c_void,t:i32) -> *mut c_void {
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe {
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
            OleSetClipboard(&data)?;
            OleFlushClipboard() // file selections survive closing this manager
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
    unsafe fn hide_system_zip(cm: &IContextMenu, menu: HMENU) {
        use windows::Win32::UI::WindowsAndMessaging::{DeleteMenu, GetMenuItemCount, GetMenuStringW, GetSubMenu, MF_BYPOSITION};
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
            if super::recycle::is(&to){let result=fileop(self.main as *mut c_void,Op::Delete,&paths,&to);unsafe{*e=if result.is_ok(){DROPEFFECT_MOVE}else{DROPEFFECT_NONE};let _=PostMessageW(Some(HWND(self.main as *mut c_void)),WM_APP,WPARAM(0),LPARAM(0));}return result;}
            // like Explorer: same drive moves, other drive copies, Ctrl forces copy
            let copy = k.0 & 8 != 0 || paths.first().and_then(|p| p.components().next()) != to.components().next();
            unsafe { *e = if copy { DROPEFFECT_COPY } else { DROPEFFECT_MOVE } };
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
    use std::{cell::Cell, collections::{HashMap, HashSet}};
    use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, FillRect, HDC, ScreenToClient};
    use super::desktop_grid::{Grid,Footprint,contains_rect};
    pub const CHECK_LAYOUT:u32=WM_APP+24;

    pub const SPAWN_WORKERW: u32 = 0x052C; // what wallpaper apps send to "Progman" to get a window behind the icons
    const KEY: u32 = 0x0001_0001; // rgb(1, 0, 1): the list's background colour, made see-through while a live wallpaper runs

    thread_local! {
        static DRAG_FROM: Cell<Option<(i32, i32)>> = const { Cell::new(None) }; // our own icon drag: where it started (list coords)
        static DROP_AT: Cell<Option<(i32, i32)>> = const { Cell::new(None) }; // ...and where it was dropped (screen coords)
        static WORKERW: Cell<isize> = const { Cell::new(0) };
        static LIVE: Cell<bool> = const { Cell::new(false) }; // a live wallpaper is running in WORKERW
        static SCREENS:RefCell<Vec<winrust_common::screens::Screen>>=const{RefCell::new(vec![])};
    }
    pub fn update_screens(){SCREENS.set(winrust_common::screens::all());}
    pub unsafe fn resize_worker(){unsafe{let h=WORKERW.get() as HWND;if !h.is_null(){let b=desktop_bounds();MoveWindow(h,0,0,b.width(),b.height(),1);}}}

    pub fn pos_file() -> PathBuf {
        PathBuf::from(env::var_os("APPDATA").unwrap_or_default()).join(r"rust-explorer\desktop-icons.txt")
    }

    /// Saved icon positions: name -> (x, y) in list pixels.
    fn saved() -> HashMap<String, (i32, i32)> {
        let b=desktop_bounds();
        fs::read_to_string(pos_file()).unwrap_or_default().lines().filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?, p.next()?.to_string()))
        }).map(|(x, y, n): (i32, i32, String)| (n,(x-b.left,y-b.top))).collect()
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
        let b=desktop_bounds();
        let lines: String = names().iter().enumerate().map(|(i, n)| { let (x, y) = pos(list, i); format!("{}\t{}\t{n}\n",x+b.left,y+b.top) }).collect();
        let f = pos_file();
        let _ = fs::create_dir_all(f.parent().unwrap_or(&f));
        let _ = fs::write(f, lines);
    }

    /// Measure the native bounds, including wrapped text and the selection box.
    /// LVM_SETITEMPOSITION uses the icon anchor; a label can extend to its left.
    fn footprint(list:HWND,spacing:(i32,i32),icon:(i32,i32))->Footprint {
        let half=(spacing.0-icon.0).max(0)/2;
        let mut result=Footprint{left:-half,top:0,right:icon.0+half,bottom:spacing.1};
        let n=put(list,LVM_GETITEMCOUNT,0,0).max(0)as usize;
        for i in 0..n {unsafe{
            let mut r=RECT{left:LVIR_BOUNDS as i32,..std::mem::zeroed()};
            if put(list,LVM_GETITEMRECT,i,&mut r as *mut _ as isize)!=0 {
                let anchor=pos(list,i); // querying bounds can finish the control's initial layout
                result.left=result.left.min(r.left-anchor.0);result.top=result.top.min(r.top-anchor.1);
                result.right=result.right.max(r.right-anchor.0);result.bottom=result.bottom.max(r.bottom-anchor.1);
            }
        }}
        result
    }
    impl Grid {
        fn of(list:HWND)->Grid{unsafe{
            let size=put(list,LVM_GETITEMSPACING,0,0)as u32;
            let spacing=(((size&0xffff)as i32).max(1),((size>>16)as i32).max(1));
            let images=put(list,LVM_GETIMAGELIST,LVSIL_NORMAL as usize,0);
            let(mut iw,mut ih)=(ui::sc(32),ui::sc(32));ImageList_GetIconSize(images as _,&mut iw,&mut ih);
            let mut origin=POINT{x:0,y:0};windows_sys::Win32::Graphics::Gdi::ClientToScreen(list,&mut origin);
            let mut client:RECT=std::mem::zeroed();GetClientRect(list,&mut client);
            if client.right<=0||client.bottom<=0{let b=desktop_bounds();client.right=b.width();client.bottom=b.height()-desk_top()-desk_bottom();}
            let canvas=winrust_common::screens::Bounds{left:origin.x,top:origin.y,right:origin.x+client.right,bottom:origin.y+client.bottom};
            SCREENS.with_borrow(|screens|Grid::new(screens,canvas,spacing,footprint(list,spacing,(iw,ih)),(iw,ih),ui::sc(8)))
        }}
    }
    pub fn outside_count(list:HWND)->usize{unsafe{
        let mut origin=POINT{x:0,y:0};windows_sys::Win32::Graphics::Gdi::ClientToScreen(list,&mut origin);
        let screens=winrust_common::screens::all();
        (0..put(list,LVM_GETITEMCOUNT,0,0).max(0)as usize).filter(|&i|{
            let mut r=RECT{left:LVIR_BOUNDS as i32,..std::mem::zeroed()};
            if put(list,LVM_GETITEMRECT,i,&mut r as *mut _ as isize)==0{return true;}
            let global=winrust_common::screens::Bounds{left:r.left+origin.x,top:r.top+origin.y,right:r.right+origin.x,bottom:r.bottom+origin.y};
            !screens.iter().any(|s|contains_rect(s.work,global))
        }).count()
    }}

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
                None => later.push((i,old.get(name).copied())),
            }
        }
        for (i,want) in later {
            let Some(c) = g.free(&taken, want) else {continue};
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
                let _ = sh::fileop(h as _, sh::Op::Move, &paths, &t);
                let _ = dir;
                unsafe { PostMessageW(h, WM_APP, 0, 0) }; // reload: the moved icons are gone from here
                return true;
            }
        }
        let g = Grid::of(list);
        let n = put(list, LVM_GETITEMCOUNT, 0, 0).max(0) as usize;
        let mut taken: HashSet<usize> = (0..n).filter(|i| !sel.contains(i)).filter_map(|i| g.cell(pos(list, i))).collect();
        let (dx, dy) = (pt.x - from.0, pt.y - from.1);
        for &i in &sel {
            let (x, y) = pos(list, i);
            let Some(c) = g.free(&taken,Some((x+dx,y+dy))) else {continue};
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
        if WORKERW.get() != 0 && IsWindow(WORKERW.get() as HWND) != 0 { return; }
        let mut r: RECT = std::mem::zeroed();
        GetClientRect(h, &mut r);
        let ww = CreateWindowExW(0, w("WorkerW").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE, 0, 0, r.right, r.bottom, h, 0 as _, GetModuleHandleW(ptr::null()), ptr::null());
        SetWindowPos(ww, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); // under the icon list
        WORKERW.set(ww as isize);
    }

    /// Every couple of seconds: a wallpaper window inside WorkerW = live mode on, none = back to the picture.
    pub unsafe fn check_live(list: HWND) {
        let ww = WORKERW.get() as HWND;
        let live = !ww.is_null() && IsWindow(ww) != 0 && !GetWindow(ww, GW_CHILD).is_null();
        if live == LIVE.replace(live) { return; }
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
            if bmp != 0 { wall::paint(wp as _, bmp, cw, ch); } else { let mut r: RECT = std::mem::zeroed(); GetClientRect(h, &mut r); FillRect(wp as _, &r, GetStockObject(BLACK_BRUSH) as _); }
            return 1;
        }
        DefWindowProcW(h, m, wp, lp)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn native_item_labels_fit_each_monitor(){unsafe{
            use winrust_common::screens::{Bounds,Screen};
            sh::init();ui::init_scale();InitCommonControlsEx(&INITCOMMONCONTROLSEX{dwSize:size_of::<INITCOMMONCONTROLSEX>()as u32,dwICC:ICC_LISTVIEW_CLASSES});
            let hi=GetModuleHandleW(ptr::null());
            let root=CreateWindowExW(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE,w("STATIC").as_ptr(),ptr::null(),WS_POPUP,0,0,880,500,ptr::null_mut(),ptr::null_mut(),hi,ptr::null());
            let list=CreateWindowExW(0,w("SysListView32").as_ptr(),ptr::null(),WS_CHILD|LVS_ICON|LVS_SHAREIMAGELISTS,0,0,880,484,root,ptr::null_mut(),hi,ptr::null());assert!(!list.is_null());
            let font=ui::font(13,400,ui::ui_face());put(list,WM_SETFONT,font as usize,0);
            let mut info:SHFILEINFOW=std::mem::zeroed();let images=SHGetFileInfoW(w("C:\\").as_ptr(),0,&mut info,size_of::<SHFILEINFOW>()as u32,SHGFI_SYSICONINDEX|SHGFI_LARGEICON);put(list,LVM_SETIMAGELIST,LVSIL_NORMAL as usize,images as isize);put(list,LVM_SETICONSPACING,0,(96<<16)|88);
            for i in 0..24{let mut title=w(&format!("Ярлык {i}: длинная подпись приложения"));let item=LVITEMW{mask:LVIF_TEXT|LVIF_IMAGE,iItem:i,pszText:title.as_mut_ptr(),iImage:info.iIcon,..std::mem::zeroed()};put(list,LVM_INSERTITEMW,0,&item as *const _ as isize);}
            set_pos(list,0,(0,0));let fp=footprint(list,(88,96),(32,32));println!("Native item footprint relative to icon anchor: {fp:?}");assert!(fp.left<0,"fixture must reproduce a label extending left of the icon anchor");
            let left=Bounds{left:-440,top:0,right:0,bottom:500};let right=Bounds{left:0,top:0,right:440,bottom:500};let screens=[Screen{bounds:left,work:left,dpi:96,primary:false},Screen{bounds:right,work:right,dpi:96,primary:true}];let canvas=Bounds{left:-440,top:8,right:440,bottom:492};
            let grid=Grid::new(&screens,canvas,(88,96),fp,(32,32),8);let mut taken=HashSet::new();let mut on_left=0;
            for i in 0..24{let cell=grid.free(&taken,None).expect("fixture has enough cells on two monitors");taken.insert(cell);set_pos(list,i,grid.at(cell));}
            for i in 0..24{let mut r=RECT{left:LVIR_BOUNDS as i32,..std::mem::zeroed()};assert_ne!(put(list,LVM_GETITEMRECT,i,&mut r as *mut _ as isize),0);let actual=Bounds{left:r.left+canvas.left,top:r.top+canvas.top,right:r.right+canvas.left,bottom:r.bottom+canvas.top};assert!(screens.iter().any(|s|contains_rect(s.work,actual)),"native label crosses a monitor boundary: {actual:?}");if actual.right<=0{on_left+=1;}}
            assert!(on_left>0,"must also test items on the left monitor");DestroyWindow(root);DeleteObject(font as _);
        }}
    }
}

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
unsafe extern "system" fn label_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if m == WM_CHAR && wp == 0x7F { delete_word(h); return 0; }
        CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(LABEL_OLD.load(std::sync::atomic::Ordering::Relaxed)), h, m, wp, lp)
    }
}

/// Address bar subclass: Enter / Esc go to the main window, everything else is the normal edit control.
unsafe extern "system" fn edit_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if edit_shortcut(h, m, wp) { return 0; }
        if matches!(wp, 13 | 27) && matches!(m, WM_KEYDOWN | WM_CHAR) {
            if m == WM_KEYDOWN { PostMessageW(GetAncestor(h, GA_ROOT), WM_GO, wp, 0); }
            return 0; // WM_CHAR swallowed so there is no beep
        }
        if m == WM_CHAR && wp == 0x7F { delete_word(h); return 0; }
        CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(EDIT_OLD.load(std::sync::atomic::Ordering::Relaxed)), h, m, wp, lp)
    }
}

fn previous_word(text: &[u16], caret: usize) -> usize {
    let separator = |c: u16| char::from_u32(c as u32).is_some_and(|c| c.is_whitespace() || "\\/:".contains(c));
    let mut i = caret.min(text.len());
    while i > 0 && separator(text[i - 1]) { i -= 1; }
    while i > 0 && !separator(text[i - 1]) { i -= 1; }
    i
}

unsafe fn edit_shortcut(h: HWND, m: u32, wp: WPARAM) -> bool {
    unsafe {
    if m == WM_CHAR && matches!(wp, 1 | 127) { return true; }
    if m != WM_KEYDOWN || GetKeyState(VK_CONTROL as i32) >= 0 { return false; }
    match wp {
        0x41 => { put(h, EM_SETSEL, 0, -1); true }
        0x08 => {
            let (mut start, mut end) = (0u32, 0u32);
            put(h, EM_GETSEL, &mut start as *mut _ as usize, &mut end as *mut _ as isize);
            if start == end {
                let mut text = vec![0u16; GetWindowTextLengthW(h).max(0) as usize + 1];
                let len = GetWindowTextW(h, text.as_mut_ptr(), text.len() as i32).max(0) as usize;
                start = previous_word(&text[..len], start as usize) as u32;
            }
            put(h, EM_SETSEL, start as usize, end as isize);
            put(h, EM_REPLACESEL, 1, w("").as_ptr() as isize);
            true
        }
        _ => false
    }
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
                ui::set_dpi(if desk(){winrust_common::screens::primary().dpi}else{windows_sys::Win32::UI::HiDpi::GetDpiForWindow(h)});
                // desktop mode has no tab bar / address bar: the null handles are harmless to every SendMessage below
                let (mut tab, mut edit) = (0 as HWND, 0 as HWND);
                if !desk() {
                    ui::dark_title(h);
                    // the chrome (tabs, toolbar, address pill) is one painted window; the address box is a real edit inside its pill
                    tab = CreateWindowExW(0, w("RxChrome").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN, 0, 0, 0, 0, h, 0 as _, inst, ptr::null());
                    ui::nav_init(CreateWindowExW(0, w("RxNav").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE, 0, 0, 0, 0, h, 0 as _, inst, ptr::null()));
                    edit = CreateWindowExW(0, w("EDIT").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32, 0, 0, 0, 0, tab, 0 as _, inst, ptr::null());
                    ui::edit_font(edit);
                    EDIT_OLD.store(SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_proc as *const () as isize), std::sync::atomic::Ordering::Relaxed);
                }
                let view = if desk() { LVS_ICON } else { LVS_REPORT };
                let list = CreateWindowExW(0, w("SysListView32").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE | view | if desk() { 0 } else { LVS_OWNERDATA } | LVS_EDITLABELS | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS, 0, 0, 0, 0, h, 0 as _, inst, ptr::null());
                put(list, LVM_SETEXTENDEDLISTVIEWSTYLE, 0, (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize);
                let mut fi: SHFILEINFOW = std::mem::zeroed();
                let il = SHGetFileInfoW(w("C:\\").as_ptr(), 0, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | SHGFI_SMALLICON);
                put(list, LVM_SETIMAGELIST, LVSIL_SMALL as usize, il as isize);
                if desk() {
                    // system large icons; indexes are shared with the small list, so icon_of() works unchanged
                    let big = SHGetFileInfoW(w("C:\\").as_ptr(), 0, &mut fi, size_of::<SHFILEINFOW>() as u32, SHGFI_SYSICONINDEX | SHGFI_LARGEICON);
                    put(list, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, big as isize);
                    apply_icon_size(list);
                    // transparent background (the desktop paints it) and shadowed labels, readable on light wallpapers too
                    let ex = LVS_EX_TRANSPARENTBKGND | 0x0080_0000; // LVS_EX_TRANSPARENTSHADOWTEXT
                    put(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
                    for msg in [LVM_SETBKCOLOR, LVM_SETTEXTBKCOLOR] { put(list, msg, 0, CLR_NONE); }
                    put(list, LVM_SETTEXTCOLOR, 0, 0x00FF_FFFF);
                    let b=desktop_bounds();desk::update_screens();
                    WALL.set((wall::load_desktop(), b.width(), b.height()));
                    // grey (not ClearType) smoothing: on a live wallpaper the labels sit on a colour key, where ClearType leaves coloured fringes
                    put(list, WM_SETFONT, windows_sys::Win32::Graphics::Gdi::CreateFontW(-ui::sc(13), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 4, 0, w(ui::ui_face()).as_ptr()) as usize, 1);
                    // like explorer's desktop: wallpaper apps look for a SHELLDLL_DefView under Progman
                    CreateWindowExW(0, w("SHELLDLL_DefView").as_ptr(), ptr::null(), WS_CHILD, 0, 0, 0, 0, h, 0 as _, inst, ptr::null());
                    SetTimer(h, 7, 2000, None); // live wallpaper on / off (desk::check_live)
                } else {
                    THUMB_PX.store(ui::sc(THUMB),Relaxed);
                    let himl = ImageList_Create(thumb_px(), thumb_px(), ILC_COLOR32, 64, 64);
                    put(list, LVM_SETIMAGELIST, LVSIL_NORMAL as usize, himl as isize);
                    put(list, LVM_SETICONSPACING, 0, ((ui::sc(THUMB+34) as isize) << 16) | ui::sc(THUMB+24) as isize);
                    let _ = TX.set(sh::start_thumbs(h as isize));
                    ST.with_borrow_mut(|st| st.himl = himl as isize);
                    // the same columns, in the same order, as Explorer's details view
                    col(list, 0, "Name", ui::sc(360), false); col(list, 1, "Date modified", ui::sc(160), false); col(list, 2, "Type", ui::sc(140), false); col(list, 3, "Size", ui::sc(96), true);
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
                PENDING_PICK.set(pick);load(h, start);
                if desk() {
                    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos};
                    let mut dirs: Vec<PathBuf> = [&FOLDERID_Desktop, &FOLDERID_Downloads, &FOLDERID_Documents, &FOLDERID_Pictures, &FOLDERID_Music, &FOLDERID_Videos].iter().filter_map(|id| sh::known_dir(id)).collect();
                    dirs.extend(ui::pins());
                    shellwin::register_resident(&dirs);
                } else {
                    shellwin::navigated(&cur_dir());
                    shellwin::register(h as isize);
                }
                0
            }
            // the bar came, went or changed size (it sets the work area), or the screen changed: the icon list follows
            WM_SETTINGCHANGE | WM_DISPLAYCHANGE if desk() => {
                if m == WM_DISPLAYCHANGE {
                    resize_desktop(h);
                    reload(h);
                } else if wp == SPI_SETWORKAREA as usize {
                    desk::update_screens();
                    let (list, mut r) = (ST.with_borrow(|st| st.list), std::mem::zeroed::<RECT>());
                    GetClientRect(h, &mut r);
                    MoveWindow(list, 0, desk_top(), r.right, (r.bottom - desk_top() - desk_bottom()).max(0), 1);
                    reload(h);
                }
                0
            }
            WM_DPICHANGED if !desk()=>{
                ui::change_dpi(h,(wp&0xffff)as u32);
                if lp!=0{let r=&*(lp as *const RECT);SetWindowPos(h,ptr::null_mut(),r.left,r.top,r.right-r.left,r.bottom-r.top,SWP_NOZORDER|SWP_NOACTIVATE);}0
            }
            WM_DPICHANGED if desk()=>{resize_desktop(h);0}
            // mouse buttons 4 / 5 (and keyboard media keys) arrive as APPCOMMAND_BROWSER_BACKWARD / _FORWARD
            0x0319 if !desk() && matches!((lp >> 16) & 0x7FF, 1 | 2) => { nav(h, (lp >> 16) & 0x7FF == 1); 1 }
            WM_SIZE => {
                let list = ST.with_borrow(|st| st.list);
                let (cw, ch) = ((lp & 0xFFFF) as i32, ((lp >> 16) & 0xFFFF) as i32);
                if desk() {
                    MoveWindow(list, 0, desk_top(), cw, (ch - desk_top() - desk_bottom()).max(0), 1);
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
                    3 => tab_new(),
                    4 => nav(h, true),
                    5 => nav(h, false),
                    6 => up(h),
                    7 => reload(h),
                    31..=33 => if recycle::current(){recycle::run(h,wp,recycle::selected_entries());},
                    8 => if !recycle::current(){toggle_view(h)},
                    9 => search_focus(),
                    10..=14 => {
                        let (list, dir, paths) = selected();
                        match wp {
                            10 | 11 if !paths.is_empty() => match real_paths(&paths) { Ok(p) => { let _ = sh::clip_set(h, &p, wp == 10 && !in_zip()); } Err(e) => warn(h, &e) },
                            12 => { paste(h, &dir); reload(h); }
                            13 if !in_zip() => {
                                let i = put(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
                                if i >= 0 { SetFocus(list); put(list, LVM_EDITLABELW, i as usize, 0); }
                            }
                            14 if !paths.is_empty() && !in_zip() => { let _ = sh::fileop(h, sh::Op::Delete, &paths, &dir); reload(h); }
                            _ => {}
                        }
                    }
                    20 => { if let Some(p) = ui::nav_path(lp as usize) { load(h, p); } } // a folder in the left pane
                    21 => { if let Some(p) = ui::crumb_path(lp as usize) { load(h, p); } } // a part of the path in the address pill
                    _ => {}
                }
                0
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
                    NM_CUSTOMDRAW if nm.hwndFrom == list && !desk() => return ui::list_draw(lp), // the rows are painted by hand
                    LVN_ITEMCHANGED | LVN_ODSTATECHANGED => ui::status_dirty(h), // selection count in the status line
                    LVN_GETDISPINFOW => {
                        let it = &mut (*(lp as *mut NMLVDISPINFOW)).item;
                        // a request that arrives while ST is borrowed is skipped; the list repaints later
                        ST.with(|c| {
                            let Ok(mut st) = c.try_borrow_mut() else { return };
                            let State { dir, rows, thumbs, .. } = &mut *st;
                            let Some(i) = usize::try_from(it.iItem).ok() else { return };
                            let Some(r) = rows.get_mut(i) else { return };
                            if it.mask & LVIF_TEXT != 0 && it.cchTextMax > 0 {
                                let s = if r.hdr { if it.iSubItem == 0 { r.name[1..].to_string() } else { String::new() } } else { match it.iSubItem { 0 => shown_name(r.display_name(), desk()), 1 => date_text(r.time), 2 => r.recycled.as_ref().map_or_else(||type_text(&r.name, r.dir),|e|e.original_dir.clone()), _ if r.dir => String::new(), _ => size_text(r.size) } };
                                let u: Vec<u16> = s.encode_utf16().take(it.cchTextMax as usize - 1).collect();
                                ptr::copy_nonoverlapping(u.as_ptr(), it.pszText, u.len());
                                *it.pszText.add(u.len()) = 0;
                            }
                            if r.hdr { if it.mask & LVIF_IMAGE != 0 { it.iImage = I_IMAGENONE; } return; }
                            if it.mask & LVIF_IMAGE != 0 && *thumbs {
                                if r.thumb == -2 {
                                    r.thumb = -1;
                                    if let Some(tx) = TX.get() { let _ = tx.send((GEN.load(std::sync::atomic::Ordering::Relaxed), i, dir.join(&r.name))); }
                                }
                                it.iImage = if r.thumb >= 0 { r.thumb } else { I_IMAGENONE };
                            } else if it.mask & LVIF_IMAGE != 0 {
                                queue_icon(list,i,dir,r);
                                it.iImage = if r.icon>=0{r.icon}else{I_IMAGENONE};
                            }
                        });
                    }
                    LVN_ITEMACTIVATE => {
                        if recycle::current(){recycle::run(h,recycle::RESTORE,recycle::selected_entries());return 0;}
                        let i = (*(lp as *const NMITEMACTIVATE)).iItem;
                        let hit = ST.with_borrow(|st| usize::try_from(i).ok().and_then(|i| st.rows.get(i)).filter(|r| !r.hdr).map(|r| (st.dir.join(&r.name), r.dir)));
                        if let Some((p, is_dir)) = hit { activate(h, p, is_dir); }
                    }
                    NM_RCLICK if nm.hwndFrom == list => {
                        if recycle::current(){recycle::menu(h,!recycle::selected_entries().is_empty());return 0;}
                        let (_, dir, paths) = selected();
                        if !paths.is_empty() { item_menu(h, &dir, &paths); } else { bg_menu(h); }
                    }
                    LVN_COLUMNCLICK => {
                        let c = (*(lp as *const NMLISTVIEW)).iSubItem as usize;
                        let asc = if SORT_COL.load(Relaxed) == c { !SORT_ASC.load(Relaxed) } else { true };
                        SORT_COL.store(c, Relaxed);
                        SORT_ASC.store(asc, Relaxed);
                        sort_visible_rows(list, c, asc);
                    }
                    LVN_BEGINDRAG => {
                        if recycle::current(){return 0;}
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
                        if vk==0x1B{cancel_loading(h);if jobs::busy(){jobs::cancel();}return 0;}
                        if recycle::current(){
                            match vk{
                                0x2E=>{recycle::run(h,recycle::DELETE,recycle::selected_entries());return 0;}
                                0x0D=>{recycle::run(h,recycle::RESTORE,recycle::selected_entries());return 0;}
                                0x71|0x20|0x5D=>{if vk==0x5D{recycle::menu(h,true);}return 0;}
                                0x43|0x58|0x56|0x4E|0x4D if ctrl=>return 0,
                                _=>{}
                            }
                        }
                        if desk() && matches!(vk, 0x08 | 0x46 | 0x4D | 0x54 | 0x57 | 0x09 | 0x4C | 0x73) { return 0; } // browsing keys make no sense on the desktop
                        let (list, dir, paths) = selected();
                        match vk {
                            0x08 => up(h),
                            0x1B if jobs::busy() => { jobs::cancel(); return 0; } // Esc stops packing / unpacking
                            0x2E if !paths.is_empty() && !in_zip() => { let _ = sh::fileop(h, sh::Op::Delete, &paths, &dir); }
                            // Space: a quick look. Pictures open in photoview, videos / text / anything else in its default app
                            0x20 if !desk() => if let Some(p) = paths.first().filter(|p| p.is_file()) { activate(h, p.clone(), false); },
                            0x71 if !in_zip() => {
                                let i = put(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize);
                                if i >= 0 { put(list, LVM_EDITLABELW, i as usize, 0); }
                                return 0; // refreshing here immediately destroys the rename editor
                            }
                            0x46 if ctrl => { search_focus(); return 0; } // Ctrl+F: address bar in search mode
                            0x73 | 0x4C if vk == 0x73 || ctrl => { // F4 / Ctrl+L: address bar
                                let edit = ST.with_borrow(|st| st.edit);
                                SetFocus(edit);
                                put(edit, EM_SETSEL, 0, -1);
                                return 0;
                            }
                            0x5D if !paths.is_empty() => item_menu(h, &dir, &paths),
                            0x43 | 0x58 if ctrl && !paths.is_empty() => { // from a zip: real files in the temp folder, and only a copy (nothing can be cut out of an archive)
                                match real_paths(&paths) { Ok(p) => { if let Err(e) = sh::clip_set(h, &p, vk == 0x58 && !in_zip()) { warn(h, &e.to_string()); } } Err(e) => warn(h, &e) }
                                return 0;
                            }
                            0x41 if ctrl => { // Ctrl+A: select all (state mask LVIS_SELECTED, item -1 = every row)
                                let mut it: LVITEMW = std::mem::zeroed();
                                it.stateMask = LVIS_SELECTED;
                                it.state = LVIS_SELECTED;
                                put(list, LVM_SETITEMSTATE, usize::MAX, &it as *const _ as isize);
                                return 0;
                            }
                            0x56 if ctrl => paste(h, &dir),
                            0x4E if ctrl => new_entry(&dir, "New folder", true),
                            0x48 if ctrl => { SHOW_HIDDEN.fetch_xor(true, Relaxed); } // Ctrl+H: hidden files
                            0x74 => {} // F5: just the reload below
                            0x4D if ctrl => { toggle_view(h); return 0; } // Ctrl+M: details <-> thumbnails
                            0x54 if ctrl => tab_new(),
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
                        if hdr || recycle::current() || in_zip() { return 1; } // a heading cannot be renamed
                        let e = SendMessageW(ST.with_borrow(|st| st.list), LVM_GETEDITCONTROL, 0, 0) as HWND;
                        if e != 0 as HWND { LABEL_OLD.store(SetWindowLongPtrW(e, GWLP_WNDPROC, label_proc as *const () as isize), std::sync::atomic::Ordering::Relaxed); }
                    }
                    LVN_ENDLABELEDITW => {
                        if recycle::current()||in_zip(){return 0;}
                        let it = &(*(lp as *const NMLVDISPINFOW)).item;
                        if !it.pszText.is_null() {
                            let new = wstr(it.pszText);
                            if !new.is_empty() {
                                let source = ST.with_borrow(|st| usize::try_from(it.iItem).ok().and_then(|i| st.rows.get(i)).map(|r| st.dir.join(&r.name)));
                                if let Some(source) = source { if let Err(e) = rename_entry(&source, &new) { warn(h, &e.to_string()); } }
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
            background::READY => {apply_ready(h);0}
            background::ICON_READY=>{
                let ready=ICONS.with_borrow(|i|i.as_ref().map(|i|i.take()).unwrap_or_default());
                let list=ST.with_borrow(|s|s.list);for(era,row,icon)in ready{if era==GEN.load(Relaxed){let updated=ST.with_borrow_mut(|s|s.rows.get_mut(row).map(|r|r.icon=icon).is_some());if updated{put(list,LVM_REDRAWITEMS,row,row as isize);}}}0
            }
            recycle::DONE=>{let result=recycle::take_result();ui::activity(None);reload(h);if let Some(Err(e))=result{warn(h,&e);}0}
            background::CHANGED => {
                if wp==WATCH_GEN.get()&&!REFRESH_NEEDED.replace(true){SetTimer(h,background::REFRESH_TIMER,200,None);}0
            }
            WM_TIMER if wp==background::REFRESH_TIMER => {
                KillTimer(h,background::REFRESH_TIMER);
                if put(ST.with_borrow(|s|s.list),LVM_GETEDITCONTROL,0,0)!=0 {SetTimer(h,background::REFRESH_TIMER,250,None);return 0;}
                if WORK.with_borrow(|w|w.as_ref().is_some_and(|w|w.busy)){REFRESH_NEEDED.set(true);return 0;}
                REFRESH_NEEDED.set(false);submit_load(h,cur_dir(),SEARCH_QUERY.with_borrow(Clone::clone),true);0
            }
            shellwin::WM_SHELL_SELECT => { shell_select(h as isize, *Box::from_raw(lp as *mut PathBuf)); 0 }
            WM_JOB => {
                let result = *Box::from_raw(wp as *mut jobs::Completion);
                if result.error.is_none() {
                    if let Some((sequence, paths)) = result.cut { if sequence == clipboard_sequence() { clear_cut_if_unchanged(&paths); } }
                }
                reload(h);
                ui::status_dirty(h);
                if let Some(e) = result.error { MessageBoxW(h, w(&e).as_ptr(), w("WinRust Explorer").as_ptr(), MB_ICONWARNING); }
                0
            }
            WM_NAV => {
                match wp {
                    1 => nav(h, true),
                    2 => nav(h, false),
                    3 => { let (_, dir, sel) = selected(); properties(&sel.first().cloned().unwrap_or(dir)); }
                    _ => up(h),
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
                if wp==27{cancel_loading(h);}
                let (edit, list) = ST.with_borrow(|st| (st.edit, st.list));
                if wp == 13 {
                    let mut buf = vec![0u16; GetWindowTextLengthW(edit) as usize + 1];
                    let n = GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32) as usize;
                    let p = PathBuf::from(String::from_utf16_lossy(&buf[..n]).trim().trim_matches('"'));
                    let text = String::from_utf16_lossy(&buf[..n]);
                    if let Some(q) = text.trim().strip_prefix('?') {
                        if !q.trim().is_empty() { search(h, q.trim()); SetFocus(list); return 0; } // keep the query in the bar
                    } else { load(h, p); } // validation and IO happen on the worker
                }
                SetWindowTextW(edit, w(&cur_dir().to_string_lossy()).as_ptr()); // also undoes a bad path / Esc
                SetFocus(list);
                0
            }
            desk::SPAWN_WORKERW if desk() => { desk::spawn_workerw(h); 0 }
            desk::CHECK_LAYOUT if desk()=>{let list=ST.with_borrow(|s|s.list);if wp==1{put(list,LVM_GETITEMCOUNT,0,0)}else{desk::outside_count(list)as isize}}
            WM_TIMER if desk() && wp == 7 => { desk::check_live(ST.with_borrow(|st| st.list)); 0 }
            WM_DESTROY => { WATCH.set(None);WORK.set(None);ICONS.set(None);GEN.fetch_add(1,Relaxed);shellwin::revoke(); PostQuitMessage(0); 0 }
            WM_CLOSE if recycle::busy()=>{ui::activity(Some("Дождитесь завершения операции с корзиной".into()));ui::status_dirty(h);0}
            WM_NCDESTROY=>{ui::cleanup_fonts();if !desk(){let images=ST.with_borrow_mut(|s|{let old=s.himl;s.himl=0;old});if images!=0{ImageList_Destroy(images as _);}}let old=WALL.replace((0,0,0)).0;if old!=0{windows_sys::Win32::Graphics::Gdi::DeleteObject(old as _);}DefWindowProcW(h,m,wp,lp)}
            _ => DefWindowProcW(h, m, wp, lp),
        }
    }
}

fn main() {
    sh::init();
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        ui::init_scale();
        ui::gdiplus_start();
        ui::dark_mode();
        InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES });
        let inst = GetModuleHandleW(ptr::null());
        let cls = w("RustExplorer");
        let desktop = env::args().nth(1).as_deref() == Some("--desktop");
        // desktop without a wallpaper file shows black, not window white
        let bg = if desktop { GetStockObject(BLACK_BRUSH) as _ } else { windows_sys::Win32::Graphics::Gdi::CreateSolidBrush(ui::pal().bg) };
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(ui::proc), hInstance: inst, lpszClassName: w("RxChrome").as_ptr(), hCursor: LoadCursorW(0 as _, IDC_ARROW), ..std::mem::zeroed() });
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(ui::nav_proc), hInstance: inst, lpszClassName: w("RxNav").as_ptr(), hCursor: LoadCursorW(0 as _, IDC_ARROW), ..std::mem::zeroed() });
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: cls.as_ptr(), hCursor: LoadCursorW(0 as _, IDC_ARROW), hIcon: LoadIconW(inst, 1 as _), hbrBackground: bg, ..std::mem::zeroed() });
        if desktop {
            DESKTOP.store(true, Relaxed);
            // the whole screen: it paints the wallpaper too; the icon list sits above the bar (desk_top / desk_bottom)
            // class "Progman" like explorer's desktop, so Wallpaper Engine & co. find it; title "rs-desktop" tells it apart
            // from explorer's ("Program Manager") for rs-shell
            let pm = w("Progman");
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: pm.as_ptr(), hCursor: LoadCursorW(0 as _, IDC_ARROW), hIcon: LoadIconW(inst, 1 as _), hbrBackground: bg, ..std::mem::zeroed() });
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(desk::workerw_proc), hInstance: inst, lpszClassName: w("WorkerW").as_ptr(), ..std::mem::zeroed() });
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), hInstance: inst, lpszClassName: w("SHELLDLL_DefView").as_ptr(), ..std::mem::zeroed() });
            let b=desktop_bounds();
            let d = CreateWindowExW(WS_EX_TOOLWINDOW, pm.as_ptr(), w("rs-desktop").as_ptr(), WS_POPUP, b.left, b.top, b.width(), b.height(), 0 as _, 0 as _, inst, ptr::null());
            SetWindowPos(d, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        } else {
            CreateWindowExW(0, cls.as_ptr(), ptr::null(), WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_CLIPCHILDREN, CW_USEDEFAULT, CW_USEDEFAULT, ui::sc(1000), ui::sc(660), 0 as _, 0 as _, inst, ptr::null());
        }
        let mut msg = std::mem::zeroed();
        while GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
            // Side buttons target whichever child has focus, so handle them
            // before dispatch rather than only in the main window procedure.
            if msg.message == WM_XBUTTONUP && !desk() {
                let button = (msg.wParam >> 16) & 0xffff;
                let action = match button { 1 => 1, 2 => 2, _ => 0 };
                if action != 0 { PostMessageW(GetAncestor(msg.hwnd, GA_ROOT), WM_NAV, action, 0); continue; }
            }
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
    #[ignore="read-only geometry check of the installed desktop items"]
    fn installed_desktop_labels_stay_on_one_monitor(){use super::*;unsafe{
        let h=FindWindowW(w("Progman").as_ptr(),w("rs-desktop").as_ptr());assert!(!h.is_null());
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(8);let mut count=0usize;
        loop{assert_ne!(SendMessageTimeoutW(h,desk::CHECK_LAYOUT,1,0,SMTO_ABORTIFHUNG,1500,&mut count),0);if count>0{break;}assert!(std::time::Instant::now()<deadline,"desktop listing did not finish");std::thread::sleep(std::time::Duration::from_millis(100));}
        let mut outside=0usize;assert_ne!(SendMessageTimeoutW(h,desk::CHECK_LAYOUT,0,0,SMTO_ABORTIFHUNG,1500,&mut outside),0);println!("Installed desktop: {count} items; native item bounds crossing monitor work areas: {outside}");assert_eq!(outside,0);
    }}
    #[test]
    #[ignore="read-only verification of the installed WinRust desktop"]
    fn installed_desktop_covers_every_monitor(){use super::*;unsafe{
        let h=FindWindowW(w("Progman").as_ptr(),w("rs-desktop").as_ptr());assert!(!h.is_null(),"installed desktop absent");
        let mut rect:RECT=std::mem::zeroed();assert_ne!(GetWindowRect(h,&mut rect),0);
        let expected=desktop_bounds();println!("Installed desktop: {},{}..{},{}; required {expected:?}",rect.left,rect.top,rect.right,rect.bottom);
        assert_eq!(winrust_common::screens::Bounds::from(rect),expected);
    }}
    unsafe fn pump_until(mut condition:impl FnMut()->bool){unsafe{
        let end=std::time::Instant::now()+std::time::Duration::from_secs(8);
        while !condition(){let mut msg:MSG=std::mem::zeroed();while PeekMessageW(&mut msg,ptr::null_mut(),0,0,PM_REMOVE)!=0{TranslateMessage(&msg);DispatchMessageW(&msg);}assert!(std::time::Instant::now()<end,"background UI condition timed out");std::thread::sleep(std::time::Duration::from_millis(10));}
    }}
    #[test]
    fn background_listing_updates_live_and_keeps_selection(){
        use super::*;use windows_sys::Win32::Graphics::Gdi::*;unsafe{
            sh::init();ui::init_scale();ui::gdiplus_start();InitCommonControlsEx(&INITCOMMONCONTROLSEX{dwSize:size_of::<INITCOMMONCONTROLSEX>()as u32,dwICC:ICC_LISTVIEW_CLASSES});
            let hi=GetModuleHandleW(ptr::null());
            for(class,handler)in [("RxChrome",ui::proc as unsafe extern "system" fn(HWND,u32,WPARAM,LPARAM)->LRESULT),("RxNav",ui::nav_proc),("RxAsyncTest",proc)]{RegisterClassW(&WNDCLASSW{lpfnWndProc:Some(handler),hInstance:hi,lpszClassName:w(class).as_ptr(),..std::mem::zeroed()});}
            let dir=env::temp_dir().join(format!("rx-live-test-{}",std::process::id()));fs::create_dir_all(&dir).unwrap();fs::write(dir.join("keep.txt"),"keep").unwrap();
            let root=CreateWindowExW(WS_EX_TOOLWINDOW|WS_EX_NOACTIVATE,w("RxAsyncTest").as_ptr(),ptr::null(),WS_OVERLAPPEDWINDOW|WS_CLIPCHILDREN,30,30,1000,660,ptr::null_mut(),ptr::null_mut(),hi,ptr::null());assert!(!root.is_null());
            load(root,dir.clone());pump_until(||cur_dir()==dir&&ST.with_borrow(|s|s.rows.len()==1));select_row("keep.txt");
            ShowWindow(root,SW_SHOWNOACTIVATE);RedrawWindow(root,ptr::null(),ptr::null_mut(),RDW_INVALIDATE|RDW_UPDATENOW|RDW_ALLCHILDREN);pump_until(||ST.with_borrow(|s|s.rows[0].icon>=0));
            let mut rect:RECT=std::mem::zeroed();GetWindowRect(root,&mut rect);
            let list=ST.with_borrow(|s|s.list);let width=put(list,LVM_GETCOLUMNWIDTH,3,0);
            SendMessageW(root,WM_DPICHANGED,144|(144<<16),&rect as *const _ as isize);assert_eq!(ui::sc(100),150);assert_eq!(thumb_px(),144);assert_eq!(put(list,LVM_GETCOLUMNWIDTH,3,0),(width as f32*1.5).round()as isize);
            SendMessageW(root,WM_DPICHANGED,96|(96<<16),&rect as *const _ as isize);assert_eq!(ui::sc(100),100);assert_eq!(thumb_px(),96);assert_eq!(selected().2,[dir.join("keep.txt")]);
            SetFocus(list);let editor=put(list,LVM_EDITLABELW,0,0);assert_ne!(editor,0);fs::write(dir.join("while-editing.txt"),"x").unwrap();
            let wait=std::time::Instant::now();while wait.elapsed()<std::time::Duration::from_millis(450){let mut msg:MSG=std::mem::zeroed();while PeekMessageW(&mut msg,ptr::null_mut(),0,0,PM_REMOVE)!=0{TranslateMessage(&msg);DispatchMessageW(&msg);}std::thread::sleep(std::time::Duration::from_millis(10));}
            assert_eq!(put(list,LVM_GETEDITCONTROL,0,0),editor,"filesystem events must not destroy the active rename editor");put(list,LVM_CANCELEDITLABEL,0,0);pump_until(||ST.with_borrow(|s|s.rows.len()==2));fs::remove_file(dir.join("while-editing.txt")).unwrap();pump_until(||ST.with_borrow(|s|s.rows.len()==1));
            fs::write(dir.join("new.txt"),"new").unwrap();pump_until(||ST.with_borrow(|s|s.rows.len()==2));
            assert_eq!(selected().2,[dir.join("keep.txt")]);
            fs::rename(dir.join("new.txt"),dir.join("renamed.txt")).unwrap();pump_until(||ST.with_borrow(|s|s.rows.iter().any(|r|r.name=="renamed.txt")));
            fs::remove_file(dir.join("renamed.txt")).unwrap();pump_until(||ST.with_borrow(|s|s.rows.len()==1));assert_eq!(selected().2,[dir.join("keep.txt")]);
            // A new navigation must make the previous result ineligible.
            load(root,dir.join("missing"));load(root,dir.clone());pump_until(||WORK.with_borrow(|w|w.as_ref().is_some_and(|w|!w.busy)));assert_eq!(cur_dir(),dir);
            load(root,PathBuf::from(recycle::PATH));pump_until(||recycle::current()&&WORK.with_borrow(|w|w.as_ref().is_some_and(|w|!w.busy)));
            let mut title=[0u16;64];GetWindowTextW(root,title.as_mut_ptr(),64);assert_eq!(String::from_utf16_lossy(&title[..title.iter().position(|&c|c==0).unwrap()]),"Корзина");
            let mut rename:NMLVDISPINFOW=std::mem::zeroed();rename.hdr.hwndFrom=list;rename.hdr.code=LVN_BEGINLABELEDITW;assert_eq!(SendMessageW(root,WM_NOTIFY,0,&rename as *const _ as isize),1,"recycle entries must never enter the filesystem rename path");
            ShowWindow(root,SW_SHOWNOACTIVATE);RedrawWindow(root,ptr::null(),ptr::null_mut(),RDW_INVALIDATE|RDW_UPDATENOW|RDW_ALLCHILDREN);
            DestroyWindow(root);ST.with_borrow_mut(|s|{s.list=ptr::null_mut();s.tab=ptr::null_mut();s.rows.clear();});fs::remove_dir_all(dir).unwrap();
        }
    }
    fn legacy_sort(rows: &mut [super::Row], col: usize, asc: bool) {
        let key = |r: &super::Row| r.name.rsplit('\\').next().unwrap_or("").to_lowercase();
        rows.sort_by(|a, b| {
            let o = match col { 1 => a.time.cmp(&b.time), 2 => super::type_text(&a.name, a.dir).cmp(&super::type_text(&b.name, b.dir)), 3 => a.size.cmp(&b.size), _ => key(a).cmp(&key(b)) };
            (!a.dir).cmp(&!b.dir).then(if asc { o } else { o.reverse() }).then_with(|| key(a).cmp(&key(b)))
        });
    }

    fn sort_fixture(n: usize) -> Vec<super::Row> {
        (0..n).map(|i| {
            let j = (i * 7919) % n;
            super::Row { name: format!("C:\\test\\{}-{:05}.{}", ["Я", "a", "😀", "A"][j % 4], j % 127, ["txt", "PNG", "", "rs"][j % 4]), dir: j % 9 == 0, size: (j % 23) as u64, time: (j % 13) as u64, icon: i as i32, thumb: -2, recycled: None, hdr: false }
        }).collect()
    }

    #[test]
    fn cached_sort_preserves_all_columns_directions_and_stable_ties() {
        for col in 0..4 { for asc in [true, false] {
            let (mut old, mut new) = (sort_fixture(2000), sort_fixture(2000));
            legacy_sort(&mut old, col, asc);
            super::sort_rows(&mut new, col, asc);
            assert_eq!(old.iter().map(|r| r.icon).collect::<Vec<_>>(), new.iter().map(|r| r.icon).collect::<Vec<_>>(), "column {col}, ascending {asc}");
        }}
    }

    #[test]
    fn sorting_keeps_selected_files_focus_and_shift_anchor() {
        use super::*;
        unsafe {
            InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES });
            let hi = GetModuleHandleW(ptr::null());
            let parent = CreateWindowExW(WS_EX_TOOLWINDOW, w("STATIC").as_ptr(), ptr::null(), WS_POPUP, 0, 0, 400, 240, ptr::null_mut(), ptr::null_mut(), hi, ptr::null());
            let list = CreateWindowExW(0, w("SysListView32").as_ptr(), ptr::null(), WS_CHILD | LVS_REPORT | LVS_OWNERDATA, 0, 0, 350, 170, parent, ptr::null_mut(), hi, ptr::null());
            assert!(!list.is_null());
            ST.with_borrow_mut(|s| { s.list = list; s.dir = env::temp_dir(); s.rows = ["c.txt", "a.txt", "b.txt"].into_iter().map(|n| row(n.into(), None)).collect(); });
            put(list, LVM_SETITEMCOUNT, 3, 0);
            let mut item: LVITEMW = std::mem::zeroed();
            item.stateMask = LVIS_SELECTED | LVIS_FOCUSED; item.state = item.stateMask;
            put(list, LVM_SETITEMSTATE, 0, &item as *const _ as isize);
            item.stateMask = LVIS_SELECTED; item.state = LVIS_SELECTED;
            put(list, LVM_SETITEMSTATE, 2, &item as *const _ as isize);
            for asc in [true, false] {
                sort_visible_rows(list, 0, asc);
                let mut names = selected().2.into_iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
                names.sort(); assert_eq!(names, ["b.txt", "c.txt"]);
                let focused = put(list, LVM_GETNEXTITEM, usize::MAX, LVNI_FOCUSED as isize);
                assert_eq!(ST.with_borrow(|s| s.rows[focused as usize].name.clone()), "c.txt");
                assert_eq!(put(list, LVM_GETSELECTIONMARK, 0, 0), focused);
            }
            DestroyWindow(parent);
            ST.with_borrow_mut(|s| { s.list = ptr::null_mut(); s.rows.clear(); });
        }
    }

    #[test]
    #[ignore = "local large-folder sorting measurement"]
    fn large_folder_sort_cost() {
        for col in 0..4 {
            let (mut old, mut new) = (sort_fixture(30_000), sort_fixture(30_000));
            let start = std::time::Instant::now(); legacy_sort(&mut old, col, true); let old_time = start.elapsed();
            let start = std::time::Instant::now(); super::sort_rows(&mut new, col, true); let new_time = start.elapsed();
            assert_eq!(old.iter().map(|r| r.icon).collect::<Vec<_>>(), new.iter().map(|r| r.icon).collect::<Vec<_>>());
            println!("30,000 rows column {col}: original {old_time:?}, cached {new_time:?}");
        }
    }

    #[test]
    #[ignore = "interactive local rendering performance measurement"]
    fn resizing_and_idle_rendering_cost() {
        use super::*;
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe fn cpu_ms() -> f64 {
            use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
            let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) = unsafe { std::mem::zeroed() };
            unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u); }
            let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
            (ticks(k) + ticks(u)) as f64 / 10000.0
        }
        unsafe {
            sh::init(); ui::init_scale(); ui::gdiplus_start();
            InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES });
            let hi = GetModuleHandleW(ptr::null());
            for (class, handler) in [("RxChrome", ui::proc as unsafe extern "system" fn(HWND,u32,WPARAM,LPARAM)->LRESULT), ("RxNav", ui::nav_proc), ("RxPerfRoot", proc)] {
                RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(handler), hInstance: hi, lpszClassName: w(class).as_ptr(), ..std::mem::zeroed() });
            }
            let dir = env::temp_dir().join(format!("rx-render-perf-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            for i in 0..40 { fs::write(dir.join(format!("sample-{i:02}.txt")), "sample").unwrap(); }
            let root = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, w("RxPerfRoot").as_ptr(), w("WinRust rendering test").as_ptr(), WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                40, 40, 1000, 660, ptr::null_mut(), ptr::null_mut(), hi, ptr::null());
            assert!(!root.is_null());
            load(root, dir.clone());
            pump_until(||cur_dir()==dir&&ST.with_borrow(|s|s.rows.len()==40));
            ShowWindow(root, SW_SHOWNOACTIVATE);
            RedrawWindow(root, ptr::null(), ptr::null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW);
            let cpu = cpu_ms();
            ui::PAINT_STATS.set([(0, std::time::Duration::ZERO); 4]);
            let wall = std::time::Instant::now();
            for i in 0..80 {
                MoveWindow(root, 40, 40, 1000 + (i % 2) * 160, 660 + (i % 2) * 90, 0);
                if i % 4 == 3 { RedrawWindow(root, ptr::null(), ptr::null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN); }
            }
            println!("80 resizes: wall {:?}; CPU {:.3} ms", wall.elapsed(), cpu_ms() - cpu);
            println!("paint counts/times (chrome, navigation, rows, status): {:?}", ui::PAINT_STATS.with_borrow(|s| *s));
            DestroyWindow(root);
            fs::remove_dir_all(dir).unwrap();
        }
    }
    #[test]
    fn renames_without_overwriting_and_reports_invalid_names() {
        let d = std::env::temp_dir().join(format!("rx-safe-rename-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("source.txt"), "source contents").unwrap();
        fs::write(d.join("existing.txt"), "keep these contents").unwrap();
        assert!(super::rename_entry(&d.join("source.txt"), "existing.txt").is_err());
        assert_eq!(fs::read_to_string(d.join("existing.txt")).unwrap(), "keep these contents");
        assert_eq!(fs::read_to_string(d.join("source.txt")).unwrap(), "source contents");
        for name in ["..", "bad/name", "bad\\name", "file:stream", "trailing.", "trailing "] {
            assert!(super::rename_entry(&d.join("source.txt"), name).is_err());
        }
        super::rename_entry(&d.join("source.txt"), "renamed.txt").unwrap();
        super::rename_entry(&d.join("renamed.txt"), "RENAMED.txt").unwrap();
        assert_eq!(fs::read_to_string(d.join("RENAMED.txt")).unwrap(), "source contents");
        assert!(super::rename_entry(&d.join("missing.txt"), "new.txt").is_err());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn ctrl_backspace_uses_utf16_positions_and_path_boundaries() {
        for (text, expected) in [("C:\\Users\\John", "C:\\Users\\"), ("hello мир", "hello "), ("one 😀", "one "), ("abc   ", ""), ("", "")] {
            let text: Vec<u16> = text.encode_utf16().collect();
            let start = super::previous_word(&text, text.len());
            assert_eq!(String::from_utf16(&text[..start]).unwrap(), expected);
        }
    }

    #[test]
    fn f2_keeps_the_native_label_editor_open() {
        use super::*;
        unsafe extern "system" fn parent(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
            if m == WM_NOTIFY { unsafe { super::proc(h, m, wp, lp) } } else { unsafe { DefWindowProcW(h, m, wp, lp) } }
        }
        unsafe {
            InitCommonControlsEx(&INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES });
            let hi = GetModuleHandleW(ptr::null());
            let cls = w("RxRenameRegressionTest");
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(parent), hInstance: hi, lpszClassName: cls.as_ptr(), ..std::mem::zeroed() });
            let h = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, cls.as_ptr(), w("Rename test").as_ptr(), WS_OVERLAPPEDWINDOW, 0, 0, 400, 240, ptr::null_mut(), ptr::null_mut(), hi, ptr::null());
            let list = CreateWindowExW(0, w("SysListView32").as_ptr(), ptr::null(), WS_CHILD | WS_VISIBLE | LVS_REPORT | LVS_OWNERDATA | LVS_EDITLABELS, 0, 0, 350, 170, h, ptr::null_mut(), hi, ptr::null());
            ST.with_borrow_mut(|s| { s.list = list; s.dir = std::env::temp_dir(); s.rows = vec![row("f2-regression.txt".into(), None)]; });
            col(list, 0, "Name", 300, false);
            put(list, LVM_SETITEMCOUNT, 1, 0);
            let mut item: LVITEMW = std::mem::zeroed();
            item.stateMask = LVIS_SELECTED | LVIS_FOCUSED;
            item.state = item.stateMask;
            put(list, LVM_SETITEMSTATE, 0, &item as *const _ as isize);
            ShowWindow(h, SW_SHOWNOACTIVATE);
            SetFocus(list);
            assert_eq!(put(list, LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize), 0);
            let mut key: NMLVKEYDOWN = std::mem::zeroed();
            key.hdr.hwndFrom = list;
            key.hdr.code = LVN_KEYDOWN;
            key.wVKey = 0x71;
            SendMessageW(h, WM_NOTIFY, 0, &key as *const _ as isize);
            let edit = put(list, LVM_GETEDITCONTROL, 0, 0);
            assert_ne!(edit, 0, "F2 must not refresh the list and destroy its editor");
            DestroyWindow(h);
            ST.with_borrow_mut(|s| { s.list = ptr::null_mut(); s.rows.clear(); });
        }
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

    fn r(name: &str, dir: bool, size: u64, time: u64) -> Row { Row { name: name.into(), dir, size, time, icon: -1, thumb: -2, recycled: None, hdr: false } }
    #[test]
    fn background_sort_keeps_date_groups_and_heading_rows() {
        bucket_refresh(); let (today,off)=BCTX.get();
        let now=(today*DAY-off+DAY/2) as u64;
        let mut rows=vec![r("old.txt",false,1,now-90*DAY as u64),r("b.txt",false,1,now),r("a.txt",false,1,now)];
        sort_rows_mode(&mut rows,0,true,true); add_headers_mode(&mut rows,true);
        assert_eq!(rows.iter().filter(|r|r.hdr).count(),2);
        assert_eq!(rows.iter().filter(|r|!r.hdr).map(|r|r.name.as_str()).collect::<Vec<_>>(),vec!["a.txt","b.txt","old.txt"]);
        assert!(rows[0].hdr && rows[3].hdr);
        assert!(rows.iter().filter(|r|r.hdr).all(|r|r.recycled.is_none() && r.thumb==-1));
        rows.retain(|r|!r.hdr);sort_rows_mode(&mut rows,0,false,true);add_headers_mode(&mut rows,true);
        assert_eq!(rows.iter().filter(|r|!r.hdr).map(|r|r.name.as_str()).collect::<Vec<_>>(),vec!["b.txt","a.txt","old.txt"]);
    }
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

    #[test]
    fn type_and_size_read_like_explorers() {
        assert_eq!(type_text("x", true), "File folder");
        assert_eq!(type_text("a.txt", false), "TXT file");
        assert_eq!(type_text("C:\\d\\b.tar.gz", false), "GZ file");
        assert_eq!(type_text(".gitignore", false), "File"); // a dot file has no extension
        assert_eq!(type_text("Makefile", false), "File");
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
