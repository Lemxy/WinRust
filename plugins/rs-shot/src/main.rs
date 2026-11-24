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
mod audio;
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

/// Product name: the same in every language, so it is not a translated key.
pub(crate) const TITLE: &str = "rs-shot";

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
    /// True when --region/--full/--monitor/--window was given: CLI sources skip the selection bar.
    pub source_explicit: bool,
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
    // Read %ProgramData%\WinRust\lang.txt once, at start (`winrust_common::lang()` behind the macro): rs-shot is
    // short-lived and never keeps a language of its own.
    let _ = lang!();
    unsafe {
        SetProcessDpiAwarenessContext(-4isize as _); // per-monitor v2: one physical coordinate space for the virtual desktop
        save::gdiplus_init();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        msg(&usage(), TITLE);
        return;
    }
    if args.iter().any(|a| a == "--print-strings") {
        print_strings();
        return;
    }
    let o = match parse(&args) {
        Ok(o) => o,
        Err(e) => {
            err(&format!("{e}\n\n{}", usage()), TITLE);
            std::process::exit(1);
        }
    };
    let code = if o.record { start_record(&o) } else { capture(&o) };
    std::process::exit(code);
}

/// The help, in the language lang.txt asks for.
fn usage() -> String {
    let head = t!(
        "rs-shot — снимок экрана и запись экрана WinRust",
        "rs-shot — WinRust screenshot and screen recorder",
        ja: "rs-shot — WinRust のスクリーンショットと画面録画",
        pt: "rs-shot — captura de tela e gravação de tela da WinRust",
        zh: "rs-shot — WinRust 截图与屏幕录制"
    );
    let usage = t!(
        "Использование:\n  rs-shot                         выделение области + редактор (по умолчанию)\n  rs-shot --full                  все мониторы, редактор\n  rs-shot --monitor [N]           монитор под курсором или номер N (1 = крайний левый), редактор\n  rs-shot --window                активное окно, редактор\n  rs-shot --region                выделение области + редактор",
        "Usage:\n  rs-shot                         region overlay + editor (default)\n  rs-shot --full                  all monitors, editor\n  rs-shot --monitor [N]           monitor under the cursor, or number N (1 = leftmost), editor\n  rs-shot --window                foreground window, editor\n  rs-shot --region                region overlay + editor",
        ja: "使い方:\n  rs-shot                         範囲選択 + エディター（既定）\n  rs-shot --full                  すべてのモニター、エディター\n  rs-shot --monitor [N]           カーソル下のモニター、または番号 N（1 = 一番左）、エディター\n  rs-shot --window                前面のウィンドウ、エディター\n  rs-shot --region                範囲選択 + エディター",
        pt: "Uso:\n  rs-shot                         seleção de região + editor (padrão)\n  rs-shot --full                  todos os monitores, editor\n  rs-shot --monitor [N]           monitor sob o cursor, ou número N (1 = mais à esquerda), editor\n  rs-shot --window                janela em primeiro plano, editor\n  rs-shot --region                seleção de região + editor",
        zh: "用法：\n  rs-shot                         区域选择 + 编辑器（默认）\n  rs-shot --full                  所有显示器，编辑器\n  rs-shot --monitor [N]           光标下的显示器，或编号 N（1 = 最左侧），编辑器\n  rs-shot --window                前台窗口，编辑器\n  rs-shot --region                区域选择 + 编辑器"
    );
    let capture = t!(
        "Параметры снимка:\n  --delay N                       подождать N секунд перед снимком (0, 2, 5, 10, ...)\n  --cursor                        включить указатель мыши в снимок\n  --clipboard                     с --no-ui: ещё и скопировать в буфер обмена (он и так копируется)\n  --file PATH                     сохранить сюда при сохранении/--no-ui (по умолчанию Pictures\\Screenshots\\Снимок_<дата>.png)\n  --no-ui                         без редактора и интерфейса записи: только буфер обмена (и --file); с --record\n                                  записывать --duration N секунд без интерфейса (область берётся с монитора курсора)",
        "Capture options:\n  --delay N                       wait N seconds before capturing (0, 2, 5, 10, ...)\n  --cursor                        include the mouse pointer in the capture\n  --clipboard                     with --no-ui: also copy to the clipboard (it is copied anyway)\n  --file PATH                     save here on Save/--no-ui (default Pictures\\Screenshots\\Screenshot_<date>.png)\n  --no-ui                         skip the editor/recording UI: clipboard only (plus --file); with --record,\n                                  record --duration N seconds headlessly (Region uses the cursor monitor)",
        ja: "キャプチャのオプション:\n  --delay N                       キャプチャ前に N 秒待つ（0、2、5、10、...）\n  --cursor                        マウスポインターをキャプチャに含める\n  --clipboard                     --no-ui と併用: クリップボードにもコピー（元からコピーされます）\n  --file PATH                     保存/--no-ui 時の保存先（既定 Pictures\\Screenshots\\スクリーンショット_<日付>.png）\n  --no-ui                         エディター/録画 UI を出さない: クリップボードのみ（--file も可）。--record と併用時は\n                                  --duration N 秒を画面なしで録画（範囲はカーソル下のモニター）",
        pt: "Opções de captura:\n  --delay N                       esperar N segundos antes de capturar (0, 2, 5, 10, ...)\n  --cursor                        incluir o ponteiro do mouse na captura\n  --clipboard                     com --no-ui: copiar também para a área de transferência (já é copiado)\n  --file PATH                     salvar aqui ao salvar/--no-ui (padrão Pictures\\Screenshots\\Captura de tela_<data>.png)\n  --no-ui                         sem editor/interface de gravação: só a área de transferência (e --file); com --record,\n                                  gravar --duration N segundos sem interface (a região usa o monitor do cursor)",
        zh: "截图选项：\n  --delay N                       截图前等待 N 秒（0、2、5、10、……）\n  --cursor                        截图中包含鼠标指针\n  --clipboard                     与 --no-ui 同用：同时复制到剪贴板（本来就会复制）\n  --file PATH                     保存/--no-ui 时保存到此路径（默认 Pictures\\Screenshots\\截图_<日期>.png）\n  --no-ui                         跳过编辑器/录制界面：仅复制到剪贴板（可加 --file）；与 --record 同用时\n                                  无界面录制 --duration N 秒（区域取光标所在显示器）"
    );
    let record = t!(
        "Запись (сначала WGC + Media Foundation, при необходимости ffmpeg):\n  --record                        открыть панель выбора «Фото/Видео» на области\n  --record --monitor [N]          записать монитор\n  --record --window               записать активное окно\n  --duration N                    остановить через N секунд (по умолчанию: до «Стоп»/Esc/Win+Alt+S)\n  --fps N                         кадров в секунду (по умолчанию 30, 1..60)\n  --out PATH                      путь MP4 (по умолчанию Videos\\Screen Records\\Запись_<дата>.mp4)\n  --encoder auto|wgc|ffmpeg       auto (по умолчанию): WGC + Media Foundation, если работает, иначе ffmpeg\n  --audio                         записывать звук ПК (WASAPI loopback + AAC, включено по умолчанию)\n  --no-audio                      только видео",
        "Recording (WGC + Media Foundation first, ffmpeg bridge when needed):\n  --record                        open the Photo/Video selection bar on the region\n  --record --monitor [N]          record a monitor\n  --record --window               record the foreground window\n  --duration N                    stop after N seconds (default: unlimited until Stop/Esc/Win+Alt+S)\n  --fps N                         frames per second (default 30, 1..60)\n  --out PATH                      MP4 path (default Videos\\Screen Records\\Recording_<date>.mp4)\n  --encoder auto|wgc|ffmpeg       auto (default): WGC + Media Foundation when it works, else ffmpeg\n  --audio                         record PC sound (WASAPI loopback + AAC, on by default)\n  --no-audio                      video only",
        ja: "録画（まず WGC + Media Foundation、必要なら ffmpeg）:\n  --record                        範囲に「写真/動画」選択バーを開く\n  --record --monitor [N]          モニターを録画\n  --record --window               前面のウィンドウを録画\n  --duration N                    N 秒後に停止（既定: 停止/Esc/Win+Alt+S まで無制限）\n  --fps N                         フレーム/秒（既定 30、1..60）\n  --out PATH                      MP4 のパス（既定 Videos\\Screen Records\\録画_<日付>.mp4）\n  --encoder auto|wgc|ffmpeg       auto（既定）: 動けば WGC + Media Foundation、だめなら ffmpeg\n  --audio                         PC の音声を録音（WASAPI ループバック + AAC、既定でオン）\n  --no-audio                      映像のみ",
        pt: "Gravação (WGC + Media Foundation primeiro, ffmpeg quando necessário):\n  --record                        abrir a barra de seleção Foto/Vídeo na região\n  --record --monitor [N]          gravar um monitor\n  --record --window               gravar a janela em primeiro plano\n  --duration N                    parar após N segundos (padrão: sem limite até Parar/Esc/Win+Alt+S)\n  --fps N                         quadros por segundo (padrão 30, 1..60)\n  --out PATH                      caminho do MP4 (padrão Videos\\Screen Records\\Gravação_<data>.mp4)\n  --encoder auto|wgc|ffmpeg       auto (padrão): WGC + Media Foundation quando funciona, senão ffmpeg\n  --audio                         gravar o som do PC (WASAPI loopback + AAC, ligado por padrão)\n  --no-audio                      somente vídeo",
        zh: "录制（优先 WGC + Media Foundation，必要时使用 ffmpeg）：\n  --record                        在区域上打开“照片/视频”选择栏\n  --record --monitor [N]          录制显示器\n  --record --window               录制前台窗口\n  --duration N                    N 秒后停止（默认：直到停止/Esc/Win+Alt+S）\n  --fps N                         每秒帧数（默认 30，1..60）\n  --out PATH                      MP4 路径（默认 Videos\\Screen Records\\录制_<日期>.mp4）\n  --encoder auto|wgc|ffmpeg       auto（默认）：WGC + Media Foundation 可用时用它，否则用 ffmpeg\n  --audio                         录制电脑声音（WASAPI 回环 + AAC，默认开启）\n  --no-audio                      仅视频"
    );
    let keys = t!(
        "Клавиши: PrintScreen / Win+Shift+S = фото + область; Win+Alt+R = видео + область (через оболочку WinRust).\n      В панели выбор источника срабатывает сразу: область -> растянуть, экран/монитор -> сразу, окно -> щёлкнуть.\n      Во время записи: Win+Alt+S или Esc останавливает (Fn — это прошивка, его нельзя назначить).\n      В редакторе: Ctrl+S сохранить+копировать, Ctrl+C копировать, Ctrl+Z/Y отменить/повторить, Esc закрыть.\n\n  --print-strings                 напечатать тексты интерфейса на текущем языке (диагностика)\n  -h, --help                      эта справка",
        "Keys: PrintScreen / Win+Shift+S = Photo + Region; Win+Alt+R = Video + Region (through the WinRust shell).\n      In the bar, choosing a source acts at once: Region -> drag, Full/Monitor -> immediate, Window -> click it.\n      While recording: Win+Alt+S or Esc stops (Fn is firmware and cannot be a shortcut).\n      In the editor: Ctrl+S save+copy, Ctrl+C copy, Ctrl+Z/Y undo/redo, Esc close.\n\n  --print-strings                 print the UI texts in the current language (diagnostics)\n  -h, --help                      this help",
        ja: "キー: PrintScreen / Win+Shift+S = 写真 + 範囲、Win+Alt+R = 動画 + 範囲（WinRust シェル経由）。\n      バーでは選んだ瞬間に動きます: 範囲 -> ドラッグ、全画面/モニター -> すぐ、ウィンドウ -> クリック。\n      録画中: Win+Alt+S または Esc で停止（Fn はファームウェアなのでショートカットにできません）。\n      エディター: Ctrl+S 保存+コピー、Ctrl+C コピー、Ctrl+Z/Y 元に戻す/やり直す、Esc 閉じる。\n\n  --print-strings                 現在の言語で UI テキストを出力（診断用）\n  -h, --help                      このヘルプ",
        pt: "Teclas: PrintScreen / Win+Shift+S = Foto + Região; Win+Alt+R = Vídeo + Região (pelo shell WinRust).\n      Na barra, escolher uma fonte age na hora: Região -> arrastar, Tela/Monitor -> imediato, Janela -> clicar.\n      Durante a gravação: Win+Alt+S ou Esc para (Fn é firmware e não pode ser atalho).\n      No editor: Ctrl+S salvar+copiar, Ctrl+C copiar, Ctrl+Z/Y desfazer/refazer, Esc fechar.\n\n  --print-strings                 imprimir os textos da interface no idioma atual (diagnóstico)\n  -h, --help                      esta ajuda",
        zh: "按键：PrintScreen / Win+Shift+S = 照片 + 区域；Win+Alt+R = 视频 + 区域（通过 WinRust shell）。\n      在选择栏中，选择来源立即生效：区域 → 拖动，全屏/显示器 → 立即，窗口 → 单击。\n      录制中：Win+Alt+S 或 Esc 停止（Fn 是固件，不能作为快捷键）。\n      编辑器中：Ctrl+S 保存并复制，Ctrl+C 复制，Ctrl+Z/Y 撤销/重做，Esc 关闭。\n\n  --print-strings                 以当前语言输出界面文本（诊断）\n  -h, --help                      此帮助"
    );
    format!("{head}\n\n{usage}\n\n{capture}\n\n{record}\n\n{keys}")
}

/// `--print-strings`: a fixed sample of UI texts through the real macros, so a run with lang.txt=en and one with
/// lang.txt=es can be diffed without a screen. GUI subsystem: redirect the output to a file.
fn print_strings() {
    let lang = lang!();
    let name = "x.png";
    let lines = [
        ("language", lang.name().to_string()),
        ("title.editor", t!("rs-shot — Снимок", "rs-shot — Screenshot", ja: "rs-shot — スクリーンショット", pt: "rs-shot — Captura", zh: "rs-shot — 截图").to_string()),
        ("title.recording", t!("rs-shot — запись", "rs-shot — recording", ja: "rs-shot — 録画", pt: "rs-shot — gravando", zh: "rs-shot — 录制").to_string()),
        ("button.crop", t!("Обрезать", "Crop", ja: "切り抜き", pt: "Recortar", zh: "裁剪").to_string()),
        ("button.stop", t!("Стоп", "Stop", ja: "停止", pt: "Parar", zh: "停止").to_string()),
        ("status.recording", t!("ЗАПИСЬ", "REC", ja: "録画中", pt: "GRAVANDO", zh: "录制中").to_string()),
        ("audio.on", t!("звук", "audio", ja: "音声", pt: "áudio", zh: "音频").to_string()),
        ("audio.off", t!("без звука", "mute", ja: "無音", pt: "mudo", zh: "静音").to_string()),
        ("status.copied", t!("Скопировано в буфер обмена", "Copied to the clipboard", ja: "クリップボードにコピーしました", pt: "Copiado para a área de transferência", zh: "已复制到剪贴板").to_string()),
        ("status.save_failed", t!("Не удалось сохранить файл", "Could not save the file", ja: "ファイルを保存できませんでした", pt: "Não foi possível salvar", zh: "无法保存文件").to_string()),
        ("status.saved", tf!("Сохранено: {name}", "Saved: {name}", ja: "保存しました: {name}", pt: "Salvo: {name}", zh: "已保存：{name}")),
        ("file.screenshot", save::screenshot_base()),
        ("file.recording", save::recording_base()),
        ("help.first_line", usage().lines().next().unwrap_or_default().to_string()),
    ];
    for (key, value) in lines {
        println!("{key}\t{value}");
    }
}

fn parse(a: &[String]) -> Result<Opts, String> {
    let mut o = Opts {
        mode: Mode::Region,
        source_explicit: false,
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
        audio: true,
        encoder: Encoder::Auto,
    };
    let mut i = 0;
    while i < a.len() {
        let take = |i: &mut usize| -> String {
            *i += 1;
            a.get(*i).cloned().unwrap_or_default()
        };
        match a[i].as_str() {
            "--full" => {
                o.mode = Mode::Full;
                o.source_explicit = true;
            }
            "--region" => {
                o.mode = Mode::Region;
                o.source_explicit = true;
            }
            "--window" => {
                o.mode = Mode::Window;
                o.source_explicit = true;
            }
            "--monitor" => {
                o.mode = Mode::Monitor;
                o.source_explicit = true;
                if let Some(n) = a.get(i + 1).and_then(|s| s.parse::<usize>().ok()) {
                    o.monitor = Some(n.saturating_sub(1));
                    i += 1;
                }
            }
            "--delay" => {
                let v = take(&mut i);
                o.delay = v
                    .parse::<u32>()
                    .map_err(|_| tf!("--delay принимает только секунды: {}", "--delay needs seconds, got {}", ja: "--delay は秒数が必要です: {}", pt: "--delay precisa de segundos, recebido {}", zh: "--delay 需要秒数，收到 {}", v))?
                    .min(600);
            }
            "--cursor" => o.cursor = true,
            "--no-cursor" => o.cursor = false,
            "--clipboard" => o.clipboard = true,
            "--file" => o.file = Some(PathBuf::from(take(&mut i))),
            "--no-ui" => o.no_ui = true,
            "--record" => o.record = true,
            "--fps" => {
                let v = take(&mut i);
                o.fps = v
                    .parse::<u32>()
                    .map_err(|_| tf!("--fps принимает только число: {}", "--fps needs a number, got {}", ja: "--fps は数値が必要です: {}", pt: "--fps precisa de um número, recebido {}", zh: "--fps 需要数字，收到 {}", v))?
                    .clamp(1, 60);
            }
            "--duration" => {
                let v = take(&mut i);
                o.duration = v
                    .parse::<u32>()
                    .map_err(|_| tf!("--duration принимает только секунды: {}", "--duration needs seconds, got {}", ja: "--duration は秒数が必要です: {}", pt: "--duration precisa de segundos, recebido {}", zh: "--duration 需要秒数，收到 {}", v))?
                    .min(86_400);
            }
            "--out" => o.out = Some(PathBuf::from(take(&mut i))),
            "--encoder" => {
                let v = take(&mut i);
                o.encoder = match v.to_ascii_lowercase().as_str() {
                    "auto" => Encoder::Auto,
                    "wgc" | "mf" | "mediafoundation" => Encoder::Wgc,
                    "ffmpeg" => Encoder::Ffmpeg,
                    _ => {
                        return Err(tf!(
                            "--encoder принимает auto, wgc или ffmpeg: {}",
                            "--encoder needs auto, wgc or ffmpeg, got {}",
                            ja: "--encoder は auto、wgc、ffmpeg のいずれかです: {}",
                            pt: "--encoder precisa de auto, wgc ou ffmpeg, recebido {}",
                            zh: "--encoder 需要 auto、wgc 或 ffmpeg，收到 {}",
                            v
                        ));
                    }
                };
            }
            "--audio" => o.audio = true,
            "--no-audio" => o.audio = false,
            other => {
                return Err(tf!("неизвестный параметр {}", "unknown option {}", ja: "不明なオプション {}", pt: "opção desconhecida {}", zh: "未知选项 {}", other));
            }
        }
        i += 1;
    }
    Ok(o)
}

/// Interactive `--record`: the same selection bar as PrintScreen, preselected on Vídeo + Región. Explicit
/// `--monitor/--window/--full` and `--no-ui` keep the direct, bar-less path.
fn start_record(o: &Opts) -> i32 {
    if o.no_ui || o.mode != Mode::Region || o.source_explicit {
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
        let pick = if o.source_explicit {
            overlay::select_direct(o.cursor, true, o.delay)
        } else {
            overlay::choose(overlay::Options {
                video: false,
                kind: overlay::Kind::Region,
                delay: o.delay,
                cursor: o.cursor,
                want_image: true,
            })
        };
        let Some(pick) = pick else {
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
            TITLE,
        );
        return 2;
    };
    if !o.no_ui {
        return editor::run(img, o.file.clone());
    }
    // No UI: clipboard is the primary result; --file additionally writes the file.
    let copied = unsafe { save::copy_to_clipboard(&img) };
    let saved = match &o.file {
        Some(path) => {
            let mut path = path.clone();
            if path.extension().is_none() {
                path.set_extension("png");
            }
            unsafe { save::save(&img, &path) }
        }
        None => true,
    };
    if saved && (copied || o.file.is_some()) {
        0
    } else {
        msg(
            t!("Не удалось сохранить снимок", "Could not save the screenshot", ja: "スクリーンショットを保存できませんでした", pt: "Não foi possível salvar a captura", zh: "无法保存截图"),
            TITLE,
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

