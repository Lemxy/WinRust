//! A bigger picture next to the pointer while it rests on a picture or a video (#139), like QTTabBar.
//! shortcut: a video shows its thumbnail frame, it does not play; playing needs Media Foundation.
#![allow(unsafe_op_in_unsafe_fn)]
use std::{cell::Cell, fs, os::windows::fs::MetadataExt, path::{Path, PathBuf}, ptr};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, Input::KeyboardAndMouse::*, Shell::*, WindowsAndMessaging::*},
};

const WM_IMAGE: u32 = WM_APP + 20; // worker -> list: wp = which hover asked, lp = HBITMAP
const SIZE: i32 = 400;

thread_local! {
    static POPUP: Cell<isize> = const { Cell::new(0) };
    static ROW: Cell<isize> = const { Cell::new(-1) }; // the row being previewed
    static ASKED: Cell<usize> = const { Cell::new(0) }; // counts requests: a late picture of an earlier one is dropped
    static BMP: Cell<isize> = const { Cell::new(0) };
}

/// The marker's presence turns the preview off.
pub fn off_marker() -> PathBuf {
    PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join(r"rust-explorer\no-hover-preview")
}

fn is_media(p: &Path) -> bool {
    super::is_pic(p) || p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["mp4", "mkv", "avi", "mov", "wmv", "webm", "m4v", "mpg", "mpeg", "ts"].contains(&e.to_ascii_lowercase().as_str()))
}

/// Only a file that is really here: reading a cloud-only one would download it (#134).
fn local_file(p: &Path) -> bool {
    const REMOTE: u32 = 0x1000 | 0x0004_0000 | 0x0040_0000; // OFFLINE, RECALL_ON_OPEN, RECALL_ON_DATA_ACCESS
    fs::metadata(p).is_ok_and(|m| m.is_file() && m.file_attributes() & REMOTE == 0)
}

/// Top left of a w x h popup at the pointer: below right of it, on the other side where the work area ends.
fn place((x, y): (i32, i32), (w, h): (i32, i32), a: RECT) -> (i32, i32) {
    const GAP: i32 = 20;
    let px = if x + GAP + w <= a.right { x + GAP } else { x - GAP - w };
    let py = if y + GAP + h <= a.bottom { y + GAP } else { y - GAP - h };
    (px.max(a.left), py.max(a.top))
}

pub unsafe fn attach(list: HWND) {
    SetWindowSubclass(list, Some(list_proc), 1, 0);
}

unsafe fn hide() {
    ROW.set(-1);
    ASKED.set(ASKED.get().wrapping_add(1));
    if POPUP.get() != 0 {
        ShowWindow(POPUP.get() as HWND, SW_HIDE);
    }
}

unsafe fn row_at(list: HWND) -> isize {
    let mut ht: LVHITTESTINFO = std::mem::zeroed();
    GetCursorPos(&mut ht.pt);
    ScreenToClient(list, &mut ht.pt);
    SendMessageW(list, LVM_HITTEST, 0, &mut ht as *mut _ as isize)
}

unsafe fn follow() {
    let popup = POPUP.get() as HWND;
    if popup.is_null() || IsWindowVisible(popup) == 0 {
        return;
    }
    let mut c = POINT { x: 0, y: 0 };
    GetCursorPos(&mut c);
    let mut r: RECT = std::mem::zeroed();
    GetWindowRect(popup, &mut r);
    let mut mi: MONITORINFO = std::mem::zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromPoint(c, MONITOR_DEFAULTTONEAREST), &mut mi);
    let (x, y) = place((c.x, c.y), (r.right - r.left, r.bottom - r.top), mi.rcWork);
    SetWindowPos(popup, HWND_TOPMOST, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
}

unsafe fn ask(list: HWND) {
    let row = row_at(list);
    if row < 0 || row == ROW.get() || off_marker().exists() || GetForegroundWindow() != GetAncestor(list, GA_ROOT) {
        return;
    }
    let path = super::ST.with(|c| c.try_borrow().ok().and_then(|st| st.rows.get(row as usize).filter(|r| !r.hdr && !r.dir).map(|r| st.dir.join(&r.name))));
    hide();
    let Some(path) = path.filter(|p| is_media(p)) else { return };
    ROW.set(row);
    let (asked, to) = (ASKED.get(), list as usize);
    std::thread::spawn(move || {
        if !local_file(&path) {
            return;
        }
        if let Some(bmp) = super::sh::image(&path, SIZE) {
            if PostMessageW(to as HWND, WM_IMAGE, asked, bmp) == 0 {
                DeleteObject(bmp as _);
            }
        }
    });
}

unsafe fn show(list: HWND, bmp: isize) {
    let mut bm: BITMAP = std::mem::zeroed();
    GetObjectW(bmp as _, size_of::<BITMAP>() as i32, &mut bm as *mut _ as _);
    let inst = GetModuleHandleW(ptr::null());
    if POPUP.get() == 0 {
        let class = super::w("RxPreview");
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(popup_proc);
        wc.hInstance = inst;
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc);
        let p = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT, class.as_ptr(), ptr::null(), WS_POPUP | WS_BORDER, 0, 0, 1, 1, GetAncestor(list, GA_ROOT), ptr::null_mut(), inst, ptr::null());
        POPUP.set(p as isize);
    }
    let old = BMP.replace(bmp);
    if old != 0 {
        DeleteObject(old as _);
    }
    let popup = POPUP.get() as HWND;
    SetWindowPos(popup, HWND_TOPMOST, 0, 0, bm.bmWidth + 2, bm.bmHeight + 2, SWP_NOMOVE | SWP_NOACTIVATE);
    ShowWindow(popup, SW_SHOWNOACTIVATE);
    InvalidateRect(popup, ptr::null(), 0);
    follow();
}

unsafe extern "system" fn popup_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if m != WM_PAINT {
        return DefWindowProcW(h, m, wp, lp);
    }
    let mut ps: PAINTSTRUCT = std::mem::zeroed();
    let dc = BeginPaint(h, &mut ps);
    let mem = CreateCompatibleDC(dc);
    let old = SelectObject(mem, BMP.get() as _);
    let mut r: RECT = std::mem::zeroed();
    GetClientRect(h, &mut r);
    BitBlt(dc, 0, 0, r.right, r.bottom, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    DeleteDC(mem);
    EndPaint(h, &ps);
    0
}

unsafe extern "system" fn list_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match m {
        WM_MOUSEMOVE => {
            if ROW.get() >= 0 && row_at(h) != ROW.get() {
                hide();
            }
            follow();
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_HOVER | TME_LEAVE, hwndTrack: h, dwHoverTime: 400 };
            TrackMouseEvent(&mut t);
        }
        // ours alone (TME_HOVER above): the list view selects the row under the pointer on WM_MOUSEHOVER once it has been
        // clicked, so passing it on made every row the pointer rested on the selection (#230)
        WM_MOUSEHOVER => {
            ask(h);
            return 0;
        }
        WM_IMAGE => {
            if wp == ASKED.get() && ROW.get() >= 0 {
                show(h, lp);
            } else {
                DeleteObject(lp as _);
            }
            return 0;
        }
        // anything that moves or replaces the rows under the pointer
        WM_MOUSELEAVE | WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_MOUSEWHEEL | WM_KEYDOWN | WM_KILLFOCUS | LVM_SETITEMCOUNT => hide(),
        _ => {}
    }
    DefSubclassProc(h, m, wp, lp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_stays_in_the_work_area() {
        let a = RECT { left: 0, top: 0, right: 1000, bottom: 800 };
        assert_eq!(place((100, 100), (400, 300), a), (120, 120));
        assert_eq!(place((900, 700), (400, 300), a), (480, 380)); // flips to the other side of the pointer
        assert_eq!(place((10, 10), (2000, 2000), a), (0, 0));
    }

    #[test]
    fn pictures_and_videos_only() {
        assert!(is_media(Path::new("a.MP4")) && is_media(Path::new("b.jpg")));
        assert!(!is_media(Path::new("c.txt")) && !is_media(Path::new("mp4")));
    }
}
