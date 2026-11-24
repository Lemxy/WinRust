//! topbar: settings (split out of main.rs; shares its items through `super::*`).
use super::*;

/// The gear's menu: three ready looks on top, then the look in small pages (theme, transparency, colours, templates,
/// language) and the bar itself (buttons and position, auto-hide). Picking on a page keeps it open, so choices can be tried
/// in a row; the page is redrawn in place.
pub(crate) unsafe fn style_menu() {
    use i18n::style as st;
    // the hint is how see-through it is, for the levels the transparency page offers
    let opacity = match st::bar_opacity() {
        100 => t!("нет", "none", ja: "なし", pt: "nenhuma", zh: "无", es: "ninguna"),
        0 => t!("без фона", "no background", ja: "背景なし", pt: "sem fundo", zh: "无背景", es: "sin fondo"),
        85 => "15 %",
        70 => "30 %",
        50 => "50 %",
        30 => "70 %",
        _ => "›",
    };
    let items = vec![
        fly::MItem::head(t!("Готовый вид, в один клик", "A ready look, one click", ja: "ワンクリックで外観を適用", pt: "Uma aparência pronta, com um clique", zh: "一键套用现成外观", es: "Un estilo listo, con un clic")),
        fly::MItem::new(t!("Классика (как Windows 10)", "Classic (like Windows 10)", ja: "クラシック (Windows 10 風)", pt: "Clássico (como o Windows 10)", zh: "经典（如同 Windows 10）", es: "Clásica (como Windows 10)")).hint(t!("кнопки слева", "buttons left", ja: "左寄せ", pt: "à esquerda", zh: "靠左", es: "a la izquierda")),
        fly::MItem::new(t!("Современный (как Windows 11)", "Modern (like Windows 11)", ja: "モダン (Windows 11 風)", pt: "Moderno (como o Windows 11)", zh: "现代（如同 Windows 11）", es: "Moderna (como Windows 11)")).hint(t!("стекло", "glass", ja: "ガラス", pt: "vidro", zh: "玻璃", es: "cristal")),
        fly::MItem::new(t!("Минимум (только значки)", "Minimal (icons only)", ja: "ミニマル (アイコンのみ)", pt: "Mínimo (só os ícones)", zh: "极简（仅图标）", es: "Mínima (solo los íconos)")).hint(t!("без фона", "no background", ja: "背景なし", pt: "sem fundo", zh: "无背景", es: "sin fondo")),
        fly::MItem::head(t!("Внешний вид", "Look", ja: "外観", pt: "Aparência", zh: "外观", es: "Apariencia")).sep(),
        fly::MItem::new(t!("Тема", "Theme", ja: "テーマ", pt: "Tema", zh: "主题", es: "Tema")).hint(style_name(style())).swatch(st::pal().accent),
        fly::MItem::new(t!("Прозрачность", "Transparency", ja: "透明度", pt: "Transparência", zh: "透明度", es: "Transparencia")).hint(opacity),
        fly::MItem::new(t!("Цвета", "Colors", ja: "色", pt: "Cores", zh: "颜色", es: "Colores")).hint("›"),
        fly::MItem::new(t!("Мои шаблоны", "My templates", ja: "マイ テンプレート", pt: "Meus modelos", zh: "我的模板", es: "Mis plantillas")).hint("›"),
        fly::MItem::new(t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言", es: "Idioma")).hint(i18n::lang().name()),
        fly::MItem::head(t!("Панель", "Bar", ja: "バー", pt: "Barra", zh: "任务栏", es: "Barra")).sep(),
        fly::MItem::new(t!("Кнопки и положение", "Buttons and position", ja: "ボタンと位置", pt: "Botões e posição", zh: "按钮和位置", es: "Botones y posición")).hint("›"),
        fly::MItem::new(t!("Прятать панель, пока мышь не внизу", "Hide the bar until the pointer reaches it", ja: "ポインターが来るまでバーを隠す", pt: "Esconder a barra até o ponteiro chegar", zh: "指针移过去之前隐藏任务栏", es: "Ocultar la barra hasta que llegue el puntero")).on(st::autohide()),
        fly::MItem::new(t!("Разложить окна плиткой", "Tile the windows", ja: "ウィンドウを並べて表示", pt: "Lado a lado", zh: "平铺窗口", es: "Organizar las ventanas en mosaico")),
        fly::MItem::new(t!("Сбросить всё по умолчанию…", "Reset everything to defaults…", ja: "すべて初期設定に戻す…", pt: "Restaurar tudo ao padrão…", zh: "将所有设置恢复默认…", es: "Restablecer todo a los valores predeterminados…")).danger().sep(),
    ];
    gear_page("\u{E713}", t!("Настройки панели", "Bar settings", ja: "バーの設定", pt: "Configurações da barra", zh: "任务栏设置", es: "Configuración de la barra"), t!("Вид панели и программ WinRust", "Look of the bar and the WinRust programs", ja: "バーと WinRust プログラムの外観", pt: "Aparência da barra e dos programas da WinRust", zh: "任务栏与 WinRust 程序的外观", es: "Apariencia de la barra y de los programas de WinRust"), items, |i| match i {
        1..=3 => {
            preset(i - 1);
            style_menu();
        }
        5 => theme_page(),
        6 => transparency_page(),
        7 => colors_page(),
        8 => templates_page(),
        9 => language_page(),
        11 => bar_page(),
        12 => {
            st::set_autohide(!st::autohide()); // the reserved strip comes or goes with it
            relook(false);
            style_menu();
        }
        13 => tile_windows(),
        14 => reset_page(),
        _ => {}
    });
}

/// The three ready looks: 0 classic (Windows 10: buttons on the left, solid), 1 modern (Windows 11: centred, glass),
/// 2 minimal (no background, no search / Task View buttons, small). The theme and colours stay as they are.
pub(crate) unsafe fn preset(k: usize) {
    use i18n::style as st;
    let (left, opacity, blur, buttons, size) = match k {
        0 => (true, 100, false, true, 1),
        1 => (false, 80, true, true, 1),
        _ => (false, 0, false, false, 0),
    };
    st::set_align_left(left);
    st::set_bar_opacity(opacity);
    st::set_bar_blur(blur);
    st::set_show_search(buttons);
    st::set_show_taskview(buttons);
    st::set_bar_size(size);
    relook(true); // the size may change the bar's height
}

/// How see-through the bar is: only its background, the buttons, icons and clock stay sharp. And Windows' blur behind.
pub(crate) unsafe fn transparency_page() {
    use i18n::style as st;
    const PCT: [u8; 6] = [100, 85, 70, 50, 30, 0];
    let op = st::bar_opacity();
    let mut items = vec![back(), fly::MItem::head(t!("Насколько виден фон панели", "How much of the bar's background shows", ja: "バーの背景の見え方", pt: "Quanto do fundo da barra aparece", zh: "任务栏背景的可见程度", es: "Cuánto se ve el fondo de la barra")).sep()];
    items.extend(PCT.iter().map(|&v| {
        let name = match v {
            100 => t!("Сплошной фон", "Solid background", ja: "不透明な背景", pt: "Fundo sólido", zh: "不透明背景", es: "Fondo sólido").to_string(),
            0 => t!("Без фона (только значки)", "No background (icons only)", ja: "背景なし (アイコンのみ)", pt: "Sem fundo (só os ícones)", zh: "无背景（仅图标）", es: "Sin fondo (solo los íconos)").to_string(),
            v => tf!("Прозрачность {} %", "{} % see-through", ja: "透明度 {} %", pt: "{} % transparente", zh: "透明度 {} %", es: "{} % transparente", 100 - v as u32),
        };
        fly::MItem::new(name).on(op == v)
    }));
    items.push(fly::MItem::head(t!("Эффект", "Effect", ja: "効果", pt: "Efeito", zh: "效果", es: "Efecto")).sep());
    items.push(fly::MItem::new(t!("Размытие под панелью, как в Windows", "Blur behind the bar, like Windows", ja: "バーの背後をぼかす (Windows 風)", pt: "Desfoque atrás da barra, como no Windows", zh: "像 Windows 那样模糊任务栏背后", es: "Desenfoque detrás de la barra, como en Windows")).on(st::bar_blur()));
    let blur_row = 2 + PCT.len() + 1;
    gear_page("\u{E790}", t!("Прозрачность", "Transparency", ja: "透明度", pt: "Transparência", zh: "透明度", es: "Transparencia"), t!("Значки и часы всегда остаются чёткими", "The icons and the clock always stay sharp", ja: "アイコンと時計は常にくっきり表示", pt: "Os ícones e o relógio continuam nítidos", zh: "图标和时钟始终清晰", es: "Los íconos y el reloj siempre quedan nítidos"), items, move |i| {
        if i == 0 {
            return style_menu();
        }
        if let Some(&v) = i.checked_sub(2).and_then(|k| PCT.get(k)) {
            st::set_bar_opacity(v);
        } else if i == blur_row {
            let on = !st::bar_blur();
            st::set_bar_blur(on);
            if on && st::bar_opacity() == 100 {
                st::set_bar_opacity(85); // a solid bar shows no blur at all
            }
        }
        relook(false);
        transparency_page();
    });
}

/// One page of the gear's menu, opened at the gear.
pub(crate) unsafe fn gear_page(glyph: &'static str, title: &str, sub: &str, items: Vec<fly::MItem>, pick: impl FnOnce(usize) + 'static) {
    let bar = BAR.load(Relaxed) as HWND;
    fly::menu(tip_cx(&bar_layout(bar), GEAR), Some((null_mut(), glyph, title.to_string(), sub.to_string())), items, pick);
}

pub(crate) fn back() -> fly::MItem {
    fly::MItem::new(t!("‹   Назад", "‹   Back", ja: "‹   戻る", pt: "‹   Voltar", zh: "‹   返回", es: "‹   Atrás"))
}

pub(crate) fn style_name(s: i18n::style::Style) -> &'static str {
    use i18n::style::Style::*;
    match s {
        Normal => t!("Обычный", "Normal", ja: "通常", pt: "Normal", zh: "正常", es: "Normal"),
        Terminal => t!("Терминал", "Terminal", ja: "ターミナル", pt: "Terminal", zh: "终端", es: "Terminal"),
        Dock => t!("Плавающий док", "Floating dock", ja: "フローティング ドック", pt: "Dock flutuante", zh: "悬浮停靠", es: "Dock flotante"),
        Tiling => t!("Тайлинг", "Tiling", ja: "タイル表示", pt: "Lado a lado", zh: "平铺", es: "Mosaico"),
        Road => t!("Белая дорога", "White road", ja: "白い道", pt: "Estrada branca", zh: "白色道路", es: "Camino blanco"),
        Ruby => t!("Рубин", "Ruby", ja: "ルビー", pt: "Rubi", zh: "红宝石", es: "Rubí"),
        Win98 => "Windows 98",
        Win7 => "Windows 7",
    }
}

/// Repaints the bar in the new look; `moved` = its place or size may have changed too.
pub(crate) unsafe fn relook(moved: bool) {
    let bar = BAR.load(Relaxed) as HWND;
    place(bar);
    reload_icons(); // the small bar has smaller icons
    if moved {
        bar_anim(bar);
    }
    apply_opacity(bar);
    InvalidateRect(bar, null(), 0);
}

/// The language of every WinRust program (written to lang.txt; open windows switch on the broadcast).
pub(crate) unsafe fn language_page() {
    use i18n::Lang;
    let mut items = vec![back()];
    for (k, l) in Lang::ALL.into_iter().enumerate() {
        let it = fly::MItem::new(l.name()).on(l == i18n::lang());
        items.push(if k == 0 { it.sep() } else { it });
    }
    gear_page("\u{E774}", t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言", es: "Idioma"), t!("Язык всех программ WinRust", "The language of every WinRust program", ja: "すべての WinRust プログラムの言語", pt: "O idioma de todos os programas da WinRust", zh: "所有 WinRust 程序的语言", es: "El idioma de todos los programas de WinRust"), items, move |i| {
        let Some(&l) = i.checked_sub(1).and_then(|k| Lang::ALL.get(k)) else { return style_menu() };
        i18n::set_lang(l);
        InvalidateRect(BAR.load(Relaxed) as HWND, null(), 0);
        language_page();
    });
}

pub(crate) unsafe fn theme_page() {
    let mut items = vec![back()];
    for (k, &s) in i18n::style::ALL.iter().enumerate() {
        let it = fly::MItem::new(style_name(s)).swatch(i18n::style::PALS[k].accent).on(s == style()).beta(s.beta());
        items.push(if k == 0 { it.sep() } else { it });
    }
    gear_page("\u{E790}", t!("Тема", "Theme", ja: "テーマ", pt: "Tema", zh: "主题", es: "Tema"), t!("Готовый вид панели и окон", "A ready-made look for the bar and windows", ja: "バーとウィンドウのできあいの外観", pt: "Uma aparência pronta para a barra e as janelas", zh: "任务栏和窗口的现成外观", es: "Una apariencia lista para la barra y las ventanas"), items, |i| {
        let Some(&s) = i.checked_sub(1).and_then(|k| i18n::style::ALL.get(k)) else { return style_menu() };
        i18n::style::set_style(s);
        relook(false);
        theme_page();
    });
}

pub(crate) unsafe fn bar_page() {
    use i18n::style::{self as st, Side};
    let sides = [Side::Bottom, Side::Left, Side::Right];
    let items = vec![
        back(),
        fly::MItem::head(t!("Положение", "Position", ja: "位置", pt: "Posição", zh: "位置", es: "Posición")).sep(),
        fly::MItem::new(t!("Снизу", "Bottom", ja: "下", pt: "Embaixo", zh: "底部", es: "Abajo")).on(st::side() == Side::Bottom),
        fly::MItem::new(t!("Слева", "Left", ja: "左", pt: "Esquerda", zh: "左侧", es: "Izquierda")).on(st::side() == Side::Left),
        fly::MItem::new(t!("Справа", "Right", ja: "右", pt: "Direita", zh: "右侧", es: "Derecha")).on(st::side() == Side::Right),
        fly::MItem::head(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小", es: "Tamaño")),
        fly::MItem::new(t!("Маленькая", "Small", ja: "小", pt: "Pequeno", zh: "小", es: "Pequeña")).on(st::bar_size() == 0),
        fly::MItem::new(t!("Обычная", "Normal", ja: "通常", pt: "Normal", zh: "正常", es: "Normal")).on(st::bar_size() == 1),
        fly::MItem::new(t!("Большая", "Large", ja: "大", pt: "Grande", zh: "大", es: "Grande")).on(st::bar_size() == 2),
        fly::MItem::head(t!("Кнопки", "Buttons", ja: "ボタン", pt: "Botões", zh: "按钮", es: "Botones")),
        fly::MItem::new(t!("По центру, как в Windows 11", "Centered, like Windows 11", ja: "中央に配置 (Windows 11 風)", pt: "Centralizados, como no Windows 11", zh: "居中，如同 Windows 11", es: "Centrados, como en Windows 11")).on(!st::align_left()),
        fly::MItem::new(t!("Кнопка поиска", "Search button", ja: "検索ボタン", pt: "Botão de pesquisa", zh: "搜索按钮", es: "Botón de búsqueda")).on(st::show_search()),
        fly::MItem::new(t!("Кнопка представления задач", "Task View button", ja: "タスク ビュー ボタン", pt: "Botão Visão de Tarefas", zh: "任务视图按钮", es: "Botón de vista de tareas")).on(st::show_taskview()),
        fly::MItem::new(t!("Все значки трея на панели", "Every tray icon on the bar", ja: "すべてのトレイ アイコンをバーに表示", pt: "Todos os ícones da bandeja na barra", zh: "所有托盘图标都显示在任务栏上", es: "Todos los íconos de la bandeja en la barra")).on(st::tray_all()),
        fly::MItem::head(t!("Поведение", "Behavior", ja: "動作", pt: "Comportamento", zh: "行为", es: "Comportamiento")),
        fly::MItem::new(t!("Прятать панель, пока мышь не внизу", "Hide the bar until the pointer reaches it", ja: "ポインターが来るまでバーを隠す", pt: "Esconder a barra até o ponteiro chegar", zh: "指针移过去之前隐藏任务栏", es: "Ocultar la barra hasta que llegue el puntero")).on(st::autohide()),
        fly::MItem::new(t!("Alt+Tab: только этот монитор", "Alt+Tab: this monitor only", ja: "Alt+Tab: このモニターのみ", pt: "Alt+Tab: somente este monitor", zh: "Alt+Tab：仅限此显示器", es: "Alt+Tab: solo este monitor")).on(st::alttab_per_monitor()),
    ];
    gear_page("\u{E8A1}", t!("Кнопки и положение", "Buttons and position", ja: "ボタンと位置", pt: "Botões e posição", zh: "按钮和位置", es: "Botones y posición"), t!("Где панель, её размер и что на ней", "Where the bar sits, its size and what is on it", ja: "バーの位置、サイズ、表示する内容", pt: "Onde a barra fica, seu tamanho e o que há nela", zh: "任务栏的位置、大小和上面显示的内容", es: "Dónde está la barra, su tamaño y qué hay en ella"), items, move |i| {
        match i {
            2..=4 => st::set_side(sides[i - 2]),
            6..=8 => st::set_bar_size((i - 6) as u8),
            10 => st::set_align_left(!st::align_left()),
            11 => st::set_show_search(!st::show_search()),
            12 => st::set_show_taskview(!st::show_taskview()),
            13 => st::set_tray_all(!st::tray_all()),
            15 => st::set_autohide(!st::autohide()), // the reserved strip comes or goes with it
            16 => st::set_alttab_per_monitor(!st::alttab_per_monitor()),
            _ => return style_menu(),
        }
        relook(matches!(i, 2..=8)); // only the edge and the size move the bar: the rest repaints where it is
        bar_page();
    });
}

/// Bar transparency (100 % = opaque). The see-through part is per pixel (glass.rs) and the window itself stays whole:
/// fading the layered window faded the icons and the clock along with the background.
pub(crate) unsafe fn apply_opacity(hwnd: HWND) {
    SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
    glass::set_accent(hwnd, glass::accent());
}

/// The user's own look: every colour of the bar and the WinRust windows. Each colour opens the Windows colour dialog; the
/// last row goes back to the chosen theme's colours. (The transparency has its own page.)
pub(crate) unsafe fn colors_page() {
    use windows_sys::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
    let colors = [
        t!("Фон", "Background", ja: "背景", pt: "Fundo", zh: "背景", es: "Fondo"),
        t!("Линии и рамки", "Lines and borders", ja: "線と枠", pt: "Linhas e bordas", zh: "线条与边框", es: "Líneas y bordes"),
        t!("Наведение", "Hover", ja: "ホバー", pt: "Passar o mouse", zh: "悬停", es: "Al pasar el puntero"),
        t!("Нажатие и выбранное", "Pressed and selected", ja: "押下中と選択中", pt: "Pressionado e selecionado", zh: "按下和选中", es: "Presionado y seleccionado"),
        t!("Акцент", "Accent", ja: "アクセント", pt: "Destaque", zh: "强调色", es: "Énfasis"),
        t!("Текст", "Text", ja: "テキスト", pt: "Texto", zh: "文本", es: "Texto"),
        t!("Второстепенный текст", "Secondary text", ja: "補助テキスト", pt: "Texto secundário", zh: "次要文本", es: "Texto secundario"),
    ];
    let cur = i18n::style::custom_colors().unwrap_or([u32::MAX; 7]);
    let p = i18n::style::pal();
    let now = [p.bg, p.line, p.hover, p.active, p.accent, p.text, p.dim]; // what is shown now: the dialog starts there
    let mut items = vec![back(), fly::MItem::head(t!("Цвета (точка справа: изменён вами)", "Colors (dot on the right: changed by you)", ja: "色 (右の点: 変更済み)", pt: "Cores (ponto à direita: alterada por você)", zh: "颜色（右侧圆点：已由你更改）", es: "Colores (punto a la derecha: cambiado por ti)")).sep()];
    items.extend(colors.iter().enumerate().map(|(i, n)| fly::MItem::new(*n).swatch(now[i]).on(cur[i] != u32::MAX)));
    items.push(fly::MItem::new(t!("Вернуть цвета темы", "Back to the theme's colors", ja: "テーマの色に戻す", pt: "Voltar às cores do tema", zh: "恢复主题颜色", es: "Volver a los colores del tema")).danger().sep());
    let c0 = 2; // first colour row
    gear_page("\u{E790}", t!("Цвета", "Colors", ja: "色", pt: "Cores", zh: "颜色", es: "Colores"), t!("Поверх выбранной темы", "On top of the chosen theme", ja: "選択したテーマの上に適用", pt: "Sobre o tema escolhido", zh: "在所选主题之上", es: "Sobre el tema elegido"), items, move |i| {
        let bar = BAR.load(Relaxed) as HWND;
        if i == 0 {
            return style_menu();
        } else if (c0..c0 + colors.len()).contains(&i) {
            static CUST: std::sync::Mutex<[u32; 16]> = std::sync::Mutex::new([0xFFFFFF; 16]);
            let mut cust = CUST.lock().unwrap();
            let mut cc: CHOOSECOLORW = zeroed();
            cc.lStructSize = size_of::<CHOOSECOLORW>() as u32;
            cc.hwndOwner = bar;
            cc.rgbResult = now[i - c0];
            cc.lpCustColors = cust.as_mut_ptr();
            cc.Flags = CC_RGBINIT | CC_FULLOPEN;
            if ChooseColorW(&mut cc) != 0 {
                i18n::style::set_custom_color(i - c0, Some(cc.rgbResult));
            }
        } else {
            i18n::style::reset_custom();
        }
        apply_opacity(bar);
        InvalidateRect(bar, null(), 0);
        colors_page();
    });
}

/// Saved looks (theme + colours + opacity): apply one, save what is on screen, delete.
pub(crate) unsafe fn templates_page() {
    use i18n::style::{ALL, PALS, Theme};
    let list = i18n::style::themes();
    let now = Theme::current("");
    let mut items = vec![back(), fly::MItem::head(t!("Мои шаблоны", "My templates", ja: "マイ テンプレート", pt: "Meus modelos", zh: "我的模板", es: "Mis plantillas")).sep()];
    if list.is_empty() {
        items.push(fly::MItem::head(t!("Пока пусто: настройте вид и сохраните", "None yet: set up a look and save it", ja: "まだありません: 外観を設定して保存してください", pt: "Nenhum ainda: monte uma aparência e salve", zh: "暂无：设置好外观后保存", es: "Ninguna todavía: crea una apariencia y guárdala")));
    }
    let first = items.len(); // index of the first template row
    for th in &list {
        let accent = if th.colors[4] != u32::MAX { th.colors[4] } else { PALS[ALL.iter().position(|&s| s == th.style).unwrap_or(0)].accent };
        let same = (th.style, th.colors, th.opacity) == (now.style, now.colors, now.opacity);
        items.push(fly::MItem::new(th.name.clone()).hint(style_name(th.style)).swatch(accent).on(same));
    }
    let save = items.len();
    items.push(fly::MItem::new(t!("Сохранить текущий вид как шаблон", "Save the current look as a template", ja: "現在の外観をテンプレートとして保存", pt: "Salvar a aparência atual como modelo", zh: "将当前外观保存为模板", es: "Guardar la apariencia actual como plantilla")).sep());
    if !list.is_empty() {
        items.push(fly::MItem::new(t!("Удалить шаблон…", "Delete a template…", ja: "テンプレートを削除…", pt: "Excluir um modelo…", zh: "删除模板…", es: "Eliminar una plantilla…")).danger());
    }
    gear_page("\u{E8F1}", t!("Шаблоны", "Templates", ja: "テンプレート", pt: "Modelos", zh: "模板", es: "Plantillas"), t!("Свои наборы на любое настроение", "Your own looks for any mood", ja: "気分に合わせたオリジナルの外観", pt: "Suas próprias aparências para qualquer humor", zh: "随心保存自己的外观", es: "Tus propias apariencias para cada momento"), items, move |i| {
        let mut list = list;
        if i == 0 {
            return style_menu();
        } else if (first..save).contains(&i) {
            list[i - first].apply();
            relook(false);
        } else if i == save {
            let name = |n: usize| tf!("Шаблон {}", "Template {}", ja: "テンプレート {}", pt: "Modelo {}", zh: "模板 {}", es: "Plantilla {}", n);
            let n = (1..).find(|&n| !list.iter().any(|t| t.name == name(n))).unwrap_or(1);
            list.push(Theme::current(&name(n)));
            i18n::style::save_themes(&list);
        } else {
            return delete_page();
        }
        templates_page();
    });
}

pub(crate) unsafe fn delete_page() {
    let list = i18n::style::themes();
    let mut items = vec![back()];
    for (k, th) in list.iter().enumerate() {
        let it = fly::MItem::new(th.name.clone()).hint(style_name(th.style)).danger();
        items.push(if k == 0 { it.sep() } else { it });
    }
    gear_page("\u{E74D}", t!("Удалить шаблон", "Delete a template", ja: "テンプレートを削除", pt: "Excluir um modelo", zh: "删除模板", es: "Eliminar una plantilla"), t!("Нажмите на шаблон, чтобы удалить", "Click a template to delete it", ja: "削除するテンプレートをクリック", pt: "Clique em um modelo para excluí-lo", zh: "点击模板即可删除", es: "Haz clic en una plantilla para eliminarla"), items, move |i| {
        let mut list = list;
        if i == 0 || i > list.len() {
            return templates_page();
        }
        list.remove(i - 1);
        i18n::style::save_themes(&list);
        if list.is_empty() { templates_page() } else { delete_page() }
    });
}

pub(crate) unsafe fn reset_page() {
    let items = vec![fly::MItem::new(t!("Да, сбросить", "Yes, reset", ja: "はい、リセットします", pt: "Sim, restaurar", zh: "是，重置", es: "Sí, restablecer")).danger(), fly::MItem::new(t!("Отмена", "Cancel", ja: "キャンセル", pt: "Cancelar", zh: "取消", es: "Cancelar"))];
    gear_page("\u{E777}", t!("Сбросить всё?", "Reset everything?", ja: "すべてをリセットしますか?", pt: "Restaurar tudo?", zh: "全部重置？", es: "¿Restablecer todo?"), t!("Тема, цвета, панель. Шаблоны останутся", "Theme, colors, bar. Templates stay", ja: "テーマ、色、バー。テンプレートは残ります", pt: "Tema, cores, barra. Os modelos permanecem", zh: "主题、颜色、任务栏。模板保留", es: "Tema, colores, barra. Las plantillas se conservan"), items, |i| {
        if i == 0 {
            i18n::style::reset_all();
            relook(true);
        }
        style_menu();
    });
}
