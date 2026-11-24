//! topbar: settings (split out of main.rs; shares its items through `super::*`).
//!
//! The settings window is a program of its own (`rust-settings`, its WinRust hub): this module only starts it on the right
//! page and announces, to the programs that are already running, that a value changed. There is no settings UI in the bar
//! any more: one implementation, one window.
use super::*;

/// The pages `rust-settings --page` takes (the categories of its rail).
pub(crate) const PAGE_LOOK: &str = "apariencia";
pub(crate) const PAGE_BAR: &str = "barra";
pub(crate) const PAGE_START: &str = "inicio";

/// The bar's context menu: the settings program, on the look page.
pub(crate) unsafe fn style_menu() {
    style_menu_at(PAGE_LOOK);
}

/// The same on another page. A second launch only switches the page of the window that is already open.
pub(crate) unsafe fn style_menu_at(page: &str) {
    if launch_settings(page) {
        return;
    }
    // not installed: say so above the bar instead of doing nothing (the bar keeps working)
    crate::tlog("settings: rust-settings.exe is not installed");
    let msg = t!(
        "Программа настроек (rust-settings) не установлена.\nУстановите её через установщик WinRust.",
        "The settings program (rust-settings) is not installed.\nInstall it with the WinRust installer.",
        ja: "設定プログラム (rust-settings) がインストールされていません。\nWinRust インストーラーでインストールしてください。",
        pt: "O programa de ajustes (rust-settings) não está instalado.\nInstale-o com o instalador do WinRust.",
        zh: "设置程序 (rust-settings) 未安装。\n请使用 WinRust 安装程序安装。"
    );
    show_tip(msg.to_string(), GetSystemMetrics(SM_CXSCREEN) / 2);
    SetTimer(crate::leader_bar(), 6, 3000, None);
}

/// Where the settings program is: beside the bar, in the installer's folder, or in the launcher's bin.
fn settings_exe() -> Option<PathBuf> {
    let mut v = Vec::new();
    if let Ok(e) = std::env::current_exe() {
        v.push(e.with_file_name("rust-settings.exe"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        v.push(PathBuf::from(pf).join(r"rust-settings\rust-settings.exe"));
    }
    if let Some(la) = std::env::var_os("LOCALAPPDATA") {
        v.push(PathBuf::from(la).join(r"plugin-launcher\bin\rust-settings.exe"));
    }
    v.into_iter().find(|p| p.is_file())
}

fn launch_settings(page: &str) -> bool {
    let Some(exe) = settings_exe() else { return false };
    let page = page.to_string();
    // off the bar's thread: a slow start must not freeze the bar
    std::thread::spawn(move || {
        let _ = Command::new(exe).arg("--page").arg(page).creation_flags(0x0800_0000).spawn();
    });
    true
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

/// Bar transparency from the shared look (the window is layered; 100 % = opaque).
pub(crate) unsafe fn apply_opacity(hwnd: HWND) {
    // With the blur on, the transparency lives in the tinted background instead of the whole window: that keeps the
    // icons and the text crisp instead of washing them out.
    let a = if crate::blur::on() { 255 } else { (255 * i18n::style::bar_opacity() as u32 / 100) as u8 };
    SetLayeredWindowAttributes(hwnd, 0, a, LWA_ALPHA);
}
