//! Installed software from the classic Uninstall keys, with a real uninstall button.

use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

use crate::{
    model::{col, Action, BtnKind, Button, Row, Tone},
    sys,
};

struct Program {
    name: String,
    publisher: String,
    version: String,
    size_kb: u64,
    date: String,
    uninstall: String,
}

fn key_roots() -> [(&'static str, windows_sys::Win32::System::Registry::HKEY); 4] {
    [
        (r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", HKEY_LOCAL_MACHINE),
        (r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall", HKEY_LOCAL_MACHINE),
        (r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", HKEY_CURRENT_USER),
        (r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall", HKEY_CURRENT_USER),
    ]
}

fn programs() -> Vec<Program> {
    let mut out: Vec<Program> = vec![];
    for (path, root) in key_roots() {
        for sub in sys::subkeys(root, path) {
            let key = format!(r"{path}\{sub}");
            if sys::dword(root, &key, "SystemComponent") == Some(1) || sys::string_long(root, &key, "ParentKeyName").is_some() {
                continue;
            }
            let Some(name) = sys::string_long(root, &key, "DisplayName").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) else {
                continue;
            };
            if out.iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let uninstall = sys::string_long(root, &key, "UninstallString")
                .filter(|s| !s.trim().is_empty())
                .or_else(|| sys::string_long(root, &key, "QuietUninstallString").filter(|s| !s.trim().is_empty()))
                .unwrap_or_default();
            out.push(Program {
                publisher: sys::string_long(root, &key, "Publisher").unwrap_or_default(),
                version: sys::string_long(root, &key, "DisplayVersion").unwrap_or_default(),
                size_kb: sys::dword(root, &key, "EstimatedSize").unwrap_or(0) as u64,
                date: sys::string_long(root, &key, "InstallDate").unwrap_or_default(),
                uninstall,
                name,
            });
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

pub fn rows() -> Vec<Row> {
    let list = programs();
    let mut rows = vec![Row::note(
        tf!("Установленных программ: {}", "Installed programs: {}", ja: "インストール済みのプログラム: {}", pt: "Programas instalados: {}", zh: "已安装程序：{}", list.len()),
        Tone::Dim,
    )];
    if !sys::is_admin() {
        rows.push(Row::note(
            t!("Некоторые программы запросят права администратора при удалении.", "Some uninstallers will ask for administrator rights.", ja: "一部のアンインストーラーは管理者権限を要求します。", pt: "Alguns desinstaladores solicitarão direitos de administrador.", zh: "部分卸载程序会要求管理员权限。"),
            Tone::Dim,
        ));
    }
    if list.is_empty() {
        rows.push(Row::note(
            t!("Список программ пуст или недоступен.", "The program list is empty or could not be read.", ja: "プログラム一覧が空か、読み取れませんでした。", pt: "A lista de programas está vazia ou não pôde ser lida.", zh: "程序列表为空或无法读取。"),
            Tone::Warn,
        ));
        return rows;
    }
    let head = || {
        vec![
            col(t!("Название", "Name", ja: "名前", pt: "Nome", zh: "名称"), 4, Tone::Dim),
            col(t!("Издатель", "Publisher", ja: "発行元", pt: "Editor", zh: "发布者"), 3, Tone::Dim),
            col(t!("Версия", "Version", ja: "バージョン", pt: "Versão", zh: "版本"), 2, Tone::Dim),
            col(t!("Размер", "Size", ja: "サイズ", pt: "Tamanho", zh: "大小"), 2, Tone::Dim),
            col(t!("Дата", "Date", ja: "日付", pt: "Data", zh: "日期"), 2, Tone::Dim),
        ]
    };
    rows.push(Row::head(head(), 104));
    for p in list {
        let mut buttons = vec![];
        if !p.uninstall.is_empty() {
            buttons.push(Button::new(
                t!("Удалить", "Uninstall", ja: "アンインストール", pt: "Desinstalar", zh: "卸载"),
                Action::Uninstall { name: p.name.clone(), command: p.uninstall.clone() },
                BtnKind::Danger,
            ));
        }
        let cols = vec![
            col(p.name, 4, Tone::Neutral),
            col(if p.publisher.is_empty() { "—".into() } else { p.publisher }, 3, Tone::Dim),
            col(if p.version.is_empty() { "—".into() } else { p.version }, 2, Tone::Dim),
            col(if p.size_kb == 0 { "—".into() } else { sys::format_kb(p.size_kb) }, 2, Tone::Dim),
            col(if p.date.is_empty() { "—".into() } else { sys::format_date(&p.date) }, 2, Tone::Dim),
        ];
        rows.push(Row::table(cols, Tone::Neutral, None, buttons, 104));
    }
    rows
}
