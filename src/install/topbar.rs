//! topbar: the bottom bar instead of the taskbar, plus reshell (desktop hotkeys, Win+E/D/I and the bar's watchdog) and,
//! on request, the PowerShell profile block.
use crate::*;
use std::sync::atomic::AtomicU8;

pub(crate) fn bar_dir() -> PathBuf {
    program_files().join("topbar")
}

pub(crate) const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

// topbar.cfg next to topbar.exe (the bar re-reads it every few seconds). Missing file = OneDrive hidden, the bar's own
// default; a fresh install writes the file with OneDrive shown, so nobody loses an icon they did not ask to lose.
pub(crate) fn cfg_path() -> PathBuf {
    bar_dir().join("topbar.cfg")
}

pub(crate) fn od_hidden() -> bool {
    fs::read_to_string(cfg_path()).map_or_else(|_| bar_dir().join("topbar.exe").is_file(), |c| !c.contains("hide_onedrive=0"))
}

/// What the installer applied, so an update keeps it and the window can tell what a switch would change.
pub(crate) const TB_STATE: &str = r"HKLM\SOFTWARE\plugin-launcher\topbar";

/// The PowerShell aliases are in the user's profile now. Installs from before the switch wrote them with every topbar.
pub(crate) fn ps_aliases_now() -> bool {
    bar_dir().join("topbar.exe").is_file() && i18n::reg::get_dword(TB_STATE, "PsProfile").is_none_or(|d| d == 1)
}

/// What the next `bar_install` does with the aliases: 0 keep, 1 leave them out, 2 write them.
static PS_ALIASES: AtomicU8 = AtomicU8::new(0);

/// `None` = keep what is there (an update), which on a fresh install means no aliases.
pub(crate) fn set_ps_aliases(v: Option<bool>) {
    PS_ALIASES.store(v.map_or(0, |s| 1 + s as u8), Relaxed);
}

pub(crate) fn set_od_hidden(v: bool) -> Result<(), String> {
    fs::create_dir_all(bar_dir()).and_then(|_| fs::write(cfg_path(), format!("hide_onedrive={}\n", v as u8))).map_err(|e| e.to_string())
}

// PowerShell profile block (profile.ps1 next to this file: zoxide, grep->rg, ll->eza, cat->bat) that lives exactly as long as topbar is installed.
// Written to Windows PowerShell 5.1 and, if installed, pwsh 7 (each has its own $PROFILE).
// The profile is the signed-in user's (session.rs), also when UAC was answered with another administrator.
pub(crate) fn ps_strip() -> String {
    format!(r"{} $t=[string]$(if(Test-Path $p){{Get-Content $p -Raw}}); $t=[regex]::Replace($t,'(?s)# >>> topbar >>>.*?# <<< topbar <<<\r?\n?','');", profile_path())
}
pub(crate) const PS_BLOCK: &str = include_str!("profile.ps1");
pub(crate) const SHELLS: [&str; 2] = ["powershell", "pwsh"];

pub(crate) fn profile_install() -> Result<(), String> {
    shells_run(&format!("{}$b=@'\n{PS_BLOCK}'@; New-Item -Force -ItemType Directory (Split-Path $p) | Out-Null; Set-Content $p ($t.TrimEnd() + [Environment]::NewLine + $b) -Encoding utf8", ps_strip()))
}

pub(crate) fn profile_uninstall() -> Result<(), String> {
    // delete the file if nothing but our block was in it
    shells_run(&format!("{} if($t.Trim()){{Set-Content $p $t -Encoding utf8}}elseif(Test-Path $p){{Remove-Item $p}}", ps_strip()))
}

/// Runs `script` in every shell. Only the first (always present) can fail the call: a missing pwsh is not an error.
pub(crate) fn shells_run(script: &str) -> Result<(), String> {
    let r = run(SHELLS[0], &["-NoProfile", "-Command", script]);
    let _ = run(SHELLS[1], &["-NoProfile", "-Command", script]);
    r
}

pub(crate) fn bar_install() -> Result<(), String> {
    let had_ps = ps_aliases_now();
    let want_ps = match PS_ALIASES.load(Relaxed) {
        1 => false,
        2 => true,
        _ => had_ps,
    };
    let od = od_hidden(); // before the exe lands: a fresh install shows OneDrive
    with_payload(&["topbar.exe", "rs-du.exe", "reshell.exe"], |d| {
        let dst = bar_dir().join("topbar.exe");
        let _ = run("taskkill", &["/f", "/im", "topbar.exe"]);
        fs::create_dir_all(bar_dir()).map_err(|e| e.to_string())?;
        put(&d.join("topbar.exe"), &dst)?;
        if !cfg_path().is_file() {
            soft("topbar.cfg", set_od_hidden(od));
        }
        fs::create_dir_all(tm_dir()).map_err(|e| e.to_string())?;
        put(&d.join("rs-du.exe"), &tm_dir().join("rs-du.exe"))?; // topbar menu "Чистильщик диска"
        let rsh = bar_dir().join("reshell.exe"); // Win+1/2/3 desktop hotkeys, no args = hotkeys
        let _ = run("taskkill", &["/f", "/im", "reshell.exe"]);
        put(&d.join("reshell.exe"), &rsh)?;
        if logon_task("reshell", &rsh) {
            let _ = run("reg", &["delete", RUN_KEY, "/v", "reshell", "/f"]);
        } else {
            run("reg", &["add", RUN_KEY, "/v", "reshell", "/t", "REG_SZ", "/d", &format!("\"{}\"", rsh.display()), "/f"])?; // fallback
        }
        let _ = winrust_common::launch::program(&rsh.to_string_lossy());
        run(dst.to_str().unwrap(), &["--restore"])?; // clean state in case a killed instance left the taskbar hidden
        // The bar runs elevated (a task with the highest run level starts it at logon without a UAC prompt): that lets it close
        // elevated windows and click elevated apps' tray icons. It starts everything else through explorer, i.e. unelevated.
        // Fallback when the task cannot be made: the plain Run key and a normal-rights bar.
        let task = format!("{} Register-ScheduledTask -TaskName topbar -Force -Action (New-ScheduledTaskAction -Execute '{}') -Trigger (New-ScheduledTaskTrigger -AtLogOn -User $u) -Principal (New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Highest) -Settings (New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)) | Out-Null", task_user(), dst.display());
        if run("powershell", &["-NoProfile", "-Command", &task]).is_ok() && run("schtasks", &["/run", "/tn", "topbar"]).is_ok() {
            let _ = run("reg", &["delete", RUN_KEY, "/v", "topbar", "/f"]);
        } else {
            WARN.lock().unwrap().push(t!("задача topbar не создана: запуск через Run, без прав администратора", "topbar task was not created: it starts via Run, without administrator rights", ja: "topbar タスクを作成できませんでした: 管理者権限なしで Run 経由で起動します", pt: "a tarefa topbar não foi criada: ela inicia pelo Run, sem direitos de administrador", zh: "topbar 计划任务未创建：它将通过 Run 启动，没有管理员权限", es: "no se creó la tarea topbar: se inicia mediante Run, sin permisos de administrador").into());
            run("reg", &["add", RUN_KEY, "/v", "topbar", "/t", "REG_SZ", "/d", &format!("\"{}\"", dst.display()), "/f"])?;
            let _ = winrust_common::launch::program(&dst.to_string_lossy()); // normal user token, like rs-launcher
        }
        // a shell nicety: only on request, and never a reason to fail the bar install
        if want_ps {
            soft(t!("профиль PowerShell не записан", "PowerShell profile was not written", ja: "PowerShell プロファイルを書き込めませんでした", pt: "o perfil do PowerShell não foi gravado", zh: "PowerShell 配置文件未写入", es: "no se escribió el perfil de PowerShell"), profile_install());
        } else if had_ps {
            soft(t!("профиль PowerShell не очищен", "PowerShell profile was not cleaned up", ja: "PowerShell プロファイルを元に戻せませんでした", pt: "o perfil do PowerShell não foi limpo", zh: "PowerShell 配置文件未清理", es: "no se limpió el perfil de PowerShell"), profile_uninstall());
        }
        soft("PsProfile", run("reg", &["add", TB_STATE, "/v", "PsProfile", "/t", "REG_DWORD", "/d", if want_ps { "1" } else { "0" }, "/f"]));
        Ok(())
    })
}

pub(crate) fn bar_uninstall() -> Result<(), String> {
    // rust-explorer as the shell leans on this bar: Explorer takes the session back first, the file manager stays
    if rx_shell_on() {
        shell_off()?;
        WARN.lock().unwrap().push(t!("rust-explorer вернул оболочку Проводнику: без topbar ему нечем её заменить", "rust-explorer handed the shell back to Explorer: without topbar there is nothing to replace it with", ja: "rust-explorer はシェルをエクスプローラーに戻しました: topbar がないと置き換えられません", pt: "o rust-explorer devolveu o shell ao Explorer: sem a topbar não há com o que substituí-lo", zh: "rust-explorer 已把外壳交还给资源管理器：没有 topbar 就无法替换它", es: "rust-explorer devolvió el shell al Explorador: sin la topbar no hay con qué reemplazarlo").into());
    }
    let _ = run("reg", &["delete", RUN_KEY, "/v", "topbar", "/f"]);
    let _ = run("schtasks", &["/delete", "/tn", "topbar", "/f"]);
    let _ = run("reg", &["delete", RUN_KEY, "/v", "reshell", "/f"]);
    let _ = run("schtasks", &["/delete", "/tn", "reshell", "/f"]);
    let _ = run("taskkill", &["/f", "/im", "reshell.exe"]);
    let _ = run("taskkill", &["/f", "/im", "topbar.exe"]);
    let _ = profile_uninstall();
    let r = run(bar_dir().join("topbar.exe").to_str().unwrap(), &["--restore"]); // a killed bar cannot restore the taskbar itself
    let _ = fs::remove_dir_all(bar_dir());
    let _ = run("reg", &["delete", TB_STATE, "/f"]);
    cleaner_release(tm_dir().join("rtaskmgr-win32.exe").is_file());
    r
}
