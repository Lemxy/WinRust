//! Clipboard images.
//!
//! Windows hands an image over as a DIB (a BITMAPINFOHEADER followed by palette/bit masks and the pixels).
//! `dib_to_images` normalises that into the two files the history keeps: the original as a 32-bpp bottom-up BMP
//! (which is also the DIB that goes back to the clipboard, minus the 14-byte file header) and a small BMP thumbnail
//! for the list. No image crate and no GDI+: GDI itself does the palette/16-bpp conversion.
use std::{
    fs,
    path::Path,
    slice,
};

use windows_sys::Win32::Graphics::Gdi::*;

/// Refuse absurd clipboard images (a 100 000 x 100 000 DIB would ask for 40 GB).
const MAX_PIXELS: i64 = 40_000_000;
const MAX_SIDE: i32 = 16_384;

fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

struct Dib {
    w: i32,
    h: i32, // signed: negative is top-down
    bpp: u16,
    size_image: u32,
    offset: usize, // first pixel byte inside the DIB
}

/// Enough of a BITMAPINFOHEADER/V4/V5 to know where the pixels start and whether GDI can read them.
fn parse(dib: &[u8]) -> Option<Dib> {
    if dib.len() < 40 {
        return None;
    }
    let size = le32(dib, 0) as usize;
    if size < 40 || size > dib.len() {
        return None;
    }
    let w = le32(dib, 4) as i32;
    let h = le32(dib, 8) as i32;
    let bpp = le16(dib, 14);
    let compression = le32(dib, 16);
    let size_image = le32(dib, 20);
    let colors = le32(dib, 32) as usize;
    if w <= 0 || h == 0 || w > MAX_SIDE || h.abs() > MAX_SIDE || (w as i64) * (h.unsigned_abs() as i64) > MAX_PIXELS {
        return None;
    }
    // BI_RGB and BI_BITFIELDS can be converted by GDI; JPEG/PNG payloads inside a DIB are rare and not a DIB we can read.
    if compression != BI_RGB && compression != BI_BITFIELDS {
        return None;
    }
    let palette = if bpp <= 8 { (if colors > 0 { colors } else { 1usize << bpp }) * 4 } else { 0 };
    // A 40-byte header keeps the three colour masks right after itself; V4/V5 carry them inside the header.
    let masks = if compression == BI_BITFIELDS && size == 40 { 12 } else { 0 };
    let offset = size + palette + masks;
    (offset < dib.len()).then_some(Dib { w, h, bpp, size_image, offset })
}

/// The bytes a DIB really occupies. `GlobalSize` may round the clipboard block up; hashing that padding would make a
/// paste look like a new image. Also repairs the common `biSizeImage == 0`.
pub fn dib_len(dib: &[u8]) -> Option<usize> {
    let d = parse(dib)?;
    let stride = (((d.w as u64 * d.bpp as u64 + 31) / 32) * 4) as u64;
    let need = stride * d.h.unsigned_abs() as u64;
    let image = if d.size_image as u64 > need { d.size_image as u64 } else { need };
    let total = d.offset as u64 + image;
    (total <= dib.len() as u64).then_some(total as usize)
}

unsafe fn dib_section(dc: HDC, w: i32, h: i32) -> Option<(HDC, *mut u32, HBITMAP)> {
    let mut bi: BITMAPINFO = std::mem::zeroed();
    bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.bmiHeader.biWidth = w;
    bi.bmiHeader.biHeight = h;
    bi.bmiHeader.biPlanes = 1;
    bi.bmiHeader.biBitCount = 32;
    bi.bmiHeader.biCompression = BI_RGB;
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let hb = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
    if hb.is_null() || bits.is_null() {
        if !hb.is_null() {
            DeleteObject(hb);
        }
        return None;
    }
    let mem = CreateCompatibleDC(dc);
    if mem.is_null() {
        DeleteObject(hb);
        return None;
    }
    SelectObject(mem, hb);
    Some((mem, bits as *mut u32, hb))
}

unsafe fn release(mem: HDC, hb: HBITMAP) {
    DeleteDC(mem);
    DeleteObject(hb);
}

/// 32-bpp bottom-up BMP (what `bmp_to_dib` turns back into CF_DIB). `px` is top-down BGRA as GDI wrote it.
fn save_bmp(path: &Path, w: i32, h: i32, px: &[u32]) -> std::io::Result<()> {
    let stride = w as usize * 4;
    let mut out = Vec::with_capacity(54 + stride * h as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + stride * h as usize) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&((stride * h as usize) as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for y in (0..h as usize).rev() {
        let row = &px[y * w as usize..(y + 1) * w as usize];
        let bytes = unsafe { slice::from_raw_parts(row.as_ptr().cast::<u8>(), stride) };
        out.extend_from_slice(bytes);
    }
    fs::write(path, out)
}

/// Area-ish sample (4x4 points per target pixel) for the list thumbnail.
fn thumbnail(px: &[u32], w: i32, h: i32) -> (i32, i32, Vec<u32>) {
    let scale = (96.0 / w.max(h) as f32).min(1.0);
    let tw = ((w as f32 * scale).round() as i32).max(1);
    let th = ((h as f32 * scale).round() as i32).max(1);
    let mut out = vec![0u32; (tw * th) as usize];
    for ty in 0..th {
        for tx in 0..tw {
            let x0 = tx * w / tw;
            let x1 = (((tx + 1) * w + tw - 1) / tw).max(x0 + 1).min(w);
            let y0 = ty * h / th;
            let y1 = (((ty + 1) * h + th - 1) / th).max(y0 + 1).min(h);
            let (mut b, mut g, mut r, mut n) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..4 {
                let y = (y0 + (y1 - y0) * sy / 4).min(h - 1);
                for sx in 0..4 {
                    let x = (x0 + (x1 - x0) * sx / 4).min(w - 1);
                    let p = px[(y * w + x) as usize];
                    b += p & 255;
                    g += (p >> 8) & 255;
                    r += (p >> 16) & 255;
                    n += 1;
                }
            }
            out[(ty * tw + tx) as usize] = (b / n) | (g / n) << 8 | (r / n) << 16;
        }
    }
    (tw, th, out)
}

/// 32-bpp and 24-bpp BI_RGB DIBs (the overwhelming majority of clipboard images) are copied directly: no GDI batching,
/// no DC, no surprises. Paletted, 16-bpp and bitfield DIBs go through GDI below.
fn direct(dib: &[u8], d: &Dib) -> Option<Vec<u32>> {
    if le32(dib, 16) != BI_RGB || (d.bpp != 32 && d.bpp != 24) {
        return None;
    }
    let (w, h) = (d.w as usize, d.h.unsigned_abs() as usize);
    let bytes = d.bpp as usize / 8;
    let stride = ((w * d.bpp as usize + 31) / 32) * 4;
    let end = d.offset.checked_add(stride.checked_mul(h)?)?;
    if end > dib.len() {
        return None;
    }
    let src = &dib[d.offset..end];
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        let sy = if d.h > 0 { h - 1 - y } else { y };
        let row = &src[sy * stride..sy * stride + stride];
        for x in 0..w {
            let i = x * bytes;
            out[y * w + x] = if bytes == 4 {
                u32::from_le_bytes([row[i], row[i + 1], row[i + 2], row[i + 3]]) & 0x00FF_FFFF
            } else {
                row[i] as u32 | (row[i + 1] as u32) << 8 | (row[i + 2] as u32) << 16
            };
        }
    }
    Some(out)
}

/// DIB -> original BMP + thumbnail BMP. Returns the image size when the BMP was written.
pub fn dib_to_images(dib: &[u8], bmp: &Path, thumb: &Path) -> Option<(i32, i32)> {
    let d = parse(dib)?;
    let w = d.w;
    let h = d.h.abs();
    let mut px = direct(dib, &d);
    if px.is_none() {
        px = unsafe { convert_with_gdi(dib, &d, w, h) };
    }
    let px = px?;
    save_bmp(bmp, w, h, &px).ok()?;
    let (tw, th, tpx) = thumbnail(&px, w, h);
    if save_bmp(thumb, tw, th, &tpx).is_err() {
        let _ = fs::remove_file(thumb);
    }
    Some((w, h))
}

unsafe fn convert_with_gdi(dib: &[u8], d: &Dib, w: i32, h: i32) -> Option<Vec<u32>> {
    let scr = GetDC(std::ptr::null_mut());
    if scr.is_null() {
        return None;
    }
    let (mem, bits, hb) = match dib_section(scr, w, -h) {
        Some(v) => v,
        None => {
            ReleaseDC(std::ptr::null_mut(), scr);
            return None;
        }
    };
    // StretchDIBits converts palettes and 16-bpp layouts and respects the DIB's own orientation.
    let lines = StretchDIBits(
        mem,
        0,
        0,
        w,
        h,
        0,
        0,
        w,
        h,
        dib[d.offset..].as_ptr().cast(),
        dib.as_ptr().cast::<BITMAPINFO>(),
        DIB_RGB_COLORS,
        SRCCOPY,
    );
    // GDI batches drawing: without this the DIB section can still read as zeros.
    GdiFlush();
    let px = if lines > 0 { slice::from_raw_parts(bits, (w * h) as usize).to_vec() } else { Vec::new() };
    release(mem, hb);
    ReleaseDC(std::ptr::null_mut(), scr);
    (!px.is_empty()).then_some(px)
}

/// CF_BITMAP fallback: some apps put only an HBITMAP on the clipboard. Read it as a 32-bpp top-down DIB.
pub fn hbitmap_to_dib(hbm: HBITMAP) -> Option<Vec<u8>> {
    unsafe {
        let mut bm: BITMAP = std::mem::zeroed();
        if GetObjectW(hbm, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut _) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight);
        if w <= 0 || h <= 0 || w > MAX_SIDE || h > MAX_SIDE || (w as i64) * (h as i64) > MAX_PIXELS {
            return None;
        }
        let scr = GetDC(std::ptr::null_mut());
        if scr.is_null() {
            return None;
        }
        let stride = w as usize * 4;
        let mut px = vec![0u8; stride * h as usize];
        let mut bi: BITMAPINFO = std::mem::zeroed();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = w;
        bi.bmiHeader.biHeight = -h; // top-down
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB;
        bi.bmiHeader.biSizeImage = (stride * h as usize) as u32;
        let lines = GetDIBits(scr, hbm, 0, h as u32, px.as_mut_ptr().cast(), &mut bi, DIB_RGB_COLORS);
        ReleaseDC(std::ptr::null_mut(), scr);
        if lines != h {
            return None;
        }
        let mut out = Vec::with_capacity(40 + px.len());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&(-h).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(px.len() as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&px);
        Some(out)
    }
}

/// The DIB bytes for CF_DIB: a BMP without its 14-byte BITMAPFILEHEADER.
pub fn bmp_to_dib(path: &Path) -> Option<Vec<u8>> {
    let data = fs::read(path).ok()?;
    if data.len() < 54 || &data[..2] != b"BM" {
        return None;
    }
    let off = le32(&data, 10) as usize;
    if off < 14 || off > data.len() {
        return None;
    }
    Some(data[14..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::{dib_to_images, parse};

    #[test]
    fn parses_a_plain_header() {
        let mut dib = vec![0u8; 40 + 16];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&4i32.to_le_bytes());
        dib[8..12].copy_from_slice(&(-4i32).to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        let d = parse(&dib).unwrap();
        assert_eq!((d.w, d.h, d.offset), (4, -4, 40));
        // a 1-bpp DIB with a two-colour palette starts after header + 8 bytes
        dib[14..16].copy_from_slice(&1u16.to_le_bytes());
        dib[32..36].copy_from_slice(&2u32.to_le_bytes());
        dib.resize(40 + 8 + 16, 0);
        assert_eq!(parse(&dib).unwrap().offset, 48);
    }

    #[test]
    fn converts_16_bpp_through_gdi() {
        // 2x2 RGB565, bottom-up: bottom blue/green, top red/white.
        let mut dib = vec![0u8; 40 + 16];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&16u16.to_le_bytes());
        dib[20..24].copy_from_slice(&16u32.to_le_bytes());
        for (i, px) in [0x001Fu16, 0x07E0, 0xF800, 0xFFFF].into_iter().enumerate() {
            dib[40 + i * 2..42 + i * 2].copy_from_slice(&px.to_le_bytes());
        }
        let dir = std::env::temp_dir().join(format!("rs-clip-img16-{}", std::process::id()));
        let (bmp, thumb) = (dir.join("a.bmp"), dir.join("a.thumb.bmp"));
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(dib_to_images(&dib, &bmp, &thumb), Some((2, 2)));
        let file = std::fs::read(&bmp).unwrap();
        // BI_RGB 16-bpp is RGB555; GDI expands the 5-bit channels with slight rounding, so compare with tolerance.
        let p = u32::from_le_bytes(file[54..58].try_into().unwrap());
        assert_eq!((p & 0xFF, p >> 16), (255, 0)); // blue
        let p = u32::from_le_bytes(file[58..62].try_into().unwrap());
        assert_eq!((p & 0xFF, (p >> 8) & 0xFF, p >> 16 <= 16), (0, 255, true)); // green (RGB555's R bit is set by 0x07E0)
        let p = u32::from_le_bytes(file[62..66].try_into().unwrap());
        assert_eq!((p & 0xFF, (p >> 8) & 0xFF, p >> 16 >= 240), (0, 0, true)); // red
        let p = u32::from_le_bytes(file[66..70].try_into().unwrap());
        assert_eq!((p & 0xFF >= 240, (p >> 8) & 0xFF >= 240, p >> 16 >= 240), (true, true, true)); // white
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn converts_a_paletted_dib() {
        // 2x2, 8-bpp with a four-colour palette: bottom blue/green, top red/white.
        let mut dib = vec![0u8; 40 + 16 + 8];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&8u16.to_le_bytes());
        dib[20..24].copy_from_slice(&8u32.to_le_bytes());
        dib[32..36].copy_from_slice(&4u32.to_le_bytes()); // biClrUsed
        for (i, (b, g, r)) in [(255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)].into_iter().enumerate() {
            let o = 40 + i * 4;
            dib[o] = b;
            dib[o + 1] = g;
            dib[o + 2] = r;
        }
        // stride 4: [blue, green, pad, pad] then [red, white, pad, pad]
        dib[56..64].copy_from_slice(&[0, 1, 0, 0, 2, 3, 0, 0]);
        let dir = std::env::temp_dir().join(format!("rs-clip-img8-{}", std::process::id()));
        let (bmp, thumb) = (dir.join("a.bmp"), dir.join("a.thumb.bmp"));
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(dib_to_images(&dib, &bmp, &thumb), Some((2, 2)));
        let file = std::fs::read(&bmp).unwrap();
        let px: Vec<u32> = (54..70).step_by(4).map(|i| u32::from_le_bytes(file[i..i + 4].try_into().unwrap())).collect();
        assert_eq!(px, [0x0000_00FF, 0x0000_FF00, 0x00FF_0000, 0x00FF_FFFF]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn keeps_the_picture_the_right_way_up() {
        // 2x2, bottom-up: the bottom row is blue/green, the top row red/white (BGRA in memory).
        let mut dib = vec![0u8; 40 + 16];
        dib[..4].copy_from_slice(&40u32.to_le_bytes());
        dib[4..8].copy_from_slice(&2i32.to_le_bytes());
        dib[8..12].copy_from_slice(&2i32.to_le_bytes());
        dib[12..14].copy_from_slice(&1u16.to_le_bytes());
        dib[14..16].copy_from_slice(&32u16.to_le_bytes());
        dib[20..24].copy_from_slice(&16u32.to_le_bytes());
        for (i, px) in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0x00FF_FFFF].into_iter().enumerate() {
            dib[40 + i * 4..44 + i * 4].copy_from_slice(&px.to_le_bytes());
        }
        let dir = std::env::temp_dir().join(format!("rs-clip-img-{}", std::process::id()));
        let (bmp, thumb) = (dir.join("a.bmp"), dir.join("a.thumb.bmp"));
        let _ = std::fs::create_dir_all(&dir);
        assert_eq!(dib_to_images(&dib, &bmp, &thumb), Some((2, 2)));
        let file = std::fs::read(&bmp).unwrap();
        assert_eq!(&file[..2], b"BM");
        // BMP rows are bottom-up too: the first pixel row in the file is the bottom blue/green one.
        assert_eq!(u32::from_le_bytes(file[54..58].try_into().unwrap()), 0x00FF_0000);
        assert_eq!(u32::from_le_bytes(file[58..62].try_into().unwrap()), 0x0000_FF00);
        assert_eq!(u32::from_le_bytes(file[62..66].try_into().unwrap()), 0x0000_00FF);
        assert_eq!(u32::from_le_bytes(file[66..70].try_into().unwrap()), 0x00FF_FFFF);
        assert!(thumb.is_file());
        // CF_DIB for pasting is the same file without the 14-byte header, with its positive (bottom-up) height.
        let round = super::bmp_to_dib(&bmp).unwrap();
        assert_eq!(round.len(), 40 + 16);
        assert_eq!(i32::from_le_bytes(round[8..12].try_into().unwrap()), 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
