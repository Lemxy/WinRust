//! Recording. This pass records a real frame sequence: GDI captures the chosen monitor/region (BitBlt) or window
//! (PrintWindow) at the requested fps and writes numbered JPEG frames to %TEMP%; if ffmpeg is on PATH the frames are
//! encoded to H.264/MP4, otherwise the frame folder is kept and reported. See tools/rs-shot-spec.md: Windows.Graphics.Capture
//! + Media Foundation is the planned next step, not what this code claims to do.
use std::{
    cell::RefCell,
    mem::zeroed,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::VK_ESCAPE,
        WindowsAndMessaging::*,
    },
};

use winrust_common::{
    monitors,
    style::{pal, rgb},
};

use crate::{cap, overlay, save, ui, wgc, Opts};

#[derive(Clone, Copy)]
pub enum Source {
    Rect(RECT),
    Window(HWND),
}

// HWND is an opaque handle; PrintWindow/BitBlt accept it from the recorder thread.
unsafe impl Send for Source {}

struct Ui {
    source: RECT,
    start: u64,
    duration: u32,
    fps: u32,
    stop: HWND,
    frame: HWND,
    hover: bool,
    stopped: bool,
}

thread_local! {
    static REC: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

static ESC_STOP: AtomicBool = AtomicBool::new(false);
static HOOK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// A low-level hook only for the length of a recording: a quick Esc tap is caught even when a timer tick would miss it,
/// and the recorded application does not receive the key.
unsafe extern "system" fn esc_hook(n: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if n >= 0 {
        let k = &*(l as *const KBDLLHOOKSTRUCT);
        if (w == WM_KEYDOWN as usize || w == WM_SYSKEYDOWN as usize) && k.vkCode == VK_ESCAPE as u32 {
            ESC_STOP.store(true, Relaxed);
            return 1;
        }
    }
    CallNextHookEx(HOOK.load(Relaxed) as _, n, w, l)
}

pub fn run(o: &Opts) -> i32 {
    unsafe { start(o) }
}

unsafe fn start(o: &Opts) -> i32 {
    if o.delay > 0 {
        std::thread::sleep(Duration::from_secs(o.delay as u64));
    }
    clean_stale_frames();
    let source = match o.mode {
        crate::Mode::Region => match overlay::select(o.cursor, false) {
            Some(p) if p.window.is_some() => Source::Window(p.window.unwrap()),
            Some(p) => Source::Rect(p.rect),
            None => return 0,
        },
        crate::Mode::Monitor => Source::Rect(cap::monitor_rect(o.monitor)),
        crate::Mode::Full => Source::Rect(cap::full_rect()),
        crate::Mode::Window => {
            let h = cap::foreground();
            if h.is_null() {
                crate::msg(t!("Нет окна для записи", "There is no foreground window to record", ja: "録画するウィンドウがありません", pt: "Não há janela em primeiro plano para gravar", zh: "没有可录制的前台窗口"), t!("rs-shot", "rs-shot"));
                return 2;
            }
            Source::Window(h)
        }
    };
    let rect = match source {
        Source::Rect(r) => r,
        Source::Window(h) => cap::window_rect(h),
    };
    if rect.right - rect.left < 8 || rect.bottom - rect.top < 8 {
        return 2;
    }
    let fps = o.fps.clamp(1, 60);
    let out = o.out.clone().map(norm_mp4).unwrap_or_else(save::default_video_path);
    if let Some(parent) = out.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            crate::msg(
                &tf!("Не удалось создать папку для видео: {}", "Could not create the video folder: {}", ja: "動画フォルダーを作成できません: {}", pt: "Não foi possível criar a pasta do vídeo: {}", zh: "无法创建视频文件夹：{}", e),
                t!("rs-shot", "rs-shot"),
            );
            return 4;
        }
    }

    // ---- encoder choice: WGC + Media Foundation first, ffmpeg only when WGC cannot take this source ----
    let target = wgc_target(source);
    let mut use_wgc = false;
    let mut wgc_err: Option<String> = None;
    if o.encoder != crate::Encoder::Ffmpeg && wgc::supported() {
        match target {
            Some(t) => match wgc::probe(&t, fps, o.cursor) {
                Ok(()) => use_wgc = true,
                Err(e) => {
                    let _ = std::fs::write(std::env::temp_dir().join("rs-shot-wgc.log"), format!("WGC probe failed: {e}\n{target:?}\n"));
                    wgc_err = Some(e);
                }
            },
            None => {
                if o.encoder == crate::Encoder::Wgc {
                    crate::msg(
                        &t!(
                            "Windows.Graphics.Capture захватывает один монитор или окно; весь виртуальный столбец из нескольких мониторов им не покрывается. Используйте --encoder ffmpeg.",
                            "Windows.Graphics.Capture captures one monitor or one window; the full multi-monitor desktop is not one item. Use --encoder ffmpeg.",
                            ja: "Windows.Graphics.Capture は 1 つのモニターまたはウィンドウをキャプチャします。複数モニター全体は 1 つの項目にならないため、--encoder ffmpeg を使ってください。",
                            pt: "O Windows.Graphics.Capture captura um monitor ou uma janela; a área de trabalho com vários monitores não é um único item. Use --encoder ffmpeg.",
                            zh: "Windows.Graphics.Capture 一次捕获一个显示器或一个窗口；整个多显示器桌面不是单个项目。请使用 --encoder ffmpeg。"
                        ),
                        t!("rs-shot", "rs-shot"),
                    );
                    return 4;
                }
            }
        }
    } else if o.encoder == crate::Encoder::Wgc {
        wgc_err = Some(t!("Windows.Graphics.Capture недоступен", "Windows.Graphics.Capture is not available", ja: "Windows.Graphics.Capture を利用できません", pt: "Windows.Graphics.Capture não está disponível", zh: "Windows.Graphics.Capture 不可用").to_string());
    }
    if o.encoder == crate::Encoder::Wgc && !use_wgc {
        crate::msg(&tf!("Запись через WGC не запустилась: {}", "WGC recording could not start: {}", ja: "WGC 録画を開始できません: {}", pt: "A gravação por WGC não iniciou: {}", zh: "WGC 录制无法启动：{}", wgc_err.unwrap_or_default()), t!("rs-shot", "rs-shot"));
        return 4;
    }
    let ffmpeg = if use_wgc { None } else { find_ffmpeg() };
    if !use_wgc && ffmpeg.is_none() {
        let detail = wgc_err.unwrap_or_else(|| t!("Windows.Graphics.Capture недоступен", "Windows.Graphics.Capture is not available", ja: "Windows.Graphics.Capture を利用できません", pt: "Windows.Graphics.Capture não está disponível", zh: "Windows.Graphics.Capture 不可用").to_string());
        crate::msg(
            &tf!(
                "Запись недоступна: {}.\n\nЗапасной вариант требует ffmpeg:\nwinget install --id Gyan.FFmpeg -e\n\nЗапись не начата.",
                "Recording is unavailable: {}.\n\nThe fallback needs ffmpeg:\nwinget install --id Gyan.FFmpeg -e\n\nNothing was recorded.",
                ja: "録画できません: {}。\n\nフォールバックには ffmpeg が必要です:\nwinget install --id Gyan.FFmpeg -e\n\n録画は開始していません。",
                pt: "A gravação não está disponível: {}.\n\nO fallback precisa do ffmpeg:\nwinget install --id Gyan.FFmpeg -e\n\nNada foi gravado.",
                zh: "无法录制：{}。\n\n备用方案需要 ffmpeg：\nwinget install --id Gyan.FFmpeg -e\n\n未开始录制。",
                detail
            ),
            t!("rs-shot", "rs-shot"),
        );
        return 4;
    }

    let dir = save::frames_dir();
    if !use_wgc {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            crate::msg(&tf!("Не удалось создать временную папку: {}", "Could not create the temporary folder: {}", ja: "一時フォルダーを作成できません: {}", pt: "Não foi possível criar a pasta temporária: {}", zh: "无法创建临时文件夹：{}", e), t!("rs-shot", "rs-shot"));
            return 4;
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let out2 = out.clone();
    let dir2 = dir.clone();
    let rec = if use_wgc {
        let target = target.unwrap();
        let cursor = o.cursor;
        std::thread::spawn(move || -> RecRes {
            match wgc::record(&target, fps, cursor, &out2, stop2) {
                Ok((frames, elapsed_ms)) => RecRes { frames, elapsed_ms, err: String::new(), direct: true },
                Err(e) => {
                    let _ = std::fs::write(std::env::temp_dir().join("rs-shot-wgc.log"), format!("WGC record failed: {e}\n"));
                    RecRes { frames: 0, elapsed_ms: 0, err: e, direct: true }
                }
            }
        })
    } else {
        std::thread::spawn(move || -> RecRes {
            let (frames, elapsed_ms, err) = record_loop(source, stop2, dir2, fps);
            RecRes { frames: frames as u64, elapsed_ms, err, direct: false }
        })
    };

    if let Err(e) = make_ui(rect, fps, o.duration) {
        stop.store(true, Relaxed);
        let _ = rec.join();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&out);
        crate::msg(&e, t!("rs-shot", "rs-shot"));
        return 4;
    }
    REC.with_borrow_mut(|r| {
        if let Some(r) = r {
            r.start = Ui::now();
        }
    });

    ESC_STOP.store(false, Relaxed);
    let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(esc_hook), GetModuleHandleW(null()), 0);
    HOOK.store(hook as isize, Relaxed);
    let mut m: MSG = zeroed();
    while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
    if !hook.is_null() {
        UnhookWindowsHookEx(hook);
    }
    HOOK.store(0, Relaxed);
    stop.store(true, Relaxed);
    let res = rec.join().unwrap_or(RecRes { frames: 0, elapsed_ms: 0, err: "recorder thread panicked".into(), direct: false });
    destroy_ui();

    if res.direct {
        if res.frames > 0 && res.err.is_empty() && out.is_file() {
            let secs = (res.elapsed_ms as f64 / 1000.0).max(0.05);
            let real_fps = (res.frames as f64 / secs).clamp(1.0, 120.0);
            crate::msg(
                &tf!("Готово (WGC + Media Foundation, H.264/MP4, {} кадров, {:.1} fps):\n{}", "Done (WGC + Media Foundation, H.264/MP4, {} frames, {:.1} fps):\n{}", ja: "完了 (WGC + Media Foundation、H.264/MP4、{} フレーム、{:.1} fps):\n{}", pt: "Pronto (WGC + Media Foundation, H.264/MP4, {} quadros, {:.1} fps):\n{}", zh: "完成（WGC + Media Foundation，H.264/MP4，{} 帧，{:.1} fps）：\n{}", res.frames, real_fps, out.display()),
                t!("rs-shot", "rs-shot"),
            );
            return 0;
        }
        let _ = std::fs::remove_file(&out);
        crate::msg(
            &tf!("WGC/Media Foundation не смог записать видео; частичный файл удалён.\n{}", "WGC/Media Foundation could not record the video; the partial file was removed.\n{}", ja: "WGC/Media Foundation は録画できませんでした。途中のファイルは削除しました。\n{}", pt: "WGC/Media Foundation não conseguiu gravar o vídeo; o arquivo parcial foi removido.\n{}", zh: "WGC/Media Foundation 无法录制视频；已删除不完整文件。\n{}", if res.err.is_empty() { "no frames encoded".to_string() } else { res.err }),
            t!("rs-shot", "rs-shot"),
        );
        return 4;
    }

    // ---- ffmpeg bridge ----
    if res.frames == 0 {
        let _ = std::fs::remove_dir_all(&dir);
        crate::msg(
            &tf!("Кадры не записались: {}", "No frames were captured: {}", ja: "フレームを取得できませんでした: {}", pt: "Nenhum quadro foi capturado: {}", zh: "未能捕获帧：{}", if res.err.is_empty() { t!("неизвестная ошибка", "unknown error") } else { &res.err }),
            t!("rs-shot", "rs-shot"),
        );
        return 4;
    }
    let secs = (res.elapsed_ms as f64 / 1000.0).max(0.05);
    let real_fps = (res.frames as f64 / secs).clamp(1.0, 120.0);
    let ffmpeg = ffmpeg.unwrap();
    match to_mp4(&ffmpeg, &dir, real_fps, &out) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&dir); // no orphan frames on success
            crate::msg(
                &tf!("Готово (ffmpeg, H.264/MP4, {} кадров, {:.1} fps):\n{}", "Done (ffmpeg, H.264/MP4, {} frames, {:.1} fps):\n{}", ja: "完了 (ffmpeg、H.264/MP4、{} フレーム、{:.1} fps):\n{}", pt: "Pronto (ffmpeg, H.264/MP4, {} quadros, {:.1} fps):\n{}", zh: "完成（ffmpeg，H.264/MP4，{} 帧，{:.1} fps）：\n{}", res.frames, real_fps, out.display()),
                t!("rs-shot", "rs-shot"),
            );
            0
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir); // and no orphan frames on failure either
            crate::msg(
                &tf!("ffmpeg не смог собрать MP4; временные кадры удалены.\n{}", "ffmpeg could not build the MP4; the temporary frames were removed.\n{}", ja: "ffmpeg は MP4 を作成できませんでした。一時フレームは削除しました。\n{}", pt: "O ffmpeg não conseguiu criar o MP4; os quadros temporários foram removidos.\n{}", zh: "ffmpeg 无法生成 MP4；临时帧已删除。\n{}", e),
                t!("rs-shot", "rs-shot"),
            );
            4
        }
    }
}

struct RecRes {
    frames: u64,
    elapsed_ms: u128,
    err: String,
    direct: bool,
}

/// One monitor or one window is a single WGC item. The full virtual desktop (or a region crossing monitors) is not;
/// those sources use the ffmpeg frame bridge instead.
fn wgc_target(source: Source) -> Option<wgc::Target> {
    match source {
        Source::Window(h) => Some(wgc::window_target(h as isize)),
        Source::Rect(r) => {
            let mon = monitors::all().into_iter().find(|m| {
                m.rc.left <= r.left && m.rc.top <= r.top && m.rc.right >= r.right && m.rc.bottom >= r.bottom
            })?;
            Some(wgc::monitor_target(mon.handle as isize, (mon.rc.left, mon.rc.top, mon.rc.right, mon.rc.bottom), (r.left, r.top, r.right, r.bottom)))
        }
    }
}

fn norm_mp4(p: PathBuf) -> PathBuf {
    if p.extension().is_none() { p.with_extension("mp4") } else { p }
}

impl Ui {
    fn now() -> u64 {
        unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
    }
}

unsafe fn record_loop(source: Source, stop: Arc<AtomicBool>, dir: PathBuf, fps: u32) -> (u32, u128, String) {
    let t0 = Instant::now();
    let interval = Duration::from_secs_f64(1.0 / fps as f64);
    let mut n = 0u32;
    let mut err = String::new();
    while !stop.load(Relaxed) {
        let tick = Instant::now();
        let bmp = match source {
            Source::Rect(r) => cap::grab_rect(r, false),
            Source::Window(h) => cap::grab_window(h, false),
        };
        match bmp {
            Some(b) => {
                let p = dir.join(format!("frame-{:06}.jpg", n + 1));
                if save::encode_jpeg(b, &p) {
                    n += 1;
                } else if err.is_empty() {
                    err = format!("JPEG encode failed for {}", p.display());
                }
                DeleteObject(b as HGDIOBJ);
            }
            None if err.is_empty() => err = "screen capture returned no bitmap".to_string(),
            None => {}
        }
        let spent = tick.elapsed();
        if spent < interval {
            std::thread::sleep(interval - spent);
        }
    }
    (n, t0.elapsed().as_millis(), err)
}

unsafe fn make_ui(rect: RECT, fps: u32, duration: u32) -> Result<(), String> {
    let hi = GetModuleHandleW(null());
    let cls = ui::wide("rs-shot-recbar");
    let mut wc: WNDCLASSW = zeroed();
    wc.lpfnWndProc = Some(bar_proc);
    wc.hInstance = hi;
    wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
    wc.lpszClassName = cls.as_ptr();
    RegisterClassW(&wc);
    let fcls = ui::wide("rs-shot-recbox");
    let mut fc: WNDCLASSW = zeroed();
    fc.lpfnWndProc = Some(box_proc);
    fc.hInstance = hi;
    fc.lpszClassName = fcls.as_ptr();
    RegisterClassW(&fc);

    // accent frame around the source, hollow through SetWindowRgn
    let fw = rect.right - rect.left + 6;
    let fh = rect.bottom - rect.top + 6;
    let frame = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
        fcls.as_ptr(),
        fcls.as_ptr(),
        WS_POPUP | WS_VISIBLE,
        rect.left - 3,
        rect.top - 3,
        fw,
        fh,
        null_mut(),
        null_mut(),
        hi,
        null(),
    );
    if !frame.is_null() {
        let outer = CreateRectRgn(0, 0, fw, fh);
        let inner = CreateRectRgn(3, 3, fw - 3, fh - 3);
        CombineRgn(outer, outer, inner, RGN_DIFF);
        SetWindowRgn(frame, outer, 1);
        DeleteObject(inner as HGDIOBJ);
        SetWindowDisplayAffinity(frame, WDA_EXCLUDEFROMCAPTURE); // never record our own border
    }

    let mon = monitors::at(rect.left + (rect.right - rect.left) / 2, rect.top + (rect.bottom - rect.top) / 2);
    let sys_dpi = windows_sys::Win32::UI::HiDpi::GetDpiForSystem() as i32;
    let bw = (310 * sys_dpi.max(96) / 96).max(310);
    let bh = (68 * sys_dpi.max(96) / 96).max(64);
    let bx = mon.work.left + (mon.work.right - mon.work.left - bw) / 2;
    let by = mon.work.top + 12;
    let title = t!("rs-shot — запись", "rs-shot — recording", ja: "rs-shot — 録画", pt: "rs-shot — gravando", zh: "rs-shot — 录制");
    let bar = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        cls.as_ptr(),
        ui::wide(title).as_ptr(),
        WS_POPUP | WS_VISIBLE,
        bx,
        by,
        bw,
        bh,
        null_mut(),
        null_mut(),
        hi,
        null(),
    );
    if bar.is_null() {
        if !frame.is_null() {
            DestroyWindow(frame);
        }
        return Err(t!("Не удалось открыть панель записи", "Could not open the recording bar", ja: "録画バーを開けませんでした", pt: "Não foi possível abrir a barra de gravação", zh: "无法打开录制栏").to_string());
    }
    SetWindowDisplayAffinity(bar, WDA_EXCLUDEFROMCAPTURE);
    REC.set(Some(Ui { source: rect, start: Ui::now(), duration, fps, stop: bar, frame, hover: false, stopped: false }));
    SetTimer(bar, 1, 200, None);
    ShowWindow(bar, SW_SHOW);
    Ok(())
}

unsafe fn destroy_ui() {
    let (bar, frame) = REC.with_borrow(|r| r.as_ref().map(|r| (r.stop, r.frame)).unwrap_or((null_mut(), null_mut())));
    if !bar.is_null() {
        DestroyWindow(bar);
    }
    if !frame.is_null() {
        DestroyWindow(frame);
    }
    REC.set(None);
}

unsafe fn stop_now() {
    let finished = REC.with_borrow_mut(|r| {
        if let Some(r) = r {
            if r.stopped {
                return true;
            }
            r.stopped = true;
        }
        false
    });
    if finished {
        return;
    }
    // destroy outside the borrow above
    let (bar, frame) = REC.with_borrow(|r| r.as_ref().map(|r| (r.stop, r.frame)).unwrap_or((null_mut(), null_mut())));
    if !bar.is_null() {
        DestroyWindow(bar);
    }
    if !frame.is_null() {
        DestroyWindow(frame);
    }
    PostQuitMessage(0);
}

fn elapsed_text(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}", s / 60, s % 60)
}

unsafe fn bar_paint(hw: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hw, &mut ps);
    let mut c: RECT = zeroed();
    GetClientRect(hw, &mut c);
    ui::fill(hdc, c, pal().bg);
    ui::outline(hdc, c, pal().line, 1);
    let (start, duration, fps, source, hover, stopped) = REC.with_borrow(|r| {
        r.as_ref().map(|r| (r.start, r.duration, r.fps, r.source, r.hover, r.stopped)).unwrap_or((0, 0, 30, RECT { left: 0, top: 0, right: 0, bottom: 0 }, false, false))
    });
    if stopped {
        EndPaint(hw, &ps);
        return;
    }
    let ms = Ui::now().saturating_sub(start);
    // pulsing dot
    let on = (ms / 500) % 2 == 0;
    let dot = CreateSolidBrush(if on { rgb(255, 69, 58) } else { rgb(120, 30, 26) });
    let old = SelectObject(hdc, dot as HGDIOBJ);
    let old_b = SelectObject(hdc, GetStockObject(NULL_PEN) as HGDIOBJ);
    Ellipse(hdc, 18, c.bottom / 2 - 7, 32, c.bottom / 2 + 7);
    SelectObject(hdc, old);
    SelectObject(hdc, old_b);
    DeleteObject(dot as HGDIOBJ);
    let f = ui::make_font(ui::sc(hw, 15), 700, winrust_common::style::ui_face());
    let f2 = ui::make_font(ui::sc(hw, 11), 400, winrust_common::style::ui_face());
    let rec = t!("ЗАПИСЬ", "REC", ja: "録画中", pt: "GRAVANDO", zh: "录制中");
    ui::text(hdc, &format!("{rec}   {}", elapsed_text(ms)), RECT { left: 40, top: 6, right: c.right - 100, bottom: 32 }, f, pal().text, 0);
    let dims = format!("{}×{}  ·  {} fps{}", source.right - source.left, source.bottom - source.top, fps, if duration > 0 { format!("  ·  max {}s", duration) } else { String::new() });
    ui::text(hdc, &dims, RECT { left: 40, top: 30, right: c.right - 100, bottom: 54 }, f2, pal().dim, 0);
    let br = RECT { left: c.right - ui::sc(hw, 96), top: ui::sc(hw, 15), right: c.right - ui::sc(hw, 12), bottom: c.bottom - ui::sc(hw, 15) };
    ui::rounded(hdc, br, if hover { rgb(220, 50, 45) } else { rgb(170, 38, 35) }, ui::sc(hw, 8));
    let stop = t!("Стоп", "Stop", ja: "停止", pt: "Parar", zh: "停止");
    let sf = ui::make_font(ui::sc(hw, 13), 600, winrust_common::style::ui_face());
    ui::text(hdc, stop, br, sf, pal().text, DT_CENTER as u32);
    DeleteObject(sf as HGDIOBJ);
    DeleteObject(f as HGDIOBJ);
    DeleteObject(f2 as HGDIOBJ);
    EndPaint(hw, &ps);
}

unsafe extern "system" fn bar_proc(hw: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let x = (lp & 0xffff) as i16 as i32;
    let y = ((lp >> 16) & 0xffff) as i16 as i32;
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            bar_paint(hw);
            0
        }
        WM_MOUSEMOVE => {
            let mut c: RECT = zeroed();
            GetClientRect(hw, &mut c);
            let br = RECT { left: c.right - ui::sc(hw, 96), top: ui::sc(hw, 15), right: c.right - ui::sc(hw, 12), bottom: c.bottom - ui::sc(hw, 15) };
            let hover = x >= br.left && x < br.right && y >= br.top && y < br.bottom;
            REC.with_borrow_mut(|r| {
                if let Some(r) = r {
                    if r.hover != hover {
                        r.hover = hover;
                    }
                }
            });
            InvalidateRect(hw, null(), 0);
            0
        }
        WM_LBUTTONUP => {
            stop_now();
            0
        }
        WM_TIMER if wp == 1 => {
            InvalidateRect(hw, null(), 0);
            let (start, duration) = REC.with_borrow(|r| r.as_ref().map(|r| (r.start, r.duration)).unwrap_or((0, 0)));
            let el = Ui::now().saturating_sub(start) / 1000;
            if ESC_STOP.swap(false, Relaxed) || (duration > 0 && el >= duration as u64) {
                stop_now();
            }
            0
        }
        WM_CLOSE => {
            stop_now();
            0
        }
        WM_DESTROY => 0,
        _ => DefWindowProcW(hw, msg, wp, lp),
    }
}

unsafe extern "system" fn box_proc(hw: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let hdc = BeginPaint(hw, &mut ps);
            let mut c: RECT = zeroed();
            GetClientRect(hw, &mut c);
            ui::fill(hdc, c, pal().accent);
            EndPaint(hw, &ps);
            0
        }
        _ => DefWindowProcW(hw, msg, wp, lp),
    }
}

/// ffmpeg lookup, in the order the shell integration documents:
///   1. `ffmpeg.exe` beside `rs-shot.exe`
///   2. `%ProgramFiles%\rs-shot\ffmpeg.exe`
///   3. `%ProgramFiles%\ffmpeg\*\bin\ffmpeg.exe` (versioned install, e.g. `ffmpeg-9.1-essentials_build`)
///   4. every directory on PATH
///   5. WinGet links and `%LOCALAPPDATA%\Microsoft\WinGet\Packages\Gyan.FFmpeg*`
fn find_ffmpeg() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("ffmpeg.exe");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let pf = PathBuf::from(pf);
        let beside = pf.join(r"rs-shot\ffmpeg.exe");
        if beside.is_file() {
            return Some(beside);
        }
        if let Some(p) = find_below(&pf.join("ffmpeg"), 3) {
            return Some(p);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in ["ffmpeg.exe", "ffmpeg"] {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        let winget = PathBuf::from(la).join(r"Microsoft\WinGet");
        let link = winget.join(r"Links\ffmpeg.exe");
        if link.is_file() {
            return Some(link);
        }
        if let Ok(rd) = std::fs::read_dir(winget.join("Packages")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                if name.starts_with("gyan.ffmpeg") {
                    if let Some(p) = find_below(&e.path(), 4) {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

/// Bounded recursive search for `ffmpeg.exe` (direct, or in a `bin` subfolder).
fn find_below(dir: &Path, depth: u32) -> Option<PathBuf> {
    for rel in ["ffmpeg.exe", r"bin\ffmpeg.exe"] {
        let p = dir.join(rel);
        if p.is_file() {
            return Some(p);
        }
    }
    if depth == 0 {
        return None;
    }
    let rd = std::fs::read_dir(dir).ok()?;
    for e in rd.flatten() {
        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            if let Some(p) = find_below(&e.path(), depth - 1) {
                return Some(p);
            }
        }
    }
    None
}

/// Removes frame folders a killed rs-shot left behind (older than 12 h; a live recording is never touched).
fn clean_stale_frames() {
    let base = std::env::temp_dir();
    let Ok(rd) = std::fs::read_dir(base) else { return };
    let now = std::time::SystemTime::now();
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with("rs-shot-rec-") {
            continue;
        }
        let old = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d > Duration::from_secs(12 * 3600))
            .unwrap_or(false);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Encodes the JPEG frame sequence. `%06d` numbering starts at 1; the measured fps keeps a slow encoder in real time.
fn to_mp4(ff: &Path, dir: &Path, fps: f64, out: &Path) -> Result<(), String> {
    let pattern = dir.join("frame-%06d.jpg");
    let output = std::process::Command::new(ff)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-y")
        .arg("-framerate")
        .arg(format!("{fps:.3}"))
        .arg("-start_number")
        .arg("1")
        .arg("-i")
        .arg(&pattern)
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg(out)
        .output()
        .map_err(|e| format!("{}: {e}", ff.display()))?;
    if output.status.success() {
        Ok(())
    } else {
        let mut s = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if s.len() > 1200 {
            s.truncate(1200);
        }
        Err(if s.is_empty() { format!("ffmpeg exited with {}", output.status) } else { s })
    }
}
