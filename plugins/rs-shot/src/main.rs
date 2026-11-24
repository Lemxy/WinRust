//! rs-shot: WinRust's screenshot and screen-recording tool, in the spirit of KDE Spectacle.
//!
//! Default (no arguments): the region overlay, then the annotation editor. `--no-ui` writes and copies at once.
//! Recording (`--record`) uses Windows.Graphics.Capture + Media Foundation for a window/monitor, and falls back to an
//! ffmpeg-encoded JPEG frame sequence when WGC cannot take the source (or on Windows builds with the CreateForMonitor
//! regression). See tools/rs-shot-spec.md for the full list.
#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
#[macro_use]
extern crate winrust_common;

mod annot;
mod cap;
mod editor;
mod img;
mod overlay;
mod record;
mod save;
mod ui;
mod wgc;

use std::{path::PathBuf, thread::sleep, time::Duration};

use windows_sys::Win32::UI::{
    HiDpi::SetProcessDpiAwarenessContext,
    WindowsAndMessaging::{MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Region,
    Full,
    Monitor,
    Window,
}

/// Recording encoder: WGC + Media Foundation is the built-in path; ffmpeg is the frame-sequence bridge.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Encoder {
    Auto,
    Wgc,
    Ffmpeg,
}

#[derive(Clone)]
pub(crate) struct Opts {
    pub mode: Mode,
    pub delay: u32,
    pub cursor: bool,
    pub clipboard: bool,
    pub file: Option<PathBuf>,
    pub no_ui: bool,
    pub record: bool,
    pub monitor: Option<usize>,
    pub fps: u32,
    pub duration: u32,
    pub out: Option<PathBuf>,
    pub audio: bool,
    pub encoder: Encoder,
}

fn main() {
    winrust_common::appid::set("WinRust");
    unsafe {
        SetProcessDpiAwarenessContext(-4isize as _); // per-monitor v2: one physical coordinate space for the virtual desktop
        save::gdiplus_init();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        msg(USAGE, "rs-shot");
        return;
    }
    let o = match parse(&args) {
        Ok(o) => o,
        Err(e) => {
            err(&format!("{e}\n\n{USAGE}"), "rs-shot");
            std::process::exit(1);
        }
    };
    let code = if o.record { start_record(&o) } else { capture(&o) };
    std::process::exit(code);
}

const USAGE: &str = "\
rs-shot — WinRust screenshot and screen recorder

Usage:
  rs-shot                         region overlay + editor (default)
  rs-shot --full                  all monitors, editor
  rs-shot --monitor [N]           monitor under the cursor, or number N (1 = leftmost), editor
  rs-shot --window                foreground window, editor
  rs-shot --region                region overlay + editor

Capture options:
  --delay N                       wait N seconds before capturing (0, 2, 5, 10, ...)
  --cursor                        include the mouse pointer in the capture
  --clipboard                     with --no-ui: also copy to the clipboard (it is copied anyway)
  --file PATH                     save here instead of Pictures\\Screenshots\\Screenshot_<date>.png
  --no-ui                         skip the editor: save and copy immediately

Recording (no audio in this pass; WGC + Media Foundation first, ffmpeg bridge when needed):
  --record                        record the region overlay selection
  --record --monitor [N]          record a monitor
  --record --window               record the foreground window
  --duration N                    stop after N seconds (default: until Stop/Esc)
  --fps N                         frames per second (default 30, 1..60)
  --out PATH                      MP4 path (default Videos\\rs-shot\\Recording_<date>.mp4)
  --encoder auto|wgc|ffmpeg       auto (default): WGC + Media Foundation when it works, else ffmpeg
  --audio                         not implemented yet; the flag is accepted and ignored

Keys: PrintScreen / Win+Shift+S = region capture, Win+Alt+R = region recording (through the WinRust shell).
      In the editor: Ctrl+S save+copy, Ctrl+C copy, Ctrl+Z/Y undo/redo, Esc close.
";

fn parse(a: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        mode: Mode::Region,
        delay: 0,
        cursor: false,
        clipboard: false,
        file: None,
        no_ui: false,
        record: false,
        monitor: None,
        fps: 30,
        duration: 0,
        out: None,
        audio: false,
        encoder: Encoder::Auto,
    };
    let mut i = 0;
    while i < a.len() {
        let take = |i: &mut usize| -> String {
            *i += 1;
            a.get(*i).cloned().unwrap_or_default()
        };
        match a[i].as_str() {
            "--full" => o.mode = Mode::Full,
            "--region" => o.mode = Mode::Region,
            "--window" => o.mode = Mode::Window,
            "--monitor" => {
                o.mode = Mode::Monitor;
                if let Some(n) = a.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                    o.monitor = Some(n.saturating_sub(1));
                    i += 1;
                }
            }
            "--delay" => {
                let v = take(&mut i);
                o.delay = v.parse::<u32>().map_err(|_| format!("--delay needs seconds, got {v:?}"))?.min(600);
            }
            "--cursor" => o.cursor = true,
            "--no-cursor" => o.cursor = false,
            "--clipboard" => o.clipboard = true,
            "--file" => o.file = Some(PathBuf::from(take(&mut i))),
            "--no-ui" => o.no_ui = true,
            "--record" => o.record = true,
            "--fps" => {
                let v = take(&mut i);
                o.fps = v.parse::<u32>().map_err(|_| format!("--fps needs a number, got {v:?}"))?.clamp(1, 60);
            }
            "--duration" => {
                let v = take(&mut i);
                o.duration = v.parse::<u32>().map_err(|_| format!("--duration needs seconds, got {v:?}"))?.min(86_400);
            }
            "--out" => o.out = Some(PathBuf::from(take(&mut i))),
            "--encoder" => {
                let v = take(&mut i);
                o.encoder = match v.to_ascii_lowercase().as_str() {
                    "auto" => Encoder::Auto,
                    "wgc" | "mf" | "mediafoundation" => Encoder::Wgc,
                    "ffmpeg" => Encoder::Ffmpeg,
                    _ => return Err(format!("--encoder needs auto, wgc or ffmpeg, got {v:?}")),
                };
            }
            "--audio" => o.audio = true,
            other => return Err(format!("unknown option {other:?}")),
        }
        i += 1;
    }
    if o.audio {
        // accepted on purpose: the spec marks audio as a pending feature, and scripts can already pass it
    }
    Ok(o)
}

/// Interactive `--record`: the same selection bar as PrintScreen, preselected on Vídeo + Región. Explicit
/// `--monitor/--window/--full` and `--no-ui` keep the direct, bar-less path.
fn start_record(o: &Opts) -> i32 {
    if o.no_ui || o.mode != Mode::Region {
        return record::run(o);
    }
    let Some(pick) = overlay::choose(overlay::Options {
        video: true,
        kind: overlay::Kind::Region,
        delay: o.delay,
        cursor: o.cursor,
        want_image: false,
    }) else {
        return 0;
    };
    after_pick(pick, o)
}

fn capture(o: &Opts) -> i32 {
    if o.mode == Mode::Region {
        let Some(pick) = overlay::choose(overlay::Options {
            video: false,
            kind: overlay::Kind::Region,
            delay: o.delay,
            cursor: o.cursor,
            want_image: true,
        }) else {
            return 0;
        };
        return after_pick(pick, o);
    }
    sleep_delay(o.delay);
    let img = unsafe {
        match o.mode {
            Mode::Full => cap::capture_rect(cap::full_rect(), o.cursor),
            Mode::Monitor => cap::capture_rect(cap::monitor_rect(o.monitor), o.cursor),
            Mode::Window => {
                let h = cap::foreground();
                if h.is_null() { None } else { cap::capture_window(h, o.cursor) }
            }
            Mode::Region => None,
        }
    };
    finish_image(img, o)
}

/// The selection bar can switch between Photo and Video and between Region/Full/Monitor/Window.
fn after_pick(pick: overlay::Pick, o: &Opts) -> i32 {
    sleep_delay(pick.delay);
    if pick.video {
        let source = match pick.kind {
            overlay::Kind::Region => pick.window.map(record::Source::Window).unwrap_or(record::Source::Rect(pick.rect)),
            overlay::Kind::Full => record::Source::Rect(unsafe { cap::full_rect() }),
            overlay::Kind::Monitor => record::Source::Rect(pick.rect),
            overlay::Kind::Window => {
                let h = pick.window.unwrap_or_else(|| unsafe { cap::foreground() });
                if h.is_null() {
                    return 2;
                }
                record::Source::Window(h)
            }
        };
        let mut r = o.clone();
        r.record = true;
        r.delay = 0;
        return record::run_source(&r, source);
    }
    let img = unsafe {
        match pick.kind {
            overlay::Kind::Region => {
                if pick.delay == 0 && pick.image.is_some() {
                    pick.image
                } else {
                    cap::capture_rect(pick.rect, o.cursor)
                }
            }
            overlay::Kind::Full => cap::capture_rect(cap::full_rect(), o.cursor),
            overlay::Kind::Monitor => cap::capture_rect(pick.rect, o.cursor),
            overlay::Kind::Window => match pick.window {
                Some(h) => cap::capture_window(h, o.cursor),
                None => {
                    let h = cap::foreground();
                    if h.is_null() { None } else { cap::capture_window(h, o.cursor) }
                }
            },
        }
    };
    finish_image(img, o)
}

fn sleep_delay(seconds: u32) {
    if seconds > 0 {
        sleep(Duration::from_secs(seconds as u64));
    }
}

fn finish_image(img: Option<img::Image>, o: &Opts) -> i32 {
    let Some(img) = img else {
        msg(
            t!("Не удалось сделать снимок экрана", "Could not take the screenshot", ja: "スクリーンショットを撮れませんでした", pt: "Não foi possível capturar a tela", zh: "无法截取屏幕"),
            t!("rs-shot", "rs-shot"),
        );
        return 2;
    };
    if !o.no_ui {
        return editor::run(img, o.file.clone());
    }
    let mut path = o.file.clone().unwrap_or_else(save::auto_path);
    if path.extension().is_none() {
        path.set_extension("png");
    }
    let saved = unsafe { save::save(&img, &path) };
    let copied = unsafe { save::copy_to_clipboard(&img) };
    if saved || copied {
        0
    } else {
        msg(
            t!("Не удалось сохранить снимок", "Could not save the screenshot", ja: "スクリーンショットを保存できませんでした", pt: "Não foi possível salvar a captura", zh: "无法保存截图"),
            t!("rs-shot", "rs-shot"),
        );
        3
    }
}

pub(crate) fn msg(text: &str, title: &str) {
    unsafe {
        let t: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let m: Vec<u16> = text.encode_utf16().chain([0]).collect();
        MessageBoxW(null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
}

pub(crate) fn err(text: &str, title: &str) {
    unsafe {
        let t: Vec<u16> = title.encode_utf16().chain([0]).collect();
        let m: Vec<u16> = text.encode_utf16().chain([0]).collect();
        MessageBoxW(null_mut(), m.as_ptr(), t.as_ptr(), MB_OK | MB_ICONERROR);
    }
}

use std::ptr::null_mut;

