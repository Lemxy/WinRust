//! UI-thread-owned GDI resources. Reuse them between animation frames.
use super::*;

struct Surface { dc: HDC, bitmap: HBITMAP, original: HGDIOBJ, width: i32, height: i32 }
impl Drop for Surface {
    fn drop(&mut self) { unsafe {
        SelectObject(self.dc, self.original);
        DeleteObject(self.bitmap);
        DeleteDC(self.dc);
    }}
}

thread_local! {
    static SURFACE: RefCell<Option<Surface>> = const { RefCell::new(None) };
    static FONTS: RefCell<HashMap<(i32, i32, String), HFONT>> = RefCell::new(HashMap::new());
}

pub unsafe fn surface(target: HDC, width: i32, height: i32) -> HDC {
    SURFACE.with_borrow_mut(|slot| {
        if let Some(s) = slot.as_ref() {
            if s.width >= width && s.height >= height { return s.dc; }
        }
        let width = width.max(1).max(slot.as_ref().map_or(0, |s| s.width));
        let height = height.max(1).max(slot.as_ref().map_or(0, |s| s.height));
        let dc = CreateCompatibleDC(target);
        let bitmap = CreateCompatibleBitmap(target, width, height);
        if dc.is_null() || bitmap.is_null() {
            if !dc.is_null() { DeleteDC(dc); }
            if !bitmap.is_null() { DeleteObject(bitmap); }
            return null_mut();
        }
        let original = SelectObject(dc, bitmap);
        *slot = Some(Surface { dc, bitmap, original, width, height });
        dc
    })
}

pub unsafe fn font(px: i32, weight: i32, face: &str) -> HFONT {
    FONTS.with_borrow_mut(|fonts| *fonts.entry((sc(px), weight, face.into())).or_insert_with(|| {
        CreateFontW(-sc(px), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(face).as_ptr())
    }))
}

pub fn clear() {
    SURFACE.set(None);
    FONTS.with_borrow_mut(|fonts| { for (_, f) in fonts.drain() { unsafe { DeleteObject(f); } } });
}

pub unsafe fn drawable(h: HWND) -> bool {
    let mut r: RECT = zeroed();
    IsWindowVisible(h) != 0 && IsIconic(h) == 0 && GetClientRect(h, &mut r) != 0 && r.right > 0 && r.bottom > 0
}

pub unsafe fn schedule(h: HWND, busy: bool) {
    if busy && drawable(h) { SetTimer(h, 4, 16, None); } else { KillTimer(h, 4); }
}

pub unsafe fn tick(h: HWND) {
    // If no paint follows (hidden/occluded window), do not keep waking the UI.
    KillTimer(h, 4);
    if drawable(h) { InvalidateRect(h, null(), 0); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resources_are_reused_resized_and_released() { unsafe {
        let screen = GetDC(null_mut());
        let first = surface(screen, 200, 60);
        assert!(!first.is_null());
        let bitmap = SURFACE.with_borrow(|s| s.as_ref().unwrap().bitmap);
        assert_eq!(surface(screen, 190, 50), first);
        let f = font(13, 400, ui_face());
        assert!(!f.is_null());
        assert_eq!(font(13, 400, ui_face()), f);
        assert!(!surface(screen, 400, 80).is_null());
        let mut info: BITMAP = zeroed();
        assert_eq!(GetObjectW(bitmap, size_of::<BITMAP>() as i32, &mut info as *mut _ as _), 0);
        let bitmap = SURFACE.with_borrow(|s| s.as_ref().unwrap().bitmap);
        clear();
        assert_eq!(GetObjectW(bitmap, size_of::<BITMAP>() as i32, &mut info as *mut _ as _), 0);
        let mut lf: LOGFONTW = zeroed();
        assert_eq!(GetObjectW(f, size_of::<LOGFONTW>() as i32, &mut lf as *mut _ as _), 0);
        ReleaseDC(null_mut(), screen);
    }}

    #[test]
    fn hidden_animation_stops_without_a_paint() { unsafe {
        let h = CreateWindowExW(WS_EX_TOOLWINDOW, wide("STATIC").as_ptr(), null(), WS_POPUP, 0, 0, 100, 50, null_mut(), null_mut(), GetModuleHandleW(null()), null());
        assert!(!h.is_null());
        SetTimer(h, 4, 16, None);
        tick(h);
        schedule(h, true);
        thread::sleep(std::time::Duration::from_millis(70));
        let mut msg: MSG = zeroed();
        assert_eq!(PeekMessageW(&mut msg, h, WM_TIMER, WM_TIMER, PM_REMOVE), 0);
        DestroyWindow(h);
    }}
}
