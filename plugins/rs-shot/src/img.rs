//! A plain top-down 32-bit BGRA image. Everything that leaves the screen or the editor is one of these; the file and
//! clipboard writers turn it into a 24-bit DIB so no alpha channel confuses GDI+ or other applications.
use std::ptr::null_mut;

use windows_sys::Win32::{
    Foundation::{HBITMAP, RECT},
    Graphics::Gdi::{
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BitBlt, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC,
        GetDIBits, GetObjectW, HDC, HGDIOBJ, ReleaseDC, SRCCOPY, SelectObject,
    },
};

#[derive(Clone)]
pub struct Image {
    pub w: i32,
    pub h: i32,
    /// Top-down BGRA, stride = w * 4.
    pub px: Vec<u8>,
}

impl Image {
    pub fn new(w: i32, h: i32) -> Image {
        Image { w: w.max(1), h: h.max(1), px: vec![0; (w.max(1) * h.max(1) * 4) as usize] }
    }

    pub fn valid(&self) -> bool {
        self.w > 0 && self.h > 0 && self.px.len() >= (self.w * self.h * 4) as usize
    }

    pub fn at(&self, x: i32, y: i32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }

    /// Copies an HBITMAP (any depth) into a top-down BGRA buffer.
    pub unsafe fn from_hbitmap(hbm: HBITMAP) -> Option<Image> {
        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(hbm as HGDIOBJ, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut _) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut img = Image::new(w, h);
        let dc = GetDC(null_mut());
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h; // top-down
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        let ok = GetDIBits(dc, hbm, 0, h as u32, img.px.as_mut_ptr() as *mut _, &mut bi, DIB_RGB_COLORS);
        ReleaseDC(null_mut(), dc);
        (ok != 0).then_some(img)
    }

    pub fn crop(&self, r: RECT) -> Image {
        let l = r.left.max(0).min(self.w);
        let t = r.top.max(0).min(self.h);
        let rr = r.right.max(l).min(self.w);
        let b = r.bottom.max(t).min(self.h);
        let (w, h) = (rr - l, b - t);
        let mut out = Image::new(w, h);
        for y in 0..h {
            let src = ((t + y) * self.w + l) as usize * 4;
            let dst = (y * w) as usize * 4;
            out.px[dst..dst + (w as usize) * 4].copy_from_slice(&self.px[src..src + (w as usize) * 4]);
        }
        out
    }

    /// A 24-bit top-down DIB section, the shape GDI+ and CF_DIB both like. The caller deletes the bitmap.
    pub unsafe fn to_dib24(&self) -> Option<HBITMAP> {
        let dc = GetDC(null_mut());
        let (src, src_bits) = make_dib(dc, self.w, self.h, 32)?;
        let (dst, _) = make_dib(dc, self.w, self.h, 24)?;
        std::ptr::copy_nonoverlapping(self.px.as_ptr(), src_bits, self.px.len());
        let sdc = CreateCompatibleDC(dc);
        let ddc = CreateCompatibleDC(dc);
        let so = SelectObject(sdc, src as HGDIOBJ);
        let do_ = SelectObject(ddc, dst as HGDIOBJ);
        BitBlt(ddc, 0, 0, self.w, self.h, sdc, 0, 0, SRCCOPY);
        SelectObject(sdc, so);
        SelectObject(ddc, do_);
        DeleteDC(sdc);
        DeleteDC(ddc);
        DeleteObject(src as HGDIOBJ);
        ReleaseDC(null_mut(), dc);
        Some(dst)
    }

    pub fn from_bgra(w: i32, h: i32, px: Vec<u8>) -> Image {
        Image { w, h, px }
    }
}

/// Creates a 32-bit-per-pixel (or 24) top-down DIB section and returns it with its pixel pointer.
pub unsafe fn make_dib(dc: HDC, w: i32, h: i32, bits: u16) -> Option<(HBITMAP, *mut u8)> {
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = -h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = bits;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut p: *mut core::ffi::c_void = null_mut();
    let hbm = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut p, null_mut(), 0);
    (!hbm.is_null() && !p.is_null()).then_some((hbm, p as *mut u8))
}
