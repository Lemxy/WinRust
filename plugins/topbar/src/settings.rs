//! topbar: settings (split out of main.rs; shares its items through `super::*`).
//!
//! There is one settings window (`settingswin`) and every entry point goes straight to it: the bar's context menu, the
//! Start gear, the "Start menu" row and the tray clock's menu. This module keeps only the way in and the pieces the hub
//! uses from the bar (the look broadcast and the bar's opacity); the flyout pages that used to hold these options are gone,
//! so nothing opens a second window or an intermediate menu.
use super::*;

/// The bar's context menu: the hub, on the look category.
pub(crate) unsafe fn style_menu() {
    style_menu_at(crate::settingswin::LOOK);
}

/// The same on another category (`settingswin::DESK`, `BAR`, `START`...).
pub(crate) unsafe fn style_menu_at(cat: u8) {
    if !crate::settingswin::open_at(cat) {
        crate::tlog("settings: the settings window is not available");
    }
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
