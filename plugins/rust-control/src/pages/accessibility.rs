//! Accessibility switches, read-only, with shortcuts into Windows Settings.

use windows_sys::Win32::System::Registry::HKEY_CURRENT_USER;

use crate::{
    model::{Action, Row, Target, Tone},
    sys,
};

const A11Y: &str = r"Control Panel\Accessibility";

fn flags_on(key: &str) -> bool {
    let path = format!(r"{A11Y}\{key}");
    let raw = sys::string_long(HKEY_CURRENT_USER, &path, "Flags").or_else(|| sys::string_long(HKEY_CURRENT_USER, &path, "On")).unwrap_or_default();
    let first = raw.split_whitespace().next().unwrap_or("0");
    first.parse::<u32>().map(|v| v & 1 != 0).unwrap_or(first == "1")
}

fn on_off(on: bool) -> String {
    if on {
        t!("Включено", "On", ja: "オン", pt: "Ativado", zh: "开").into()
    } else {
        t!("Выключено", "Off", ja: "オフ", pt: "Desativado", zh: "关").into()
    }
}

pub fn rows() -> Vec<Row> {
    let magnifier = sys::dword(HKEY_CURRENT_USER, r"Software\Microsoft\ScreenMagnifier", "RunningState").unwrap_or(0) != 0;
    let contrast = flags_on("HighContrast");
    let sticky = flags_on("StickyKeys");
    let toggle = flags_on("ToggleKeys");
    let filter = flags_on("Keyboard Response");
    let sound = flags_on("SoundSentry");
    let mouse = flags_on("MouseKeys");
    let audio = flags_on("AudioDescription");
    let mut rows = vec![
        Row::entry(
            t!("Открыть специальные возможности", "Open Accessibility settings", ja: "ユーザー補助の設定を開く", pt: "Abrir configurações de acessibilidade", zh: "打开辅助功能设置"),
            "ms-settings:easeofaccess",
            "",
            t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
            Tone::Accent,
            Some(Action::Open(Target::Uri("ms-settings:easeofaccess"))),
            vec![],
        ),
        Row::section(t!("Экран", "Display", ja: "画面", pt: "Tela", zh: "显示")),
        Row::info(t!("Лупа", "Magnifier", ja: "拡大鏡", pt: "Lupa", zh: "放大镜"), on_off(magnifier), if magnifier { Tone::Good } else { Tone::Dim }),
        Row::info(t!("Высокая контрастность", "High contrast", ja: "ハイ コントラスト", pt: "Alto contraste", zh: "高对比度"), on_off(contrast), if contrast { Tone::Good } else { Tone::Dim }),
        Row::section(t!("Клавиатура и мышь", "Keyboard and mouse", ja: "キーボードとマウス", pt: "Teclado e mouse", zh: "键盘和鼠标")),
        Row::info(t!("Залипание клавиш", "Sticky Keys", ja: "固定キー", pt: "Teclas de aderência", zh: "粘滞键"), on_off(sticky), if sticky { Tone::Good } else { Tone::Dim }),
        Row::info(t!("Клавиши переключения", "Toggle Keys", ja: "切り替えキー", pt: "Teclas de alternância", zh: "切换键"), on_off(toggle), if toggle { Tone::Good } else { Tone::Dim }),
        Row::info(t!("Фильтрация ввода", "Filter Keys", ja: "フィルター キー", pt: "Teclas de filtro", zh: "筛选键"), on_off(filter), if filter { Tone::Good } else { Tone::Dim }),
        Row::info(t!("Управление указателем с клавиатуры", "Mouse Keys", ja: "マウス キー", pt: "Teclas do mouse", zh: "鼠标键"), on_off(mouse), if mouse { Tone::Good } else { Tone::Dim }),
        Row::section(t!("Звук и текст", "Sound and text", ja: "サウンドとテキスト", pt: "Som e texto", zh: "声音和文本")),
        Row::info(t!("Звуковые оповещения", "Sound Sentry", ja: "サウンド セントリー", pt: "Sentry de som", zh: "声音卫士"), on_off(sound), if sound { Tone::Good } else { Tone::Dim }),
        Row::info(t!("Описание звука", "Audio description", ja: "音声解説", pt: "Descrição de áudio", zh: "音频描述"), on_off(audio), if audio { Tone::Good } else { Tone::Dim }),
    ];
    let scheme = sys::string_long(HKEY_CURRENT_USER, &format!(r"{A11Y}\HighContrast"), "High Contrast Scheme").unwrap_or_default();
    if !scheme.is_empty() {
        rows.push(Row::info(t!("Схема контраста", "Contrast theme", ja: "コントラスト テーマ", pt: "Tema de contraste", zh: "对比度主题"), scheme, Tone::Dim));
    }
    rows.push(Row::entry(
        t!("Открыть лупу", "Open Magnifier", ja: "拡大鏡を開く", pt: "Abrir a Lupa", zh: "打开放大镜"),
        "magnify.exe",
        "",
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
        Tone::Neutral,
        Some(Action::Open(Target::File("magnify.exe"))),
        vec![],
    ));
    rows.push(Row::note(
        t!("Эти параметры только читаются здесь; включайте их в параметрах Windows или сочетаниями клавиш.", "These switches are read here; change them in Windows Settings or with their keyboard shortcuts.", ja: "ここでは状態のみを表示します。Windows の設定またはキーボード ショートカットで変更してください。", pt: "Estes interruptores são apenas lidos aqui; altere-os nas Configurações do Windows ou com os atalhos de teclado.", zh: "此处仅读取这些开关；请在 Windows 设置或使用键盘快捷方式更改。"),
        Tone::Dim,
    ));
    rows
}
