//! topbar: settings (split out of main.rs; shares its items through `super::*`).
use super::*;

/// The gear's menu: a short main page that leads to pages for the theme, the colours, the templates and the bar itself
/// (the look is saved for the other WinRust programs too). Picking on a page keeps it open, so choices can be tried in a row.
pub(crate) unsafe fn style_menu() {
    crate::settingswin::open();
}

/// One page of the gear's menu, opened at the gear of the bar the menu belongs to. A page opened from a panel keeps that
/// panel's monitor (the bar's one-second timer may have moved `CURRENT` to another bar meanwhile).
pub(crate) unsafe fn gear_page(glyph: &'static str, title: &str, sub: &str, items: Vec<fly::MItem>, pick: impl FnOnce(usize) + 'static) {
    let from_panel = fly::open_rect().map(bar_on).filter(|h| !h.is_null());
    let bar = from_panel.unwrap_or_else(|| cur_bar());
    CURRENT.set(bar);
    fly::menu(tip_cx(&bar_layout(bar), 0), Some((null_mut(), glyph, title.to_string(), sub.to_string())), items, pick);
}

pub(crate) fn back() -> fly::MItem {
    fly::MItem::new(t!("‹   Назад", "‹   Back", ja: "‹   戻る", pt: "‹   Voltar", zh: "‹   返回"))
}

pub(crate) fn style_name(s: i18n::style::Style) -> &'static str {
    use i18n::style::Style::*;
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

/// Repaints the bars in the new look; `moved` = their place or size may have changed too.
pub(crate) unsafe fn relook(moved: bool) {
    let bar = leader_bar();
    place_all();
    if moved {
        bar_anim(bar);
    }
    invalidate_bars(); // place() already did, but keep a style change visible even if a place was a no-op
}

/// The language of every WinRust program (written to lang.txt; the others pick it up when they start).
pub(crate) unsafe fn language_page() {
    use i18n::Lang::*;
    let mut items = vec![back()];
    for (k, l) in [En, Ru, Ja, Pt, Zh, Es].into_iter().enumerate() {
        let it = fly::MItem::new(l.name()).on(l == i18n::lang());
        items.push(if k == 0 { it.sep() } else { it });
    }
    gear_page("\u{E774}", t!("Язык", "Language", ja: "言語", pt: "Idioma", zh: "语言"), t!("Язык всех программ WinRust", "The language of every WinRust program", ja: "すべての WinRust プログラムの言語", pt: "O idioma de todos os programas da WinRust", zh: "所有 WinRust 程序的语言"), items, move |i| {
        let Some(&l) = i.checked_sub(1).and_then(|k| [En, Ru, Ja, Pt, Zh, Es].get(k)) else { return style_menu() };
        i18n::set_lang(l);
        invalidate_bars();
        language_page();
    });
}

#[derive(Clone, Copy)]
enum BlurOp {
    BarToggle,
    BarR(i32),
    MenuToggle,
    MenuA(u8),
    MenuR(i32),
}

/// The frost editor: the blur behind the bar and behind the context menu, and how solid the menu looks.
pub(crate) unsafe fn blur_page() {
    use winrust_common::look as lk;
    let mut items = vec![back()];
    let mut ops: Vec<Option<BlurOp>> = vec![None];
    items.push(fly::MItem::head(t!("Панель", "Bar", ja: "バー", pt: "Barra", zh: "任务栏")).sep());
    ops.push(None);
    items.push(fly::MItem::new(t!("Размытие за панелью", "Blur behind the bar", ja: "バーの後ろをぼかす", pt: "Desfoque atrás da barra", zh: "任务栏后模糊")).on(lk::bar_blur()));
    ops.push(Some(BlurOp::BarToggle));
    items.push(fly::MItem::new(t!("Слабое", "Soft", ja: "弱い", pt: "Suave", zh: "弱")).hint("4 px").on(lk::bar_radius() == 4));
    ops.push(Some(BlurOp::BarR(4)));
    items.push(fly::MItem::new(t!("Среднее", "Medium", ja: "中", pt: "Médio", zh: "中")).hint("8 px").on(lk::bar_radius() == 8));
    ops.push(Some(BlurOp::BarR(8)));
    items.push(fly::MItem::new(t!("Сильное", "Strong", ja: "強い", pt: "Forte", zh: "强")).hint("14 px").on(lk::bar_radius() == 14));
    ops.push(Some(BlurOp::BarR(14)));
    items.push(fly::MItem::head(t!("Контекстное меню", "Context menu", ja: "コンテキスト メニュー", pt: "Menu de contexto", zh: "上下文菜单")).sep());
    ops.push(None);
    items.push(fly::MItem::new(t!("Размытие за меню", "Blur behind the menu", ja: "メニューの後ろをぼかす", pt: "Desfoque atrás do menu", zh: "菜单后模糊")).on(lk::menu_blur()));
    ops.push(Some(BlurOp::MenuToggle));
    items.push(fly::MItem::head(t!("Непрозрачность меню", "Menu opacity", ja: "メニューの不透明度", pt: "Opacidade do menu", zh: "菜单不透明度")));
    ops.push(None);
    for (a, hint) in [(90u8, "90 %"), (75, "75 %"), (67, "67 %"), (50, "50 %"), (35, "35 %")] {
        items.push(fly::MItem::new(hint).on(lk::menu_alpha() == a));
        ops.push(Some(BlurOp::MenuA(a)));
    }
    items.push(fly::MItem::head(t!("Сила размытия", "Blur strength", ja: "ぼかしの強さ", pt: "Intensidade do desfoque", zh: "模糊强度")));
    ops.push(None);
    for (r, hint) in [(12, "12 px"), (18, "18 px"), (26, "26 px")] {
        items.push(fly::MItem::new(hint).on(lk::menu_radius() == r));
        ops.push(Some(BlurOp::MenuR(r)));
    }
    gear_page("\u{E7C4}", t!("Размытие и меню", "Blur and menus", ja: "ぼかしとメニュー", pt: "Desfoque e menus", zh: "模糊与菜单"), t!("Мороз за панелью и контекстным меню", "Frost behind the bar and the context menu", ja: "バーとコンテキスト メニューのぼかし", pt: "Desfoque atrás da barra e do menu", zh: "任务栏和上下文菜单后的模糊"), items, move |i| {
        let bar = cur_bar();
        if i == 0 {
            return style_menu();
        }
        match ops.get(i).copied().flatten() {
            Some(BlurOp::BarToggle) => lk::bar_blur_set(!lk::bar_blur()),
            Some(BlurOp::BarR(r)) => lk::bar_radius_set(r),
            Some(BlurOp::MenuToggle) => lk::menu_blur_set(!lk::menu_blur()),
            Some(BlurOp::MenuA(a)) => lk::menu_alpha_set(a),
            Some(BlurOp::MenuR(r)) => lk::menu_radius_set(r),
            None => {}
        }
        apply_opacity(bar);
        invalidate_bars();
        blur_page();
    });
}

/// The Start menu: where it opens and which of the three classic looks it wears.
pub(crate) unsafe fn start_page() {
    use winrust_common::look as lk;
    let (left, st) = (lk::start_left(), lk::start_style());
    let items = vec![
        back(),
        fly::MItem::head(t!("Положение", "Position", ja: "位置", pt: "Posição", zh: "位置")).sep(),
        fly::MItem::new(t!("По центру", "Centered", ja: "中央", pt: "Centralizado", zh: "居中")).on(!left),
        fly::MItem::new(t!("Слева", "On the left", ja: "左", pt: "À esquerda", zh: "靠左")).on(left),
        fly::MItem::head(t!("Стиль", "Style", ja: "スタイル", pt: "Estilo", zh: "样式")),
        fly::MItem::new("Windows 11").on(st == 0),
        fly::MItem::new("Windows 10").on(st == 1),
        fly::MItem::new("Windows 7").on(st == 2),
        fly::MItem::head(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小")),
        fly::MItem::new(t!("Мелкий", "Small", ja: "小", pt: "Pequeno", zh: "小")).on(lk::start_scale() == 90),
        fly::MItem::new(t!("Обычный", "Normal", ja: "標準", pt: "Normal", zh: "标准")).on(lk::start_scale() == 100),
        fly::MItem::new(t!("Крупный", "Large", ja: "大", pt: "Grande", zh: "大")).on(lk::start_scale() == 115),
    ];
    gear_page("\u{E700}", t!("Меню «Пуск»", "Start menu", ja: "スタート メニュー", pt: "Menu Iniciar", zh: "开始菜单"), t!("Где открывается и как выглядит", "Where it opens and how it looks", ja: "開く位置と外観", pt: "Onde abre e como aparece", zh: "打开位置和外观"), items, move |i| {
        match i {
            2 => lk::start_left_set(false),
            3 => lk::start_left_set(true),
            5 => lk::start_style_set(0),
            6 => lk::start_style_set(1),
            7 => lk::start_style_set(2),
            9 => lk::start_scale_set(0),
            10 => lk::start_scale_set(1),
            11 => lk::start_scale_set(2),
            _ => return style_menu(),
        }
        start_page();
    });
}

pub(crate) unsafe fn theme_page() {
    let mut items = vec![back()];
    for (k, &s) in i18n::style::ALL.iter().enumerate() {
        let it = fly::MItem::new(style_name(s)).swatch(i18n::style::PALS[k].accent).on(s == style()).beta(s.beta());
        items.push(if k == 0 { it.sep() } else { it });
    }
    gear_page("\u{E790}", t!("Тема", "Theme", ja: "テーマ", pt: "Tema", zh: "主题"), t!("Готовый вид панели и окон", "A ready-made look for the bar and windows", ja: "バーとウィンドウのできあいの外観", pt: "Uma aparência pronta para a barra e as janelas", zh: "任务栏和窗口的现成外观"), items, |i| {
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
        fly::MItem::head(t!("Положение", "Position", ja: "位置", pt: "Posição", zh: "位置")).sep(),
        fly::MItem::new(t!("Снизу", "Bottom", ja: "下", pt: "Embaixo", zh: "底部")).on(st::side() == Side::Bottom),
        fly::MItem::new(t!("Слева", "Left", ja: "左", pt: "Esquerda", zh: "左侧")).on(st::side() == Side::Left),
        fly::MItem::new(t!("Справа", "Right", ja: "右", pt: "Direita", zh: "右侧")).on(st::side() == Side::Right),
        fly::MItem::head(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小")),
        fly::MItem::new(t!("Маленькая", "Small", ja: "小", pt: "Pequeno", zh: "小")).on(st::bar_size() == 0),
        fly::MItem::new(t!("Обычная", "Normal", ja: "通常", pt: "Normal", zh: "正常")).on(st::bar_size() == 1),
        fly::MItem::new(t!("Большая", "Large", ja: "大", pt: "Grande", zh: "大")).on(st::bar_size() == 2),
        fly::MItem::head(t!("Поведение", "Behavior", ja: "動作", pt: "Comportamento", zh: "行为")),
        fly::MItem::new(t!("Скрывать автоматически", "Hide automatically", ja: "自動的に隠す", pt: "Ocultar automaticamente", zh: "自动隐藏")).on(st::autohide()),
        fly::MItem::new(t!("Кнопки слева, как в Windows 10", "Buttons on the left, like Windows 10", ja: "ボタンを左に配置 (Windows 10 風)", pt: "Botões à esquerda, como no Windows 10", zh: "按钮靠左，如同 Windows 10")).on(st::align_left()),
        fly::MItem::new(t!("Alt+Tab: только этот монитор", "Alt+Tab: this monitor only", ja: "Alt+Tab: このモニターのみ", pt: "Alt+Tab: somente este monitor", zh: "Alt+Tab：仅限此显示器")).on(st::alttab_per_monitor()),
        fly::MItem::new(t!("Кнопка поиска", "Search button", ja: "検索ボタン", pt: "Botão de pesquisa", zh: "搜索按钮")).on(winrust_common::look::bar_search()),
        fly::MItem::new(t!("Кнопка задач", "Task View button", ja: "タスク ビュー ボタン", pt: "Botão de exibição de tarefas", zh: "任务视图按钮")).on(winrust_common::look::bar_taskview()),
    ];
    gear_page("\u{E8A1}", t!("Панель", "Bar", ja: "バー", pt: "Barra", zh: "任务栏"), t!("Где она, какого размера и как себя ведёт", "Where it sits, its size and behavior", ja: "配置場所、サイズ、動作", pt: "Onde fica, seu tamanho e comportamento", zh: "其位置、大小和行为"), items, move |i| {
        match i {
            2..=4 => st::set_side(sides[i - 2]),
            6..=8 => st::set_bar_size((i - 6) as u8),
            10 => st::set_autohide(!st::autohide()), // the reserved strip comes or goes with it
            11 => st::set_align_left(!st::align_left()),
            12 => st::set_alttab_per_monitor(!st::alttab_per_monitor()),
            13 => winrust_common::look::bar_search_set(!winrust_common::look::bar_search()),
            14 => winrust_common::look::bar_taskview_set(!winrust_common::look::bar_taskview()),
            _ => return style_menu(),
        }
        relook(i != 12);
        bar_page();
    });
}

/// Bar transparency from the colours page (the window is layered; 100 % = opaque).
pub(crate) unsafe fn apply_opacity(hwnd: HWND) {
    // With the blur on, the transparency lives in the tinted background instead of the whole window: that keeps the
    // icons and the text crisp instead of washing them out.
    let a = if crate::blur::on() { 255 } else { (255 * i18n::style::bar_opacity() as u32 / 100) as u8 };
    SetLayeredWindowAttributes(hwnd, 0, a, LWA_ALPHA);
}

/// The user's own look: every colour of the bar and the WinRust windows, and the bar's transparency. Each colour opens
/// the Windows colour dialog; the last row goes back to the chosen theme's colours.
pub(crate) unsafe fn colors_page() {
    use windows_sys::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
    const PCT: [u8; 5] = [100, 85, 70, 55, 40];
    let colors = [
        t!("Фон", "Background", ja: "背景", pt: "Fundo", zh: "背景"),
        t!("Линии и рамки", "Lines and borders", ja: "線と枠", pt: "Linhas e bordas", zh: "线条与边框"),
        t!("Наведение", "Hover", ja: "ホバー", pt: "Passar o mouse", zh: "悬停"),
        t!("Нажатие и выбранное", "Pressed and selected", ja: "押下中と選択中", pt: "Pressionado e selecionado", zh: "按下和选中"),
        t!("Акцент", "Accent", ja: "アクセント", pt: "Destaque", zh: "强调色"),
        t!("Текст", "Text", ja: "テキスト", pt: "Texto", zh: "文本"),
        t!("Второстепенный текст", "Secondary text", ja: "補助テキスト", pt: "Texto secundário", zh: "次要文本"),
    ];
    let cur = i18n::style::custom_colors().unwrap_or([u32::MAX; 7]);
    let p = i18n::style::pal();
    let now = [p.bg, p.line, p.hover, p.active, p.accent, p.text, p.dim]; // what is shown now: the dialog starts there
    let mut items = vec![back(), fly::MItem::head(t!("Цвета (точка справа: изменён вами)", "Colors (dot on the right: changed by you)", ja: "色 (右の点: 変更済み)", pt: "Cores (ponto à direita: alterada por você)", zh: "颜色（右侧圆点：已由你更改）")).sep()];
    items.extend(colors.iter().enumerate().map(|(i, n)| fly::MItem::new(*n).swatch(now[i]).on(cur[i] != u32::MAX)));
    items.push(fly::MItem::head(t!("Непрозрачность панели", "Bar opacity", ja: "バーの不透明度", pt: "Opacidade da barra", zh: "任务栏不透明度")));
    items.extend(PCT.iter().map(|&v| fly::MItem::new(if v == 100 { t!("100 %, без прозрачности", "100 %, solid", ja: "100 %、不透明", pt: "100 %, sólida", zh: "100 %，不透明").to_string() } else { format!("{v} %") }).on(i18n::style::bar_opacity() == v)));
    items.push(fly::MItem::new(t!("Вернуть цвета темы", "Back to the theme's colors", ja: "テーマの色に戻す", pt: "Voltar às cores do tema", zh: "恢复主题颜色")).danger().sep());
    let (c0, o0) = (2, 3 + colors.len()); // first colour row, first opacity row
    gear_page("\u{E790}", t!("Цвета и прозрачность", "Colors and transparency", ja: "色と透明度", pt: "Cores e transparência", zh: "颜色与透明度"), t!("Поверх выбранной темы", "On top of the chosen theme", ja: "選択したテーマの上に適用", pt: "Sobre o tema escolhido", zh: "在所选主题之上"), items, move |i| {
        let bar = cur_bar();
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
        } else if let Some(&v) = i.checked_sub(o0).and_then(|k| PCT.get(k)) {
            i18n::style::set_bar_opacity(v);
        } else {
            i18n::style::reset_custom();
        }
        apply_opacity(bar);
        invalidate_bars();
        colors_page();
    });
}

/// Saved looks (theme + colours + opacity): apply one, save what is on screen, delete.
pub(crate) unsafe fn templates_page() {
    use i18n::style::{ALL, PALS, Theme};
    let list = i18n::style::themes();
    let now = Theme::current("");
    let mut items = vec![back(), fly::MItem::head(t!("Мои шаблоны", "My templates", ja: "マイ テンプレート", pt: "Meus modelos", zh: "我的模板")).sep()];
    if list.is_empty() {
        items.push(fly::MItem::head(t!("Пока пусто: настройте вид и сохраните", "None yet: set up a look and save it", ja: "まだありません: 外観を設定して保存してください", pt: "Nenhum ainda: monte uma aparência e salve", zh: "暂无：设置好外观后保存")));
    }
    let first = items.len(); // index of the first template row
    for th in &list {
        let accent = if th.colors[4] != u32::MAX { th.colors[4] } else { PALS[ALL.iter().position(|&s| s == th.style).unwrap_or(0)].accent };
        let same = (th.style, th.colors, th.opacity) == (now.style, now.colors, now.opacity);
        items.push(fly::MItem::new(th.name.clone()).hint(style_name(th.style)).swatch(accent).on(same));
    }
    let save = items.len();
    items.push(fly::MItem::new(t!("Сохранить текущий вид как шаблон", "Save the current look as a template", ja: "現在の外観をテンプレートとして保存", pt: "Salvar a aparência atual como modelo", zh: "将当前外观保存为模板")).sep());
    if !list.is_empty() {
        items.push(fly::MItem::new(t!("Удалить шаблон…", "Delete a template…", ja: "テンプレートを削除…", pt: "Excluir um modelo…", zh: "删除模板…")).danger());
    }
    gear_page("\u{E8F1}", t!("Шаблоны", "Templates", ja: "テンプレート", pt: "Modelos", zh: "模板"), t!("Свои наборы на любое настроение", "Your own looks for any mood", ja: "気分に合わせたオリジナルの外観", pt: "Suas próprias aparências para qualquer humor", zh: "随心保存自己的外观"), items, move |i| {
        let mut list = list;
        if i == 0 {
            return style_menu();
        } else if (first..save).contains(&i) {
            list[i - first].apply();
            relook(false);
        } else if i == save {
            let name = |n: usize| tf!("Шаблон {}", "Template {}", ja: "テンプレート {}", pt: "Modelo {}", zh: "模板 {}", n);
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
    gear_page("\u{E74D}", t!("Удалить шаблон", "Delete a template", ja: "テンプレートを削除", pt: "Excluir um modelo", zh: "删除模板"), t!("Нажмите на шаблон, чтобы удалить", "Click a template to delete it", ja: "削除するテンプレートをクリック", pt: "Clique em um modelo para excluí-lo", zh: "点击模板即可删除"), items, move |i| {
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
    let items = vec![fly::MItem::new(t!("Да, сбросить", "Yes, reset", ja: "はい、リセットします", pt: "Sim, restaurar", zh: "是，重置")).danger(), fly::MItem::new(t!("Отмена", "Cancel", ja: "キャンセル", pt: "Cancelar", zh: "取消"))];
    gear_page("\u{E777}", t!("Сбросить всё?", "Reset everything?", ja: "すべてをリセットしますか?", pt: "Restaurar tudo?", zh: "全部重置？"), t!("Тема, цвета, панель. Шаблоны останутся", "Theme, colors, bar. Templates stay", ja: "テーマ、色、バー。テンプレートは残ります", pt: "Tema, cores, barra. Os modelos permanecem", zh: "主题、颜色、任务栏。模板保留"), items, |i| {
        if i == 0 {
            i18n::style::reset_all();
            relook(true);
        }
        style_menu();
    });
}
