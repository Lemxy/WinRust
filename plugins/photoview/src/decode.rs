// Decodes any picture Windows has a codec for into top-down opaque BGRA, rotated by its EXIF orientation,
// transparent pixels composited over `bg` (BGR).
use windows::{
    Win32::{Foundation::GENERIC_READ, Graphics::Imaging::*, System::Com::*},
    core::*,
};

pub struct Img {
    pub w: i32,
    pub h: i32,
    pub bgra: Vec<u8>,
}

/// Call once on every thread that decodes.
pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

/// EXIF orientation (1..8) -> the WIC transform that makes the picture upright.
fn transform(orientation: u16) -> i32 {
    match orientation {
        2 => 8,      // mirrored
        3 => 2,      // 180
        4 => 16,     // mirrored vertically
        5 => 1 | 8,  // transpose
        6 => 1,      // 90 clockwise
        7 => 3 | 8,  // transverse
        8 => 3,      // 90 counter-clockwise
        _ => 0,
    }
}

pub fn decode(path: &str, bg: [u8; 3]) -> Result<Img> {
    unsafe {
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let dec = f.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let frame = dec.GetFrame(0)?;
        let orientation = frame
            .GetMetadataQueryReader()
            .and_then(|q| {
                let mut v = windows::Win32::System::Com::StructuredStorage::PROPVARIANT::default();
                q.GetMetadataByName(w!("/app1/ifd/{ushort=274}"), &mut v)?;
                u16::try_from(&v)
            })
            .unwrap_or(1);
        let conv = f.CreateFormatConverter()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let mut src: IWICBitmapSource = conv.cast()?;
        let t = transform(orientation);
        if t != 0 {
            let r = f.CreateBitmapFlipRotator()?;
            r.Initialize(&src, WICBitmapTransformOptions(t))?;
            src = r.cast()?;
        }
        let (mut w, mut h) = (0u32, 0u32);
        src.GetSize(&mut w, &mut h)?;
        let mut bgra = vec![0u8; w as usize * h as usize * 4];
        src.CopyPixels(std::ptr::null(), w * 4, &mut bgra)?;
        for p in bgra.chunks_exact_mut(4) {
            let inv = 255 - p[3] as u32; // premultiplied: result = colour + bg * (1 - alpha)
            for c in 0..3 {
                p[c] = (p[c] as u32 + bg[c] as u32 * inv / 255).min(255) as u8;
            }
            p[3] = 255;
        }
        Ok(Img { w: w as i32, h: h as i32, bgra })
    }
}

/// `img` scaled to `w` x `h` with WIC's high-quality cubic filter (Fant on a Windows without it): a big photo shown
/// small stays sharp and smooth, where GDI's halftone left it soft and jagged.
pub fn scaled(img: &Img, w: i32, h: i32) -> Option<Vec<u8>> {
    unsafe {
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let bmp = f.CreateBitmapFromMemory(img.w as u32, img.h as u32, &GUID_WICPixelFormat32bppBGRA, img.w as u32 * 4, &img.bgra).ok()?;
        let sc = f.CreateBitmapScaler().ok()?;
        if sc.Initialize(&bmp, w as u32, h as u32, WICBitmapInterpolationModeHighQualityCubic).is_err() {
            sc.Initialize(&bmp, w as u32, h as u32, WICBitmapInterpolationModeFant).ok()?;
        }
        let mut out = vec![0u8; w as usize * h as usize * 4];
        sc.CopyPixels(std::ptr::null(), w as u32 * 4, &mut out).ok()?;
        Some(out)
    }
}

/// `img` turned a quarter clockwise (`cw`) or counter-clockwise: for a photo the camera saved the wrong way up.
pub fn turned(img: &Img, cw: bool) -> Img {
    let (w, h) = (img.w as usize, img.h as usize);
    let mut out = vec![0u8; img.bgra.len()];
    for y in 0..h {
        for x in 0..w {
            // the new picture is h wide and w tall
            let (nx, ny) = if cw { (h - 1 - y, x) } else { (y, w - 1 - x) };
            let (s, d) = ((y * w + x) * 4, (ny * h + nx) * 4);
            out[d..d + 4].copy_from_slice(&img.bgra[s..s + 4]);
        }
    }
    Img { w: img.h, h: img.w, bgra: out }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2x1 PNG, left pixel opaque red, right pixel fully transparent: the right one must come out as the background.
    #[test]
    fn decodes_png_and_composites_alpha_over_background() {
        // 8-bit RGBA, no interlace; IDAT built by hand (stored deflate block) so the test needs no image file.
        fn crc(d: &[u8]) -> u32 {
            let mut c = !0u32;
            for &b in d {
                c ^= b as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, ty: &[u8], data: &[u8]) {
            out.extend((data.len() as u32).to_be_bytes());
            let mut body = ty.to_vec();
            body.extend(data);
            out.extend(&body);
            out.extend(crc(&body).to_be_bytes());
        }
        let raw = [0u8, 255, 0, 0, 255, 0, 0, 0, 0]; // filter 0, red opaque, transparent
        let (mut a, mut b) = (1u32, 0u32);
        for &x in &raw {
            a = (a + x as u32) % 65521;
            b = (b + a) % 65521;
        }
        let mut z = vec![0x78, 0x01, 0x01, raw.len() as u8, 0, !(raw.len() as u8), 0xFF];
        z.extend(raw);
        z.extend(((b << 16) | a).to_be_bytes());
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        chunk(&mut png, b"IHDR", &[0, 0, 0, 2, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        chunk(&mut png, b"IDAT", &z);
        chunk(&mut png, b"IEND", &[]);
        let p = std::env::temp_dir().join("photoview-test.png");
        std::fs::write(&p, png).unwrap();
        com_init();
        let img = decode(p.to_str().unwrap(), [10, 20, 30]).unwrap();
        let _ = std::fs::remove_file(p);
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(img.bgra[..4], [0, 0, 255, 255]); // BGRA of opaque red
        assert_eq!(img.bgra[4..8], [10, 20, 30, 255]); // the background, as given (BGR)
    }

    #[test]
    fn a_quarter_turn_moves_the_corners() {
        let px = |v: u8| [v, v, v, 255];
        let img = Img { w: 2, h: 1, bgra: [px(1), px(2)].concat() }; // one row: 1 2
        let cw = turned(&img, true);
        assert_eq!((cw.w, cw.h), (1, 2));
        assert_eq!(cw.bgra, [px(1), px(2)].concat()); // turned right, the row stands up: 1 on top
        let ccw = turned(&img, false);
        assert_eq!(ccw.bgra, [px(2), px(1)].concat()); // turned left: 2 on top
        let back = turned(&cw, false);
        assert_eq!((back.w, back.h), (2, 1));
        assert_eq!(back.bgra, img.bgra);
    }
}
