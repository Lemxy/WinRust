//! Windows.Graphics.Capture (WGC) + Media Foundation: the built-in H.264/MP4 encoder, no ffmpeg.
//!
//! A free-threaded frame pool grabs the window or monitor (DWM-composited, works without injection), each frame's
//! D3D11 texture becomes an MF sample and the SinkWriter encodes it to MP4. A region inside one monitor is the monitor
//! item cropped with `CopySubresourceRegion`; a selection spanning monitors has no single WGC item and is left to the
//! ffmpeg frame bridge in `record.rs`.
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use windows::{
    Graphics::{
        Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession},
        DirectX::{
            Direct3D11::{IDirect3DDevice, IDirect3DSurface},
            DirectXPixelFormat,
        },
    },
    Win32::{
        Foundation::{HMODULE, HWND},
        Graphics::{
            Direct3D::D3D_DRIVER_TYPE_HARDWARE,
            Direct3D11::{
                D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
                ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
            },
            Dxgi::{Common::DXGI_FORMAT_B8G8R8A8_UNORM, IDXGIDevice},
            Gdi::HMONITOR,
        },
        Media::MediaFoundation::{
            IMFDXGIDeviceManager, IMFMediaType, IMFSinkWriter, MFAudioFormat_AAC, MFAudioFormat_PCM, MFMediaType_Audio, MFMediaType_Video,
            MFSTARTUP_FULL, MFVideoFormat_H264, MFVideoInterlace_Progressive, MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, MF_MT_AAC_PAYLOAD_TYPE,
            MF_MT_ALL_SAMPLES_INDEPENDENT, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE, MF_MT_AUDIO_BLOCK_ALIGNMENT,
            MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
            MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS,
            MF_SINK_WRITER_D3D_MANAGER, MF_VERSION, MFVideoFormat_RGB32, MFStartup, MFCreateAttributes, MFCreateDXGIDeviceManager,
            MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL,
        },
        System::{
            Com::{COINIT_MULTITHREADED, CoInitializeEx},
            WinRT::{RoGetActivationFactory, Graphics::Capture::IGraphicsCaptureItemInterop},
            WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess},
        },
    },
    core::{HSTRING, Interface},
};

use crate::audio;

#[derive(Clone, Copy, Debug)]
pub enum Target {
    Window { hwnd: isize },
    /// A monitor item cropped to `crop` (both rectangles in screen pixels, left/top/right/bottom).
    Monitor { hmon: isize, mon: (i32, i32, i32, i32), crop: (i32, i32, i32, i32) },
}

pub fn window_target(hwnd: isize) -> Target {
    Target::Window { hwnd }
}

pub fn monitor_target(hmon: isize, mon: (i32, i32, i32, i32), crop: (i32, i32, i32, i32)) -> Target {
    Target::Monitor { hmon, mon, crop }
}

pub fn supported() -> bool {
    // Windows 10 1903+ and all Windows 11; false on older builds or when capture is disabled by policy.
    GraphicsCaptureSession::IsSupported().unwrap_or(false)
}

fn err<E: std::fmt::Display>(e: E) -> String {
    format!("{e}")
}

/// Builds the whole pipeline (device, item, session, SinkWriter) once, writes a tiny file and throws it away. Selecting
/// WGC only after this succeeds means a probe failure can fall back to ffmpeg before anything is recorded.
pub fn probe(target: &Target, fps: u32, cursor: bool) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("rs-shot-wgc-probe-{}.mp4", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let result = (|| -> Result<(), String> {
        let enc = Enc::open(target, fps, cursor, &path, false)?;
        enc.session.Close().map_err(err)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&path);
    result
}

struct Enc {
    writer: IMFSinkWriter,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    crop: Option<RectU>,
    /// (width, height) written into both media types.
    dims: (i32, i32),
    stream: u32,
    /// CPU-readable copy of the frame; MF takes memory-buffer samples, which every H.264 encoder accepts.
    staging: ID3D11Texture2D,
    audio: Option<audio::Audio>,
    audio_stream: Option<u32>,
    audio_note: Option<String>,
}

#[derive(Clone, Copy)]
struct RectU {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Enc {
    fn open(target: &Target, fps: u32, cursor: bool, out: &Path, with_audio: bool) -> Result<Enc, String> {
        unsafe {
            // MTA is enough for WGC/MF; both probe and recorder threads initialize their own COM apartment.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(err)?;

            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(err)?;
            let device = device.ok_or("D3D11CreateDevice returned no device")?;
            let context = context.ok_or("D3D11CreateDevice returned no context")?;
            let dxgi: IDXGIDevice = device.cast().map_err(err)?;
            let d3d_device: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi).map_err(err)?.cast().map_err(err)?;

            let item: GraphicsCaptureItem = match target {
                Target::Window { hwnd } => create_item_window(*hwnd)?,
                Target::Monitor { hmon, .. } => create_item_monitor(*hmon)?,
            };
            let item_size = item.Size().map_err(err)?;
            let crop = match target {
                Target::Monitor { mon, crop, .. } => Some(RectU {
                    x: (crop.0 - mon.0).max(0) as u32,
                    y: (crop.1 - mon.1).max(0) as u32,
                    w: (crop.2 - crop.0).max(2) as u32,
                    h: (crop.3 - crop.1).max(2) as u32,
                }),
                Target::Window { .. } => None,
            };
            let dims = match crop {
                Some(c) => (c.w as i32, c.h as i32),
                None => (item_size.Width, item_size.Height),
            };
            if dims.0 < 2 || dims.1 < 2 {
                return Err(t!(
                    "у источника захвата пустой размер",
                    "capture source has an empty size",
                    ja: "キャプチャ元のサイズが空です",
                    pt: "a fonte de captura tem tamanho vazio",
                    zh: "捕获源尺寸为空"
                )
                .to_string());
            }

            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d_device, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, item_size).map_err(err)?;
            let session = pool.CreateCaptureSession(&item).map_err(err)?;
            let _ = session.SetIsCursorCaptureEnabled(cursor);
            let _ = session.SetIsBorderRequired(false); // Windows 10 ignores this; Windows 11 hides the yellow frame
            session.StartCapture().map_err(err)?;

            let (mut loopback, mut audio_note) = if with_audio {
                match audio::open() {
                    Ok(a) => (Some(a), None),
                    Err(e) => (None, Some(e)),
                }
            } else {
                (None, None)
            };
            let audio_fmt = loopback.as_ref().map(|a| a.format());
            let (writer, stream, audio_stream, note) = make_writer(out, dims, fps, &device, audio_fmt)?;
            if let Some(n) = note {
                audio_note = Some(n);
            }
            if audio_stream.is_none() {
                loopback = None;
            }
            let staging = make_staging(&device, dims.0 as u32, dims.1 as u32)?;
            Ok(Enc { writer, pool, session, device, context, crop, dims, stream, staging, audio: loopback, audio_stream, audio_note })
        }
    }

    fn step(&mut self, n: u64, fps: u32) -> Result<bool, String> {
        unsafe {
            let frame = match self.pool.TryGetNextFrame() {
                Ok(f) => f,
                Err(_) => return Ok(false), // no frame this tick
            };
            let surface: IDirect3DSurface = frame.Surface().map_err(|e| format!("frame.Surface: {e}"))?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(|e| format!("surface -> IDirect3DDxgiInterfaceAccess: {e}"))?;
            let texture: ID3D11Texture2D = access.GetInterface().map_err(|e| format!("GetInterface(ID3D11Texture2D): {e}"))?;
            let sample_tex = match self.crop {
                Some(c) => crop_texture(&self.device, &self.context, &texture, c)?,
                None => texture,
            };
            // Texture -> CPU-readable staging -> memory sample. MF's H.264 encoder takes a plain NV12/RGB buffer; a
            // DXGI texture sample is rejected by many encoder MFTs with E_INVALIDARG, so the copy is the portable path.
            self.context.CopySubresourceRegion(&self.staging, 0, 0, 0, 0, &sample_tex, 0, None);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|e| format!("Map(staging): {e}"))?;
            let row_bytes = (self.dims.0 * 4) as usize;
            let size = row_bytes * self.dims.1 as usize;
            let buffer = MFCreateMemoryBuffer(size as u32).map_err(|e| format!("MFCreateMemoryBuffer: {e}"))?;
            let mut dst: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&mut dst, None, None).map_err(|e| format!("buffer.Lock: {e}"))?;
            let src = mapped.pData as *const u8;
            for y in 0..self.dims.1 as usize {
                std::ptr::copy_nonoverlapping(src.add(y * mapped.RowPitch as usize), dst.add(y * row_bytes), row_bytes);
            }
            self.context.Unmap(&self.staging, 0);
            buffer.SetCurrentLength(size as u32).map_err(|e| format!("SetCurrentLength: {e}"))?;
            buffer.Unlock().map_err(|e| format!("buffer.Unlock: {e}"))?;
            let sample = MFCreateSample().map_err(|e| format!("MFCreateSample: {e}"))?;
            sample.AddBuffer(&buffer).map_err(|e| format!("AddBuffer: {e}"))?;
            let unit = 10_000_000i64 / fps.max(1) as i64;
            sample.SetSampleTime((n as i64) * unit).map_err(|e| format!("SetSampleTime: {e}"))?;
            sample.SetSampleDuration(unit).map_err(|e| format!("SetSampleDuration: {e}"))?;
            self.writer.WriteSample(self.stream, &sample).map_err(|e| format!("WriteSample: {e}"))?;
            self.pump_audio();
            let _ = frame.Close();
            Ok(true)
        }
    }

    fn pump_audio(&mut self) {
        let (Some(a), Some(idx)) = (self.audio.as_mut(), self.audio_stream) else { return };
        if let Err(e) = a.pump(&self.writer, idx) {
            self.audio = None;
            if self.audio_note.is_none() {
                self.audio_note = Some(e);
            }
        }
    }
}

unsafe fn create_item_window(hwnd: isize) -> Result<GraphicsCaptureItem, String> {
    let interop: IGraphicsCaptureItemInterop =
        RoGetActivationFactory(&HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureItem")).map_err(err)?;
    interop.CreateForWindow(HWND(hwnd as *mut _)).map_err(err)
}

unsafe fn create_item_monitor(hmon: isize) -> Result<GraphicsCaptureItem, String> {
    let interop: IGraphicsCaptureItemInterop =
        RoGetActivationFactory(&HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureItem")).map_err(err)?;
    interop.CreateForMonitor(HMONITOR(hmon as *mut _)).map_err(err)
}

fn pack(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

unsafe fn video_type(w: i32, h: i32, fps: u32, subtype: windows::core::GUID) -> Result<IMFMediaType, String> {
    let t = MFCreateMediaType().map_err(err)?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(err)?;
    t.SetGUID(&MF_MT_SUBTYPE, &subtype).map_err(err)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w as u32, h as u32)).map_err(err)?;
    t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps.max(1), 1)).map_err(err)?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1)).map_err(err)?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32).map_err(err)?;
    Ok(t)
}

unsafe fn make_writer(
    out: &Path,
    dims: (i32, i32),
    fps: u32,
    device: &ID3D11Device,
    audio_fmt: Option<audio::Format>,
) -> Result<(IMFSinkWriter, u32, Option<u32>, Option<String>), String> {
    match build_writer(out, dims, fps, device, audio_fmt) {
        Ok((w, v, a)) => Ok((w, v, a, None)),
        Err(e) if audio_fmt.is_some() => {
            // AAC rejected (or no AAC encoder): rebuild video-only and keep the recording alive.
            let (w, v, _) = build_writer(out, dims, fps, device, None)?;
            Ok((w, v, None, Some(tf!("звук недоступен: {}", "audio unavailable: {}", ja: "音声を利用できません: {}", pt: "áudio indisponível: {}", zh: "音频不可用：{}", e))))
        }
        Err(e) => Err(e),
    }
}

unsafe fn build_writer(
    out: &Path,
    dims: (i32, i32),
    fps: u32,
    device: &ID3D11Device,
    audio_fmt: Option<audio::Format>,
) -> Result<(IMFSinkWriter, u32, Option<u32>), String> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 2).map_err(err)?;
    let attrs = attrs.ok_or("MFCreateAttributes returned nothing")?;
    attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(err)?;

    let mut token = 0u32;
    let mut manager: Option<IMFDXGIDeviceManager> = None;
    MFCreateDXGIDeviceManager(&mut token, &mut manager).map_err(err)?;
    let manager = manager.ok_or("MFCreateDXGIDeviceManager returned nothing")?;
    manager.ResetDevice(device, token).map_err(err)?;
    attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &manager).map_err(err)?;

    let path = HSTRING::from(out.to_string_lossy().as_ref());
    let writer = MFCreateSinkWriterFromURL(&path, None, &attrs).map_err(err)?;
    let out_type = video_type(dims.0, dims.1, fps, MFVideoFormat_H264)?;
    let bitrate = ((dims.0 as u64 * dims.1 as u64 * fps as u64 * 3) / 10).clamp(2_000_000, 40_000_000) as u32;
    out_type.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).map_err(err)?;
    let stream = writer.AddStream(&out_type).map_err(err)?;
    let in_type = video_type(dims.0, dims.1, fps, MFVideoFormat_RGB32)?;
    in_type.SetUINT32(&MF_MT_DEFAULT_STRIDE, (dims.0 * 4) as u32).map_err(err)?;
    in_type.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1).map_err(err)?;
    writer.SetInputMediaType(stream, &in_type, None).map_err(err)?;

    let audio_stream = match audio_fmt {
        Some(f) => {
            let out_a = audio_type(f, false)?;
            let idx = writer.AddStream(&out_a).map_err(err)?;
            let in_a = audio_type(f, true)?;
            writer.SetInputMediaType(idx, &in_a, None).map_err(err)?;
            Some(idx)
        }
        None => None,
    };
    writer.BeginWriting().map_err(err)?;
    Ok((writer, stream, audio_stream))
}

unsafe fn audio_type(f: audio::Format, input: bool) -> Result<IMFMediaType, String> {
    let t = MFCreateMediaType().map_err(err)?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(err)?;
    t.SetGUID(&MF_MT_SUBTYPE, if input { &MFAudioFormat_PCM } else { &MFAudioFormat_AAC }).map_err(err)?;
    t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, f.rate).map_err(err)?;
    t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, f.channels as u32).map_err(err)?;
    t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(err)?;
    let block = f.channels as u32 * 2;
    if input {
        t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block).map_err(err)?;
        t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, f.rate * block).map_err(err)?;
    } else {
        t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 16_000).map_err(err)?; // ~128 kbps
        t.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0).map_err(err)?;
        t.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29).map_err(err)?;
    }
    Ok(t)
}

unsafe fn make_staging(device: &ID3D11Device, w: u32, h: u32) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    device.CreateTexture2D(&desc, None, Some(&mut tex)).map_err(err)?;
    tex.ok_or_else(|| "CreateTexture2D(staging) returned nothing".into())
}

unsafe fn crop_texture(device: &ID3D11Device, context: &ID3D11DeviceContext, src: &ID3D11Texture2D, c: RectU) -> Result<ID3D11Texture2D, String> {    let desc = D3D11_TEXTURE2D_DESC {
        Width: c.w,
        Height: c.h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    device.CreateTexture2D(&desc, None, Some(&mut tex)).map_err(err)?;
    let tex = tex.ok_or("CreateTexture2D returned nothing")?;
    let bx = D3D11_BOX { left: c.x, top: c.y, front: 0, right: c.x + c.w, bottom: c.y + c.h, back: 1 };
    context.CopySubresourceRegion(&tex, 0, 0, 0, 0, src, 0, Some(&bx));
    Ok(tex)
}

/// Records until `stop`; returns (frames, elapsed milliseconds).
pub fn record(target: &Target, fps: u32, cursor: bool, out: &Path, stop: Arc<AtomicBool>, with_audio: bool) -> Result<(u64, u128, bool, Option<String>), String> {
    let mut enc = Enc::open(target, fps, cursor, out, with_audio)?;
    let start = Instant::now();
    let interval = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let mut frames = 0u64;
    let mut misses = 0u32;
    while !stop.load(Relaxed) {
        let tick = Instant::now();
        match enc.step(frames, fps) {
            Ok(true) => {
                frames += 1;
                misses = 0;
            }
            Ok(false) => {
                // WGC produces frames at the compositor rate; wait for one instead of spinning.
                misses += 1;
                if misses > 200 && frames == 0 {
                    return Err(t!(
                        "Windows.Graphics.Capture не дал ни одного кадра (свёрнутое окно, защищённое содержимое или захват отключён)",
                        "Windows.Graphics.Capture produced no frames (minimized window, protected content or capture disabled)",
                        ja: "Windows.Graphics.Capture がフレームを生成しませんでした（ウィンドウ最小化、保護されたコンテンツ、またはキャプチャ無効）",
                        pt: "O Windows.Graphics.Capture não produziu quadros (janela minimizada, conteúdo protegido ou captura desativada)",
                        zh: "Windows.Graphics.Capture 未生成任何帧（窗口最小化、受保护内容或捕获已禁用）"
                    )
                    .to_string());
                }
            }
            Err(e) => return Err(e),
        }
        let spent = tick.elapsed();
        if spent < interval {
            std::thread::sleep(interval - spent);
        }
    }
    unsafe {
        enc.pump_audio(); // drain the last loopback packets
        let _ = enc.session.Close();
        let _ = enc.writer.Finalize();
    }
    if frames == 0 {
        return Err(t!("кадры не закодированы", "no frames were encoded", ja: "フレームがエンコードされていません", pt: "nenhum quadro foi codificado", zh: "没有编码任何帧").to_string());
    }
    let audio_ok = enc.audio.is_some() && enc.audio_stream.is_some();
    let note = enc.audio_note.take();
    Ok((frames, start.elapsed().as_millis(), audio_ok, note))
}



