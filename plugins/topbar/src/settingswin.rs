//! The settings window: one real popup with a rail of categories on the left and the options of the chosen one on the
//! right, instead of one long dropdown per page. Every option the bar owns (look, bar, Start, language) lives here, the
//! apps' own settings are reached from the Apps category, and the values are still written to the files the other
//! WinRust programs read (`look.txt`, `style.txt`, `lang.txt`), so a change applies while the user is still clicking.
//!
//! Rows are toggles, choices and actions; `layout()` is used by both the painter and the hit test, so a click always lands
//! where the row is drawn. Long categories scroll with the wheel; the window is double buffered.
use std::{cell::Cell, mem::zeroed, ptr};

use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::VK_ESCAPE, WindowsAndMessaging::*},
};

use winrust_common::{look as lk, style as st};

/// Window size. A fixed popup big enough for a rail and a full category without a second window.
const W: i32 = 900;
const H: i32 = 640;
const RAIL: i32 = 210; // the category rail
const PAD: i32 = 26; // padding of the content area
const ROW_H: i32 = 40;
const HEAD_H: i32 = 30;
const TOP: i32 = 86; // first row, under the category header

/// Categories of the rail, in order: this is the index `open_at()` takes.
pub(crate) const LOOK: u8 = 0;
pub(crate) const BAR: u8 = 1;
pub(crate) const START: u8 = 2;
pub(crate) const LANG: u8 = 3;
pub(crate) const APPS: u8 = 4;
pub(crate) const ABOUT: u8 = 5;
const CATS: usize = 6;

/// Pages inside a category (the look's own colours and templates, the reset confirmation).
const ROOT: u8 = 0;
const COLORS: u8 = 1;
const TEMPLATES: u8 = 2;
const DELETE: u8 = 3;
const RESET: u8 = 4;

fn sc(v: i32) -> i32 {
    crate::startctx::sc(v)
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    None,
    Cat(u8),
    Page(u8),
    Theme(u8),
    Color(u8),
    Opacity(u8),
    ResetColors,
    ApplyTheme(u8),
    SaveTheme,
    DeleteTheme(u8),
    BarSide(u8),
    BarSize(u8),
    BarMonitor(i8),
    Autohide,
    AlignLeft,
    AltTab,
    SearchBtn,
    TaskView,
    Notify,
    NotifyTimeout,
    BarBlur,
    BarRadius(i32),
    WinBlur,
    MenuBlur,
    MenuAlpha(u8),
    MenuRadius(i32),
    StartPos(bool),
    StartStyle(u8),
    StartScale(u8),
    Lang(u8),
    OpenApp(u8),
    ResetAll,
    Close,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Head,
    Check(bool),
    Choice(bool),
    Action,
    Danger,
    Info,
}

struct Row {
    label: String,
    hint: String,
    kind: Kind,
    op: Op,
    dot: u32, // colour dot before the label (COLORREF), u32::MAX = none
}

fn head(label: &str) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Head, op: Op::None, dot: u32::MAX }
}
fn check(label: &str, on: bool, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Check(on), op, dot: u32::MAX }
}
fn choice(label: &str, on: bool, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Choice(on), op, dot: u32::MAX }
}
fn action(label: &str, hint: &str, op: Op) -> Row {
    Row { label: label.into(), hint: hint.into(), kind: Kind::Action, op, dot: u32::MAX }
}
fn danger(label: &str, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Danger, op, dot: u32::MAX }
}
fn info(label: &str) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Info, op: Op::None, dot: u32::MAX }
}
fn swatch(label: &str, on: bool, dot: u32, op: Op) -> Row {
    Row { dot, ..choice(label, on, op) }
}

fn cat_title(i: u8) -> String {
    match i {
        LOOK => t!("Внешний вид", "Appearance", ja: "外観", pt: "Aparência", zh: "外观"),
        BAR => t!("Панель", "Bar", ja: "バー", pt: "Barra", zh: "任务栏"),
        START => t!("Меню «Пуск»", "Start menu", ja: "スタート メニュー", pt: "Menu Iniciar", zh: "开始菜单"),
        LANG => t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言"),
        APPS => t!("Приложения", "Apps", ja: "アプリ", pt: "Aplicativos", zh: "应用"),
        _ => t!("Дополнительно", "Advanced", ja: "詳細設定", pt: "Avançado", zh: "高级"),
    }
    .to_string()
}

fn cat_sub(i: u8) -> String {
    match i {
        LOOK => t!("Тема, цвета, прозрачность и шаблоны", "Theme, colors, transparency and templates", ja: "テーマ、色、透明度、テンプレート", pt: "Tema, cores, transparência e modelos", zh: "主题、颜色、透明度与模板"),
        BAR => t!("Где она, как выглядит и как себя ведёт", "Where it sits, how it looks and behaves", ja: "配置場所、外観、動作", pt: "Onde fica, como aparece e se comporta", zh: "位置、外观与行为"),
        START => t!("Где открывается и как выглядит", "Where it opens and how it looks", ja: "開く位置と外観", pt: "Onde abre e como aparece", zh: "打开位置和外观"),
        LANG => t!("Язык всех программ WinRust", "The language of every WinRust program", ja: "すべての WinRust プログラムの言語", pt: "O idioma de todos os programas da WinRust", zh: "所有 WinRust 程序的语言"),
        APPS => t!("Свои страницы каждой программы", "Each app's own pages", ja: "各アプリの設定", pt: "As páginas de cada aplicativo", zh: "各应用自己的设置"),
        _ => t!("Сброс, язык и сведения", "Reset, language and about", ja: "リセット、言語、情報", pt: "Restaurar, idioma e sobre", zh: "重置、语言与关于"),
    }
    .to_string()
}

fn page_title(p: u8) -> String {
    match p {
        COLORS => t!("Цвета и прозрачность", "Colors and transparency", ja: "色と透明度", pt: "Cores e transparência", zh: "颜色与透明度"),
        TEMPLATES => t!("Шаблоны", "Templates", ja: "テンプレート", pt: "Modelos", zh: "模板"),
        DELETE => t!("Удалить шаблон", "Delete a template", ja: "テンプレートを削除", pt: "Excluir um modelo", zh: "删除模板"),
        RESET => t!("Сбросить всё?", "Reset everything?", ja: "すべてをリセットします?", pt: "Restaurar tudo?", zh: "全部重置？"),
        _ => "",
    }
    .to_string()
}

fn page_sub(p: u8) -> String {
    match p {
        COLORS => t!("Поверх выбранной темы", "On top of the chosen theme", ja: "選択したテーマの上に適用", pt: "Sobre o tema escolhido", zh: "在所选主题之上"),
        TEMPLATES => t!("Свои наборы на любое настроение", "Your own looks for any mood", ja: "気分に合わせたオリジナルの外観", pt: "Suas próprias aparências para qualquer humor", zh: "随心保存自己的外观"),
        DELETE => t!("Нажмите на шаблон, чтобы удалить", "Click a template to delete it", ja: "削除するテンプレートをクリック", pt: "Clique em um modelo para excluí-lo", zh: "点击模板即可删除"),
        RESET => t!("Тема, цвета, панель и язык. Шаблоны останутся", "Theme, colors, bar and language. Templates stay", ja: "テーマ、色、バー、言語。テンプレートは残ります", pt: "Tema, cores, barra e idioma. Os modelos permanecem", zh: "主题、颜色、任务栏和语言。模板保留"),
        _ => "",
    }
    .to_string()
}

/// Colours the colours page edits, in `style::PAL` order (`bg line hover active accent text dim`).
fn color_names() -> [&'static str; 7] {
    [
        t!("Фон", "Background", ja: "背景", pt: "Fundo", zh: "背景"),
        t!("Линии и рамки", "Lines and borders", ja: "線と枠", pt: "Linhas e bordas", zh: "线条与边框"),
        t!("Наведение", "Hover", ja: "ホバー", pt: "Passar o mouse", zh: "悬停"),
        t!("Нажатие и выбранное", "Pressed and selected", ja: "押下中と選択中", pt: "Pressionado e selecionado", zh: "按下和选中"),
        t!("Акцент", "Accent", ja: "アクセント", pt: "Destaque", zh: "强调色"),
        t!("Текст", "Text", ja: "テキスト", pt: "Texto", zh: "文本"),
        t!("Второстепенный текст", "Secondary text", ja: "補助テキスト", pt: "Texto secundário", zh: "次要文本"),
    ]
}

const PCT: [u8; 5] = [100, 85, 70, 55, 40];

/// The WinRust programs whose own settings the Apps category opens, with where the installer puts them.
const APPS_LIST: [(&str, &str); 5] = [
    ("rust-explorer", "rust-explorer.exe"),
    ("rtaskmgr", "rtaskmgr-win32.exe"),
    ("rust-control", "rust-control.exe"),
    ("rs-shot", "rs-shot.exe"),
    ("rust-settings", "rust-settings.exe"),
];

/// Finds an installed WinRust program: Program Files (the installer's place), then beside the bar, then the launcher's bin.
fn installed(app: &str, exe: &str) -> bool {
    let mut v = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        v.push(std::path::PathBuf::from(pf).join(app).join(exe));
    }
    if let Ok(e) = std::env::current_exe() {
        v.push(e.with_file_name(exe));
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        v.push(std::path::PathBuf::from(la).join(r"plugin-launcher\bin").join(exe));
    }
    v.into_iter().any(|p| p.is_file())
}

thread_local! {
    static CAT: Cell<u8> = const { Cell::new(LOOK) };
    static PAGE: Cell<u8> = const { Cell::new(ROOT) };
    static SCROLL: Cell<i32> = const { Cell::new(0) };
    static HOT: Cell<i32> = const { Cell::new(-1) }; // hovered row index of the current layout
    static HOT_CAT: Cell<i32> = const { Cell::new(-1) };
    static HOTX: Cell<bool> = const { Cell::new(false) }; // the close button
    static BITS: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
    static FONT: Cell<[usize; 4]> = const { Cell::new([0; 4]) };
    static WND: Cell<HWND> = const { Cell::new(ptr::null_mut()) };
    /// The saved templates: read from disk when the window opens or one is saved, never on every repaint.
    static THEMES: std::cell::RefCell<Vec<st::Theme>> = const { std::cell::RefCell::new(Vec::new()) };
    /// How many monitors there were when the window opened (the monitor list is not re-read on every paint).
    static MONS: Cell<i32> = const { Cell::new(1) };
    /// Which of `APPS_LIST` are installed (checked when the window opens, not on every repaint).
    static HAVE: Cell<[bool; 5]> = const { Cell::new([false; 5]) };
}

const EMPTY: [bool; 5] = [false; 5];

fn rows(cat: u8, page: u8) -> Vec<Row> {
    let mut v = Vec::new();
    match (cat, page) {
        (LOOK, COLORS) => {
            v.push(action(t!("‹   Назад", "‹   Back", ja: "‹   戻る", pt: "‹   Voltar", zh: "‹   返回"), "", Op::Page(ROOT)));
            v.push(head(t!("Цвета (точка: изменён вами)", "Colors (dot: changed by you)", ja: "色 (点: 変更済み)", pt: "Cores (ponto: alterada por você)", zh: "颜色（圆点：已更改）")));
            let cur = st::custom_colors().unwrap_or([u32::MAX; 7]);
            let p = st::pal();
            let now = [p.bg, p.line, p.hover, p.active, p.accent, p.text, p.dim];
            for (i, n) in color_names().iter().enumerate() {
                v.push(swatch(n, cur[i] != u32::MAX, now[i], Op::Color(i as u8)));
            }
            v.push(head(t!("Непрозрачность панели", "Bar opacity", ja: "バーの不透明度", pt: "Opacidade da barra", zh: "任务栏不透明度")));
            for a in PCT {
                let name = if a == 100 { t!("100 %, без прозрачности", "100 %, solid", ja: "100 %、不透明", pt: "100 %, sólida", zh: "100 %，不透明").to_string() } else { format!("{a} %") };
                v.push(choice(&name, st::bar_opacity() == a, Op::Opacity(a)));
            }
            v.push(danger(t!("Вернуть цвета темы", "Back to the theme's colors", ja: "テーマの色に戻す", pt: "Voltar às cores do tema", zh: "恢复主题颜色"), Op::ResetColors));
        }
        (LOOK, TEMPLATES) | (LOOK, DELETE) => {
            v.push(action(t!("‹   Назад", "‹   Back", ja: "‹   戻る", pt: "‹   Voltar", zh: "‹   返回"), "", Op::Page(TEMPLATES)));
            let deleting = page == DELETE;
            v.push(head(if deleting {
                t!("Нажмите на шаблон, чтобы удалить", "Click a template to delete it", ja: "削除するテンプレートをクリック", pt: "Clique em um modelo para excluí-lo", zh: "点击模板即可删除")
            } else {
                t!("Мои шаблоны", "My templates", ja: "マイ テンプレート", pt: "Meus modelos", zh: "我的模板")
            }));
            let now = st::Theme::current("");
            let n = THEMES.with_borrow(|t| t.len());
            if n == 0 {
                v.push(info(t!("Пока пусто: настройте вид и сохраните", "None yet: set up a look and save it", ja: "まだありません: 外観を設定して保存してください", pt: "Nenhum ainda: monte uma aparência e salve", zh: "暂无：设置好外观后保存")));
            }
            for i in 0..n {
                let (name, style, dot) = THEMES.with_borrow(|t| {
                    let th = &t[i];
                    let dot = if th.colors[4] != u32::MAX { th.colors[4] } else { st::PALS[st::ALL.iter().position(|&s| s == th.style).unwrap_or(0)].accent };
                    (th.name.clone(), crate::settings::style_name(th.style), dot)
                });
                let same = THEMES.with_borrow(|t| {
                    let th = &t[i];
                    (th.style, th.colors, th.opacity) == (now.style, now.colors, now.opacity)
                });
                if deleting {
                    v.push(Row { dot, hint: style.into(), ..danger(&name, Op::DeleteTheme(i as u8)) });
                } else {
                    v.push(Row { dot, hint: style.into(), ..choice(&name, same, Op::ApplyTheme(i as u8)) });
                }
            }
            if !deleting {
                v.push(action(t!("Сохранить текущий вид как шаблон", "Save the current look as a template", ja: "現在の外観をテンプレートとして保存", pt: "Salvar a aparência atual como modelo", zh: "将当前外观保存为模板"), "", Op::SaveTheme));
                if n > 0 {
                    v.push(danger(t!("Удалить шаблон…", "Delete a template…", ja: "テンプレートを削除…", pt: "Excluir um modelo…", zh: "删除模板…"), Op::Page(DELETE)));
                }
            }
        }
        (LOOK, _) => {
            v.push(head(t!("Тема", "Theme", ja: "テーマ", pt: "Tema", zh: "主题")));
            for (i, s) in st::ALL.iter().enumerate() {
                v.push(swatch(crate::settings::style_name(*s), st::style() == *s, st::PALS[i].accent, Op::Theme(i as u8)));
            }
            v.push(head(t!("Цвета и прозрачность", "Colors and transparency", ja: "色と透明度", pt: "Cores e transparência", zh: "颜色与透明度")));
            v.push(action(t!("Цвета и прозрачность…", "Colors and transparency…", ja: "色と透明度…", pt: "Cores e transparência…", zh: "颜色与透明度…"), "›", Op::Page(COLORS)));
            v.push(action(t!("Шаблоны…", "Templates…", ja: "テンプレート…", pt: "Modelos…", zh: "模板…"), "›", Op::Page(TEMPLATES)));
            v.push(head(t!("Непрозрачность панели", "Bar opacity", ja: "バーの不透明度", pt: "Opacidade da barra", zh: "任务栏不透明度")));
            for a in PCT {
                let name = if a == 100 { t!("100 %, без прозрачности", "100 %, solid", ja: "100 %、不透明", pt: "100 %, sólida", zh: "100 %，不透明").to_string() } else { format!("{a} %") };
                v.push(choice(&name, st::bar_opacity() == a, Op::Opacity(a)));
            }
            v.push(head(t!("Окна WinRust", "WinRust windows", ja: "WinRust のウィンドウ", pt: "Janelas do WinRust", zh: "WinRust 窗口")));
            v.push(check(t!("Мороз в окнах WinRust", "Frost in WinRust windows", ja: "WinRust のウィンドウをぼかす", pt: "Desfoque nas janelas do WinRust", zh: "WinRust 窗口模糊"), lk::win_blur(), Op::WinBlur));
        }
        (BAR, _) => {
            v.push(head(t!("Положение", "Position", ja: "位置", pt: "Posição", zh: "位置")));
            for (i, (s, name)) in [(st::Side::Bottom, t!("Снизу", "Bottom", ja: "下", pt: "Embaixo", zh: "底部")), (st::Side::Left, t!("Слева", "Left", ja: "左", pt: "Esquerda", zh: "左侧")), (st::Side::Right, t!("Справа", "Right", ja: "右", pt: "Direita", zh: "右侧"))].into_iter().enumerate() {
                v.push(choice(name, st::side() == s, Op::BarSide(i as u8)));
            }
            v.push(head(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小")));
            for (i, name) in [(0u8, t!("Мелкий", "Small", ja: "小", pt: "Pequeno", zh: "小")), (1, t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常")), (2, t!("Крупный", "Large", ja: "大", pt: "Grande", zh: "大"))] {
                v.push(choice(name, st::bar_size() == i, Op::BarSize(i)));
            }
            // one bar on every monitor, or one on a chosen one
            let mons = MONS.with(|m| m.get());
            if mons > 1 {
                v.push(head(t!("Монитор", "Monitor", ja: "モニター", pt: "Monitor", zh: "显示器")));
                v.push(choice(t!("Все экраны", "All screens", ja: "すべての画面", pt: "Todas as telas", zh: "所有屏幕"), lk::bar_monitor() < 0, Op::BarMonitor(-1)));
                for i in 0..mons {
                    let name = format!("{} {}", t!("Монитор", "Monitor", ja: "モニター", pt: "Monitor", zh: "显示器"), i + 1);
                    v.push(choice(&name, lk::bar_monitor() == i, Op::BarMonitor(i as i8)));
                }
            }
            v.push(head(t!("Размытие", "Blur", ja: "ぼかし", pt: "Desfoque", zh: "模糊")));
            v.push(check(t!("Размытие за панелью", "Blur behind the bar", ja: "バーの後ろをぼかす", pt: "Desfoque atrás da barra", zh: "任务栏后模糊"), lk::bar_blur(), Op::BarBlur));
            for (r, name) in [(4, t!("Слабое", "Soft", ja: "弱い", pt: "Suave", zh: "弱")), (8, t!("Среднее", "Medium", ja: "中", pt: "Média", zh: "中")), (14, t!("Сильное", "Strong", ja: "強い", pt: "Forte", zh: "强"))] {
                v.push(choice(name, lk::bar_radius() == r, Op::BarRadius(r)));
            }
            v.push(head(t!("Контекстное меню", "Context menu", ja: "コンテキスト メニュー", pt: "Menu de contexto", zh: "上下文菜单")));
            v.push(check(t!("Размытие за меню", "Blur behind the menu", ja: "メニューの後ろをぼかす", pt: "Desfoque atrás do menu", zh: "菜单后模糊"), lk::menu_blur(), Op::MenuBlur));
            v.push(head(t!("Непрозрачность меню", "Menu opacity", ja: "メニューの不透明度", pt: "Opacidade do menu", zh: "菜单不透明度")));
            for a in [90u8, 75, 67, 50, 35] {
                v.push(choice(&format!("{a} %"), lk::menu_alpha() == a, Op::MenuAlpha(a)));
            }
            v.push(head(t!("Сила размытия", "Blur strength", ja: "ぼかしの強さ", pt: "Intensidade do desfoque", zh: "模糊强度")));
            for r in [12, 18, 26] {
                v.push(choice(&format!("{r} px"), lk::menu_radius() == r, Op::MenuRadius(r)));
            }
            v.push(head(t!("Поведение", "Behavior", ja: "動作", pt: "Comportamento", zh: "行为")));
            v.push(check(t!("Скрывать автоматически", "Hide automatically", ja: "自動的に隠す", pt: "Ocultar automaticamente", zh: "自动隐藏"), st::autohide(), Op::Autohide));
            v.push(check(t!("Кнопки слева, как в Windows 10", "Buttons on the left, like Windows 10", ja: "ボタンを左に配置 (Windows 10 風)", pt: "Botões à esquerda, como no Windows 10", zh: "按钮靠左，如同 Windows 10"), st::align_left(), Op::AlignLeft));
            v.push(check(t!("Alt+Tab: только этот монитор", "Alt+Tab: this monitor only", ja: "Alt+Tab: このモニターのみ", pt: "Alt+Tab: somente este monitor", zh: "Alt+Tab：仅限此显示器"), st::alttab_per_monitor(), Op::AltTab));
            v.push(check(t!("Кнопка поиска", "Search button", ja: "検索ボタン", pt: "Botão de pesquisa", zh: "搜索按钮"), lk::bar_search(), Op::SearchBtn));
            v.push(check(t!("Кнопка задач", "Task View button", ja: "タスク ビュー ボタン", pt: "Botão de exibição de tarefas", zh: "任务视图按钮"), lk::bar_taskview(), Op::TaskView));
            v.push(head(t!("Уведомления", "Notifications", ja: "通知", pt: "Notificações", zh: "通知")));
            v.push(check(t!("Показывать уведомления", "Show notifications", ja: "通知を表示", pt: "Mostrar notificações", zh: "显示通知"), lk::notify(), Op::Notify));
            v.push(action(t!("Время на экране", "On screen for", ja: "表示時間", pt: "Tempo na tela", zh: "显示时长"), &format!("{} s ›", lk::notify_timeout()), Op::NotifyTimeout));
        }
        (START, _) => {
            v.push(head(t!("Положение", "Position", ja: "位置", pt: "Posição", zh: "位置")));
            v.push(choice(t!("По центру", "Centered", ja: "中央", pt: "Centralizado", zh: "居中"), !lk::start_left(), Op::StartPos(false)));
            v.push(choice(t!("Слева", "On the left", ja: "左", pt: "À esquerda", zh: "靠左"), lk::start_left(), Op::StartPos(true)));
            v.push(head(t!("Стиль", "Style", ja: "スタイル", pt: "Estilo", zh: "样式")));
            for (i, name) in ["Windows 11", "Windows 10", "Windows 7"].iter().enumerate() {
                v.push(choice(name, lk::start_style() == i as i32, Op::StartStyle(i as u8)));
            }
            v.push(head(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小")));
            for (i, name) in [(0u8, t!("Мелкий", "Small", ja: "小", pt: "Pequeno", zh: "小")), (1, t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常")), (2, t!("Крупный", "Large", ja: "大", pt: "Grande", zh: "大"))] {
                v.push(choice(name, lk::start_scale() == (match i { 0 => 90, 2 => 115, _ => 100 }), Op::StartScale(i)));
            }
        }
        (LANG, _) => {
            v.push(head(t!("Язык всех программ WinRust", "The language of every WinRust program", ja: "すべての WinRust プログラムの言語", pt: "O idioma de todos os programas da WinRust", zh: "所有 WinRust 程序的语言")));
            for (i, l) in winrust_common::Lang::ALL.iter().enumerate() {
                v.push(choice(l.name(), winrust_common::lang::get() == *l, Op::Lang(i as u8)));
            }
            v.push(info(t!("Изменение сразу применяется ко всем программам WinRust.", "The change applies to every WinRust app at once.", ja: "変更はすべての WinRust アプリにすぐ適用されます。", pt: "A mudança vale para todos os programas da WinRust de uma vez.", zh: "更改会立即应用到所有 WinRust 程序。")));
        }
        (APPS, _) => {
            let names = [
                t!("Проводник", "File manager", ja: "ファイル マネージャー", pt: "Gerenciador de arquivos", zh: "文件管理器"),
                t!("Диспетчер задач", "Task Manager", ja: "タスク マネージャー", pt: "Gerenciador de Tarefas", zh: "任务管理器"),
                t!("Панель управления", "Control Panel", ja: "コントロール パネル", pt: "Painel de Controle", zh: "控制面板"),
                t!("Снимки и запись", "Screenshots and recording", ja: "スクリーンショットと録画", pt: "Capturas e gravação", zh: "截图与录制"),
                t!("Параметры системы (экран, звук…)", "System settings (display, sound, …)", ja: "システム設定 (画面、サウンド…)", pt: "Configurações do sistema (tela, som…)", zh: "系统设置（显示、声音…）"),
            ];
            let have = HAVE.with(|h| h.get());
            v.push(head(t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开")));
            for i in 0..APPS_LIST.len() {
                if have[i] {
                    v.push(action(names[i], APPS_LIST[i].1, Op::OpenApp(i as u8)));
                } else {
                    v.push(Row { hint: t!("Не установлено", "Not installed", ja: "インストールされていません", pt: "Não instalado", zh: "未安装").into(), ..info(names[i]) });
                }
            }
            v.push(head(t!("Настройки", "Settings", ja: "設定", pt: "Configurações", zh: "设置")));
            v.push(info(t!("Каждая программа WinRust хранит свои настройки в своём окне.", "Every WinRust app keeps its own options in its own window.", ja: "各 WinRust アプリの設定はそのアプリのウィンドウにあります。", pt: "Cada programa da WinRust guarda suas opções na própria janela.", zh: "每个 WinRust 程序都在自己的窗口中保存设置。")));
        }
        (ABOUT, RESET) => {
            v.push(action(t!("‹   Назад", "‹   Back", ja: "‹   戻る", pt: "‹   Voltar", zh: "‹   返回"), "", Op::Page(ROOT)));
            v.push(head(t!("Сбросить всё?", "Reset everything?", ja: "すべてをリセットします?", pt: "Restaurar tudo?", zh: "全部重置？")));
            v.push(danger(t!("Да, сбросить", "Yes, reset", ja: "はい、リセットします", pt: "Sim, restaurar", zh: "是，重置"), Op::ResetAll));
            v.push(action(t!("Отмена", "Cancel", ja: "キャンセル", pt: "Cancelar", zh: "取消"), "", Op::Page(ROOT)));
        }
        _ => {
            v.push(head(t!("Панель", "Bar", ja: "バー", pt: "Barra", zh: "任务栏")));
            v.push(danger(t!("Сбросить всё по умолчанию…", "Reset everything to defaults…", ja: "すべてを既定に戻す…", pt: "Restaurar tudo para o padrão…", zh: "全部恢复默认…"), Op::Page(RESET)));
            v.push(head(t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言")));
            v.push(action(t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言"), &format!("{} ›", winrust_common::lang::get().name()), Op::Cat(LANG)));
            v.push(head(t!("О программе", "About", ja: "バージョン情報", pt: "Sobre", zh: "关于")));
            v.push(action(t!("Версия", "Version", ja: "バージョン", pt: "Versão", zh: "版本"), env!("CARGO_PKG_VERSION"), Op::None));
            v.push(info(r"%ProgramData%\WinRust\look.txt"));
            v.push(info(r"%ProgramData%\WinRust\lang.txt"));
            v.push(info(r"%ProgramData%\WinRust\themes.txt"));
            v.push(action(t!("Закрыть", "Close", ja: "閉じる", pt: "Fechar", zh: "关闭"), "Esc", Op::Close));
        }
    }
    v
}

/// Row geometry: the rect and the row of every visible item of the current page, already scrolled.
fn layout(w: i32, cat: u8, page: u8, scroll: i32) -> Vec<(RECT, Row)> {
    let mut y = sc(TOP) - scroll;
    let mut v = Vec::new();
    for r in rows(cat, page) {
        let h = if r.kind == Kind::Head { sc(HEAD_H) } else { sc(ROW_H) };
        v.push((RECT { left: sc(RAIL + PAD), top: y, right: w - sc(PAD), bottom: y + h }, r));
        y += h;
    }
    v
}

fn content_bottom(h: i32) -> i32 {
    h - sc(10)
}

fn max_scroll(w: i32, h: i32, cat: u8, page: u8) -> i32 {
    let last = layout(w, cat, page, 0).last().map_or(0, |(r, _)| r.bottom);
    (last + sc(8) - content_bottom(h)).max(0)
}

fn clamp_scroll(w: i32, h: i32) {
    let (cat, page) = (CAT.with(|c| c.get()), PAGE.with(|p| p.get()));
    let s = SCROLL.with(|s| s.get()).clamp(0, max_scroll(w, h, cat, page));
    SCROLL.with(|x| x.set(s));
}

fn tab_rect(i: usize) -> RECT {
    let top = sc(96) + i as i32 * sc(46);
    RECT { left: sc(10), top, right: sc(RAIL - 10), bottom: top + sc(38) }
}

/// The close button at the right end of the content header (Esc does the same).
fn close_rect(w: i32) -> RECT {
    RECT { left: w - sc(46), top: sc(18), right: w - sc(16), bottom: sc(48) }
}

unsafe fn font(px: i32, weight: i32) -> HFONT {
    let face: Vec<u16> = winrust_common::style::ui_face().encode_utf16().chain([0]).collect();
    CreateFontW(-sc(px), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr())
}

fn ensure_fonts() {
    if FONT.with(|f| f.get()[0]) != 0 {
        return;
    }
    unsafe {
        FONT.with(|f| f.set([font(13, 400) as usize, font(17, 600) as usize, font(11, 400) as usize, font(13, 600) as usize]));
    }
}

unsafe fn paint(h: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let wdc = BeginPaint(h, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(h, &mut cr);
    if cr.right <= 0 || cr.bottom <= 0 {
        EndPaint(h, &ps);
        return;
    }
    ensure_fonts();
    clamp_scroll(cr.right, cr.bottom); // a page that shrank (a template deleted) must not leave the rows off screen
    // double buffer: the blurred backdrop is expensive, and painting it straight to the window flickers
    let dc = CreateCompatibleDC(wdc);
    let bmp = CreateCompatibleBitmap(wdc, cr.right, cr.bottom);
    let obmp = SelectObject(dc, bmp);
    let p = st::pal();
    let (cat, page, scroll) = (CAT.with(|c| c.get()), PAGE.with(|g| g.get()), SCROLL.with(|s| s.get()));
    let inner = RECT { left: 1, top: 1, right: cr.right - 1, bottom: cr.bottom - 1 };
    // backdrop: the blurred screen captured when the window opened, then the theme colour with a floor of 62 %
    let drew = BITS.with(|b| {
        let b = b.borrow();
        if b.is_empty() {
            return false;
        }
        crate::blur::paint_capture(dc, &b, inner, p.bg, (255 * lk::menu_alpha().max(60) as u32 / 100) as u8);
        true
    });
    if !drew {
        let br = CreateSolidBrush(p.bg);
        FillRect(dc, &inner, br);
        DeleteObject(br);
    }
    let edge = CreateSolidBrush(st::mix(p.bg, p.text, 0.12));
    FrameRect(dc, &cr, edge);
    DeleteObject(edge);
    // the rail
    let rail = CreateSolidBrush(st::mix(p.bg, p.text, 0.05));
    FillRect(dc, &RECT { left: sc(1), top: sc(1), right: sc(RAIL), bottom: cr.bottom - sc(1) }, rail);
    DeleteObject(rail);
    let (f, f_head, f_small, f_tab) = FONT.with(|x| {
        let a = x.get();
        (a[0] as HFONT, a[1] as HFONT, a[2] as HFONT, a[3] as HFONT)
    });
    SetBkMode(dc, TRANSPARENT as i32);
    // "WinRust" over the rail, then the categories
    SelectObject(dc, f_small);
    SetTextColor(dc, p.dim);
    let mut rr = RECT { left: sc(20), top: sc(18), right: sc(RAIL - 10), bottom: sc(40) };
    DrawTextW(dc, wide("WinRust").as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, f_head);
    SetTextColor(dc, p.text);
    let mut rr = RECT { left: sc(20), top: sc(40), right: sc(RAIL - 10), bottom: sc(72) };
    DrawTextW(dc, wide(t!("Настройки", "Settings", ja: "設定", pt: "Configurações", zh: "设置")).as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    for i in 0..CATS {
        let r = tab_rect(i);
        let on = i as u8 == cat;
        let hot = HOT_CAT.with(|x| x.get()) == i as i32;
        if on || hot {
            let br = CreateSolidBrush(if on { st::mix(p.bg, p.accent, 0.20) } else { p.hover });
            let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
            RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, sc(9), sc(9));
            SelectObject(dc, o1);
            SelectObject(dc, o2);
            DeleteObject(br);
        }
        SelectObject(dc, f_tab);
        SetTextColor(dc, if on { p.text } else { p.dim });
        let mut rr = RECT { left: r.left + sc(12), top: r.top, right: r.right - sc(8), bottom: r.bottom };
        DrawTextW(dc, wide(&cat_title(i as u8)).as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    }
    // the header of the content
    SelectObject(dc, f_head);
    SetTextColor(dc, p.text);
    let title = if page == ROOT { cat_title(cat) } else { page_title(page) };
    let mut hr = RECT { left: sc(RAIL + PAD), top: sc(20), right: cr.right - sc(60), bottom: sc(50) };
    DrawTextW(dc, wide(&title).as_ptr(), -1, &mut hr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, f_small);
    SetTextColor(dc, p.dim);
    let sub = if page == ROOT { cat_sub(cat) } else { page_sub(page) };
    let mut sr = RECT { left: sc(RAIL + PAD), top: sc(50), right: cr.right - sc(60), bottom: sc(70) };
    DrawTextW(dc, wide(&sub).as_ptr(), -1, &mut sr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    // the close button (Esc does the same)
    let cx = close_rect(cr.right);
    if HOTX.with(|x| x.get()) {
        let br = CreateSolidBrush(p.hover);
        let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
        RoundRect(dc, cx.left, cx.top, cx.right + 1, cx.bottom + 1, sc(8), sc(8));
        SelectObject(dc, o1);
        SelectObject(dc, o2);
        DeleteObject(br);
    }
    SelectObject(dc, f);
    SetTextColor(dc, p.text);
    let mut xr = cx;
    DrawTextW(dc, wide("\u{2715}").as_ptr(), -1, &mut xr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    let line = CreateSolidBrush(st::mix(p.bg, p.text, 0.10));
    FillRect(dc, &RECT { left: sc(RAIL + PAD), top: sc(74), right: cr.right - sc(PAD), bottom: sc(75) }, line);
    DeleteObject(line);
    // the rows, clipped to the content area so a scrolled one does not run over the header
    let saved = SaveDC(dc);
    IntersectClipRect(dc, sc(RAIL + 1), sc(TOP) - sc(4), cr.right - sc(1), content_bottom(cr.bottom));
    let hot = HOT.with(|x| x.get());
    for (i, (r, row)) in layout(cr.right, cat, page, scroll).into_iter().enumerate() {
        if r.bottom < sc(TOP) - sc(4) || r.top > content_bottom(cr.bottom) {
            continue;
        }
        match row.kind {
            Kind::Head => {
                SelectObject(dc, f_small);
                SetTextColor(dc, p.dim);
                let mut rr = RECT { left: r.left + sc(4), top: r.top + sc(6), right: r.right, bottom: r.bottom };
                DrawTextW(dc, wide(&row.label).as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
            Kind::Info => {
                SelectObject(dc, f_small);
                SetTextColor(dc, p.dim);
                let mut rr = RECT { left: r.left + sc(6), top: r.top, right: if row.hint.is_empty() { r.right } else { r.right - sc(54) }, bottom: r.bottom };
                DrawTextW(dc, wide(&row.label).as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
                if !row.hint.is_empty() {
                    let mut hr = RECT { left: r.right - sc(60), top: r.top, right: r.right, bottom: r.bottom };
                    DrawTextW(dc, wide(&row.hint).as_ptr(), -1, &mut hr, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                }
            }
            _ => {
                let hovered = hot == i as i32;
                if hovered {
                    let br = CreateSolidBrush(p.hover);
                    let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
                    RoundRect(dc, r.left - sc(6), r.top, r.right + 1, r.bottom, sc(6), sc(6));
                    SelectObject(dc, o1);
                    SelectObject(dc, o2);
                    DeleteObject(br);
                }
                // the marker: a colour dot, a dot for the chosen choice, a tick for a toggle that is on
                let mut left = r.left + sc(6);
                if row.dot != u32::MAX {
                    let br = CreateSolidBrush(row.dot);
                    let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
                    let (cx, cy) = (r.left + sc(12), (r.top + r.bottom) / 2);
                    Ellipse(dc, cx - sc(6), cy - sc(6), cx + sc(6), cy + sc(6));
                    SelectObject(dc, o1);
                    SelectObject(dc, o2);
                    DeleteObject(br);
                    left = r.left + sc(30);
                } else if let Kind::Choice(on) = row.kind {
                    SelectObject(dc, f);
                    SetTextColor(dc, p.text);
                    let mut mr = RECT { left: r.left + sc(4), top: r.top, right: r.left + sc(28), bottom: r.bottom };
                    DrawTextW(dc, wide(if on { "\u{25CF}" } else { "\u{25CB}" }).as_ptr(), -1, &mut mr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                    left = r.left + sc(32);
                } else if let Kind::Check(on) = row.kind {
                    if on {
                        SelectObject(dc, f);
                        SetTextColor(dc, p.accent);
                        let mut mr = RECT { left: r.left + sc(4), top: r.top, right: r.left + sc(28), bottom: r.bottom };
                        DrawTextW(dc, wide("\u{2713}").as_ptr(), -1, &mut mr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                    }
                    left = r.left + sc(32);
                }
                SelectObject(dc, f);
                SetTextColor(dc, if row.kind == Kind::Danger { st::rgb(232, 92, 92) } else { p.text });
                let mut rr = RECT { left, top: r.top, right: if row.hint.is_empty() { r.right } else { r.right - sc(54) }, bottom: r.bottom };
                DrawTextW(dc, wide(&row.label).as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
                if !row.hint.is_empty() {
                    SelectObject(dc, f_small);
                    SetTextColor(dc, p.dim);
                    let mut hr = RECT { left: r.right - sc(70), top: r.top, right: r.right, bottom: r.bottom };
                    DrawTextW(dc, wide(&row.hint).as_ptr(), -1, &mut hr, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
                }
            }
        }
    }
    RestoreDC(dc, saved);
    // a slim scroll indicator when the category is longer than the window
    let max = max_scroll(cr.right, cr.bottom, cat, page);
    if max > 0 {
        let track = RECT { left: cr.right - sc(9), top: sc(TOP), right: cr.right - sc(5), bottom: content_bottom(cr.bottom) };
        let br = CreateSolidBrush(st::mix(p.bg, p.text, 0.08));
        FillRect(dc, &track, br);
        DeleteObject(br);
        let view = (track.bottom - track.top).max(1);
        let hgt = (view as i64 * view as i64 / (view as i64 + max as i64)).max(sc(24) as i64) as i32;
        let off = ((view - hgt) as i64 * scroll as i64 / max as i64) as i32;
        let br = CreateSolidBrush(st::mix(p.bg, p.text, 0.30));
        let bar = RECT { left: track.left, top: track.top + off, right: track.right, bottom: track.top + off + hgt };
        let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
        RoundRect(dc, bar.left, bar.top, bar.right + 1, bar.bottom + 1, sc(4), sc(4));
        SelectObject(dc, o1);
        SelectObject(dc, o2);
        DeleteObject(br);
    }
    BitBlt(wdc, 0, 0, cr.right, cr.bottom, dc, 0, 0, SRCCOPY);
    SelectObject(dc, obmp);
    DeleteObject(bmp);
    DeleteDC(dc);
    EndPaint(h, &ps);
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

unsafe fn hit_row(h: HWND, x: i32, y: i32) -> Option<(usize, RECT, Row)> {
    let mut cr: RECT = zeroed();
    GetClientRect(h, &mut cr);
    if y < sc(TOP) - sc(4) || y > content_bottom(cr.bottom) || x < sc(RAIL + PAD) - sc(6) {
        return None;
    }
    let (cat, page, scroll) = (CAT.with(|c| c.get()), PAGE.with(|p| p.get()), SCROLL.with(|s| s.get()));
    layout(cr.right, cat, page, scroll)
        .into_iter()
        .enumerate()
        .find(|(_, (r, _))| x >= r.left - sc(6) && x < r.right && y >= r.top && y < r.bottom)
        .map(|(i, (r, row))| (i, r, row))
}

fn hit_cat(x: i32, y: i32) -> Option<usize> {
    if x >= sc(RAIL) {
        return None;
    }
    (0..CATS).find(|&i| {
        let r = tab_rect(i);
        y >= r.top && y < r.bottom
    })
}

unsafe fn apply(op: Op, h: HWND) -> bool {
    match op {
        Op::Cat(c) => {
            CAT.with(|x| x.set(c.min(CATS as u8 - 1)));
            PAGE.with(|x| x.set(ROOT));
            SCROLL.with(|x| x.set(0));
        }
        Op::Page(p) => {
            PAGE.with(|x| x.set(p));
            SCROLL.with(|x| x.set(0));
        }
        Op::Theme(i) => {
            if let Some(s) = st::ALL.get(i as usize) {
                st::set_style(*s);
            }
            crate::relook(true);
        }
        Op::Color(i) => {
            use windows_sys::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
            let p = st::pal();
            let now = [p.bg, p.line, p.hover, p.active, p.accent, p.text, p.dim];
            static CUST: std::sync::Mutex<[u32; 16]> = std::sync::Mutex::new([0xFFFFFF; 16]);
            let mut cust = CUST.lock().unwrap_or_else(|e| e.into_inner());
            let mut cc: CHOOSECOLORW = zeroed();
            cc.lStructSize = std::mem::size_of::<CHOOSECOLORW>() as u32;
            cc.hwndOwner = h;
            cc.rgbResult = now[(i as usize).min(6)];
            cc.lpCustColors = cust.as_mut_ptr();
            cc.Flags = CC_RGBINIT | CC_FULLOPEN;
            if ChooseColorW(&mut cc) != 0 {
                st::set_custom_color((i as usize).min(6), Some(cc.rgbResult));
                crate::relook(false);
            }
        }
        Op::Opacity(v) => {
            st::set_bar_opacity(v);
            crate::relook(false); // every bar: place() applies the opacity to each of them
        }
        Op::ResetColors => {
            st::reset_custom();
            crate::relook(false);
        }
        Op::ApplyTheme(i) => {
            let th = THEMES.with_borrow(|t| t.get(i as usize).cloned());
            if let Some(th) = th {
                th.apply();
                crate::relook(true);
            }
        }
        Op::SaveTheme => {
            let name = |n: usize| tf!("Шаблон {}", "Template {}", ja: "テンプレート {}", pt: "Modelo {}", zh: "模板 {}", n);
            let mut list = THEMES.with_borrow(|t| t.clone());
            let n = (1..).find(|&n| !list.iter().any(|t| t.name == name(n))).unwrap_or(1);
            list.push(st::Theme::current(&name(n)));
            st::save_themes(&list);
            THEMES.with(|t| *t.borrow_mut() = list);
        }
        Op::DeleteTheme(i) => {
            let mut list = THEMES.with_borrow(|t| t.clone());
            if (i as usize) < list.len() {
                list.remove(i as usize);
                st::save_themes(&list);
            }
            THEMES.with(|t| *t.borrow_mut() = list);
            PAGE.with(|x| x.set(TEMPLATES));
        }
        Op::BarSide(i) => {
            st::set_side([st::Side::Bottom, st::Side::Left, st::Side::Right][(i as usize).min(2)]);
            crate::relook(true);
        }
        Op::BarSize(i) => {
            st::set_bar_size(i.min(2));
            crate::relook(true);
        }
        Op::BarMonitor(i) => {
            lk::bar_monitor_set(i as i32);
            crate::bars_sync(); // close the bars that are not wanted, create the missing ones, place them all
            crate::relook(true);
        }
        Op::Autohide => {
            st::set_autohide(!st::autohide());
            crate::relook(true);
        }
        Op::AlignLeft => {
            st::set_align_left(!st::align_left());
            crate::relook(true);
        }
        Op::AltTab => st::set_alttab_per_monitor(!st::alttab_per_monitor()),
        Op::SearchBtn => {
            lk::bar_search_set(!lk::bar_search());
            crate::invalidate_bars();
        }
        Op::TaskView => {
            lk::bar_taskview_set(!lk::bar_taskview());
            crate::invalidate_bars();
        }
        Op::Notify => {
            lk::notify_set(!lk::notify());
            crate::notify::refresh();
        }
        Op::NotifyTimeout => {
            let next = match lk::notify_timeout() {
                3 => 5,
                5 => 8,
                8 => 10,
                _ => 3,
            };
            lk::notify_timeout_set(next);
        }
        Op::BarBlur => {
            lk::bar_blur_set(!lk::bar_blur());
            crate::relook(false); // every bar: place() applies the opacity that goes with the blur
        }
        Op::BarRadius(r) => lk::bar_radius_set(r),
        Op::WinBlur => {
            lk::win_blur_set(!lk::win_blur());
            // the apps that draw their own frost read look.txt on their next paint; the bar only forwards the news
            crate::invalidate_bars();
        }
        Op::MenuBlur => lk::menu_blur_set(!lk::menu_blur()),
        Op::MenuAlpha(a) => lk::menu_alpha_set(a),
        Op::MenuRadius(r) => lk::menu_radius_set(r),
        Op::StartPos(left) => lk::start_left_set(left),
        Op::StartStyle(i) => lk::start_style_set(i as i32),
        Op::StartScale(i) => lk::start_scale_set(i as i32),
        Op::Lang(i) => {
            if let Some(l) = winrust_common::Lang::ALL.get(i as usize) {
                if !winrust_common::lang::set(*l) {
                    crate::tlog("settings: lang.txt could not be written (run the installer as administrator to apply it everywhere)");
                }
                crate::invalidate_bars();
            }
        }
        Op::OpenApp(i) => open_app(i),
        Op::ResetAll => {
            st::reset_all();
            lk::reset_all();
            crate::bars_sync();
            crate::relook(true);
            crate::notify::refresh();
            PAGE.with(|x| x.set(ROOT));
        }
        Op::Close => return false,
        Op::None => {}
    }
    crate::invalidate_bars();
    true
}

/// Starts one of the programs in `APPS_LIST` (rs-shot through the helper that knows where it is installed).
fn open_app(i: u8) {
    if i as usize == 3 {
        crate::shot::rs_shot_start(&[]); // the capture bar, which is rs-shot's own way in
        return;
    }
    let Some(&(app, exe)) = APPS_LIST.get(i as usize) else { return };
    let mut paths = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        paths.push(std::path::PathBuf::from(pf).join(app).join(exe));
    }
    if let Ok(e) = std::env::current_exe() {
        paths.push(e.with_file_name(exe));
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        paths.push(std::path::PathBuf::from(la).join(r"plugin-launcher\bin").join(exe));
    }
    if let Some(p) = paths.into_iter().find(|p| p.is_file()) {
        // off the bar's thread: a slow launch must not freeze the window
        std::thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new(p).creation_flags(0x0800_0000).spawn();
        });
    }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(h);
            0
        }
        WM_MOUSEMOVE => {
            let cat = hit_cat(x, y);
            let row = hit_row(h, x, y).map(|(i, _, _)| i as i32).unwrap_or(-1);
            let (old_cat, old_row) = (HOT_CAT.with(|c| c.replace(cat.map_or(-1, |i| i as i32))), HOT.with(|c| c.replace(row)));
            let mut cr: RECT = zeroed();
            GetClientRect(h, &mut cr);
            let xr = close_rect(cr.right);
            let on_x = x >= xr.left && x < xr.right && y >= xr.top && y < xr.bottom;
            let was_x = HOTX.with(|c| c.replace(on_x));
            if old_cat != cat.map_or(-1, |i| i as i32) || old_row != row || was_x != on_x {
                InvalidateRect(h, ptr::null(), 0);
            }
            // and ask to be told when the pointer leaves, so the last row does not stay lit
            let mut tme: windows_sys::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT = zeroed();
            tme.cbSize = std::mem::size_of_val(&tme) as u32;
            tme.dwFlags = windows_sys::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE;
            tme.hwndTrack = h;
            windows_sys::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut tme);
            0
        }
        WM_MOUSELEAVE => {
            let (c, r, x) = (HOT_CAT.with(|v| v.replace(-1)), HOT.with(|v| v.replace(-1)), HOTX.with(|v| v.replace(false)));
            if c != -1 || r != -1 || x {
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let mut cr: RECT = zeroed();
            GetClientRect(h, &mut cr);
            let (cat, page) = (CAT.with(|c| c.get()), PAGE.with(|p| p.get()));
            let step = sc(ROW_H * 3);
            let up = (wp >> 16) as i16 > 0;
            let s = SCROLL.with(|v| v.get());
            SCROLL.with(|v| v.set((if up { s - step } else { s + step }).clamp(0, max_scroll(cr.right, cr.bottom, cat, page))));
            InvalidateRect(h, ptr::null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            let mut cr: RECT = zeroed();
            GetClientRect(h, &mut cr);
            let xr = close_rect(cr.right);
            if x >= xr.left && x < xr.right && y >= xr.top && y < xr.bottom {
                ShowWindow(h, SW_HIDE);
                return 0;
            }
            if let Some(i) = hit_cat(x, y) {
                CAT.with(|c| c.set(i as u8));
                PAGE.with(|p| p.set(ROOT));
                SCROLL.with(|s| s.set(0));
                InvalidateRect(h, ptr::null(), 0);
                return 0;
            }
            if let Some((_, _, row)) = hit_row(h, x, y) {
                if row.op != Op::None {
                    if !apply(row.op, h) {
                        ShowWindow(h, SW_HIDE);
                        return 0;
                    }
                    InvalidateRect(h, ptr::null(), 0);
                }
            }
            0
        }
        WM_KEYDOWN => {
            if wp as u16 == VK_ESCAPE {
                ShowWindow(h, SW_HIDE); // the window is kept: the next open is one ShowWindow, not a rebuild
            }
            0
        }
        WM_CLOSE => {
            ShowWindow(h, SW_HIDE);
            0
        }
        WM_DESTROY => {
            for f in FONT.with(|x| x.replace([0; 4])) {
                if f != 0 {
                    DeleteObject(f as HFONT);
                }
            }
            BITS.with(|b| b.borrow_mut().clear());
            WND.with(|w| w.set(ptr::null_mut()));
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

/// Registers the window class and creates the window (hidden). Called once at startup with the bar's `hInstance`: the
/// class and the window then exist before the first click, which is the same order every other popup of the bar uses.
pub(crate) unsafe fn init(hi: windows_sys::Win32::Foundation::HINSTANCE) {
    let class = wide("topbar_settings");
    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(proc),
        hInstance: hi,
        hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    if GetClassInfoW(hi, class.as_ptr(), &mut zeroed()) == 0 {
        let atom = RegisterClassW(&wc);
        if atom == 0 {
            // A class with this name that GetClassInfoW cannot describe (registered by another module, or left behind
            // half-made): drop it and put ours in its place instead of creating a window of a class nobody can use.
            let first = windows_sys::Win32::Foundation::GetLastError();
            UnregisterClassW(class.as_ptr(), hi);
            let atom = RegisterClassW(&wc);
            crate::tlog(&format!("settings: RegisterClassW failed ({first}), after UnregisterClassW atom {atom} (last error {})", windows_sys::Win32::Foundation::GetLastError()));
        } else {
            crate::tlog(&format!("settings: RegisterClassW -> atom {atom}"));
        }
    }
    let h = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, sc(W), sc(H), ptr::null_mut(), ptr::null_mut(), hi, ptr::null());
    if h.is_null() {
        crate::tlog(&format!("settings: CreateWindowExW failed (last error {})", windows_sys::Win32::Foundation::GetLastError()));
    } else {
        crate::tlog(&format!("settings: window ready at {h:?}"));
    }
    WND.with(|w| w.set(h));
    refresh_cache();
}

/// Re-reads what the window shows but should not read on every repaint: the templates, the monitor count, the apps.
fn refresh_cache() {
    THEMES.with(|t| *t.borrow_mut() = st::themes());
    MONS.with(|m| m.set(winrust_common::monitors::all().len().max(1) as i32));
    let mut have = EMPTY;
    for (i, &(app, exe)) in APPS_LIST.iter().enumerate() {
        have[i] = installed(app, exe);
    }
    HAVE.with(|h| h.set(have));
}

unsafe fn existing() -> Option<HWND> {
    let class = wide("topbar_settings");
    let h = FindWindowW(class.as_ptr(), ptr::null());
    (!h.is_null()).then_some(h)
}

/// Where the window goes and how big it is: centered on the monitor of the bar that asked for it, never taller or wider
/// than that monitor allows (a 640 px template at 125 % would otherwise run off a 768 px screen).
fn place() -> (i32, i32, i32, i32) {
    let m = winrust_common::monitors::of(unsafe { crate::cur_bar() }).rc;
    let (mw, mh) = (m.right - m.left, m.bottom - m.top);
    let w = sc(W).min(mw - sc(32)).max(sc(720));
    let h = sc(H).min(mh - sc(48)).max(sc(560));
    let x = m.left + (mw - w) / 2;
    let y = m.top + ((mh - h) / 2).max(sc(16));
    (x, y, w, h)
}

unsafe fn show(h: HWND, cat: u8, r: (i32, i32, i32, i32)) {
    CAT.with(|c| c.set(cat.min(CATS as u8 - 1)));
    PAGE.with(|p| p.set(ROOT));
    SCROLL.with(|s| s.set(0));
    HOT.with(|v| v.set(-1));
    HOT_CAT.with(|v| v.set(-1));
    HOTX.with(|v| v.set(false));
    let (x, y, w, hh) = r;
    // above every other topmost window (the bar is one) and visible even when it was only hidden
    SetWindowPos(h, HWND_TOPMOST, x, y, w, hh, SWP_SHOWWINDOW);
    SetForegroundWindow(h);
    InvalidateRect(h, ptr::null(), 0);
}

/// Opens the settings window on `cat` (one of `LOOK`, `BAR`, `START`, `LANG`, `APPS`, `ABOUT`). False when the window
/// cannot be created at all: the caller then keeps a shorter flyout page instead of leaving the click doing nothing.
pub(crate) unsafe fn open_at(cat: u8) -> bool {
    refresh_cache();
    let mut h = existing().or_else(|| { let w = WND.with(|x| x.get()); (!w.is_null()).then_some(w) }).unwrap_or(ptr::null_mut());
    if h.is_null() || IsWindow(h) == 0 {
        crate::tlog("settings: no window yet, creating one now");
        init(GetModuleHandleW(ptr::null()));
        h = WND.with(|w| w.get());
        if h.is_null() || IsWindow(h) == 0 {
            let msg = t!("Не удалось открыть настройки WinRust.", "WinRust settings could not be opened.", ja: "WinRust の設定を開けませんでした。", pt: "Não foi possível abrir as Configurações do WinRust.", zh: "无法打开 WinRust 设置。");
            crate::tlog(&format!("settings: the window could not be created (last error {}): {msg}", windows_sys::Win32::Foundation::GetLastError()));
            MessageBoxW(ptr::null_mut(), wide(msg).as_ptr(), wide("WinRust").as_ptr(), MB_OK | MB_ICONERROR);
            return false;
        }
    }
    // the fading ghost of the menu that opened this window would sit on top and eat the clicks (and end up in the blur)
    crate::fly::kill_ghost();
    // the blurred screen behind it, captured while it is still hidden (its own pixels would end up in the blur otherwise)
    let (x, y, w, ht) = place();
    SetWindowRgn(h, CreateRoundRectRgn(0, 0, w + 1, ht + 1, sc(24), sc(24)), 1);
    if IsWindowVisible(h) == 0 {
        let radius = sc(lk::menu_radius()).max(ht / 22).max(sc(10));
        let px = crate::blur::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + ht }, radius);
        BITS.with(|b| *b.borrow_mut() = px);
    }
    ensure_fonts();
    show(h, cat, (x, y, w, ht));
    crate::tlog(&format!("settings: opened on category {cat} at {x},{y} {w}x{ht} (visible {})", IsWindowVisible(h)));
    true
}

