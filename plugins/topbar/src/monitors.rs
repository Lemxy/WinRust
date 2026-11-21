//! One taskbar process per display. Only the primary process owns global hooks and the tray broker.
use super::*;
use winrust_common::screens::{Bounds, Screen};
thread_local! {
    static SCREEN: Cell<Option<Screen>> = const { Cell::new(None) };
    static PIN_STAMP: RefCell<Option<Vec<(PathBuf, std::time::SystemTime, u64)>>> = const { RefCell::new(None) };
    static DEVICE: RefCell<Option<String>> = const { RefCell::new(None) };
    static PARENT: Cell<isize> = const { Cell::new(0) };
    static CHILDREN: RefCell<Vec<(String, std::process::Child)>> = const { RefCell::new(Vec::new()) };
}
pub fn secondary() -> bool { DEVICE.with_borrow(|d| d.is_some()) }
pub fn configure() {
    let args: Vec<_> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--monitor") {
        DEVICE.set(args.get(i + 1).cloned());
        PARENT.set(args.get(i + 2).and_then(|a| a.parse().ok()).unwrap_or(0));
    }
}
pub fn displays() -> Vec<(String, Screen)> {
    unsafe extern "system" fn collect(m: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
        unsafe {
            let mut info: MONITORINFOEXW = zeroed();
            info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(m, &mut info as *mut _ as *mut MONITORINFO) != 0 {
                let b: Bounds = info.monitorInfo.rcMonitor.into();
                if let Some(s) = winrust_common::screens::all().into_iter().find(|s| s.bounds == b) {
                    let end = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
                    (*(data as *mut Vec<(String, Screen)>)).push((String::from_utf16_lossy(&info.szDevice[..end]), s));
                }
            }
        }
        1
    }
    let mut result = Vec::new();
    unsafe { EnumDisplayMonitors(null_mut(), null(), Some(collect), &mut result as *mut _ as isize); }
    result
}
pub fn update() { SCREEN.set(lookup()); }
pub fn current() -> Option<Screen> { SCREEN.get() }
fn lookup() -> Option<Screen> {
    DEVICE.with_borrow(|d| match d {
        Some(name) => displays().into_iter().find(|(n, _)| n == name).map(|(_, s)| s),
        None => Some(winrust_common::screens::primary()),
    })
}
pub fn bounds() -> Bounds { current().unwrap_or_else(winrust_common::screens::primary).bounds }
pub fn mutex_name() -> String {
    DEVICE.with_borrow(|d| match d { Some(d) => format!("Local\\topbar-monitor-{}", d.replace('\\', "_")), None => "Local\\topbar-single".into() })
}
unsafe fn sync_pins() {
    let dir = appdata_file(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar");
    let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e|e.path()).filter(|p|p.extension().is_some_and(|e|e.eq_ignore_ascii_case("lnk"))).collect();
    files.push(appdata_file("topbar-pin-order.txt")); files.sort();
    let stamp: Vec<_> = files.into_iter().filter_map(|p| { let m=p.metadata().ok()?;Some((p,m.modified().ok()?,m.len())) }).collect();
    let changed = PIN_STAMP.with_borrow_mut(|last| { let changed=last.as_ref().is_some_and(|old|*old!=stamp);*last=Some(stamp);changed });
    if changed {
        previews::close();
        PINS.with_borrow_mut(|p| { for pin in p.drain(..) { if !pin.icon.is_null() { DestroyIcon(pin.icon); } } });
        load_pins(); refresh_buttons();
        InvalidateRect(BAR.load(Relaxed) as HWND, null(), 0);
    }
}
pub unsafe fn reconcile() {
    sync_pins();
    if secondary() {
        update();
        if current().is_none_or(|s| s.primary) || IsWindow(PARENT.get() as HWND) == 0 { PostMessageW(BAR.load(Relaxed) as HWND, WM_CLOSE, 0, 0); }
        return;
    }
    let names: Vec<_> = displays().into_iter().filter(|(_, s)| !s.primary).map(|(n, _)| n).collect();
    CHILDREN.with_borrow_mut(|children| {
        children.retain_mut(|(name, child)| {
            // Disconnected children close themselves and return their appbar registration.
            names.contains(name) && child.try_wait().ok().flatten().is_none()
        });
        for name in names {
            if !children.iter().any(|(n, _)| *n == name) {
                if let Ok(exe) = std::env::current_exe() {
                    if let Ok(child) = Command::new(exe).arg("--monitor").arg(&name).arg(BAR.load(Relaxed).to_string()).creation_flags(NO_WINDOW).spawn() {
                        children.push((name, child));
                    }
                }
            }
        }
    });
}
pub unsafe fn stop_children() {
    // The parent HWND is the lifetime token. Children detect its destruction without relying on Drop after a crash.
    let mut h = null_mut();
    loop {
        h = FindWindowExW(null_mut(), h, wide("topbar-secondary").as_ptr(), null());
        if h.is_null() { break; }
        let mut pid = 0; GetWindowThreadProcessId(h, &mut pid);
        if CHILDREN.with_borrow(|c| c.iter().any(|(_, child)| child.id() == pid)) { SendMessageW(h, WM_CLOSE, 0, 0); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secondary_geometry_uses_its_own_origin_and_dpi() {
        let _serial = NATIVE_TEST.lock().unwrap();
        let old = current();
        let b = Bounds { left: -2560, top: -200, right: 0, bottom: 1240 };
        SCREEN.set(Some(Screen { bounds:b, work:b, dpi:144, primary:false }));
        DEVICE.set(Some("test-display".into()));
        let scale = SCALE.replace(1.5);
        unsafe {
            assert_eq!(bar_x_w(), (-2560,2560));
            assert_eq!(bar_top(), 1180);
            assert_eq!(pop_y(300), 868);
            assert_eq!(axis_len(), 2560);
        }
        assert_eq!(tray_count(), 0);
        assert_ne!(mutex_name(), "Local\\topbar-single");
        DEVICE.set(None); SCREEN.set(old); SCALE.set(scale);
    }
    #[test]
    fn real_displays_have_unique_stable_names() {
        let screens = displays(); assert!(!screens.is_empty());
        for (i,(name,s)) in screens.iter().enumerate() {
            assert!(!name.is_empty()); assert!(screens[..i].iter().all(|(n,_)| n != name));
            assert!(s.bounds.width()>0 && s.bounds.height()>0);
        }
    }
}
