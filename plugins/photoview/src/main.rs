#![windows_subsystem = "windows"]
// photoview [picture]   Fast picture viewer (WIC decoder, GDI window). The other pictures of the folder are one key away.
// Left/Right, Space, Backspace, PgUp/PgDn: previous / next    Home/End: first / last    wheel: zoom    drag: pan
// double click, F: fit <-> 100%    1: 100%    R / Shift+R: turn right / left    Del: to the Recycle Bin    Esc: close
// shortcuts: files sorted by plain case-insensitive name (no "2 before 10" ordering), only the shown picture is decoded
// (no preloading of neighbours). Shown smaller than it is, the picture is scaled by WIC's high-quality filter, once per size.
mod decode;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::atomic::{AtomicU32, Ordering::Relaxed},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::GetDpiForSystem,
        Input::KeyboardAndMouse::*,
        Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, FOF_ALLOWUNDO, FO_DELETE, HDROP, SHFileOperationW, SHFILEOPSTRUCTW},
        WindowsAndMessaging::*,
    },
};

pub const EXTS: [&str; 11] = ["jpg", "jpeg", "jfif", "png", "bmp", "gif", "tif", "tiff", "webp", "heic", "avif"];
const WM_IMG: u32 = WM_APP + 1; // lparam = Box<(u32, Result<Img, String>)>
const BG: [u8; 3] = [24, 22, 22]; // BGR of rgb(22, 22, 24), the shared graphite

static GEN: AtomicU32 = AtomicU32::new(0); // newest load; older decodes that finish late are dropped

#[derive(Default)]
struct App {
    files: Vec<PathBuf>,
    idx: usize,
    img: Option<decode::Img>,
    note: String,
    zoom: Option<f64>, // None = fit
    off: (f64, f64),
    drag: Option<(i32, i32)>,
    font: isize,
    dpi: i32,
    small: Option<(i32, i32, Vec<u8>)>, // the picture scaled down to the size it is shown at, kept until that changes
}
thread_local! { static APP: RefCell<App> = RefCell::new(App::default()) }

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn is_pic(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

fn open(hwnd: HWND, path: &Path) {
    let Some(dir) = path.parent() else { return };
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| is_pic(p)).collect();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    let idx = files.iter().position(|p| p.as_os_str().eq_ignore_ascii_case(path.as_os_str())).unwrap_or_else(|| {
        files.insert(0, path.to_path_buf());
        0
    });
    APP.with_borrow_mut(|a| (a.files, a.idx) = (files, idx));
    load(hwnd);
}

fn load(hwnd: HWND) {
    let Some(path) = APP.with_borrow_mut(|a| {
        (a.zoom, a.off) = (None, (0.0, 0.0));
        a.small = None;
        a.note = "...".into();
        a.files.get(a.idx).cloned()
    }) else {
        return;
    };
    let g = GEN.fetch_add(1, Relaxed) + 1;
    let h = hwnd as isize;
    std::thread::spawn(move || {
        decode::com_init();
        let r = decode::decode(&path.to_string_lossy(), BG).map_err(|e| e.message());
        unsafe { PostMessageW(h as HWND, WM_IMG, 0, Box::into_raw(Box::new((g, r))) as isize) };
    });
    unsafe { InvalidateRect(hwnd, null(), 0) };
    title(hwnd);
}

fn go(hwnd: HWND, to: impl FnOnce(usize, usize) -> usize) {
    let moved = APP.with_borrow_mut(|a| {
        let n = to(a.idx, a.files.len());
        (!a.files.is_empty() && n != a.idx).then(|| a.idx = n).is_some()
    });
    if moved {
        load(hwnd);
    }
}

/// (scale, x, y) of the picture inside a client area of cw x ch.
fn view(a: &App, cw: f64, ch: f64) -> Option<(f64, f64, f64)> {
    let i = a.img.as_ref()?;
    let z = a.zoom.unwrap_or_else(|| (cw / i.w as f64).min(ch / i.h as f64).min(1.0));
    Some((z, (cw - i.w as f64 * z) / 2.0 + a.off.0, (ch - i.h as f64 * z) / 2.0 + a.off.1))
}

fn title(hwnd: HWND) {
    let t = APP.with_borrow(|a| match (a.files.get(a.idx), &a.img) {
        (Some(p), img) => {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let size = img.as_ref().map_or(String::new(), |i| format!("  {}x{}", i.w, i.h));
            format!("{name}  ({}/{}){size}  - {}", a.idx + 1, a.files.len(), winrust_common::t!("Просмотр фото", "Photo viewer", ja: "フォト ビューアー", pt: "Visualizador de fotos", zh: "图片查看器", es: "Visor de fotos"))
        }
        _ => winrust_common::t!("Просмотр фото", "Photo viewer", ja: "フォト ビューアー", pt: "Visualizador de fotos", zh: "图片查看器", es: "Visor de fotos").into(),
    });
    unsafe {
        SetWindowTextW(hwnd, wide(&t).as_ptr());
        InvalidateRect(hwnd, null(), 0); // the title is drawn in our own caption
    }
}

/// The window draws its own caption: a thin strip with the file's name and the buttons; the picture gets the rest.
const CAP: winrust_common::caption::Caption = winrust_common::caption::Caption { height: 32, drag: 32, buttons: winrust_common::caption::MIN | winrust_common::caption::MAX | winrust_common::caption::CLOSE };

/// Where the picture area starts: under the caption strip.
fn top(hwnd: HWND) -> i32 {
    winrust_common::caption::height(hwnd, CAP)
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (cw, ch) = (rc.right, rc.bottom);
        let mem = CreateCompatibleDC(dc);
        let bmp = CreateCompatibleBitmap(dc, cw, ch);
        let old = SelectObject(mem, bmp);
        let bg = BG[2] as u32 | (BG[1] as u32) << 8 | (BG[0] as u32) << 16;
        let b = CreateSolidBrush(bg);
        FillRect(mem, &rc, b);
        DeleteObject(b as _);
        let top = top(hwnd);
        let mut zoom_pct = None;
        APP.with_borrow_mut(|a| {
            if let Some((z, x, y)) = view(a, cw as f64, (ch - top) as f64) {
                let y = y + top as f64;
                zoom_pct = Some((z * 100.0).round() as i32);
                IntersectClipRect(mem, 0, top, cw, ch); // a zoomed picture slides under the caption, not over it
                let (iw, ih) = a.img.as_ref().map_or((1, 1), |i| (i.w, i.h));
                let (dw, dh) = ((iw as f64 * z).round() as i32, (ih as f64 * z).round() as i32);
                // smaller than it is: the high-quality copy for this size (made once), drawn 1:1
                if z < 1.0 && dw > 0 && dh > 0 && a.small.as_ref().is_none_or(|s| (s.0, s.1) != (dw, dh)) {
                    a.small = a.img.as_ref().and_then(|i| decode::scaled(i, dw, dh)).map(|p| (dw, dh, p));
                }
                let Some(i) = a.img.as_ref() else { return };
                let (sw, sh, px) = match &a.small {
                    Some((w, h, p)) if z < 1.0 && (*w, *h) == (dw, dh) => (*w, *h, p.as_slice()),
                    _ => (i.w, i.h, i.bgra.as_slice()),
                };
                let mut bi: BITMAPINFO = std::mem::zeroed();
                bi.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: sw, biHeight: -sh, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB, ..std::mem::zeroed() };
                SetStretchBltMode(mem, if z < 1.0 { HALFTONE } else { COLORONCOLOR } as _);
                SetBrushOrgEx(mem, 0, 0, null_mut());
                StretchDIBits(mem, x as i32, y as i32, dw, dh, 0, 0, sw, sh, px.as_ptr() as _, &bi, DIB_RGB_COLORS, SRCCOPY);
            } else {
                SelectObject(mem, a.font as _);
                SetBkMode(mem, TRANSPARENT as _);
                SetTextColor(mem, 0x93_8e_8e);
                let mut r = RECT { top, ..rc };
                let t = wide(if a.files.is_empty() { winrust_common::t!("Откройте фото этой программой или перетащите его сюда", "Open a picture with this program, or drop one here", ja: "このプログラムで画像を開くか、ここにドロップしてください", pt: "Abra uma imagem com este programa ou solte uma aqui", zh: "用它打开一张图片，或把图片拖到这里", es: "Abre una imagen con este programa o suéltala aquí") } else { &a.note });
                DrawTextW(mem, t.as_ptr(), -1, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
        });
        // the caption strip: the window's title (file, position, size) on the left, the buttons on the right
        SelectClipRgn(mem, null_mut());
        let strip = RECT { left: 0, top: 0, right: cw, bottom: top };
        let cap_bg = winrust_common::style::mix(bg, 0xFF_FF_FF, 0.04);
        let b = CreateSolidBrush(cap_bg);
        FillRect(mem, &strip, b);
        DeleteObject(b as _);
        let mut name = [0u16; 512];
        let n = GetWindowTextW(hwnd, name.as_mut_ptr(), name.len() as i32);
        let s = winrust_common::caption::scale(hwnd);
        let f = CreateFontW(-((12.0 * s) as i32), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide(winrust_common::style::sans_face()).as_ptr());
        let of = SelectObject(mem, f as _);
        SetBkMode(mem, TRANSPARENT as _);
        SetTextColor(mem, 0xC8_C8_C8);
        let right = cw - winrust_common::caption::buttons_width(hwnd, CAP) - 8;
        // the zoom, dimmer, at the strip's right end
        let mut zr = RECT { left: right - (60.0 * s) as i32, top: 0, right, bottom: top };
        if let Some(p) = zoom_pct {
            SetTextColor(mem, 0x8A_8A_8A);
            let t = wide(&format!("{p} %"));
            DrawTextW(mem, t.as_ptr(), -1, &mut zr, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            SetTextColor(mem, 0xC8_C8_C8);
        }
        let mut tr = RECT { left: (12.0 * s) as i32, top: 0, right: zr.left - 8, bottom: top };
        DrawTextW(mem, name.as_ptr(), n, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX);
        SelectObject(mem, of);
        DeleteObject(f as _);
        winrust_common::caption::paint_buttons(mem, hwnd, CAP, cap_bg, 0xF0_F0_F0);
        BitBlt(dc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bmp as _);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

/// Moves the current file to the Recycle Bin (Explorer asks first) and shows the next picture.
fn delete_current(hwnd: HWND) {
    let Some(p) = APP.with_borrow(|a| a.files.get(a.idx).cloned()) else { return };
    let mut from = wide(&p.to_string_lossy());
    from.push(0); // the list ends with a double NUL
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    (op.hwnd, op.wFunc, op.pFrom, op.fFlags) = (hwnd, FO_DELETE, from.as_ptr(), FOF_ALLOWUNDO as u16);
    if unsafe { SHFileOperationW(&mut op) } == 0 && op.fAnyOperationsAborted == 0 {
        APP.with_borrow_mut(|a| {
            a.files.remove(a.idx);
            a.idx = a.idx.min(a.files.len().saturating_sub(1));
            a.img = None;
        });
        load(hwnd);
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        // the picture's area: the client below the caption strip
        let client = || {
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rc);
            (rc.right as f64, (rc.bottom - top(hwnd)) as f64)
        };
        if let Some(r) = winrust_common::caption::handle(hwnd, msg, wp, lp, CAP, |_, _| false) {
            return r;
        }
        match msg {
            WM_CREATE => {
                let dpi = GetDpiForSystem() as i32;
                let f = CreateFontW(-(18 * dpi / 96), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide(winrust_common::style::sans_face()).as_ptr());
                APP.with_borrow_mut(|a| (a.dpi, a.font) = (dpi, f as isize));
                DragAcceptFiles(hwnd, 1);
                0
            }
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_ERASEBKGND => 1,
            WM_SIZE => {
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_DROPFILES => {
                let d = wp as HDROP;
                let mut buf = [0u16; 1024];
                let n = DragQueryFileW(d, 0, buf.as_mut_ptr(), buf.len() as u32) as usize;
                DragFinish(d);
                open(hwnd, Path::new(&String::from_utf16_lossy(&buf[..n])));
                0
            }
            WM_IMG => {
                let (g, r) = *Box::from_raw(lp as *mut (u32, Result<decode::Img, String>));
                if g == GEN.load(Relaxed) {
                    APP.with_borrow_mut(|a| match r {
                        Ok(i) => (a.img, a.note) = (Some(i), String::new()),
                        Err(e) => (a.img, a.note) = (None, winrust_common::tf!("Не удалось открыть это изображение: {}", "Cannot open this picture: {}", ja: "この画像を開けません: {}", pt: "Não foi possível abrir esta imagem: {}", zh: "无法打开这张图片：{}", es: "No se puede abrir esta imagen: {}", e)),
                    });
                    InvalidateRect(hwnd, null(), 0);
                    title(hwnd);
                }
                0
            }
            WM_MOUSEWHEEL => {
                let mut p = POINT { x: (lp & 0xFFFF) as i16 as i32, y: (lp >> 16) as i16 as i32 };
                ScreenToClient(hwnd, &mut p);
                p.y -= top(hwnd);
                let (cw, ch) = client();
                let k = if (wp >> 16) as i16 > 0 { 1.15 } else { 1.0 / 1.15 };
                APP.with_borrow_mut(|a| {
                    if let (Some(i), Some((z, x, y))) = (&a.img, view(a, cw, ch)) {
                        let z2 = (z * k).clamp(0.02, 32.0);
                        // keep the image point under the cursor where it is
                        let (u, v) = ((p.x as f64 - x) / z, (p.y as f64 - y) / z);
                        a.off = (p.x as f64 - u * z2 - (cw - i.w as f64 * z2) / 2.0, p.y as f64 - v * z2 - (ch - i.h as f64 * z2) / 2.0);
                        a.zoom = Some(z2);
                    }
                });
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_LBUTTONDOWN => {
                SetCapture(hwnd);
                APP.with_borrow_mut(|a| a.drag = Some(((lp & 0xFFFF) as i16 as i32, (lp >> 16) as i16 as i32)));
                0
            }
            WM_MOUSEMOVE => {
                let (x, y) = ((lp & 0xFFFF) as i16 as i32, (lp >> 16) as i16 as i32);
                let moved = APP.with_borrow_mut(|a| {
                    let Some((px, py)) = a.drag else { return false };
                    a.off = (a.off.0 + (x - px) as f64, a.off.1 + (y - py) as f64);
                    a.drag = Some((x, y));
                    true
                });
                if moved {
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                APP.with_borrow_mut(|a| a.drag = None);
                0
            }
            WM_LBUTTONDBLCLK => {
                APP.with_borrow_mut(|a| (a.zoom, a.off) = (a.zoom.is_none().then_some(1.0), (0.0, 0.0)));
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_KEYDOWN => {
                match wp as u16 {
                    VK_ESCAPE => {
                        DestroyWindow(hwnd);
                    }
                    VK_RIGHT | VK_SPACE | VK_NEXT => go(hwnd, |i, n| (i + 1).min(n.saturating_sub(1))),
                    VK_LEFT | VK_BACK | VK_PRIOR => go(hwnd, |i, _| i.saturating_sub(1)),
                    VK_HOME => go(hwnd, |_, _| 0),
                    VK_END => go(hwnd, |_, n| n.saturating_sub(1)),
                    VK_DELETE => delete_current(hwnd),
                    0x52 => {
                        // R: a quarter turn right (Shift: left); the view fits again
                        let cw = GetKeyState(VK_SHIFT as i32) >= 0;
                        APP.with_borrow_mut(|a| {
                            if let Some(i) = a.img.as_ref() {
                                a.img = Some(decode::turned(i, cw));
                                (a.zoom, a.off, a.small) = (None, (0.0, 0.0), None);
                            }
                        });
                        InvalidateRect(hwnd, null(), 0);
                        title(hwnd);
                    }
                    k if k == b'1' as u16 || k == b'F' as u16 || k == b'0' as u16 => {
                        APP.with_borrow_mut(|a| (a.zoom, a.off) = (if k == b'1' as u16 { Some(1.0) } else { None }, (0.0, 0.0)));
                        InvalidateRect(hwnd, null(), 0);
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

fn main() {
    unsafe {
        decode::com_init();
        SetProcessDPIAware();
        let (inst, class) = (GetModuleHandleW(null()), wide("WinRustPhoto"));
        let wc = WNDCLASSW { style: CS_DBLCLKS, lpfnWndProc: Some(proc), cbClsExtra: 0, cbWndExtra: 0, hInstance: inst, hIcon: null_mut(), hCursor: LoadCursorW(null_mut(), IDC_ARROW), hbrBackground: null_mut(), lpszMenuName: null(), lpszClassName: class.as_ptr() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(0, class.as_ptr(), wide(winrust_common::t!("Просмотр фото", "Photo viewer", ja: "フォト ビューアー", pt: "Visualizador de fotos", zh: "图片查看器", es: "Visor de fotos")).as_ptr(), WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 1280, 800, null_mut(), null_mut(), inst, null());
        winrust_common::caption::install(hwnd);
        windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute(hwnd, 20, &1i32 as *const i32 as *const _, 4); // dark frame and window menu
        if let Some(p) = std::env::args_os().nth(1) {
            open(hwnd, Path::new(&p));
        }
        let mut m: MSG = std::mem::zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}
