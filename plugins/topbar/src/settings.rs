//! topbar: settings (split out of main.rs; shares its items through `super::*`).
//!
//! The options themselves live in `settingswin` (one window, a rail of categories). This module keeps the two things
//! around it: the way every "Settings" click gets there, and a short flyout page used only when that window cannot be
//! created at all, so a click never does nothing and the bar keeps working.
use super::*;

/// The way in for every Settings entry point (the bar's context menu, the Start gear, the tray menu): the one window,
/// on the look category. Falls back to the short flyout page if the window cannot be created.
pub(crate) unsafe fn style_menu() {
    style_menu_at(crate::settingswin::LOOK);
}

/// The same, opening on another category (`settingswin::BAR`, `settingswin::START`...).
pub(crate) unsafe fn style_menu_at(cat: u8) {
    if !crate::settingswin::open_at(cat) {
        crate::tlog("settings: the settings window is not available, showing the short bar page instead");
        bar_page();
    }
}

/// One page of the fallback flyout, opened at the gear of the bar the menu belongs to. A page opened from a panel keeps
/// that panel's monitor (the bar's one-second timer may have moved `CURRENT` to another bar meanwhile).
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

/// Bar transparency from the settings window (the window is layered; 100 % = opaque).
pub(crate) unsafe fn apply_opacity(hwnd: HWND) {
    // With the blur on, the transparency lives in the tinted background instead of the whole window: that keeps the
    // icons and the text crisp instead of washing them out.
    let a = if crate::blur::on() { 255 } else { (255 * i18n::style::bar_opacity() as u32 / 100) as u8 };
    SetLayeredWindowAttributes(hwnd, 0, a, LWA_ALPHA);
}

/// The short flyout page: position, size and behaviour of the bar. Only used when `settingswin` is not available.
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
            _ => return crate::fly::close(),
        }
        relook(i != 12);
        bar_page();
    });
}
