//! The WinRust settings window: one real window with a rail of categories on the left (an icon and a name each) and the options of
//! the chosen one on the right, in cards (`head` + rows). There are no second windows and no flyout pages: every option
//! the bar owns lives here, the desktop markers `rust-explorer` reads are written here, and the apps' own settings are
//! reached from the Apps category.
//!
//! The values still go to the files every WinRust program reads (`look.txt`, `style.txt`, `colors.txt`, `lang.txt`,
//! `%APPDATA%\WinRust\*`), so a change applies while the user is still clicking. `layout()` is used by both the painter and
//! the hit test, so a click always lands where the row is drawn. Long categories scroll with the wheel; the window is
//! double buffered.
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    cell::{Cell, RefCell},
    mem::zeroed,
    path::PathBuf,
    ptr,
    sync::OnceLock,
};

use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{HiDpi::{GetDpiForSystem, GetDpiForWindow}, Input::KeyboardAndMouse::VK_ESCAPE, Shell::ShellExecuteW, WindowsAndMessaging::*},
};

use winrust_common::{frost, look as lk, style as st};

/// Window size (a big popup, like Windows' own Settings): a rail, a page title and a couple of cards.
const W: i32 = 960;
const H: i32 = 680;
const RAIL: i32 = 244;
const PAD: i32 = 30;
const ROW_H: i32 = 40;
const HEAD_H: i32 = 34;
const TOP: i32 = 104; // first card, under the page header
const MIN_W: i32 = 720;
const MIN_H: i32 = 560;
const CLASS: &str = "WinRustSettings";
const TITLE: &str = "WinRust Settings";

/// Categories of the rail, in order: this is the index `open_at()` takes.
pub(crate) const LOOK: u8 = 0;
pub(crate) const DESK: u8 = 1;
pub(crate) const BAR: u8 = 2;
pub(crate) const START: u8 = 3;
pub(crate) const LANG: u8 = 4;
pub(crate) const APPS: u8 = 5;
pub(crate) const ADV: u8 = 6;
const CATS: usize = 7;

thread_local! {
    static DPI: Cell<i32> = const { Cell::new(96) };
}

/// A logical size in pixels at this window's DPI.
fn sc(v: i32) -> i32 {
    (v as f32 * DPI.with(|d| d.get()) as f32 / 96.0).round() as i32
}

#[derive(Clone, Copy, PartialEq)]
enum Op {
    None,
    Cat(u8),
    Theme(u8),
    Color(u8),
    Opacity(u8),
    ResetColors,
    ApplyTheme(u8),
    DeleteTheme(u8),
    SaveTheme,
    MenuBlur,
    MenuAlpha(u8),
    MenuRadius(i32),
    WinBlur,
    DeskIcons,
    SmallIcons,
    Wallpaper,
    WallpaperReload,
    Recycle,
    DeskFolder,
    BarSide(u8),
    BarSize(u8),
    BarMonitor(i8),
    BarBlur,
    BarRadius(i32),
    Autohide,
    Dodge,
    AlignLeft,
    AltTab,
    SearchBtn,
    TaskView,
    Notify,
    NotifyTimeout,
    StartPos(bool),
    StartStyle(u8),
    StartScale(u8),
    StartRanked,
    ClearHistory,
    PinsFolder,
    Lang(u8),
    OpenApp(u8),
    SystemPage(u8),
    ResetAsk,
    ResetAll,
    ResetCancel,
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
    del: Op,  // a second action on the right end of the row (the ✕ of a template), None = nothing
    dot: u32, // colour dot before the label (COLORREF), u32::MAX = none
}

fn head(label: &str) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Head, op: Op::None, del: Op::None, dot: u32::MAX }
}
fn check(label: &str, on: bool, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Check(on), op, del: Op::None, dot: u32::MAX }
}
fn choice(label: &str, on: bool, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Choice(on), op, del: Op::None, dot: u32::MAX }
}
fn action(label: &str, hint: &str, op: Op) -> Row {
    Row { label: label.into(), hint: hint.into(), kind: Kind::Action, op, del: Op::None, dot: u32::MAX }
}
fn danger(label: &str, op: Op) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Danger, op, del: Op::None, dot: u32::MAX }
}
fn info(label: &str) -> Row {
    Row { label: label.into(), hint: String::new(), kind: Kind::Info, op: Op::None, del: Op::None, dot: u32::MAX }
}
/// A choice with a colour dot before it (a theme, a colour of the palette, a template).
fn with_dot(label: &str, hint: &str, dot: u32, on: bool, op: Op, del: Op) -> Row {
    Row { label: label.into(), hint: hint.into(), kind: Kind::Choice(on), op, del, dot }
}

fn cat_icon(i: u8) -> &'static str {
    // Segoe MDL2 Assets: colour, monitor, taskbar, Start, globe, apps, gear
    ["\u{E790}", "\u{E7F4}", "\u{E8A1}", "\u{E700}", "\u{E774}", "\u{E71D}", "\u{E713}"][(i as usize).min(CATS - 1)]
}

fn cat_title(i: u8) -> String {
    match i {
        LOOK => t!("Внешний вид", "Appearance", ja: "外観", pt: "Aparência", zh: "外观"),
        DESK => t!("Рабочий стол", "Desktop", ja: "デスクトップ", pt: "Área de trabalho", zh: "桌面"),
        BAR => t!("Панель задач", "Taskbar", ja: "タスクバー", pt: "Barra de tarefas", zh: "任务栏"),
        START => t!("Меню «Пуск»", "Start menu", ja: "スタート メニュー", pt: "Menu Iniciar", zh: "开始菜单"),
        LANG => t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言"),
        APPS => t!("Приложения", "Apps", ja: "アプリ", pt: "Aplicativos", zh: "应用"),
        _ => t!("Дополнительно", "Advanced", ja: "詳細設定", pt: "Avançado", zh: "高级"),
    }
    .to_string()
}

fn cat_sub(i: u8) -> String {
    match i {
        LOOK => t!("Тема, цвета, шаблоны и эффекты", "Theme, colors, templates and effects", ja: "テーマ、色、テンプレート、効果", pt: "Tema, cores, modelos e efeitos", zh: "主题、颜色、模板与效果"),
        DESK => t!("Значки, обои и ярлыки рабочего стола", "Desktop icons, background and shortcuts", ja: "デスクトップのアイコン、背景、ショートカット", pt: "Ícones, plano de fundo e atalhos", zh: "桌面图标、背景与快捷方式"),
        BAR => t!("Где она, как выглядит и как себя ведёт", "Where it is, how it looks and behaves", ja: "配置、外観、動作", pt: "Onde fica, como aparece e se comporta", zh: "位置、外观与行为"),
        START => t!("Положение, стиль, поиск и ярлыки", "Position, style, search and shortcuts", ja: "位置、スタイル、検索、ショートカット", pt: "Posição, estilo, pesquisa e atalhos", zh: "位置、样式、搜索与快捷方式"),
        LANG => t!("Один выбор для всех программ", "One choice for every app", ja: "すべてのアプリで 1 つの設定", pt: "Uma escolha para todos os aplicativos", zh: "所有应用共用一个选择"),
        APPS => t!("Свои окна настроек", "Each app's own settings window", ja: "各アプリの設定ウィンドウ", pt: "A janela de ajustes de cada aplicativo", zh: "各应用自己的设置窗口"),
        _ => t!("Сброс, версия и файлы", "Reset, version and files", ja: "リセット、バージョン、ファイル", pt: "Restaurar, versão e arquivos", zh: "重置、版本与文件"),
    }
    .to_string()
}

/// Colours the colours card edits, in `style::PAL` order (`bg line hover active accent text dim`).
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
const MENU_ALPHA: [u8; 5] = [90, 75, 67, 50, 35];

/// The WinRust programs whose own settings the Apps category opens, with the name of their executable.
const APPS_LIST: [(&str, &str); 5] = [
    ("rust-explorer", "rust-explorer.exe"),
    ("rtaskmgr", "rtaskmgr-win32.exe"),
    ("rust-control", "rust-control.exe"),
    ("rs-shot", "rs-shot.exe"),
    ("rust-settings", "rust-settings.exe"),
];

fn installed(app: &str, exe: &str) -> bool {
    let mut v = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        v.push(PathBuf::from(pf).join(app).join(exe));
    }
    if let Ok(e) = std::env::current_exe() {
        v.push(e.with_file_name(exe));
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(la).join(r"plugin-launcher\bin").join(exe));
    }
    v.into_iter().any(|p| p.is_file())
}

/// A marker file the desktop of `rust-explorer` reads (`%APPDATA%\WinRust\...`).
fn marker(name: &str) -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("WinRust").join(name))
}

fn marker_set(name: &str, on: bool) {
    let Some(p) = marker(name) else { return };
    if on {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&p, "1");
    } else {
        let _ = std::fs::remove_file(&p);
    }
}

fn icons_hidden() -> bool {
    marker("desktop-icons-hidden.txt").is_some_and(|p| p.is_file())
}

fn small_icons() -> bool {
    marker("small-icons").is_some_and(|p| p.is_file())
}

thread_local! {
    static CAT: Cell<u8> = const { Cell::new(LOOK) };
    static SCROLL: Cell<i32> = const { Cell::new(0) };
    static HOT: Cell<i32> = const { Cell::new(-1) }; // hovered row (flat index) of the current category
    static HOT_CAT: Cell<i32> = const { Cell::new(-1) };
    static HOTX: Cell<bool> = const { Cell::new(false) }; // the close button
    static CONFIRM: Cell<bool> = const { Cell::new(false) }; // the reset is waiting for a second click
    static BITS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static FONT: Cell<[usize; 5]> = const { Cell::new([0; 5]) };
    static WND: Cell<HWND> = const { Cell::new(ptr::null_mut()) };
    /// The saved templates: read from disk when the window opens or one is saved, never on every repaint.
    static THEMES: RefCell<Vec<st::Theme>> = const { RefCell::new(Vec::new()) };
    /// How many monitors there were when the window opened (the list is not re-read on every repaint).
    static MONS: Cell<i32> = const { Cell::new(1) };
    /// Which of `APPS_LIST` are installed (checked when the window opens, not on every repaint).
    static HAVE: Cell<[bool; 5]> = const { Cell::new([false; 5]) };
    /// The pointer position of the last WM_MOUSEMOVE, so a repeat of the same one costs no hit test.
    static LAST: Cell<(i32, i32)> = const { Cell::new((-1, -1)) };
    /// Repaint accounting (see `count_paint`): paints in the current second, when that second started, and the slowest
    /// paint of it.
    static PAINTS: Cell<u32> = const { Cell::new(0) };
    static PAINT_AT: Cell<u64> = const { Cell::new(0) };
    static PAINT_SLOW: Cell<u64> = const { Cell::new(0) };
}

const EMPTY: [bool; 5] = [false; 5];

fn rows(cat: u8) -> Vec<Row> {
    let mut v = Vec::new();
    match cat {
        LOOK => {
            v.push(head(t!("Тема", "Theme", ja: "テーマ", pt: "Tema", zh: "主题")));
            for (i, s) in st::ALL.iter().enumerate() {
                v.push(with_dot(style_name(*s), "", st::PALS[i].accent, st::style() == *s, Op::Theme(i as u8), Op::None));
            }
            v.push(head(t!("Цвета", "Colors", ja: "色", pt: "Cores", zh: "颜色")));
            let cur = st::custom_colors().unwrap_or([u32::MAX; 7]);
            let pal = st::pal();
            let shown = [pal.bg, pal.line, pal.hover, pal.active, pal.accent, pal.text, pal.dim];
            for (i, n) in color_names().iter().enumerate() {
                let c = shown[i];
                let hex = format!("#{:02X}{:02X}{:02X}", c & 255, (c >> 8) & 255, (c >> 16) & 255);
                v.push(with_dot(n, &hex, c, cur[i] != u32::MAX, Op::Color(i as u8), Op::None));
            }
            v.push(danger(t!("Вернуть цвета темы", "Back to the theme's colors", ja: "テーマの色に戻す", pt: "Voltar às cores do tema", zh: "恢复主题颜色"), Op::ResetColors));
            v.push(head(t!("Шаблоны", "Templates", ja: "テンプレート", pt: "Modelos", zh: "模板")));
            let n = THEMES.with_borrow(|t| t.len());
            let now = st::Theme::current("");
            if n == 0 {
                v.push(info(t!("Пока пусто: настройте вид и сохраните", "None yet: set up a look and save it", ja: "まだありません: 外観を設定して保存してください", pt: "Nenhum ainda: monte uma aparência e salve", zh: "暂无：设置好外观后保存")));
            }
            for i in 0..n {
                let (name, style, dot) = THEMES.with_borrow(|t| {
                    let th = &t[i];
                    let dot = if th.colors[4] != u32::MAX { th.colors[4] } else { st::PALS[st::ALL.iter().position(|&s| s == th.style).unwrap_or(0)].accent };
                    (th.name.clone(), style_name(th.style), dot)
                });
                let same = THEMES.with_borrow(|t| {
                    let th = &t[i];
                    (th.style, th.colors, th.opacity) == (now.style, now.colors, now.opacity)
                });
                v.push(with_dot(&name, style, dot, same, Op::ApplyTheme(i as u8), Op::DeleteTheme(i as u8)));
            }
            v.push(action(t!("Сохранить текущий вид как шаблон", "Save the current look as a template", ja: "現在の外観をテンプレートとして保存", pt: "Salvar a aparência atual como modelo", zh: "将当前外观保存为模板"), "", Op::SaveTheme));
            v.push(head(t!("Эффекты", "Effects", ja: "効果", pt: "Efeitos", zh: "效果")));
            v.push(check(t!("Размытие за меню", "Blur behind the menu", ja: "メニューの後ろをぼかす", pt: "Desfoque atrás do menu", zh: "菜单后模糊"), lk::menu_blur(), Op::MenuBlur));
            for a in MENU_ALPHA {
                v.push(choice(&format!("{} %", a), lk::menu_alpha() == a, Op::MenuAlpha(a)));
            }
            for r in [12, 18, 26] {
                v.push(choice(&format!("{r} px"), lk::menu_radius() == r, Op::MenuRadius(r)));
            }
            v.push(check(t!("Мороз в окнах WinRust", "Frost in WinRust windows", ja: "WinRust のウィンドウをぼかす", pt: "Desfoque nas janelas do WinRust", zh: "WinRust 窗口模糊"), lk::win_blur(), Op::WinBlur));
        }
        DESK => {
            v.push(head(t!("Значки", "Icons", ja: "アイコン", pt: "Ícones", zh: "图标")));
            v.push(check(t!("Показывать значки рабочего стола", "Show desktop icons", ja: "デスクトップのアイコンを表示", pt: "Mostrar ícones da área de trabalho", zh: "显示桌面图标"), !icons_hidden(), Op::DeskIcons));
            v.push(check(t!("Мелкие значки", "Small icons", ja: "小さいアイコン", pt: "Ícones pequenos", zh: "小图标"), small_icons(), Op::SmallIcons));
            v.push(info(t!("Размер значков применится после перезапуска рабочего стола", "The icon size applies when the desktop starts again", ja: "アイコンのサイズはデスクトップの再起動後に反映されます", pt: "O tamanho dos ícones vale quando a área de trabalho reiniciar", zh: "图标大小在桌面重新启动后生效")));
            v.push(head(t!("Фон", "Background", ja: "背景", pt: "Plano de fundo", zh: "背景")));
            v.push(action(t!("Выбрать изображение…", "Pick a picture…", ja: "画像を選ぶ…", pt: "Escolher uma imagem…", zh: "选择图片…"), "›", Op::Wallpaper));
            v.push(action(t!("Обновить обои", "Reload the background", ja: "背景を再読み込み", pt: "Recarregar o plano de fundo", zh: "重新加载背景"), "", Op::WallpaperReload));
            v.push(info(t!("rs-shell преобразует картинку при входе: для новой нужно выйти из системы", "rs-shell converts the picture at sign-in: a new one needs a sign-out", ja: "rs-shell はサインイン時に画像を変換します。新しい画像はサインアウトが必要です", pt: "O rs-shell converte a imagem ao entrar: uma nova precisa sair da conta", zh: "rs-shell 在登录时转换图片：新图片需要注销")));
            v.push(head(t!("Ярлыки", "Shortcuts", ja: "ショートカット", pt: "Atalhos", zh: "快捷方式")));
            v.push(action(t!("Корзина", "Recycle Bin", ja: "ごみ箱", pt: "Lixeira", zh: "回收站"), "›", Op::Recycle));
            v.push(action(t!("Папка рабочего стола", "Desktop folder", ja: "デスクトップ フォルダー", pt: "Pasta da área de trabalho", zh: "桌面文件夹"), "›", Op::DeskFolder));
        }
        BAR => {
            v.push(head(t!("Положение", "Position", ja: "位置", pt: "Posição", zh: "位置")));
            for (i, (s, name)) in [(st::Side::Bottom, t!("Снизу", "Bottom", ja: "下", pt: "Embaixo", zh: "底部")), (st::Side::Left, t!("Слева", "Left", ja: "左", pt: "Esquerda", zh: "左侧")), (st::Side::Right, t!("Справа", "Right", ja: "右", pt: "Direita", zh: "右侧"))].into_iter().enumerate() {
                v.push(choice(name, st::side() == s, Op::BarSide(i as u8)));
            }
            v.push(head(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小")));
            for (i, name) in [(0u8, t!("Мелкая", "Small", ja: "小", pt: "Pequena", zh: "小")), (1, t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常")), (2, t!("Крупный", "Large", ja: "大", pt: "Grande", zh: "大"))] {
                v.push(choice(name, st::bar_size() == i, Op::BarSize(i)));
            }
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
            for (r, name) in [(4, t!("Слабое", "Soft", ja: "弱い", pt: "Suave", zh: "弱")), (8, t!("Среднее", "Medium", ja: "中", pt: "Médio", zh: "中")), (14, t!("Сильное", "Strong", ja: "強い", pt: "Forte", zh: "强"))] {
                v.push(choice(name, lk::bar_radius() == r, Op::BarRadius(r)));
            }
            v.push(head(t!("Непрозрачность панели", "Bar opacity", ja: "バーの不透明度", pt: "Opacidade da barra", zh: "任务栏不透明度")));
            for a in PCT {
                let name = if a == 100 { t!("100 %, без прозрачности", "100 %, solid", ja: "100 %、不透明", pt: "100 %, sólida", zh: "100 %，不透明").to_string() } else { format!("{a} %") };
                v.push(choice(&name, st::bar_opacity() == a, Op::Opacity(a)));
            }
            v.push(head(t!("Поведение", "Behavior", ja: "動作", pt: "Comportamento", zh: "行为")));
            v.push(check(t!("Скрывать автоматически", "Hide automatically", ja: "自動的に隠す", pt: "Ocultar automaticamente", zh: "自动隐藏"), st::autohide(), Op::Autohide));
            v.push(check(t!("Уступать место окнам", "Dodge windows", ja: "ウィンドウを避ける", pt: "Desviar de janelas", zh: "为窗口让路"), st::dodge(), Op::Dodge));
            v.push(info(t!("Уступать место окнам: панель уходит, когда окно спереди доходит до неё, и возвращается у края экрана. Место не резервируется.", "Dodge windows: the bar slides away while a window reaches it and comes back at the screen edge. It reserves no space.", ja: "ウィンドウを避ける: 手前のウィンドウがバーに達すると隠れ、画面の端で戻ります。画面領域は予約しません。", pt: "Desviar de janelas: a barra desaparece quando uma janela chega a ela e volta na borda da tela. Não reserva espaço.", zh: "为窗口让路：当前窗口接近任务栏时它会让开，鼠标移到屏幕边缘时回来。不保留屏幕空间。")));
            v.push(check(t!("Кнопки слева, как в Windows 10", "Buttons on the left, like Windows 10", ja: "ボタンを左に配置 (Windows 10 風)", pt: "Botões à esquerda, como no Windows 10", zh: "按钮靠左，如同 Windows 10"), st::align_left(), Op::AlignLeft));
            v.push(check(t!("Alt+Tab: только этот монитор", "Alt+Tab: this monitor only", ja: "Alt+Tab: このモニターのみ", pt: "Alt+Tab: somente este monitor", zh: "Alt+Tab：仅限此显示器"), st::alttab_per_monitor(), Op::AltTab));
            v.push(check(t!("Кнопка поиска", "Search button", ja: "検索ボタン", pt: "Botão de pesquisa", zh: "搜索按钮"), lk::bar_search(), Op::SearchBtn));
            v.push(check(t!("Кнопка задач", "Task View button", ja: "タスク ビュー ボタン", pt: "Botão de exibição de tarefas", zh: "任务视图按钮"), lk::bar_taskview(), Op::TaskView));
            v.push(head(t!("Уведомления", "Notifications", ja: "通知", pt: "Notificações", zh: "通知")));
            v.push(check(t!("Показывать уведомления", "Show notifications", ja: "通知を表示", pt: "Mostrar notificações", zh: "显示通知"), lk::notify(), Op::Notify));
            v.push(action(t!("Время на экране", "On screen for", ja: "表示時間", pt: "Tempo na tela", zh: "显示时长"), &format!("{} s ›", lk::notify_timeout()), Op::NotifyTimeout));
        }
        START => {
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
            v.push(head(t!("Поиск", "Search", ja: "検索", pt: "Pesquisa", zh: "搜索")));
            v.push(check(t!("Сортировать результаты по частоте", "Rank results by use", ja: "使用頻度順に並べる", pt: "Classificar os resultados pelo uso", zh: "按使用频率排序"), lk::start_ranked(), Op::StartRanked));
            v.push(action(t!("Очистить историю приложений", "Clear the app history", ja: "アプリの履歴を消去", pt: "Limpar o histórico de aplicativos", zh: "清除应用历史记录"), "", Op::ClearHistory));
            v.push(head(t!("Приложения", "Apps", ja: "アプリ", pt: "Aplicativos", zh: "应用")));
            v.push(action(t!("Открыть папку закреплённых программ", "Open the pinned apps folder", ja: "ピン留めしたアプリのフォルダーを開く", pt: "Abrir a pasta de aplicativos fixados", zh: "打开固定应用文件夹"), "›", Op::PinsFolder));
        }
        LANG => {
            v.push(head(t!("Язык всех программ WinRust", "The language of every WinRust program", ja: "すべての WinRust プログラムの言語", pt: "O idioma de todos os programas da WinRust", zh: "所有 WinRust 程序的语言")));
            for (i, l) in winrust_common::Lang::ALL.iter().enumerate() {
                v.push(choice(l.name(), winrust_common::lang::get() == *l, Op::Lang(i as u8)));
            }
            v.push(info(t!("Изменение сразу применяется ко всем программам.", "The change applies to every app at once.", ja: "変更はすべてのアプリにすぐ適用されます。", pt: "A mudança vale para todos os aplicativos de uma vez.", zh: "更改会立即应用到所有应用。")));
        }
        APPS => {
            let names = [
                t!("Проводник", "File manager", ja: "ファイル マネージャー", pt: "Gerenciador de arquivos", zh: "文件管理器"),
                t!("Диспетчер задач", "Task Manager", ja: "タスク マネージャー", pt: "Gerenciador de Tarefas", zh: "任务管理器"),
                t!("Панель управления", "Control Panel", ja: "コントロール パネル", pt: "Painel de Controle", zh: "控制面板"),
                t!("Снимки и запись", "Screenshots and recording", ja: "スクリーンショットと録画", pt: "Capturas e gravação", zh: "截图与录制"),
                t!("Параметры системы (экран, звук…)", "System settings (display, sound…)", ja: "システム設定 (画面、サウンド…)", pt: "Configurações do sistema (tela, som…)", zh: "系统设置（显示、声音…）"),
            ];
            let have = HAVE.with(|h| h.get());
            v.push(head(t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开")));
            for i in 0..APPS_LIST.len() {
                if have[i] {
                    let op = if i == 4 { Op::SystemPage(0) } else { Op::OpenApp(i as u8) };
                    v.push(action(names[i], APPS_LIST[i].1, op));
                } else {
                    v.push(Row { hint: t!("Не установлено", "Not installed", ja: "インストールされていません", pt: "Não instalado", zh: "未安装").into(), ..info(names[i]) });
                }
            }
            v.push(head(t!("Свои настройки", "Own settings", ja: "独自の設定", pt: "Ajustes próprios", zh: "自己的设置")));
            v.push(info(t!("Каждая программа хранит настройки в своём окне.", "Every app keeps its options in its own window.", ja: "各アプリの設定はそのアプリのウィンドウにあります。", pt: "Cada aplicativo guarda suas opções na própria janela.", zh: "每个应用都在自己的窗口中保存设置。")));
            v.push(info(t!("У rs-shot нет файла настроек: он использует параметры командной строки.", "rs-shot has no settings file: it uses command-line options.", ja: "rs-shot には設定ファイルがなく、コマンド ライン オプションを使います。", pt: "O rs-shot não tem arquivo de ajustes: usa opções de linha de comando.", zh: "rs-shot 没有设置文件：它使用命令行选项。")));
        }
        ADV => {
            v.push(head(t!("Сброс", "Reset", ja: "リセット", pt: "Restaurar", zh: "重置")));
            if CONFIRM.with(|c| c.get()) {
                v.push(danger(t!("Да, сбросить всё", "Yes, reset everything", ja: "はい、すべてリセットします", pt: "Sim, restaurar tudo", zh: "是，全部重置"), Op::ResetAll));
                v.push(action(t!("Отмена", "Cancel", ja: "キャンセル", pt: "Cancelar", zh: "取消"), "", Op::ResetCancel));
            } else {
                v.push(danger(t!("Сбросить всё по умолчанию…", "Reset everything to defaults…", ja: "すべてを既定に戻す…", pt: "Restaurar tudo para o padrão…", zh: "全部恢复默认…"), Op::ResetAsk));
                v.push(info(t!("Тема, цвета, панель, «Пуск», язык и эффекты. Шаблоны сохранятся.", "Theme, colors, bar, Start, language and effects. Templates stay.", ja: "テーマ、色、バー、スタート、言語、効果。テンプレートは残ります。", pt: "Tema, cores, barra, Iniciar, idioma e efeitos. Os modelos permanecem.", zh: "主题、颜色、任务栏、开始、语言与效果。模板保留。")));
            }
            v.push(head(t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言")));
            v.push(action(t!("Язык программы", "Program language", ja: "プログラムの言語", pt: "Idioma do programa", zh: "程序语言"), &format!("{} ›", winrust_common::lang::get().name()), Op::Cat(LANG)));
            v.push(head(t!("О программе", "About", ja: "バージョン情報", pt: "Sobre", zh: "关于")));
            v.push(action(t!("Версия", "Version", ja: "バージョン", pt: "Versão", zh: "版本"), env!("CARGO_PKG_VERSION"), Op::None));
            v.push(info(r"%ProgramData%\WinRust\look.txt"));
            v.push(info(r"%ProgramData%\WinRust\style.txt"));
            v.push(info(r"%ProgramData%\WinRust\lang.txt"));
            v.push(info(r"%APPDATA%\WinRust\desktop-icons-hidden.txt"));
            v.push(action(t!("Закрыть", "Close", ja: "閉じる", pt: "Fechar", zh: "关闭"), "Esc", Op::Close));
        }
        _ => {}
    }
    v
}

/// One card: its title, the rect it fills and its rows (already scrolled).
struct Card {
    title: String,
    rect: RECT,
    rows: Vec<(RECT, Row)>,
}

/// Cards and row geometry of a category. The painter and the hit test both use this, so they cannot disagree.
fn layout(w: i32, cat: u8, scroll: i32) -> Vec<Card> {
    let left = sc(RAIL + PAD);
    let right = w - sc(PAD);
    let mut y = sc(TOP) - scroll;
    let mut out: Vec<Card> = Vec::new();
    for r in rows(cat) {
        if r.kind == Kind::Head {
            out.push(Card { title: r.label, rect: RECT { left, top: y, right, bottom: y + sc(HEAD_H) }, rows: Vec::new() });
            y += sc(HEAD_H);
            continue;
        }
        let h = if r.kind == Kind::Info { sc(ROW_H) - sc(8) } else { sc(ROW_H) };
        if out.is_empty() {
            out.push(Card { title: String::new(), rect: RECT { left, top: y, right, bottom: y }, rows: Vec::new() });
        }
        let rect = RECT { left: left + sc(10), top: y, right: right - sc(10), bottom: y + h };
        if let Some(c) = out.last_mut() {
            c.rows.push((rect, r));
            c.rect.bottom = y + h + sc(8);
        }
        y += h;
    }
    out
}

fn content_bottom(h: i32) -> i32 {
    h - sc(12)
}

fn max_scroll(w: i32, h: i32, cat: u8) -> i32 {
    let last = layout(w, cat, 0).last().map_or(0, |c| c.rect.bottom);
    (last + sc(10) - content_bottom(h)).max(0)
}

/// Counts a repaint and says so when the window repaints too often or a paint takes too long. At most two lines per
/// second (never one per paint: the log lives on disk), so a repaint loop shows up in `topbar.log` instead of freezing
/// the bar silently.
fn count_paint(started: u64) {
    let now = unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() };
    let dt = now.saturating_sub(started);
    PAINTS.with(|c| c.set(c.get() + 1));
    let worst = PAINT_SLOW.with(|c| c.get().max(dt));
    PAINT_SLOW.with(|c| c.set(worst));
    let at = PAINT_AT.with(|c| c.get());
    if now.saturating_sub(at) >= 1000 {
        let span = now.saturating_sub(at).max(1);
        let n = PAINTS.with(|c| c.get());
        let rate = n as u64 * 1000 / span;
        let worst = PAINT_SLOW.with(|c| c.replace(0));
        if rate > 30 {
            eprintln!("rust-settings: {n} repaints in {span} ms ({rate}/s)");
        }
        if worst > 50 {
            eprintln!("rust-settings: a paint took {worst} ms");
        }
        PAINTS.with(|c| c.set(0));
        PAINT_AT.with(|c| c.set(now));
    }
}

fn tab_rect(i: usize) -> RECT {
    let top = sc(104) + i as i32 * sc(48);
    RECT { left: sc(12), top, right: sc(RAIL - 12), bottom: top + sc(40) }
}

fn close_rect(w: i32) -> RECT {
    RECT { left: w - sc(50), top: sc(22), right: w - sc(18), bottom: sc(54) }
}

unsafe fn font(px: i32, weight: i32, face: &str) -> HFONT {
    let f: Vec<u16> = face.encode_utf16().chain([0]).collect();
    CreateFontW(-sc(px), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, f.as_ptr())
}

fn ensure_fonts() {
    if FONT.with(|f| f.get()[0]) != 0 {
        return;
    }
    unsafe {
        let ui = winrust_common::style::ui_face();
        let icon = winrust_common::style::icon_face();
        FONT.with(|f| f.set([font(13, 400, ui) as usize, font(21, 600, ui) as usize, font(11, 400, ui) as usize, font(13, 600, ui) as usize, font(15, 400, icon) as usize]));
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

unsafe fn paint(h: HWND) {
    let started = windows_sys::Win32::System::SystemInformation::GetTickCount64();
    let mut ps: PAINTSTRUCT = zeroed();
    let wdc = BeginPaint(h, &mut ps);
    let mut cr: RECT = zeroed();
    GetClientRect(h, &mut cr);
    if cr.right <= 0 || cr.bottom <= 0 {
        EndPaint(h, &ps);
        return;
    }
    ensure_fonts();
    let dc = CreateCompatibleDC(wdc);
    let bmp = CreateCompatibleBitmap(wdc, cr.right, cr.bottom);
    let obmp = SelectObject(dc, bmp);
    let p = st::pal();
    // One layout for this paint. The scroll is only re-laid out when it had to be clamped, so the rows are built once
    // (they read the look files) instead of three times as they were.
    let cat = CAT.with(|c| c.get());
    let mut scroll = SCROLL.with(|s| s.get());
    let mut cards = layout(cr.right, cat, scroll);
    let last = cards.last().map_or(0, |c| c.rect.bottom);
    let max = (last + sc(10) - content_bottom(cr.bottom)).max(0);
    if scroll > max {
        scroll = max;
        SCROLL.with(|s| s.set(max));
        cards = layout(cr.right, cat, scroll);
    }
    let inner = RECT { left: 1, top: 1, right: cr.right - 1, bottom: cr.bottom - 1 };
    let drew = BITS.with(|b| {
        let b = b.borrow();
        if b.is_empty() {
            return false;
        }
        frost::paint_capture(dc, &b, inner, p.bg, (255 * lk::menu_alpha().max(60) as u32 / 100) as u8);
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
    let (f, f_title, f_small, f_tab, f_icon) = FONT.with(|x| {
        let a = x.get();
        (a[0] as HFONT, a[1] as HFONT, a[2] as HFONT, a[3] as HFONT, a[4] as HFONT)
    });
    SetBkMode(dc, TRANSPARENT as i32);
    // rail header
    SelectObject(dc, f_small);
    SetTextColor(dc, p.dim);
    let mut rh = RECT { left: sc(24), top: sc(20), right: sc(RAIL - 12), bottom: sc(42) };
    DrawTextW(dc, wide("WinRust").as_ptr(), -1, &mut rh, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, f_title);
    SetTextColor(dc, p.text);
    let mut rh = RECT { left: sc(24), top: sc(42), right: sc(RAIL - 12), bottom: sc(82) };
    DrawTextW(dc, wide(t!("Настройки WinRust", "WinRust Settings", ja: "WinRust の設定", pt: "Configurações do WinRust", zh: "WinRust 设置")).as_ptr(), -1, &mut rh, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    // rail categories
    for i in 0..CATS {
        let r = tab_rect(i);
        let on = i as u8 == cat;
        let hot = HOT_CAT.with(|x| x.get()) == i as i32;
        if on || hot {
            let br = CreateSolidBrush(if on { st::mix(p.bg, p.accent, 0.20) } else { p.hover });
            let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
            RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, sc(10), sc(10));
            SelectObject(dc, o1);
            SelectObject(dc, o2);
            DeleteObject(br);
        }
        SelectObject(dc, f_icon);
        SetTextColor(dc, if on { p.accent } else { p.dim });
        let mut ir = RECT { left: r.left + sc(14), top: r.top, right: r.left + sc(44), bottom: r.bottom };
        DrawTextW(dc, wide(cat_icon(i as u8)).as_ptr(), -1, &mut ir, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, f_tab);
        SetTextColor(dc, if on { p.text } else { p.dim });
        let mut tr = RECT { left: r.left + sc(46), top: r.top, right: r.right - sc(8), bottom: r.bottom };
        DrawTextW(dc, wide(&cat_title(i as u8)).as_ptr(), -1, &mut tr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    }
    // page header
    SelectObject(dc, f_title);
    SetTextColor(dc, p.text);
    let mut hr = RECT { left: sc(RAIL + PAD), top: sc(24), right: cr.right - sc(64), bottom: sc(58) };
    DrawTextW(dc, wide(&cat_title(cat)).as_ptr(), -1, &mut hr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    SelectObject(dc, f_small);
    SetTextColor(dc, p.dim);
    let mut sr = RECT { left: sc(RAIL + PAD), top: sc(58), right: cr.right - sc(64), bottom: sc(80) };
    DrawTextW(dc, wide(&cat_sub(cat)).as_ptr(), -1, &mut sr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    // close
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
    // the cards, clipped to the content area
    let saved = SaveDC(dc);
    IntersectClipRect(dc, sc(RAIL + 1), sc(TOP) - sc(6), cr.right - sc(1), content_bottom(cr.bottom));
    let hot = HOT.with(|x| x.get());
    let mut idx = 0;
    for card in cards {
        if card.rect.bottom < sc(TOP) - sc(6) || card.rect.top > content_bottom(cr.bottom) {
            idx += card.rows.len() as i32;
            continue;
        }
        let bg = CreateSolidBrush(st::mix(p.bg, p.text, 0.045));
        let (o1, o2) = (SelectObject(dc, bg), SelectObject(dc, GetStockObject(NULL_PEN)));
        RoundRect(dc, card.rect.left, card.rect.top, card.rect.right + 1, card.rect.bottom + 1, sc(10), sc(10));
        SelectObject(dc, o1);
        SelectObject(dc, o2);
        DeleteObject(bg);
        SelectObject(dc, f_small);
        SetTextColor(dc, p.dim);
        let mut ch = RECT { left: card.rect.left + sc(16), top: card.rect.top + sc(8), right: card.rect.right - sc(16), bottom: card.rect.top + sc(30) };
        DrawTextW(dc, wide(&card.title).as_ptr(), -1, &mut ch, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        for (ri, (r, row)) in card.rows.iter().enumerate() {
            if ri > 0 {
                let line = CreateSolidBrush(st::mix(p.bg, p.text, 0.06));
                FillRect(dc, &RECT { left: r.left, top: r.top, right: r.right, bottom: r.top + 1 }, line);
                DeleteObject(line);
            }
            draw_row(dc, r, row, hot == idx, p, &[f, f_small]);
            idx += 1;
        }
    }
    RestoreDC(dc, saved);
    // a slim scroll indicator when the category is longer than the window
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
    count_paint(started);
}

unsafe fn draw_row(dc: HDC, r: &RECT, row: &Row, hovered: bool, p: &st::Pal, fonts: &[HFONT; 2]) {
    let (f, f_small) = (fonts[0], fonts[1]);
    match row.kind {
        Kind::Head => {}
        Kind::Info => {
            SelectObject(dc, f_small);
            SetTextColor(dc, p.dim);
            let mut rr = RECT { left: r.left + sc(6), top: r.top, right: r.right, bottom: r.bottom };
            DrawTextW(dc, wide(&row.label).as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
        }
        _ => {
            if hovered {
                let br = CreateSolidBrush(p.hover);
                let (o1, o2) = (SelectObject(dc, br), SelectObject(dc, GetStockObject(NULL_PEN)));
                RoundRect(dc, r.left - sc(6), r.top + sc(2), r.right + sc(1), r.bottom - sc(2), sc(6), sc(6));
                SelectObject(dc, o1);
                SelectObject(dc, o2);
                DeleteObject(br);
            }
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
            // the right end: the hint, then the ✕ of a deletable row
            let del_w = if row.del == Op::None { 0 } else { sc(30) };
            SelectObject(dc, f);
            SetTextColor(dc, if row.kind == Kind::Danger { st::rgb(232, 92, 92) } else { p.text });
            let mut rr = RECT { left, top: r.top, right: r.right - sc(60) - del_w, bottom: r.bottom };
            DrawTextW(dc, wide(&row.label).as_ptr(), -1, &mut rr, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
            if !row.hint.is_empty() {
                SelectObject(dc, f_small);
                SetTextColor(dc, p.dim);
                let mut hr = RECT { left: r.right - sc(150) - del_w, top: r.top, right: r.right - sc(8) - del_w, bottom: r.bottom };
                DrawTextW(dc, wide(&row.hint).as_ptr(), -1, &mut hr, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
            if del_w > 0 {
                SelectObject(dc, f_small);
                SetTextColor(dc, st::rgb(232, 92, 92));
                let mut dr = RECT { left: r.right - sc(26), top: r.top, right: r.right - sc(6), bottom: r.bottom };
                DrawTextW(dc, wide("\u{2715}").as_ptr(), -1, &mut dr, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
            }
        }
    }
}

unsafe fn hit_cat(x: i32, y: i32) -> Option<usize> {
    if x >= sc(RAIL) {
        return None;
    }
    (0..CATS).find(|&i| {
        let r = tab_rect(i);
        y >= r.top && y < r.bottom
    })
}

/// The row under the pointer: its flat index, its rect and the row itself.
unsafe fn hit_row(h: HWND, x: i32, y: i32) -> Option<(i32, RECT, Row)> {
    let mut cr: RECT = zeroed();
    GetClientRect(h, &mut cr);
    if y < sc(TOP) - sc(6) || y > content_bottom(cr.bottom) || x < sc(RAIL + PAD) - sc(6) {
        return None;
    }
    let (cat, scroll) = (CAT.with(|c| c.get()), SCROLL.with(|s| s.get()));
    let mut idx = 0;
    for card in layout(cr.right, cat, scroll) {
        for (r, row) in card.rows {
            if x >= r.left - sc(6) && x < r.right && y >= r.top && y < r.bottom {
                return Some((idx, r, row));
            }
            idx += 1;
        }
    }
    None
}

/// The window of the desktop's icon list (`rust-explorer` runs a desktop whose class is "Progman", like Explorer's).
fn desk_list() -> HWND {
    unsafe {
        let prog = FindWindowW(wide("Progman").as_ptr(), ptr::null());
        if prog.is_null() {
            return ptr::null_mut();
        }
        let dv = FindWindowExW(prog, ptr::null_mut(), wide("SHELLDLL_DefView").as_ptr(), ptr::null());
        if dv.is_null() {
            return ptr::null_mut();
        }
        FindWindowExW(dv, ptr::null_mut(), wide("SysListView32").as_ptr(), ptr::null())
    }
}

fn set_icons_hidden(hide: bool) {
    marker_set("desktop-icons-hidden.txt", hide);
    // and at once on the running desktop: the list is a plain child window, hiding it is what its own menu does
    unsafe {
        let lv = desk_list();
        if !lv.is_null() {
            ShowWindow(lv, if hide { SW_HIDE } else { SW_SHOW });
        }
    }
}

fn wallpaper_reload() {
    unsafe {
        // hand Windows the picture it already knows: it re-reads it and tells every listener
        let mut buf = [0u16; 600];
        if SystemParametersInfoW(SPI_GETDESKWALLPAPER, buf.len() as u32, buf.as_mut_ptr() as *mut _, 0) != 0 && buf[0] != 0 {
            SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, buf.as_mut_ptr() as *mut _, SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
        }
        let prog = FindWindowW(wide("Progman").as_ptr(), ptr::null());
        if !prog.is_null() {
            InvalidateRect(prog, ptr::null(), 1);
        }
    }
}


fn open_app(i: u8) {
    if i as usize == 3 {
        launch_app("rs-shot", "rs-shot.exe", &[]);
        return;
    }
    let Some(&(app, exe)) = APPS_LIST.get(i as usize) else { return };
    let mut paths = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        paths.push(PathBuf::from(pf).join(app).join(exe));
    }
    if let Ok(e) = std::env::current_exe() {
        paths.push(e.with_file_name(exe));
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        paths.push(PathBuf::from(la).join(r"plugin-launcher\bin").join(exe));
    }
    if let Some(p) = paths.into_iter().find(|p| p.is_file()) {
        std::thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new(p).creation_flags(0x0800_0000).spawn();
        });
    }
}

unsafe fn apply(op: Op, h: HWND) -> bool {
    match op {
        Op::Cat(c) => {
            CAT.with(|x| x.set(c.min(CATS as u8 - 1)));
            SCROLL.with(|x| x.set(0));
            CONFIRM.with(|x| x.set(false));
        }
        Op::Theme(i) => {
            if let Some(s) = st::ALL.get(i as usize) {
                st::set_style(*s);
            }
            changed();
        }
        Op::Color(i) => {
            use windows_sys::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
            let pal = st::pal();
            let now = [pal.bg, pal.line, pal.hover, pal.active, pal.accent, pal.text, pal.dim];
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
                changed();
            }
        }
        Op::Opacity(v) => {
            st::set_bar_opacity(v);
            changed(); // every bar: place() applies the opacity to each of them
        }
        Op::ResetColors => {
            st::reset_custom();
            changed();
        }
        Op::ApplyTheme(i) => {
            let th = THEMES.with_borrow(|t| t.get(i as usize).cloned());
            if let Some(th) = th {
                th.apply();
                changed();
            }
        }
        Op::DeleteTheme(i) => {
            let mut list = THEMES.with_borrow(|t| t.clone());
            if (i as usize) < list.len() {
                list.remove(i as usize);
                st::save_themes(&list);
            }
            THEMES.with(|t| *t.borrow_mut() = list);
        }
        Op::SaveTheme => {
            let name = |n: usize| tf!("Шаблон {}", "Template {}", ja: "テンプレート {}", pt: "Modelo {}", zh: "模板 {}", n);
            let mut list = THEMES.with_borrow(|t| t.clone());
            let n = (1..).find(|&n| !list.iter().any(|t| t.name == name(n))).unwrap_or(1);
            list.push(st::Theme::current(&name(n)));
            st::save_themes(&list);
            THEMES.with(|t| *t.borrow_mut() = list);
        }
        Op::MenuBlur => lk::menu_blur_set(!lk::menu_blur()),
        Op::MenuAlpha(a) => lk::menu_alpha_set(a),
        Op::MenuRadius(r) => lk::menu_radius_set(r),
        Op::WinBlur => {
            lk::win_blur_set(!lk::win_blur());
            changed();
        }
        Op::DeskIcons => set_icons_hidden(!icons_hidden()),
        Op::SmallIcons => marker_set("small-icons", !small_icons()),
        Op::Wallpaper => open_target("ms-settings:personalization-background"),
        Op::WallpaperReload => wallpaper_reload(),
        Op::Recycle => open_target("shell:RecycleBinFolder"),
        Op::DeskFolder => open_target(&format!(r"{}\Desktop", std::env::var("USERPROFILE").unwrap_or_default())),
        Op::BarSide(i) => {
            st::set_side([st::Side::Bottom, st::Side::Left, st::Side::Right][(i as usize).min(2)]);
            changed();
        }
        Op::BarSize(i) => {
            st::set_bar_size(i.min(2));
            changed();
        }
        Op::BarMonitor(i) => {
            lk::bar_monitor_set(i as i32);
            changed(); // close the bars that are not wanted, create the missing ones, place them all
            changed();
            MONS.with(|m| m.set(winrust_common::monitors::all().len().max(1) as i32));
        }
        Op::BarBlur => {
            lk::bar_blur_set(!lk::bar_blur());
            changed(); // every bar: place() applies the opacity that goes with the blur
        }
        Op::BarRadius(r) => lk::bar_radius_set(r),
        Op::Autohide => {
            st::set_autohide(!st::autohide());
            changed();
        }
        Op::Dodge => {
            st::set_dodge(!st::dodge());
            changed();
        }
        Op::AlignLeft => {
            st::set_align_left(!st::align_left());
            changed();
        }
        Op::AltTab => st::set_alttab_per_monitor(!st::alttab_per_monitor()),
        Op::SearchBtn => {
            lk::bar_search_set(!lk::bar_search());
            changed();
        }
        Op::TaskView => {
            lk::bar_taskview_set(!lk::bar_taskview());
            changed();
        }
        Op::Notify => {
            lk::notify_set(!lk::notify());
            changed();
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
        Op::StartPos(left) => lk::start_left_set(left),
        Op::StartStyle(i) => lk::start_style_set(i as i32),
        Op::StartScale(i) => lk::start_scale_set(i as i32),
        Op::StartRanked => lk::start_ranked_set(!lk::start_ranked()),
        Op::ClearHistory => clear_history(),
        Op::PinsFolder => open_target("shell:Programs"),
        Op::Lang(i) => {
            if let Some(l) = winrust_common::Lang::ALL.get(i as usize) {
                if !winrust_common::lang::set(*l) {
                    eprintln!("rust-settings: lang.txt could not be written (run the installer as administrator to apply it everywhere)");
                }
                changed();
            }
        }
        Op::OpenApp(i) => open_app(i),
        Op::SystemPage(p) => enter_system(p as usize),
        Op::ResetAsk => CONFIRM.with(|c| c.set(true)),
        Op::ResetCancel => CONFIRM.with(|c| c.set(false)),
        Op::ResetAll => {
            st::reset_all();
            lk::reset_all();
            changed();
            changed();
            changed();
            clear_history();
            CONFIRM.with(|c| c.set(false));
        }
        Op::Close => return false,
        Op::None => {}
    }
    changed();
    true
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    // a second launch asked for another page
    if msg == page_msg() {
        MODE.with(|m| m.set(HUB));
        CAT.with(|c| c.set((wp as u8).min(CATS as u8 - 1)));
        SCROLL.with(|s| s.set(0));
        InvalidateRect(h, ptr::null(), 0);
        return 0;
    }
    // the look changed in another WinRust window: repaint in it
    if msg == changed_msg() {
        InvalidateRect(h, ptr::null(), 0);
        return 0;
    }
    if msg == WM_GETMINMAXINFO {
        let mmi = lp as *mut MINMAXINFO;
        if !mmi.is_null() {
            (*mmi).ptMinTrackSize = POINT { x: sc(MIN_W), y: sc(MIN_H) };
        }
        return 0;
    }
    if msg == WM_NCHITTEST {
        // the header drags the window (the âœ• and the rail keep their clicks); no frame anywhere else
        let mut pt = POINT { x: (lp & 0xffff) as i16 as i32, y: ((lp >> 16) & 0xffff) as i16 as i32 };
        if MODE.with(|m| m.get()) == HUB {
            ScreenToClient(h, &mut pt);
            let mut cr: RECT = zeroed();
            GetClientRect(h, &mut cr);
            let xr = close_rect(cr.right);
            if pt.y < sc(66) && !(pt.x >= xr.left && pt.x < xr.right && pt.y >= xr.top && pt.y < xr.bottom) {
                return HTCAPTION as isize;
            }
        }
        return DefWindowProcW(h, msg, wp, lp);
    }
    // the system pages (display, sound, network...) keep their own procedure and painter: they run in this window
    if MODE.with(|m| m.get()) == SYSTEM {
        if msg == WM_KEYDOWN && wp as u16 == VK_ESCAPE {
            back_to_hub();
            return 0;
        }
        if msg == WM_PAINT {
            crate::paint(h);
            return 0;
        }
        if msg == WM_ERASEBKGND {
            return 1;
        }
        if msg == WM_CLOSE {
            DestroyWindow(h);
            return 0;
        }
        if msg == WM_DESTROY {
            crate::proc(h, msg, wp, lp); // their fonts, timers and PostQuitMessage
            for f in FONT.with(|x| x.replace([0; 5])) {
                if f != 0 {
                    DeleteObject(f as HFONT);
                }
            }
            WND.with(|w| w.set(ptr::null_mut()));
            return 0;
        }
        return crate::proc(h, msg, wp, lp);
    }
    let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
    match msg {
        // Creation: answer these ourselves. A procedure that answers WM_NCCREATE with FALSE makes CreateWindowExW return
        // NULL *without setting the last error* (the "failed, last error 0" this window hit, with WM_NCDESTROY right
        // after WM_NCCREATE and no WM_CREATE in between), and leaving it to the default procedure is what did that.
        WM_NCCREATE => 1,
        WM_CREATE => 0,
        WM_NCCALCSIZE => 0, // a borderless popup: the client area is the whole window
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(h);
            0
        }
        WM_MOUSEMOVE => {
            // the same position again (the system repeats WM_MOUSEMOVE): no hit test, no repaint
            if LAST.with(|c| c.replace((x, y))) != (x, y) {
                let cat = hit_cat(x, y);
                let row = hit_row(h, x, y).map_or(-1, |(i, _, _)| i);
                let (old_cat, old_row) = (HOT_CAT.with(|c| c.replace(cat.map_or(-1, |i| i as i32))), HOT.with(|c| c.replace(row)));
                let mut cr: RECT = zeroed();
                GetClientRect(h, &mut cr);
                let xr = close_rect(cr.right);
                let on_x = x >= xr.left && x < xr.right && y >= xr.top && y < xr.bottom;
                let was_x = HOTX.with(|c| c.replace(on_x));
                if old_cat != cat.map_or(-1, |i| i as i32) || old_row != row || was_x != on_x {
                    InvalidateRect(h, ptr::null(), 0);
                }
            }
            let mut tme: windows_sys::Win32::UI::Input::KeyboardAndMouse::TRACKMOUSEEVENT = zeroed();
            tme.cbSize = std::mem::size_of_val(&tme) as u32;
            tme.dwFlags = windows_sys::Win32::UI::Input::KeyboardAndMouse::TME_LEAVE;
            tme.hwndTrack = h;
            windows_sys::Win32::UI::Input::KeyboardAndMouse::TrackMouseEvent(&mut tme);
            0
        }
        crate::WM_MOUSELEAVE => { // the crate's own constant: windows-sys has no WM_MOUSELEAVE, and a bare name here would be an irrefutable match binding that swallows every later message
            let (c, r, x) = (HOT_CAT.with(|v| v.replace(-1)), HOT.with(|v| v.replace(-1)), HOTX.with(|v| v.replace(false)));
            if c != -1 || r != -1 || x {
                InvalidateRect(h, ptr::null(), 0);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let mut cr: RECT = zeroed();
            GetClientRect(h, &mut cr);
            let cat = CAT.with(|c| c.get());
            let step = sc(ROW_H * 3);
            let up = (wp >> 16) as i16 > 0;
            let s = SCROLL.with(|v| v.get());
            SCROLL.with(|v| v.set((if up { s - step } else { s + step }).clamp(0, max_scroll(cr.right, cr.bottom, cat))));
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
                SCROLL.with(|s| s.set(0));
                CONFIRM.with(|c| c.set(false));
                InvalidateRect(h, ptr::null(), 0);
                return 0;
            }
            if let Some((_, r, row)) = hit_row(h, x, y) {
                let op = if row.del != Op::None && x >= r.right - sc(30) { row.del } else { row.op };
                if op != Op::None {
                    if !apply(op, h) {
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
                DestroyWindow(h);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(h);
            0
        }
        WM_DESTROY => {
            for f in FONT.with(|x| x.replace([0; 5])) {
                if f != 0 {
                    DeleteObject(f as HFONT);
                }
            }
            BITS.with(|b| b.borrow_mut().clear());
            WND.with(|w| w.set(ptr::null_mut()));
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

// ---------- the window and the program ----------

/// The categories the command line and the single-instance message take (`rust-settings --page barra`).
fn page_index(name: &str) -> u8 {
    match name.trim().to_ascii_lowercase().as_str() {
        "escritorio" | "desktop" => DESK,
        "barra" | "bar" | "taskbar" => BAR,
        "inicio" | "start" => START,
        "idioma" | "language" | "lang" => LANG,
        "apps" | "aplicaciones" => APPS,
        "avanzado" | "advanced" => ADV,
        _ => LOOK,
    }
}

/// `--system` (or a bare `sistema`) and an `ms-settings:` address open the system pages instead of the hub.
fn system_page_from_args() -> Option<usize> {
    std::env::args().skip(1).find_map(|a| {
        if a.starts_with("ms-settings:") {
            Some(crate::page_for(&a))
        } else if a == "--system" || a == "sistema" || a == "system" {
            Some(0)
        } else {
            None
        }
    })
}

fn page_from_args() -> u8 {
    let mut a = std::env::args().skip(1);
    while let Some(x) = a.next() {
        if x == "--page" || x == "-p" {
            if let Some(v) = a.next() {
                return page_index(&v);
            }
        } else if let Some(v) = x.strip_prefix("--page=") {
            return page_index(v);
        } else if !x.starts_with('-') {
            return page_index(&x); // a bare page name, like the old `rust-settings display`
        }
    }
    LOOK
}

fn msg(name: &str, slot: &'static OnceLock<u32>) -> u32 {
    *slot.get_or_init(|| unsafe { RegisterWindowMessageW(wide(name).as_ptr()) })
}

/// A second launch asks the running window to switch page (wParam) and come to the front.
fn page_msg() -> u32 {
    static M: OnceLock<u32> = OnceLock::new();
    msg("WinRust.SettingsPage", &M)
}

/// Every other program (the bar, the desktop) re-reads `look.txt`/`style.txt` and rebuilds itself.
fn changed_msg() -> u32 {
    static M: OnceLock<u32> = OnceLock::new();
    msg("WinRust.SettingsChanged", &M)
}

/// Announces a change that is not just a style name (side, size, monitor, autohide, search button, notifications...).
fn changed() {
    unsafe { PostMessageW(HWND_BROADCAST, changed_msg(), 0, 0) };
}

/// The one window has two faces: the WinRust hub (`HUB`) and the system pages this program already had (`SYSTEM`).
const HUB: u8 = 0;
const SYSTEM: u8 = 1;

thread_local! {
    static MODE: Cell<u8> = const { Cell::new(HUB) };
}

/// Shows the system pages (display, sound, network...) in this same window. Their procedure and painter are the ones
/// this program already had; `Esc` comes back here.
fn enter_system(page: usize) {
    MODE.with(|m| m.set(SYSTEM));
    unsafe {
        let h = WND.with(|w| w.get());
        if !h.is_null() {
            crate::goto(h, page);
            InvalidateRect(h, ptr::null(), 0);
        }
    }
}

fn back_to_hub() {
    MODE.with(|m| m.set(HUB));
    unsafe {
        let h = WND.with(|w| w.get());
        if !h.is_null() {
            InvalidateRect(h, ptr::null(), 0);
        }
    }
}

/// The style names of the shared looks (Spanish through `es.tsv`, like every other text).
fn style_name(s: st::Style) -> &'static str {
    use st::Style::*;
    match s {
        Normal => t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常"),
        Terminal => t!("Терминал", "Terminal", ja: "ターミナル", pt: "Terminal", zh: "终端"),
        Dock => t!("Плавающий док", "Floating dock", ja: "フローティング ドック", pt: "Dock flutuante", zh: "悬浮停靠"),
        Tiling => t!("Тайлинг", "Tiling", ja: "タイル表示", pt: "Lado a lado", zh: "平铺"),
        Road => t!("Белая дорога", "White road", ja: "白い道", pt: "Estrada branca", zh: "白色道路"),
        Ruby => t!("Рубин", "Ruby", ja: "ルビー", pt: "Rubi", zh: "红宝石"),
        Win98 => "Windows 98",
        Win7 => "Windows 7",
    }
}

/// Forgets how often each app was launched (the bar's Start menu ranks by it) and tells the bar to reload it.
fn clear_history() {
    if let Some(p) = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("topbar-usage.txt")) {
        let _ = std::fs::remove_file(p);
    }
    changed();
}

/// Opens a URI, a `shell:` address or a folder with the desktop user's token.
unsafe fn open_target(target: &str) {
    if winrust_common::launch::is_program(target) {
        winrust_common::launch::program(target);
        return;
    }
    let w = wide(target);
    if ShellExecuteW(ptr::null_mut(), wide("open").as_ptr(), w.as_ptr(), ptr::null(), ptr::null(), SW_SHOWNORMAL) as isize <= 32 {
        let _ = std::process::Command::new("explorer.exe").arg(target).spawn();
    }
}

/// Starts one of our programs (Program Files, beside this exe, or the launcher's bin), off the UI thread.
fn launch_app(app: &str, exe: &str, args: &[&str]) {
    let mut v = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        v.push(PathBuf::from(pf).join(app).join(exe));
    }
    if let Ok(e) = std::env::current_exe() {
        v.push(e.with_file_name(exe));
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(la).join(r"plugin-launcher\bin").join(exe));
    }
    if let Some(p) = v.into_iter().find(|p| p.is_file()) {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        std::thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new(p).args(args).creation_flags(0x0800_0000).spawn();
        });
    }
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

/// Registers the class and makes the window: frameless, centered on the monitor under the pointer, 960x680 (never
/// smaller than 720x560, never bigger than the monitor).
unsafe fn create(page: u8) -> HWND {
    // the size and the corner radius are logical: take the system scale before measuring
    DPI.with(|d| d.set((GetDpiForSystem() as i32).max(96)));
    let class = wide(CLASS);
    let inst = GetModuleHandleW(ptr::null());
    let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW), lpszClassName: class.as_ptr(), ..zeroed() };
    RegisterClassW(&wc);
    let mut pt: POINT = zeroed();
    GetCursorPos(&mut pt);
    let m = winrust_common::monitors::at(pt.x, pt.y).rc;
    let (mw, mh) = (m.right - m.left, m.bottom - m.top);
    let w = sc(W).min(mw - sc(32)).max(sc(MIN_W));
    let h = sc(H).min(mh - sc(48)).max(sc(MIN_H));
    let x = m.left + (mw - w) / 2;
    let y = m.top + ((mh - h) / 2).max(sc(16));
    let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), wide(TITLE).as_ptr(), WS_POPUP, x, y, w, h, ptr::null_mut(), ptr::null_mut(), inst, ptr::null());
    if hwnd.is_null() {
        return hwnd;
    }
    DPI.with(|d| d.set((GetDpiForWindow(hwnd) as i32).max(96)));
    CAT.with(|c| c.set(page.min(CATS as u8 - 1)));
    // the screen behind it, blurred, then the rounded corners and the fonts
    let px = frost::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + h }, frost::radius_for(w, h));
    BITS.with(|b| *b.borrow_mut() = px);
    ensure_fonts();
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, sc(24), sc(24)), 1);
    hwnd
}

/// The app entry: one window, its own message loop. A second launch only switches the page of the running one.
pub(crate) unsafe fn run() {
    let page = page_from_args();
    let h = FindWindowW(wide(CLASS).as_ptr(), ptr::null());
    if !h.is_null() {
        PostMessageW(h, page_msg(), page as usize, 0);
        winrust_common::fg::force(h);
        return;
    }
    // the system pages keep their state in thread-locals: read it before the window exists (COM calls inside can
    // dispatch messages)
    let (pg, lay) = (crate::pages(), crate::display::layout());
    let hwnd = create(page);
    if hwnd.is_null() {
        return;
    }
    WND.with(|w| w.set(hwnd));
    let dpi = DPI.with(|d| d.get());
    crate::APP.with_borrow_mut(|a| (a.pages, a.page, a.dpi, a.layout) = (pg, 0, dpi, lay));
    crate::refresh_fonts();
    crate::MAIN.store(hwnd as isize, std::sync::atomic::Ordering::Relaxed);
    crate::wifi::watch(hwnd);
    crate::frame(hwnd);
    refresh_cache();
    // `rust-settings ms-settings:sound` (how the bar's Windows Settings entries call this program): that page
    if let Some(sys) = system_page_from_args() {
        enter_system(sys);
    }
    ShowWindow(hwnd, SW_SHOW);
    winrust_common::fg::force(hwnd);
    let mut m: MSG = zeroed();
    while GetMessageW(&mut m, ptr::null_mut(), 0, 0) > 0 {
        TranslateMessage(&m);
        DispatchMessageW(&m);
    }
}
