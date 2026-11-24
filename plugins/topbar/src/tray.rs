//! topbar: tray (split out of main.rs; shares its items through `super::*`).
use super::*;

/// First top-level window of the class that is not our own tray host (explorer's real taskbar).
pub(crate) unsafe fn find_other(class: &str) -> HWND {
    let (c, mut h) = (wide(class), null_mut());
    loop {
        h = FindWindowExW(null_mut(), h, c.as_ptr(), null());
        if h.is_null() || h != HOST.get() {
            return h;
        }
    }
}

pub(crate) unsafe fn tray() -> [HWND; 2] {
    [find_other("Shell_TrayWnd"), find_other("Shell_SecondaryTrayWnd")]
}

/// `icon` at exactly `px` pixels, the size it is drawn at: DrawIconEx stretches by dropping or doubling pixels, so a 16 px
/// icon drawn at 20 blurs and a 32 px one drawn at 16 frays. One already that size comes back as it is; any other is
/// resampled once (then the original is freed) and only if that fails is it kept to be stretched.
pub(crate) unsafe fn sharp(icon: HICON, px: i32) -> HICON {
    let mut ii: ICONINFO = zeroed();
    if icon.is_null() || GetIconInfo(icon, &mut ii) == 0 {
        return icon;
    }
    let mut b: BITMAP = zeroed();
    let have = !ii.hbmColor.is_null() && GetObjectW(ii.hbmColor, size_of::<BITMAP>() as i32, (&mut b as *mut BITMAP).cast()) != 0;
    let out = if have && b.bmWidth > 0 && b.bmHeight > 0 && (b.bmWidth, b.bmHeight) != (px, px) { resample(&ii, b.bmWidth, b.bmHeight, px) } else { null_mut() };
    DeleteObject(ii.hbmColor);
    DeleteObject(ii.hbmMask);
    if out.is_null() {
        return icon; // the right size already, a black-and-white icon, or GDI+ said no
    }
    DestroyIcon(icon);
    out
}

/// The colour bitmap of an icon (`w` x `h`), resampled to `px` x `px` by GDI+ (bicubic, alpha-aware), as a new icon.
unsafe fn resample(ii: &ICONINFO, w: i32, h: i32, px: i32) -> HICON {
    use windows_sys::Win32::Graphics::GdiPlus::{GdipCreateBitmapFromScan0, GdipDisposeImage, GdipDrawImageRectI, GdipGetImageGraphicsContext, GdipSetInterpolationMode, GpImage, InterpolationModeHighQualityBicubic};
    let sdc = GetDC(null_mut());
    let read = |bmp: HBITMAP| {
        let mut bi: BITMAPINFO = zeroed();
        bi.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..zeroed() };
        let mut v = vec![0u32; (w * h) as usize];
        GetDIBits(sdc, bmp, 0, h as u32, v.as_mut_ptr().cast(), &mut bi, DIB_RGB_COLORS);
        v
    };
    let mut src = read(ii.hbmColor);
    if src.iter().all(|p| p >> 24 == 0) {
        // an old-style icon without alpha: the mask says what is see-through (white) and what is not
        let mask = read(ii.hbmMask);
        for (p, m) in src.iter_mut().zip(mask) {
            *p = if m & 0x00FF_FFFF != 0 { 0 } else { *p | 0xFF00_0000 };
        }
    }
    let Some((color, bits)) = glass::dib(sdc, px, px) else {
        ReleaseDC(null_mut(), sdc);
        return null_mut();
    };
    ReleaseDC(null_mut(), sdc);
    // GDI+ draws straight into the icon's own pixels
    let (mut from, mut to, mut g) = (null_mut(), null_mut(), null_mut());
    let ok = GdipCreateBitmapFromScan0(w, h, w * 4, glass::ARGB32, src.as_mut_ptr().cast(), &mut from) == 0
        && GdipCreateBitmapFromScan0(px, px, px * 4, glass::ARGB32, bits.cast(), &mut to) == 0
        && GdipGetImageGraphicsContext(to as *mut GpImage, &mut g) == 0;
    if ok {
        GdipSetInterpolationMode(g, InterpolationModeHighQualityBicubic);
        GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
        GdipDrawImageRectI(g, from as *mut GpImage, 0, 0, px, px);
    }
    if !g.is_null() {
        GdipDeleteGraphics(g); // flushes into the icon's pixels
    }
    [from, to].into_iter().filter(|b| !b.is_null()).for_each(|b| {
        GdipDisposeImage(b as *mut GpImage);
    });
    let mask = CreateBitmap(px, px, 1, 1, null()); // all zero: the colour bitmap's alpha decides
    let mut ni = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
    let ic = if ok { CreateIconIndirect(&mut ni) } else { null_mut() };
    DeleteObject(mask);
    DeleteObject(color);
    ic
}

/// Tells every app to re-register its tray icon with whichever Shell_TrayWnd is found first.
pub(crate) unsafe fn announce() {
    PostMessageW(HWND_BROADCAST, RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), 0, 0);
}

pub(crate) fn hide_onedrive() -> bool {
    // topbar.cfg sits next to the exe (the launcher writes it); no file or no line = OneDrive hidden
    std::env::current_exe().ok().and_then(|e| std::fs::read_to_string(e.with_file_name("topbar.cfg")).ok()).is_none_or(|c| !c.contains("hide_onedrive=0"))
}

/// Explorer's volume, network and power icons ({7820AE73/74/75-23E3-4229-82C1-E41CB67D5B9C}): the bar draws those itself.
/// Its other icons (printer queue, Safely Remove Hardware, microphone in use) stay (#127).
fn own_icon(guid: &[u8; 16]) -> bool {
    matches!(guid[0], 0x73..=0x75) && guid[1..] == [0xAE, 0x20, 0x78, 0xE3, 0x23, 0x29, 0x42, 0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C]
}

/// The installer's OneDrive switch moved (it sends the style broadcast): the icon goes, or is asked back, right away
/// instead of at the next sign-in.
pub(crate) unsafe fn onedrive_recheck() {
    thread_local! { static LAST: Cell<Option<bool>> = const { Cell::new(None) }; }
    let hide = hide_onedrive();
    if LAST.replace(Some(hide)).is_none_or(|was| was == hide) {
        return; // the first look (at start), or no change: every other broadcast (theme, language) ends here
    }
    if hide {
        TAPPS.with_borrow_mut(|t| {
            t.retain(|x| x.key != "onedrive.exe" || {
                DestroyIcon(x.icon);
                false
            })
        });
        InvalidateRect(BAR.load(Relaxed) as HWND, null(), 0);
    } else {
        announce(); // OneDrive registers its icon again, with us this time
    }
}

/// WM_COPYDATA (dwData 1) from Shell_NotifyIcon: u32 magic, u32 NIM_*, then the platform-neutral NOTIFYICONDATA
/// (32-bit handles): hWnd@12 uID@16 uFlags@20 callback@24 hIcon@28 dwState@288 mask@292 version@808 guid@944.
pub(crate) unsafe fn tray_msg(d: &[u8]) {
    let u = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let (op, flags, uid) = (u(4), u(20), u(16));
    let h = u(12) as i32 as isize as HWND;
    let guid: [u8; 16] = if flags & 0x20 != 0 { d[944..960].try_into().unwrap() } else { [0; 16] };
    TAPPS.with_borrow_mut(|t| {
        let pos = t.iter().position(|x| x.hwnd == h && x.uid == uid && x.guid == guid);
        match (op, pos) {
            (2, Some(i)) => {
                DestroyIcon(t.remove(i).icon);
            }
            (4, Some(i)) => t[i].ver = u(808),
            (0 | 1, _) => {
                let i = match pos {
                    Some(i) => i,
                    None if op == 1 => return, // modify of an icon we never saw
                    None => {
                        let exe = exe_of(h);
                        let n = exe.rsplit('\\').next().unwrap_or("");
                        if (n == "explorer.exe" && own_icon(&guid)) || n == "topbar.exe" || (n == "onedrive.exe" && hide_onedrive()) {
                            return; // the system's own icons are drawn by the bar itself
                        }
                        t.push(TIcon { hwnd: h, uid, guid, icon: null_mut(), cb: 0, ver: 0, hidden: false, tip: String::new(), key: n.to_string() });
                        t.len() - 1
                    }
                };
                let x = &mut t[i];
                if flags & 1 != 0 {
                    x.cb = u(24);
                }
                if flags & 2 != 0 {
                    let ic = sharp(CopyIcon(u(28) as i32 as isize as HICON), sc(16));
                    if !ic.is_null() {
                        if !x.icon.is_null() {
                            DestroyIcon(x.icon);
                        }
                        x.icon = ic;
                    }
                }
                if flags & 4 != 0 {
                    let w: Vec<u16> = d[32..288].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
                    x.tip = String::from_utf16_lossy(&w);
                }
                if flags & 8 != 0 && u(292) & 1 != 0 {
                    x.hidden = u(288) & 1 != 0; // NIS_HIDDEN
                }
            }
            _ => {}
        }
    });
}

pub(crate) unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA {
        let c = &*(lp as *const Cds);
        if c.dw == 1 && c.cb >= 964 {
            tray_msg(std::slice::from_raw_parts(c.p, c.cb as usize));
            return 1;
        }
        // SHAppBarMessage (our own bar's work-area reservation, ABM_SETSTATE...) also goes to "Shell_TrayWnd": hand it to explorer's
        // with a timeout: a hung explorer must not freeze the bar
        let mut r = 0usize;
        SendMessageTimeoutW(find_other("Shell_TrayWnd"), msg, wp, lp, SMTO_ABORTIFHUNG, 2000, &mut r);
        return r as LRESULT;
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

thread_local! {
    static LAST_CLICK: Cell<(usize, u32, u32)> = const { Cell::new((0, 0, 0)) }; // (icon window, icon id, tick) of the last left click
}

/// Posts the same messages explorer would for a click on the icon; the app shows its own window or menu.
pub(crate) unsafe fn tray_click(t: (HWND, u32, u32, u32), left: bool) {
    let (hwnd, uid, cb, ver) = t;
    if cb == 0 {
        return;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    AllowSetForegroundWindow(pid); // so the app can bring its popup menu to the front
    let mut pt: POINT = zeroed();
    GetCursorPos(&mut pt);
    let post = |ev: u32| {
        if ver >= 4 {
            PostMessageW(hwnd, cb, ((pt.x as u32 & 0xffff) | (pt.y as u32) << 16) as usize, (ev | (uid & 0xffff) << 16) as isize);
        } else {
            PostMessageW(hwnd, cb, uid as usize, ev as isize);
        }
    };
    // what Explorer sends: the raw button messages always, plus NIN_SELECT / WM_CONTEXTMENU for version 4. Many apps open
    // only on the button-up or the double click (Steam), so NIN_SELECT alone left them dead (#115).
    let now = windows_sys::Win32::System::SystemInformation::GetTickCount();
    let prev = if left { LAST_CLICK.replace((hwnd as usize, uid, now)) } else { (0, 0, 0) };
    let double = left && (prev.0, prev.1) == (hwnd as usize, uid) && now.wrapping_sub(prev.2) <= GetDoubleClickTime();
    if double {
        LAST_CLICK.set((0, 0, 0)); // a third click starts a new pair
    }
    match (left, ver >= 4) {
        (true, v4) => {
            [WM_LBUTTONDOWN, WM_LBUTTONUP].into_iter().for_each(post);
            if v4 { post(0x400); } // NIN_SELECT
            if double { post(WM_LBUTTONDBLCLK); }
        }
        (false, v4) => {
            [WM_RBUTTONDOWN, WM_RBUTTONUP].into_iter().for_each(post);
            if v4 { post(WM_CONTEXTMENU); }
        }
    }
}

/// Indices into TAPPS of the icons kept on the bar; the rest live in the chevron flyout ("every icon" keeps none there).
pub(crate) fn bar_idx() -> Vec<usize> {
    let shown = SHOWN.with_borrow(|s| s.clone());
    let all = i18n::style::tray_all();
    TAPPS.with_borrow(|t| t.iter().enumerate().filter(|(_, x)| !x.hidden && (all || shown.as_ref().is_some_and(|s| s.contains(&x.key)))).map(|(i, _)| i).collect())
}

/// Windows 11 keeps a "show on the taskbar" choice per icon (Settings, Personalization, Taskbar, other system tray icons):
/// the bar starts from it until the user makes choices of their own here. Exe names, lowercase, as `TIcon::key`.
/// Windows 10 has only the global "Always show all icons", which `style::tray_all` follows.
pub(crate) fn windows_promoted() -> Option<Vec<String>> {
    if !winrust_common::win11() {
        return None;
    }
    use winrust_common::reg;
    const KEY: &str = r"HKCU\Control Panel\NotifyIconSettings";
    let shown: Vec<String> = reg::subkeys(KEY)
        .into_iter()
        .map(|id| format!(r"{KEY}\{id}"))
        .filter(|k| reg::get_dword(k, "IsPromoted") == Some(1))
        .filter_map(|k| reg::get_str(&k, "ExecutablePath"))
        .filter_map(|p| p.rsplit('\\').next().map(str::to_lowercase))
        .collect();
    (!shown.is_empty()).then_some(shown)
}
/// Bar slots of the tray: the chevron plus the icons kept on the bar.
pub(crate) fn tray_count() -> usize {
    bar_idx().len() + 1
}

/// Indices into TAPPS of the icons only the flyout shows (everything not kept on the bar).
pub(crate) fn over_idx() -> Vec<usize> {
    let bar = bar_idx();
    TAPPS.with_borrow(|t| (0..t.len()).filter(|i| !t[*i].hidden && !bar.contains(i)).collect())
}

pub(crate) fn toggle_shown(key: &str) {
    let mut cur = SHOWN.with_borrow(|s| s.clone()).unwrap_or_else(|| TAPPS.with_borrow(|t| bar_idx().into_iter().map(|i| t[i].key.clone()).collect()));
    match cur.iter().position(|k| k == key) {
        Some(p) => drop(cur.remove(p)),
        None => cur.push(key.to_string()),
    }
    let _ = std::fs::write(appdata_file("topbar-tray.txt"), cur.join("\n"));
    SHOWN.set(Some(cur));
}

// ---------- tray overflow flyout (the chevron): every icon, click / right click work as in the real tray ----------

#[cfg(test)]
mod tests {
    use super::*;

    /// A `n` px icon: an opaque red square with a clear 1/4 border.
    unsafe fn red_square(n: i32) -> HICON {
        let dc = GetDC(null_mut());
        let (color, px) = glass::dib(dc, n, n).unwrap();
        ReleaseDC(null_mut(), dc);
        let p = std::slice::from_raw_parts_mut(px, (n * n) as usize);
        for y in 0..n {
            for x in 0..n {
                let inside = (n / 4..n - n / 4).contains(&x) && (n / 4..n - n / 4).contains(&y);
                p[(y * n + x) as usize] = if inside { 0xFFFF_0000 } else { 0 };
            }
        }
        let mask = CreateBitmap(n, n, 1, 1, null());
        let mut ii = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let ic = CreateIconIndirect(&mut ii);
        DeleteObject(mask);
        DeleteObject(color);
        ic
    }

    unsafe fn pixels(ic: HICON) -> (i32, Vec<u32>) {
        let mut ii: ICONINFO = zeroed();
        GetIconInfo(ic, &mut ii);
        let mut b: BITMAP = zeroed();
        GetObjectW(ii.hbmColor, size_of::<BITMAP>() as i32, (&mut b as *mut BITMAP).cast());
        let n = b.bmWidth;
        let mut bi: BITMAPINFO = zeroed();
        bi.bmiHeader = BITMAPINFOHEADER { biSize: size_of::<BITMAPINFOHEADER>() as u32, biWidth: n, biHeight: -n, biPlanes: 1, biBitCount: 32, ..zeroed() };
        let mut v = vec![0u32; (n * n) as usize];
        let dc = GetDC(null_mut());
        GetDIBits(dc, ii.hbmColor, 0, n as u32, v.as_mut_ptr().cast(), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(null_mut(), dc);
        DeleteObject(ii.hbmColor);
        DeleteObject(ii.hbmMask);
        (n, v)
    }

    #[test]
    fn tray_icons_come_out_at_the_size_they_are_drawn() {
        unsafe {
            let (mut tok, mut inp): (usize, GdiplusStartupInput) = (0, zeroed());
            inp.GdiplusVersion = 1;
            GdiplusStartup(&mut tok, &inp, null_mut());
            let small = red_square(16);
            assert_eq!(sharp(small, 16), small); // already right: left alone
            let big = sharp(red_square(32), 16);
            let (n, p) = pixels(big);
            assert_eq!(n, 16);
            assert_eq!(p[8 * 16 + 8], 0xFFFF_0000); // the middle stays solid red
            assert_eq!(p[0] >> 24, 0); // the corner stays clear
            DestroyIcon(small);
            DestroyIcon(big);
        }
    }
}
