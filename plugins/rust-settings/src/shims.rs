//! What Settings needs from the rest of the desktop, as small stand-ins: in WinRust the taskbar, file
//! manager and launcher are their own programs, so these open Windows' (or WinRust's) tools instead.

pub mod launcher {
    use windows::core::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT;

    /// No replacement apps to route to here: links open what Windows has registered.
    pub unsafe fn open_rust(_target: &str) -> bool { false }
    /// No keyboard hook of its own.
    pub unsafe fn on_key(_k: &KBDLLHOOKSTRUCT, _up: bool) -> bool { false }
    pub unsafe fn inject(keys: &[(u16, bool)]) {
        let v: Vec<INPUT> = keys.iter().map(|&(vk, up)| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() } } }).collect();
        SendInput(&v, size_of::<INPUT>() as i32);
    }
    pub fn display_name() -> String {
        unsafe {
            use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
            let mut buf = [0u16; 256];
            let mut n = buf.len() as u32;
            if GetUserNameExW(NameDisplay, Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut n) && n > 0 {
                return String::from_utf16_lossy(&buf[..n as usize]);
            }
        }
        std::env::var("USERNAME").unwrap_or_default()
    }
    pub unsafe fn icon_pixels(wic: &IWICImagingFactory, parsing: &str, px: u32) -> windows::core::Result<Vec<u8>> {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(parsing), None)?;
        let f: IShellItemImageFactory = item.cast()?;
        let ask = if px <= 32 { 48 } else { px as i32 }; // bigger requests make Windows frame small icons
        let hbm = f.GetImage(SIZE { cx: ask, cy: ask }, SIIGBF_ICONONLY)?;
        let bmp = wic.CreateBitmapFromHBITMAP(hbm, HPALETTE::default(), WICBitmapUsePremultipliedAlpha);
        let _ = DeleteObject(hbm.into());
        let conv = wic.CreateFormatConverter()?;
        conv.Initialize(&bmp?, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let (mut cw, mut ch) = (0, 0);
        conv.GetSize(&mut cw, &mut ch)?;
        let mut big = vec![0u8; (cw * ch * 4) as usize];
        conv.CopyPixels(std::ptr::null(), cw * 4, &mut big)?;
        let (mut x0, mut y0, mut x1, mut y1) = (cw, ch, 0, 0);
        for y in 0..ch {
            for x in 0..cw {
                if big[((y * cw + x) * 4 + 3) as usize] > 8 {
                    (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
                }
            }
        }
        let src = wic.CreateBitmapFromMemory(cw, ch, &GUID_WICPixelFormat32bppPBGRA, cw * 4, &big)?;
        let side = x1.saturating_sub(x0).max(y1.saturating_sub(y0));
        let r = if side > 0 && side < cw * 3 / 4 {
            // old apps ship small icons that Windows centres on an empty 256px canvas
            let side = (side + side / 8).min(cw.min(ch));
            let cx = ((x0 + x1) / 2).saturating_sub(side / 2).min(cw - side);
            let cy = ((y0 + y1) / 2).saturating_sub(side / 2).min(ch - side);
            WICRect { X: cx as i32, Y: cy as i32, Width: side as i32, Height: side as i32 }
        } else {
            WICRect { X: 0, Y: 0, Width: cw as i32, Height: ch as i32 }
        };
        let clip = wic.CreateBitmapClipper()?;
        clip.Initialize(&src, &r)?;
        let scale = wic.CreateBitmapScaler()?;
        scale.Initialize(&clip, px, px, WICBitmapInterpolationModeHighQualityCubic)?;
        let mut out = vec![0u8; (px * px * 4) as usize];
        scale.CopyPixels(std::ptr::null(), px * 4, &mut out)?;
        Ok(out)
    }
}

pub mod files {
    use windows::core::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;
    use windows::Win32::UI::Shell::*;
    /// A folder or drive: open it in the default file manager (rust-explorer when WinRust has it).
    pub fn request(path: &str) {
        unsafe { let _ = ShellExecuteW(None, w!("open"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL); }
    }
    pub fn glide(at: f32, goal: f32) -> (f32, bool) {
        let next = at + (goal - at) * 0.35;
        if (goal - next).abs() < 0.5 { (goal, true) } else { (next, false) }
    }
    pub unsafe fn image(path: &str, side: i32, flags: SIIGBF) -> Option<(u32, u32, Vec<u8>)> {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        let f: IShellItemImageFactory = windows::core::Interface::cast(&item).ok()?;
        let hbm = f.GetImage(SIZE { cx: side, cy: side }, flags).ok()?;
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let bmp = wic.CreateBitmapFromHBITMAP(hbm, HPALETTE::default(), WICBitmapUsePremultipliedAlpha);
        let _ = DeleteObject(hbm.into());
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&bmp.ok()?, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (mut w_, mut h_) = (0, 0);
        conv.GetSize(&mut w_, &mut h_).ok()?;
        let mut px = vec![0u8; (w_ * h_ * 4) as usize];
        conv.CopyPixels(std::ptr::null(), w_ * 4, &mut px).ok()?;
        // pictures without transparency come back with an empty alpha channel: make those opaque. A
        // picture that is all zeros has nothing in it at all (fully transparent) and is left that way,
        // instead of being turned into a black square.
        if px.chunks(4).all(|p| p[3] == 0) && px.iter().any(|v| *v != 0) {
            px.chunks_mut(4).for_each(|p| p[3] = 255);
        }
        Some((w_, h_, px))
    }
}

pub mod photos {
    pub fn parse_place(t: &str) -> Option<(i32, i32, i32, i32, bool)> {
        let v: Vec<i32> = t.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        (v.len() == 5 && v[2] >= 300 && v[3] >= 200).then(|| (v[0], v[1], v[2], v[3], v[4] == 1))
    }
    pub fn write_bmp(out: &str, w: i32, h: i32, px: &[u8]) {
        let mut o = Vec::with_capacity(54 + px.len());
        o.extend_from_slice(b"BM");
        o.extend_from_slice(&((54 + px.len()) as u32).to_le_bytes());
        o.extend_from_slice(&[0; 4]);
        o.extend_from_slice(&54u32.to_le_bytes());
        o.extend_from_slice(&40u32.to_le_bytes());
        o.extend_from_slice(&w.to_le_bytes());
        o.extend_from_slice(&(-h).to_le_bytes());
        o.extend_from_slice(&1u16.to_le_bytes());
        o.extend_from_slice(&32u16.to_le_bytes());
        o.extend_from_slice(&[0; 24]);
        px.chunks(4).for_each(|p| o.extend_from_slice(&[p[0], p[1], p[2], 255]));
        let _ = std::fs::write(out, o);
    }
}

pub mod notify {
    use std::ffi::c_void;
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryWnfStateData(name: *const u64, ty: *const c_void, scope: *const c_void, stamp: *mut u32, buf: *mut c_void, size: *mut u32) -> i32;
        fn NtUpdateWnfStateData(name: *const u64, buf: *const c_void, len: u32, ty: *const c_void, scope: *const c_void, matching: u32, check: u32) -> i32;
    }
    /// The shell's quiet-hours profile (0 off, 1 priority only = Do Not Disturb, 2 alarms only).
    const WNF_QUIET_HOURS: u64 = 0x0D83_063E_A3BF_1C75;
    
    pub fn dnd_get() -> bool {
        let (mut v, mut size, mut stamp) = (0u32, 4u32, 0u32);
        unsafe { NtQueryWnfStateData(&WNF_QUIET_HOURS, std::ptr::null(), std::ptr::null(), &mut stamp, &mut v as *mut _ as *mut c_void, &mut size) };
        v != 0
    }
    
    pub fn dnd_set(on: bool) {
        let v: u32 = on as u32;
        unsafe { NtUpdateWnfStateData(&WNF_QUIET_HOURS, &v as *const _ as *const c_void, 4, std::ptr::null(), std::ptr::null(), 0, 0) };
    }
}

/// Rust UI's own shell switch is not part of WinRust: reported as not installed, and a no-op.
pub mod session {
    pub fn installed() -> bool { false }
    pub fn install() -> windows::core::Result<()> { Ok(()) }
    pub fn restore() -> windows::core::Result<()> { Ok(()) }
}

pub mod wallpaper {
    pub fn video_path() -> Option<std::path::PathBuf> { None }
}

pub mod taskbar {
    use std::ffi::c_void;
    use windows::core::w;
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Registry::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    const STUCK: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StuckRects3");

    /// "Automatically hide the taskbar": bit 0 of byte 8 in Explorer's StuckRects3 record.
    pub fn auto_hide() -> bool {
        stuck_blob().is_some_and(|b| b.len() > 8 && b[8] & 1 != 0)
    }

    fn stuck_blob() -> Option<Vec<u8>> {
        let mut buf = vec![0u8; 256];
        let mut n = buf.len() as u32;
        unsafe { RegGetValueW(HKEY_CURRENT_USER, STUCK, w!("Settings"), RRF_RT_REG_BINARY, None, Some(buf.as_mut_ptr() as *mut c_void), Some(&mut n)).ok().ok()?; }
        buf.truncate(n as usize);
        Some(buf)
    }

    pub fn set_auto_hide(on: bool) {
        let Some(mut b) = stuck_blob().filter(|b| b.len() > 8) else { return };
        b[8] = if on { b[8] | 1 } else { b[8] & !1 };
        unsafe {
            let mut k = HKEY::default();
            if RegCreateKeyW(HKEY_CURRENT_USER, STUCK, &mut k).is_ok() {
                let _ = RegSetValueExW(k, w!("Settings"), None, REG_BINARY, Some(&b));
                let _ = RegCloseKey(k);
            }
            // the taskbar re-reads it when told the tray settings changed
            let _ = SendNotifyMessageW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(w!("TraySettings").as_ptr() as isize));
        }
    }
}
