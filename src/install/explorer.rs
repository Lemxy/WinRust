//! rust-explorer: the file manager that replaces Explorer as folder handler and (with rs-shell) as the shell.
use crate::*;
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, RRF_RT_REG_DWORD,
    RegCloseKey, RegFlushKey, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};

mod dword_snapshot;
use dword_snapshot::{Dword, backup_and_set, restore};

pub(crate) fn rx_dir() -> PathBuf {
    program_files().join("rust-explorer")
}

// ---------- rust-explorer: file manager + Explorer replacement ----------
//
// Install: our files in Program Files, folders / drives opened from any program go to rust-explorer (HKCU class verbs of the
// installing user), rs-shell --overlay starts at sign-in and, with the ReplaceExplorer switch, ends the shell Explorer once topbar
// and the rust-explorer desktop are on screen. AutoRestartShell is set to 0 so Windows does not bring Explorer back.
// Uninstall puts everything back and starts Explorer again. rs-shell itself hands the session back to Explorer if topbar dies.
pub(crate) const RX_STATE: &str = r"HKLM\SOFTWARE\plugin-launcher\rust-explorer";
pub(crate) const WINLOGON_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";
pub(crate) const RX_SWITCH: &str = r"HKCU\Software\rs-shell";
pub(crate) const RX_FOLDERS: [&str; 3] = [r"HKCU\Software\Classes\Directory\shell\open", r"HKCU\Software\Classes\Folder\shell\open", r"HKCU\Software\Classes\Drive\shell\open"];

// The original DWORDs are saved in the installer's HKLM state key before changing them.
const RX_SVCS: [(&str, u32); 5] = [("PcaSvc", 4), ("InventorySvc", 4), ("lfsvc", 4), ("cbdhsvc", 4), ("DoSvc", 3)];
const SPLIT_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Control";
const SPLIT_VALUE: &str = "SvcHostSplitThresholdInKB";

/// The Windows pieces WinRust replaces, blocked with an IFEO "cmd.exe /c exit" once rs-shell is the shell (the same
/// mechanism as taskmgr.exe). SearchHost belongs to rust-search, which owns the same entry; explorer.exe is never
/// blocked, folder windows and URL hand-offs still go through it. Originals go to the plugin's journal.
const RX_BLOCKS: [&str; 6] = ["StartMenuExperienceHost.exe", "SearchApp.exe", "SearchUI.exe", "Widgets.exe", "Cortana.exe", "ShellExperienceHost.exe"];
const IFEO_KEY: &str = r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options";

fn rx_journal() -> Result<Journal, String> {
    // An install from before the journal let tools\shell-phase2.ps1 block these; that script owns their restore, so the
    // legacy list is empty and uninstall only takes back what this installer wrote.
    Journal::open("rust-explorer", || vec![])
}

fn trim_settings() -> impl Iterator<Item = (&'static str, String, &'static str, u32)> {
    RX_SVCS.into_iter()
        .map(|(service, start)| (service, svc_key(service), "Start", start))
        .chain(std::iter::once((SPLIT_VALUE, SPLIT_KEY.into(), SPLIT_VALUE, 0x4000000)))
}

fn trim_subkey(key: &str) -> Vec<u16> {
    wide(key.strip_prefix(r"HKLM\").expect("service settings live under HKLM"))
}

fn trim_registry_error(key: &str, name: &str, code: u32) -> String {
    format!(r"{key}\{name}: {}", std::io::Error::from_raw_os_error(code as i32))
}

fn trim_read((key, name): Dword<'_>) -> Result<Option<u32>, String> {
    let subkey = trim_subkey(key);
    let value_name = wide(name);
    let mut value = 0u32;
    let mut len = size_of::<u32>() as u32;
    let code = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, subkey.as_ptr(), value_name.as_ptr(), RRF_RT_REG_DWORD,
            null_mut(), (&mut value as *mut u32).cast(), &mut len)
    };
    match code {
        ERROR_SUCCESS if len == size_of::<u32>() as u32 => Ok(Some(value)),
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
        ERROR_SUCCESS => Err(trim_registry_error(key, name, ERROR_INVALID_DATA)),
        _ => Err(trim_registry_error(key, name, code)),
    }
}

fn trim_write((key, name): Dword<'_>, value: u32) -> Result<(), String> {
    let subkey = trim_subkey(key);
    let value_name = wide(name);
    let mut handle = null_mut();
    // Open an existing key so a removed service is not recreated during uninstall.
    let code = unsafe {
        RegOpenKeyExW(HKEY_LOCAL_MACHINE, subkey.as_ptr(), 0, KEY_QUERY_VALUE | KEY_SET_VALUE, &mut handle)
    };
    if code != ERROR_SUCCESS {
        return Err(trim_registry_error(key, name, code));
    }
    let bytes = value.to_le_bytes();
    let mut code = unsafe { RegSetValueExW(handle, value_name.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), bytes.len() as u32) };
    if code == ERROR_SUCCESS && key == RX_STATE {
        // The backup and the service setting are in different hives. Persist the backup first.
        code = unsafe { RegFlushKey(handle) };
    }
    unsafe { RegCloseKey(handle) };
    if code != ERROR_SUCCESS {
        return Err(trim_registry_error(key, name, code));
    }
    if trim_read((key, name))? != Some(value) {
        return Err(tf!("{key}: не удалось подтвердить запись {name}", "{key}: could not verify the write to {name}", ja: "{key}: {name} への書き込みを確認できませんでした", pt: "{key}: não foi possível verificar a gravação em {name}", zh: "{key}：无法确认对 {name} 的写入"));
    }
    Ok(())
}

/// Still restored on uninstall (older versions turned them off), never turned off any more: cbdhsvc is the clipboard history
/// (Win+V), PcaSvc the compatibility assistant older games lean on (#102).
const NO_LONGER_TRIMMED: [&str; 2] = ["cbdhsvc", "PcaSvc"];

fn apply_service_trim() {
    for (backup, key, value, new) in trim_settings().filter(|s| !NO_LONGER_TRIMMED.contains(&s.0)) {
        let result = backup_and_set((&key, value), (RX_STATE, backup), new, trim_read, trim_write);
        if matches!(&result, Ok(true)) && value == "Start" && new == 4 {
            let _ = run("sc.exe", &["stop", backup]);
        }
        soft(backup, result.map(|_| ()));
    }
}

fn restore_service_trim() -> Result<(), String> {
    let mut restored = false;
    let mut errors = Vec::new();
    for (backup, key, value, _) in trim_settings() {
        match restore((&key, value), (RX_STATE, backup), trim_read, trim_write) {
            Ok(saved) => restored |= saved,
            Err(e) => errors.push(e),
        }
    }
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    if !restored && rx_dir().join("rust-explorer.exe").is_file() {
        WARN.lock().unwrap().push(t!(
            "исходные настройки служб не сохранены; текущие значения оставлены как есть",
            "no service settings were backed up; the current values were left in place", ja: "サービス設定のバックアップがありません。現在の値はそのままにしました", pt: "nenhuma configuração de serviço foi salva em backup; os valores atuais foram mantidos", zh: "未备份服务设置；已保留当前值"
        ).into());
    }
    Ok(())
}

/// In our Program Files folder (admin-only): a .reg a user could plant would be imported by the elevated uninstall.
pub(crate) fn rx_folders_bak(i: usize) -> PathBuf {
    rx_dir().join(format!("folders-{i}.reg"))
}

pub(crate) fn rx_lnk() -> PathBuf {
    program_data().join(r"plugin-launcher\rs-overlay.lnk")
}

pub(crate) fn rx_install() -> Result<(), String> {
    if !bar_dir().join("topbar.exe").is_file() {
        return Err(t!("сначала установите topbar: без него после отключения Explorer не будет панели", "install topbar first: without it there will be no bar after Explorer is disabled", ja: "先に topbar をインストールしてください。ないとエクスプローラーを無効にした後にバーがなくなります", pt: "instale a topbar primeiro: sem ela não haverá barra depois que o Explorer for desativado", zh: "请先安装 topbar：否则禁用 Explorer 后将没有任务栏").into());
    }
    // An older installation may already have applied the tweaks without saving the originals.
    let fresh = !rx_dir().join("rust-explorer.exe").is_file()
        && trim_read((RX_STATE, "AutoRestartShell"))?.is_none();
    let running_shell = shell_mode(); // an update in shell mode must never leave the session without one
    let j = rx_journal()?; // before anything changes: uninstall restores the IFEO values from it
    with_payload(&["rust-explorer.exe", "rs-shell.exe", "rs-clip.exe"], |d| {
        fs::create_dir_all(rx_dir()).map_err(|e| e.to_string())?;
        let (fm, sh, clip) = (rx_dir().join("rust-explorer.exe"), rx_dir().join("rs-shell.exe"), rx_dir().join("rs-clip.exe"));
        // A running exe cannot be overwritten, but it can be renamed aside. With rs-shell as the session's shell these
        // three ARE the session (shell, desktop, Win+V helper), so they are swapped in place instead of killed: the old
        // process keeps running until sign-in, the new file is on disk for the next start.
        for n in ["rust-explorer.exe", "rs-shell.exe", "rs-clip.exe"] {
            let dst = if n == "rust-explorer.exe" { &fm } else if n == "rs-shell.exe" { &sh } else { &clip };
            if running_shell && dst.is_file() {
                swap_all(&d.join(n), &[dst.clone()])?;
            } else {
                let _ = run("taskkill", &["/f", "/im", n]); // an update: a running exe cannot be overwritten
                put(&d.join(n), dst)?;
            }
        }
        // The user's shell: HKCU Winlogon\Shell = rs-shell.exe (the supported per-user route; the system's value stays as
        // it is). Saved once; a value that is already ours counts as "Windows had none", so uninstall takes it away.
        if reg_dword(RX_STATE, "ShellHad").is_none() {
            match reg_str(WINLOGON_USER, "Shell").filter(|v| !v.to_lowercase().contains("rs-shell")) {
                Some(v) => {
                    run("reg", &["add", RX_STATE, "/v", "Shell", "/t", "REG_SZ", "/d", &v, "/f"])?;
                    run("reg", &["add", RX_STATE, "/v", "ShellHad", "/t", "REG_DWORD", "/d", "1", "/f"])?;
                }
                None => {
                    let _ = run("reg", &["add", RX_STATE, "/v", "ShellHad", "/t", "REG_DWORD", "/d", "0", "/f"]);
                }
            }
        }
        run("reg", &["add", WINLOGON_USER, "/v", "Shell", "/t", "REG_SZ", "/d", &sh.display().to_string(), "/f"])?;
        // what Windows had, saved once (an update must not overwrite it with our own 0)
        if reg_dword(RX_STATE, "AutoRestartShell").is_none() {
            let v = reg_dword(WINLOGON_KEY, "AutoRestartShell").unwrap_or(1);
            run("reg", &["add", RX_STATE, "/v", "AutoRestartShell", "/t", "REG_DWORD", "/d", &v.to_string(), "/f"])?;
        }
        // folders and drives opened by other programs, the desktop, Win+R: rust-explorer. DelegateExecute is emptied on purpose:
        // while it is set, Windows ignores the command and uses Explorer's built-in handler.
        // Another file manager's verbs (Directory Opus, Total Commander) are exported first, once, and imported back on uninstall.
        for (i, k) in RX_FOLDERS.iter().enumerate() {
            let (bak, ours) = (rx_folders_bak(i), out("reg", &["query", &format!(r"{k}\command"), "/ve"]).is_ok_and(|c| c.to_lowercase().contains("rust-explorer")));
            if !bak.is_file() && !ours && run("reg", &["query", k]).is_ok() {
                run("reg", &["export", k, &bak.display().to_string(), "/y"])?;
            }
        }
        for k in RX_FOLDERS {
            let c = format!(r"{k}\command");
            run("reg", &["add", &c, "/ve", "/t", "REG_SZ", "/d", &format!("\"{}\" \"%1\"", fm.display()), "/f"])?;
            run("reg", &["add", &c, "/v", "DelegateExecute", "/t", "REG_SZ", "/d", "", "/f"])?;
        }
        // One shell only: rs-shell starts the bar, the desktop and the user's Run keys itself. The old overlay task (and
        // the bar's elevated logon tasks) would start a second bar next to the shell, so they go.
        for name in ["rs-overlay", "topbar", "reshell"] {
            let _ = run("schtasks", &["/delete", "/tn", name, "/f"]);
            let _ = run("reg", &["delete", RUN_KEY, "/v", name, "/f"]);
        }
        run("reg", &["add", RX_SWITCH, "/v", "ReplaceExplorer", "/t", "REG_DWORD", "/d", "1", "/f"])?;
        run("reg", &["add", WINLOGON_KEY, "/v", "AutoRestartShell", "/t", "REG_DWORD", "/d", "0", "/f"])?;
        if fresh {
            apply_service_trim();
        }
        // The Windows pieces WinRust replaces cannot start. journaled first, so uninstall puts every original back.
        // A block that is already ours (shell-phase2.ps1, or an earlier install without a journal) is journaled as
        // "delete", never as an original: restoring it would leave Start and Search dead after uninstalling.
        for exe in RX_BLOCKS {
            let key = format!(r"{IFEO_KEY}\{exe}");
            let ours = reg_str(&key, "Debugger").is_some_and(|v| v.trim().eq_ignore_ascii_case("cmd.exe /c exit"));
            soft(exe, if ours { j.reclaim_reg(&key, "Debugger") } else { j.set_reg(&key, "Debugger", "REG_SZ", "cmd.exe /c exit") });
            let _ = run("taskkill", &["/f", "/im", exe]);
        }
        // the bar's Start panel and the search list Start Menu shortcuts only: without one rust-explorer has no entry (and no icon) there
        soft(t!("ярлык «Rust Explorer» в меню Пуск не создан", "Start menu shortcut 'Rust Explorer' was not created", ja: "スタート メニューのショートカット「Rust Explorer」を作成できませんでした", pt: "O atalho 'Rust Explorer' do menu Iniciar não foi criado", zh: "开始菜单快捷方式「Rust Explorer」未创建"), shortcut(&start_lnk("Rust Explorer"), &fm, ""));
        // Explorer is still this session's shell (a fresh install, shell mode starts at the next sign-in): run the overlay
        // now, or the desktop icons would be gone until then. `explorer.exe <shortcut>` starts it with the user's token.
        if !running_shell {
            let lnk = rx_lnk();
            let _ = fs::create_dir_all(lnk.parent().unwrap());
            soft(t!("overlay не запущен сейчас: он стартует при следующем входе", "overlay is not running now: it starts at the next logon", ja: "オーバーレイは現在動作していません: 次回のログオン時に起動します", pt: "o overlay não está em execução agora: ele inicia no próximo logon", zh: "rs-overlay 当前未运行：它会在下次登录时启动"), shortcut(&lnk, &sh, "--overlay"));
            let _ = Command::new("explorer.exe").arg(&lnk).spawn();
        }
        Ok(())
    })
}

pub(crate) fn rx_uninstall() -> Result<(), String> {
    restore_service_trim()?;
    // The IFEO blocks this installer wrote (an older install by shell-phase2.ps1 is that script's to restore).
    rx_journal()?.undo()?;
    // The marker let rs-shell change ms-settings: / ms-screenclip:; --restore puts the previous commands back.
    let sh = rx_dir().join("rs-shell.exe");
    if sh.is_file() {
        let _ = run(sh.to_str().unwrap(), &["--restore"]);
    }
    let _ = fs::remove_file(winrust_data().join("handlers-enabled"));
    // Windows may restart its shell again; our hooks off; then the shell back, unelevated, only if none is running
    let back = reg_dword(RX_STATE, "AutoRestartShell").unwrap_or(1);
    let r = run("reg", &["add", WINLOGON_KEY, "/v", "AutoRestartShell", "/t", "REG_DWORD", "/d", &back.to_string(), "/f"]);
    // this user's shell: the saved value, or no value at all (= Windows starts Explorer) when there was none before
    match reg_dword(RX_STATE, "ShellHad") {
        Some(1) => {
            if let Some(v) = reg_str(RX_STATE, "Shell") {
                let _ = run("reg", &["add", WINLOGON_USER, "/v", "Shell", "/t", "REG_SZ", "/d", &v, "/f"]);
            }
        }
        _ => {
            // no saved state (an install from before this version): only remove a value that is ours
            if reg_str(WINLOGON_USER, "Shell").is_some_and(|v| v.to_lowercase().contains("rs-shell")) {
                let _ = run("reg", &["delete", WINLOGON_USER, "/v", "Shell", "/f"]);
            }
        }
    }
    let _ = run("reg", &["delete", RUN_KEY, "/v", "rs-overlay", "/f"]);
    let _ = run("schtasks", &["/delete", "/tn", "rs-overlay", "/f"]);
    let _ = run("reg", &["delete", RX_SWITCH, "/f"]);
    for (i, k) in RX_FOLDERS.iter().enumerate() {
        let _ = run("reg", &["delete", k, "/f"]); // only our HKCU override; the system's verbs under HKLM stay
        let bak = rx_folders_bak(i);
        if bak.is_file() {
            soft(&tf!("привязка папок {k} не возвращена", "folder verbs {k} were not restored", ja: "フォルダー動詞 {k} を復元できませんでした", pt: "os verbos de pasta {k} não foram restaurados", zh: "文件夹菜单命令 {k} 未恢复"), run("reg", &["import", &bak.display().to_string()]));
        }
    }
    for n in ["rs-shell.exe", "rust-explorer.exe", "rs-clip.exe"] {
        let _ = run("taskkill", &["/f", "/im", n]);
    }
    let _ = run("powershell", &["-NoProfile", "-Command", r#"if (-not (Get-CimInstance Win32_Process -Filter "Name='explorer.exe'" | Where-Object { $_.CommandLine -match '(?i)explorer\.exe"?(\s+/NoUACCheck)?\s*$' })) { runas /trustlevel:0x20000 C:\Windows\explorer.exe }"#]);
    let _ = fs::remove_file(rx_lnk());
    let _ = fs::remove_file(start_lnk("Rust Explorer"));
    let _ = fs::remove_dir_all(rx_dir());
    if r.is_ok() {
        let _ = run("reg", &["delete", RX_STATE, "/f"]);
    }
    r
}
