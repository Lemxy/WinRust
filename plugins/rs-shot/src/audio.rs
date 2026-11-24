//! WASAPI loopback: captures what the default render device is playing. Packets are converted to 16-bit PCM so the
//! Media Foundation SinkWriter can encode them as an AAC track next to the H.264 video.
use std::{ptr::null_mut, slice};

use windows::{
    Win32::{
        Media::{
            Audio::{
                AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient,
                IMMDeviceEnumerator, MMDeviceEnumerator, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
            },
            MediaFoundation::{IMFMediaBuffer, IMFSinkWriter, MFCreateMemoryBuffer, MFCreateSample},
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree},
    },
};

const IEEE_FLOAT: u128 = 0x00000003_0000_0010_8000_00aa00389b71;
const SUBTYPE_PCM: u128 = 0x00000001_0000_0010_8000_00aa00389b71;

pub struct Audio {
    _client: IAudioClient,
    capture: IAudioCaptureClient,
    rate: u32,
    channels: u16,
    float: bool,
    /// Samples already handed to the muxer, for the timestamps.
    frames: u64,
}

#[derive(Clone, Copy)]
pub struct Format {
    pub rate: u32,
    pub channels: u16,
}

pub fn open() -> Result<Audio, String> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| tf!("MMDeviceEnumerator: {}", "MMDeviceEnumerator: {}", ja: "MMDeviceEnumerator: {}", pt: "MMDeviceEnumerator: {}", zh: "MMDeviceEnumerator：{}", e))?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| {
            tf!(
                "устройство вывода по умолчанию: {}",
                "default output device: {}",
                ja: "既定の出力デバイス: {}",
                pt: "dispositivo de saída padrão: {}",
                zh: "默认输出设备：{}",
                e
            )
        })?;
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| tf!("IAudioClient: {}", "IAudioClient: {}", ja: "IAudioClient: {}", pt: "IAudioClient: {}", zh: "IAudioClient：{}", e))?;
        let mix = client.GetMixFormat().map_err(|e| tf!("GetMixFormat: {}", "GetMixFormat: {}", ja: "GetMixFormat: {}", pt: "GetMixFormat: {}", zh: "GetMixFormat：{}", e))?;
        if mix.is_null() {
            return Err(t!("GetMixFormat вернул null", "GetMixFormat returned null", ja: "GetMixFormat が null を返しました", pt: "GetMixFormat retornou null", zh: "GetMixFormat 返回了 null").into());
        }
        let wf: &WAVEFORMATEX = &*mix;
        let (rate, channels, bits) = (wf.nSamplesPerSec, wf.nChannels, wf.wBitsPerSample);
        let tag = wf.wFormatTag;
        let sub = if tag == 0xFFFE {
            std::ptr::addr_of!((*(mix as *const WAVEFORMATEXTENSIBLE)).SubFormat).read_unaligned()
        } else {
            windows::core::GUID::from_u128(0)
        };
        let float = tag == 3 || (tag == 0xFFFE && sub == windows::core::GUID::from_u128(IEEE_FLOAT));
        let pcm16 = tag == 1 && bits == 16 || (tag == 0xFFFE && sub == windows::core::GUID::from_u128(SUBTYPE_PCM) && bits == 16);
        if !(channels == 1 || channels == 2) {
            CoTaskMemFree(Some(mix as *const _));
            return Err(tf!(
                "в loopback-миксе {} каналов; поддерживаются только моно и стерео",
                "loopback mix has {} channels; only mono/stereo is supported",
                ja: "ループバックミックスは {} チャンネルです。モノラル/ステレオのみ対応しています",
                pt: "a mixagem de loopback tem {} canais; só mono/estéreo é suportado",
                zh: "回环混音有 {} 个声道；仅支持单声道/立体声",
                channels
            ));
        }
        if !float && !pcm16 {
            CoTaskMemFree(Some(mix as *const _));
            return Err(tf!(
                "неподдерживаемый формат loopback-микса (тег {}, {} бит)",
                "unsupported loopback mix format (tag {}, {} bits)",
                ja: "未対応のループバックミックス形式（タグ {}、{} ビット）",
                pt: "formato de mixagem de loopback não suportado (tag {}, {} bits)",
                zh: "不支持的回环混音格式（标记 {}，{} 位）",
                tag,
                bits
            ));
        }
        let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 20_000_000, 0, mix, None);
        CoTaskMemFree(Some(mix as *const _));
        init.map_err(|e| tf!("запуск loopback: {}", "loopback Initialize: {}", ja: "ループバック初期化: {}", pt: "inicialização do loopback: {}", zh: "回环初始化：{}", e))?;
        let capture = client.GetService::<IAudioCaptureClient>().map_err(|e| tf!("IAudioCaptureClient: {}", "IAudioCaptureClient: {}", ja: "IAudioCaptureClient: {}", pt: "IAudioCaptureClient: {}", zh: "IAudioCaptureClient：{}", e))?;
        client.Start().map_err(|e| tf!("запуск loopback: {}", "loopback Start: {}", ja: "ループバック開始: {}", pt: "início do loopback: {}", zh: "回环启动：{}", e))?;
        Ok(Audio { _client: client, capture, rate, channels, float, frames: 0 })
    }
}

impl Audio {
    pub fn format(&self) -> Format {
        Format { rate: self.rate, channels: self.channels }
    }

    /// Drains the loopback packets and writes AAC-encoder input samples. Returns how many audio frames were written.
    pub fn pump(&mut self, writer: &IMFSinkWriter, stream: u32) -> Result<u32, String> {
        unsafe {
            let mut written = 0u32;
            loop {
                let next = self.capture.GetNextPacketSize().map_err(|e| format!("GetNextPacketSize: {e}"))?;
                if next == 0 {
                    break;
                }
                let mut data: *mut u8 = null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .map_err(|e| format!("loopback GetBuffer: {e}"))?;
                if frames > 0 {
                    let samples = frames as usize * self.channels as usize;
                    let mut pcm = vec![0i16; samples];
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 && !data.is_null() {
                        if self.float {
                            let src = slice::from_raw_parts(data as *const f32, samples);
                            for (d, s) in pcm.iter_mut().zip(src) {
                                *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                            }
                        } else {
                            let src = slice::from_raw_parts(data as *const i16, samples);
                            pcm.copy_from_slice(src);
                        }
                    }
                    self.capture.ReleaseBuffer(frames).map_err(|e| format!("loopback ReleaseBuffer: {e}"))?;
                    let bytes = pcm.len() * 2;
                    let buffer: IMFMediaBuffer = MFCreateMemoryBuffer(bytes as u32).map_err(|e| format!("MFCreateMemoryBuffer(audio): {e}"))?;
                    let mut dst: *mut u8 = null_mut();
                    buffer.Lock(&mut dst, None, None).map_err(|e| format!("audio buffer Lock: {e}"))?;
                    std::ptr::copy_nonoverlapping(pcm.as_ptr() as *const u8, dst, bytes);
                    buffer.Unlock().map_err(|e| format!("audio buffer Unlock: {e}"))?;
                    buffer.SetCurrentLength(bytes as u32).map_err(|e| format!("audio SetCurrentLength: {e}"))?;
                    let sample = MFCreateSample().map_err(|e| format!("MFCreateSample(audio): {e}"))?;
                    sample.AddBuffer(&buffer).map_err(|e| format!("audio AddBuffer: {e}"))?;
                    let unit = 10_000_000i64;
                    let base = (self.frames as i64) * unit / self.rate.max(1) as i64;
                    let dur = frames as i64 * unit / self.rate.max(1) as i64;
                    sample.SetSampleTime(base).map_err(|e| format!("audio SetSampleTime: {e}"))?;
                    sample.SetSampleDuration(dur).map_err(|e| format!("audio SetSampleDuration: {e}"))?;
                    writer.WriteSample(stream, &sample).map_err(|e| format!("audio WriteSample: {e}"))?;
                    self.frames += frames as u64;
                    written += frames;
                } else {
                    self.capture.ReleaseBuffer(frames).map_err(|e| format!("loopback ReleaseBuffer: {e}"))?;
                }
            }
            Ok(written)
        }
    }
}


