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
        mpsc,
    },
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_ALT, MOD_NOREPEAT, MOD_WIN, VK_ESCAPE},
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
    stop_rec: Arc<AtomicBool>,
    audio: bool,
    back: HBITMAP,
    bdc: HDC,
    bw: i32,
    bh: i32,
}

thread_local! {
    static REC: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

static ESC_STOP: AtomicBool = AtomicBool::new(false);
static HOOK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
const HOTKEY_STOP: i32 = 0x5254; // Win+Alt+S while the recording bar exists

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
    unsafe { start(o, None) }
}

/// Recording with a source already chosen by the selection bar (`Region`/`Full`/`Monitor`/`Window`).
pub fn run_source(o: &Opts, source: Source) -> i32 {
    unsafe { start(o, Some(source)) }
}

unsafe fn start(o: &Opts, preset: Option<Source>) -> i32 {
    if o.delay > 0 {
        std::thread::sleep(Duration::from_secs(o.delay as u64));
    }
    clean_stale_frames();
    let source = match preset {
        Some(s) => s,
        None => match o.mode {
            crate::Mode::Region if o.no_ui => Source::Rect(cap::monitor_rect(None)), // no overlay to select with: cursor monitor
            crate::Mode::Region => {
                let Some(p) = overlay::select_direct(o.cursor, false, 0) else {
                    return 0;
                };
                if let Some(h) = p.window {
                    Source::Window(h)
                } else {
                    Source::Rect(p.rect)
                }
            }
            crate::Mode::Monitor => Source::Rect(cap::monitor_rect(o.monitor)),
            crate::Mode::Full => Source::Rect(cap::full_rect()),
            crate::Mode::Window => {
                let h = cap::foreground();
                if h.is_null() {
                    crate::msg(t!("Нет окна для записи", "There is no foreground window to record", ja: "録画するウィンドウがありません", pt: "Não há janela em primeiro plano para gravar", zh: "没有可录制的前台窗口"), crate::TITLE);
                    return 2;
                }
                Source::Window(h)
            }
        },
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
                crate::TITLE,
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
                        crate::TITLE,
                    );
                    return 4;
                }
            }
        }
    } else if o.encoder == crate::Encoder::Wgc {
        wgc_err = Some(t!("Windows.Graphics.Capture недоступен", "Windows.Graphics.Capture is not available", ja: "Windows.Graphics.Capture を利用できません", pt: "Windows.Graphics.Capture não está disponível", zh: "Windows.Graphics.Capture 不可用").to_string());
    }
    if o.encoder == crate::Encoder::Wgc && !use_wgc {
        crate::msg(&tf!("Запись через WGC не запустилась: {}", "WGC recording could not start: {}", ja: "WGC 録画を開始できません: {}", pt: "A gravação por WGC não iniciou: {}", zh: "WGC 录制无法启动：{}", wgc_err.unwrap_or_default()), crate::TITLE);
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
            crate::TITLE,
        );
        return 4;
    }

    let dir = save::frames_dir();
    if !use_wgc {
        if let Err(e) = std::fs::create_dir_all(&dir) {
            crate::msg(&tf!("Не удалось создать временную папку: {}", "Could not create the temporary folder: {}", ja: "一時フォルダーを作成できません: {}", pt: "Não foi possível criar a pasta temporária: {}", zh: "无法创建临时文件夹：{}", e), crate::TITLE);
            return 4;
        }
    }
    if o.no_ui && o.duration == 0 {
        report_error(
            &t!(
                "Запись без интерфейса требует --duration N (секунд): завершить её больше нечем.",
                "Headless recording needs --duration N (seconds): there is no Stop button to end it.",
                ja: "UI なしの録画には --duration N (秒) が必要です。停止ボタンがありません。",
                pt: "A gravação sem interface precisa de --duration N (segundos): não há botão Parar.",
                zh: "无界面录制需要 --duration N（秒）：没有停止按钮。"
            ),
            false,
        );
        return 4;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();
    let out2 = out.clone();
    let dir2 = dir.clone();
    let (tx, rx) = mpsc::channel::<RecRes>();
    if use_wgc {
        let target = target.unwrap();
        let cursor = o.cursor;
        let with_audio = o.audio;
        std::thread::spawn(move || {
            let res = match wgc::record(&target, fps, cursor, &out2, stop2, with_audio) {
                Ok((frames, elapsed_ms, audio, note)) => RecRes { frames, elapsed_ms, err: String::new(), audio, note },
                Err(e) => {
                    let _ = std::fs::write(std::env::temp_dir().join("rs-shot-wgc.log"), format!("WGC record failed: {e}\n"));
                    RecRes { frames: 0, elapsed_ms: 0, err: e, audio: false, note: None }
                }
            };
            let _ = tx.send(res);
        });
    } else {
        let audio_note = o.audio.then(|| {
            t!(
                "звук недоступен через мост ffmpeg; видео без звука",
                "audio is not available with the ffmpeg bridge; video is silent",
                ja: "ffmpeg ブリッジでは音声を録れません。映像のみになります",
                pt: "o áudio não está disponível com a ponte ffmpeg; o vídeo fica sem som",
                zh: "ffmpeg 桥接不支持音频；视频将没有声音"
            )
            .to_string()
        });
        std::thread::spawn(move || {
            let (frames, elapsed_ms, err) = record_loop(source, stop2, dir2, fps);
            let _ = tx.send(RecRes { frames: frames as u64, elapsed_ms, err, audio: false, note: audio_note });
        });
    }

    // Headless: no recording bar, no border, no completion dialog. Sleep for --duration, stop, finalize, exit.
    if o.no_ui {
        let deadline = Instant::now() + Duration::from_secs(o.duration as u64);
        let mut early: Option<RecRes> = None;
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(r) => {
                    early = Some(r);
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => break,
                Err(mpsc::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(40)),
            }
        }
        stop.store(true, Relaxed);
        let res = early
            .or_else(|| rx.recv_timeout(Duration::from_secs(15)).ok())
            .unwrap_or_else(|| {
                report_error(
                    t!(
                        "рекордер не завершился; сдаёмся",
                        "the recorder did not finish; giving up",
                        ja: "レコーダーが終了しませんでした。中止します",
                        pt: "o gravador não terminou; desistindo",
                        zh: "录制器未完成；放弃"
                    ),
                    false,
                );
                std::process::exit(4);
            });
        return finish(res, use_wgc, ffmpeg, &dir, &out, false);
    }

    if let Err(e) = make_ui(rect, fps, o.duration, stop.clone(), o.audio) {
        stop.store(true, Relaxed);
        let _ = rx.recv_timeout(Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&out);
        report_error(&e, true);
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
    let res = rx.recv_timeout(Duration::from_secs(20)).unwrap_or_else(|_| {
        report_error(
            t!(
                "рекордер не завершился; сдаёмся",
                "the recorder did not finish; giving up",
                ja: "レコーダーが終了しませんでした。中止します",
                pt: "o gravador não terminou; desistindo",
                zh: "录制器未完成；放弃"
            ),
            true,
        );
        std::process::exit(4);
    });
    destroy_ui();
    finish(res, use_wgc, ffmpeg, &dir, &out, true)
}

fn audio_text(res: &RecRes, _wgc: bool) -> String {
    if res.audio {
        t!("звук: AAC", "audio: AAC", ja: "音声: AAC", pt: "áudio: AAC", zh: "音频：AAC").to_string()
    } else if let Some(n) = &res.note {
        let n: String = n.chars().take(120).collect();
        tf!("без звука ({})", "no audio ({})", ja: "音声なし ({})", pt: "sem áudio ({})", zh: "无音频（{}）", n)
    } else {
        t!("без звука", "no audio", ja: "音声なし", pt: "sem áudio", zh: "无音频").to_string()
    }
}

/// Stops the recorder, finalizes the file and closes the process path. No modal dialog: a success toast (when the UI
/// ran) is fire-and-forget, so `--duration`, Stop and Esc all end in a real exit instead of a lingering message loop.
fn finish(res: RecRes, use_wgc: bool, ffmpeg: Option<PathBuf>, dir: &Path, out: &Path, notify_ui: bool) -> i32 {
    if use_wgc {
        if res.frames > 0 && res.err.is_empty() && out.is_file() {
            let secs = (res.elapsed_ms as f64 / 1000.0).max(0.05);
            let real_fps = (res.frames as f64 / secs).clamp(1.0, 120.0);
            let audio = audio_text(&res, true);
            event(
                &tf!("Записано (WGC + Media Foundation, H.264/MP4, {} кадров, {:.1} fps, {}): {}", "Recorded (WGC + Media Foundation, H.264/MP4, {} frames, {:.1} fps, {}): {}", ja: "録画しました (WGC + Media Foundation、H.264/MP4、{} フレーム、{:.1} fps、{}): {}", pt: "Gravado (WGC + Media Foundation, H.264/MP4, {} quadros, {:.1} fps, {}): {}", zh: "已录制（WGC + Media Foundation，H.264/MP4，{} 帧，{:.1} fps，{}）：{}", res.frames, real_fps, audio, out.display()),
                notify_ui,
            );
            std::thread::sleep(Duration::from_millis(80)); // short flush before process::exit
            return 0;
        }
        let _ = std::fs::remove_file(out);
        let msg = tf!(
            "WGC/Media Foundation не смог записать видео; частичный файл удалён. {}",
            "WGC/Media Foundation could not record the video; the partial file was removed. {}",
            ja: "WGC/Media Foundation は録画できませんでした。途中のファイルは削除しました。{}",
            pt: "WGC/Media Foundation não conseguiu gravar o vídeo; o arquivo parcial foi removido. {}",
            zh: "WGC/Media Foundation 无法录制视频；已删除不完整文件。{}",
            if res.err.is_empty() {
                t!("кадры не закодированы", "no frames encoded", ja: "フレームがエンコードされていません", pt: "nenhum quadro codificado", zh: "没有编码任何帧").to_string()
            } else {
                res.err
            }
        );
        report_error(&msg, notify_ui);
        return 4;
    }

    if res.frames == 0 {
        let _ = std::fs::remove_dir_all(dir);
        let msg = tf!(
            "Кадры не записались: {}",
            "No frames were captured: {}",
            ja: "フレームを取得できませんでした: {}",
            pt: "Nenhum quadro foi capturado: {}",
            zh: "未能捕获帧：{}",
            if res.err.is_empty() {
                t!("неизвестная ошибка", "unknown error", ja: "不明なエラー", pt: "erro desconhecido", zh: "未知错误")
            } else {
                &res.err
            }
        );
        report_error(&msg, notify_ui);
        return 4;
    }
    let secs = (res.elapsed_ms as f64 / 1000.0).max(0.05);
    let real_fps = (res.frames as f64 / secs).clamp(1.0, 120.0);
    let Some(ffmpeg) = ffmpeg else {
        let _ = std::fs::remove_dir_all(dir);
        report_error(
            t!(
                "ffmpeg исчез перед кодированием",
                "ffmpeg disappeared before encoding",
                ja: "エンコード前に ffmpeg が見つからなくなりました",
                pt: "o ffmpeg desapareceu antes da codificação",
                zh: "编码前 ffmpeg 消失了"
            ),
            notify_ui,
        );
        return 4;
    };
    match to_mp4(&ffmpeg, dir, real_fps, out) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(dir);
            let audio = audio_text(&res, false);
            event(
                &tf!("Записано (ffmpeg, H.264/MP4, {} кадров, {:.1} fps, {}): {}", "Recorded (ffmpeg, H.264/MP4, {} frames, {:.1} fps, {}): {}", ja: "録画しました (ffmpeg、H.264/MP4、{} フレーム、{:.1} fps、{}): {}", pt: "Gravado (ffmpeg, H.264/MP4, {} quadros, {:.1} fps, {}): {}", zh: "已录制（ffmpeg，H.264/MP4，{} 帧，{:.1} fps，{}）：{}", res.frames, real_fps, audio, out.display()),
                notify_ui,
            );
            std::thread::sleep(Duration::from_millis(80));
            0
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(dir);
            let msg = tf!(
                "ffmpeg не смог собрать MP4; временные кадры удалены. {}",
                "ffmpeg could not build the MP4; the temporary frames were removed. {}",
                ja: "ffmpeg は MP4 を作成できませんでした。一時フレームは削除しました。{}",
                pt: "O ffmpeg não conseguiu criar o MP4; os quadros temporários foram removidos. {}",
                zh: "ffmpeg 无法生成 MP4；临时帧已删除。{}",
                e
            );
            report_error(&msg, notify_ui);
            4
        }
    }
}

/// Appends a result line to %TEMP%\rs-shot.log and, when the interactive UI ran, raises a non-blocking WinRust toast.
fn event(msg: &str, notify_ui: bool) {
    use std::io::Write;
    let line = format!("{} {}\n", save::stamp(), msg);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("rs-shot.log")) {
        let _ = f.write_all(line.as_bytes());
    }
    if notify_ui {
        notify(msg);
    }
}

fn report_error(msg: &str, notify_ui: bool) {
    event(msg, notify_ui);
}

/// `rs-notify.exe` from the WinRust bar (fire and forget, never waited on). Missing bar: the log line above is the message.
fn notify(msg: &str) {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(e) = std::env::current_exe() {
        if let Some(d) = e.parent() {
            candidates.push(d.join("rs-notify.exe"));
        }
        candidates.push(e.clone());
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(&pf).join(r"topbar\rs-notify.exe"));
        candidates.push(PathBuf::from(&pf).join(r"rs-shot\rs-notify.exe"));
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(la).join(r"plugin-launcher\bin\rs-notify.exe"));
    }
    let Some(exe) = candidates.into_iter().find(|p| p.is_file() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("rs-notify.exe"))) else { return };
    let body: String = msg.chars().take(900).collect();
    let icon = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    let _ = std::process::Command::new(exe)
        .args(["--app", "rs-shot", "--title", "rs-shot", "--body", &body, "--icon", &icon])
        .creation_flags(0x0800_0000)
        .spawn();
}

struct RecRes {
    frames: u64,
    elapsed_ms: u128,
    err: String,
    audio: bool,
    note: Option<String>,
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
                    err = tf!(
                        "не удалось закодировать JPEG для {}",
                        "JPEG encode failed for {}",
                        ja: "JPEG エンコードに失敗しました: {}",
                        pt: "falha ao codificar JPEG para {}",
                        zh: "JPEG 编码失败：{}",
                        p.display()
                    );
                }
                DeleteObject(b as HGDIOBJ);
            }
            None if err.is_empty() => {
                err = t!(
                    "захват экрана не вернул изображение",
                    "screen capture returned no bitmap",
                    ja: "画面キャプチャがビットマップを返しませんでした",
                    pt: "a captura de tela não retornou um bitmap",
                    zh: "屏幕捕获未返回位图"
                )
                .to_string();
            }
            None => {}
        }
        let spent = tick.elapsed();
        if spent < interval {
            std::thread::sleep(interval - spent);
        }
    }
    (n, t0.elapsed().as_millis(), err)
}

unsafe fn make_ui(rect: RECT, fps: u32, duration: u32, stop_rec: Arc<AtomicBool>, audio: bool) -> Result<(), String> {
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
    REC.set(Some(Ui { source: rect, start: Ui::now(), duration, fps, stop: bar, frame, hover: false, stopped: false, stop_rec, audio, back: null_mut(), bdc: null_mut(), bw: 0, bh: 0 }));
    SetTimer(bar, 1, 200, None);
    // Stop shortcut while recording. RegisterHotKey reaches the bar even when another app has focus. Fn is firmware:
    // a laptop's "Fn+S" never reaches Windows, so this must be a real Win+Alt+S chord (or the Stop button).
    RegisterHotKey(bar, HOTKEY_STOP, MOD_WIN | MOD_ALT | MOD_NOREPEAT, b'S' as u32);
    ShowWindow(bar, SW_SHOW);
    Ok(())
}

unsafe fn destroy_ui() {
    let (bar, frame, back, bdc) = REC.with_borrow(|r| {
        r.as_ref().map(|r| (r.stop, r.frame, r.back, r.bdc)).unwrap_or((null_mut(), null_mut(), null_mut(), null_mut()))
    });
    if !bar.is_null() {
        UnregisterHotKey(bar, HOTKEY_STOP);
        DestroyWindow(bar);
    }
    if !frame.is_null() {
        DestroyWindow(frame);
    }
    if !back.is_null() {
        DeleteObject(back as HGDIOBJ);
    }
    if !bdc.is_null() {
        DeleteDC(bdc);
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
    // Stop the capture thread even if the message loop is slow to leave.
    let stop_flag = REC.with_borrow(|r| r.as_ref().map(|r| r.stop_rec.clone()));
    if let Some(f) = stop_flag {
        f.store(true, Relaxed);
    }
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
    let screen = BeginPaint(hw, &mut ps);
    let mut c: RECT = zeroed();
    GetClientRect(hw, &mut c);
    let (back_dc, made) = REC.with_borrow_mut(|r| {
        let Some(r) = r else { return (null_mut(), false) };
        if r.bdc.is_null() || r.bw != c.right || r.bh != c.bottom {
            if !r.back.is_null() {
                DeleteObject(r.back as HGDIOBJ);
            }
            if !r.bdc.is_null() {
                DeleteDC(r.bdc);
            }
            let g = GetDC(null_mut());
            let bmp = CreateCompatibleBitmap(g, c.right.max(1), c.bottom.max(1));
            let dc = CreateCompatibleDC(g);
            if !bmp.is_null() && !dc.is_null() {
                SelectObject(dc, bmp as HGDIOBJ);
            }
            ReleaseDC(null_mut(), g);
            r.back = bmp;
            r.bdc = dc;
            r.bw = c.right;
            r.bh = c.bottom;
            (dc, true)
        } else {
            (r.bdc, false)
        }
    });
    let mut p = ps.rcPaint;
    if made {
        p = c;
    }
    let hdc = if back_dc.is_null() { screen } else { back_dc };
    ui::fill(hdc, c, pal().bg);
    ui::outline(hdc, c, pal().line, 1);
    let (start, duration, fps, source, hover, stopped, audio) = REC.with_borrow(|r| {
        r.as_ref().map(|r| (r.start, r.duration, r.fps, r.source, r.hover, r.stopped, r.audio)).unwrap_or((0, 0, 30, RECT { left: 0, top: 0, right: 0, bottom: 0 }, false, false, false))
    });
    if stopped {
        if !back_dc.is_null() {
            BitBlt(screen, p.left, p.top, p.right - p.left, p.bottom - p.top, back_dc, p.left, p.top, SRCCOPY);
        }
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
    let audio_txt = if audio { t!("звук", "audio", ja: "音声", pt: "áudio", zh: "音频") } else { t!("без звука", "mute", ja: "無音", pt: "mudo", zh: "静音") };
    let limit = if duration > 0 {
        tf!("  ·  не дольше {} с", "  ·  max {}s", ja: "  ·  最大 {} 秒", pt: "  ·  máx. {} s", zh: "  ·  最长 {} 秒", duration)
    } else {
        String::new()
    };
    let dims = tf!(
        "{}×{}  ·  {} fps  ·  {}  ·  Win+Alt+S / Esc{}",
        "{}×{}  ·  {} fps  ·  {}  ·  Win+Alt+S / Esc{}",
        ja: "{}×{}  ·  {} fps  ·  {}  ·  Win+Alt+S / Esc{}",
        pt: "{}×{}  ·  {} fps  ·  {}  ·  Win+Alt+S / Esc{}",
        zh: "{}×{}  ·  {} fps  ·  {}  ·  Win+Alt+S / Esc{}",
        source.right - source.left,
        source.bottom - source.top,
        fps,
        audio_txt,
        limit
    );
    ui::text(hdc, &dims, RECT { left: 40, top: 30, right: c.right - 100, bottom: 54 }, f2, pal().dim, 0);
    let br = RECT { left: c.right - ui::sc(hw, 96), top: ui::sc(hw, 15), right: c.right - ui::sc(hw, 12), bottom: c.bottom - ui::sc(hw, 15) };
    ui::rounded(hdc, br, if hover { rgb(220, 50, 45) } else { rgb(170, 38, 35) }, ui::sc(hw, 8));
    let stop = t!("Стоп", "Stop", ja: "停止", pt: "Parar", zh: "停止");
    let sf = ui::make_font(ui::sc(hw, 13), 600, winrust_common::style::ui_face());
    ui::text(hdc, stop, br, sf, pal().text, DT_CENTER as u32);
    DeleteObject(sf as HGDIOBJ);
    DeleteObject(f as HGDIOBJ);
    DeleteObject(f2 as HGDIOBJ);
    if !back_dc.is_null() {
        BitBlt(screen, p.left, p.top, p.right - p.left, p.bottom - p.top, back_dc, p.left, p.top, SRCCOPY);
    }
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
            let changed = REC.with_borrow_mut(|r| {
                if let Some(r) = r {
                    if r.hover != hover {
                        r.hover = hover;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            });
            if changed {
                InvalidateRect(hw, &br, 0);
            }
            0
        }
        WM_LBUTTONUP => {
            stop_now();
            0
        }
        WM_HOTKEY if wp as i32 == HOTKEY_STOP => {
            stop_now();
            0
        }
        WM_TIMER if wp == 1 => {
            let mut c: RECT = zeroed();
            GetClientRect(hw, &mut c);
            // Only the dot and the time/size block change; the Stop button is not repainted by the timer.
            InvalidateRect(hw, &RECT { left: 0, top: 0, right: (c.right - ui::sc(hw, 100)).max(1), bottom: c.bottom }, 0);
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
        Err(if s.is_empty() {
            tf!(
                "ffmpeg завершился с кодом {}",
                "ffmpeg exited with {}",
                ja: "ffmpeg が終了しました: {}",
                pt: "o ffmpeg terminou com {}",
                zh: "ffmpeg 退出，状态 {}",
                output.status
            )
        } else {
            s
        })
    }
}

