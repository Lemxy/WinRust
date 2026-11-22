//! Who the per-user settings belong to. UAC can be answered with another administrator's password: the installer then runs as
//! that account, and its HKCU, `$PROFILE` and scheduled-task user are the wrong ones. The right account is the one whose
//! explorer.exe (or, with Explorer replaced, topbar / rust-explorer) runs in this session; its SID goes to every child in
//! WINRUST_SID, and `out()` aims `HKCU\...` at `HKU\<sid>` while it is set.
use crate::*;
use std::sync::OnceLock;

pub(crate) struct Session {
    pub sid: String,
    /// The elevated token belongs to someone else: only then is WINRUST_SID set, so the usual case keeps using plain HKCU.
    pub other: bool,
}

static SESSION: OnceLock<Result<Session, String>> = OnceLock::new();

const FIND: &str = r#"$s=(Get-Process -Id $PID).SessionId; $o=$null
foreach($n in 'explorer','topbar','rust-explorer','rs-shell'){
  $o=Get-CimInstance Win32_Process -Filter "Name='$n.exe'" | Where-Object { $_.SessionId -eq $s } | ForEach-Object { (Invoke-CimMethod -InputObject $_ -MethodName GetOwnerSid).Sid } | Where-Object { $_ } | Select-Object -First 1
  if($o){break}
}
if(-not $o){ exit 3 }
$o; [Security.Principal.WindowsIdentity]::GetCurrent().User.Value"#;

/// (session user's SID, elevated token's SID) from the two lines the script prints.
pub(crate) fn parse(out: &str) -> Result<(String, String), String> {
    let mut l = out.lines().map(str::trim).filter(|l| !l.is_empty());
    match (l.next(), l.next()) {
        (Some(a), Some(b)) if i18n::reg::valid_sid(a) && i18n::reg::valid_sid(b) => Ok((a.into(), b.into())),
        _ => Err(tf!("не удалось определить пользователя сеанса: {}", "could not determine the signed-in user: {}", ja: "サインイン中のユーザーを特定できませんでした: {}", pt: "não foi possível identificar o usuário conectado: {}", zh: "无法确定登录用户：{}", out.trim())),
    }
}

/// Base64 of UTF-16LE, as `powershell -EncodedCommand` takes it.
fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

/// Runs a PowerShell script and returns its stdout. `-Command <script>` does not survive the quoting a Rust `Command`
/// puts around an argument that contains double quotes: `FIND` is full of them (`"Name='topbar.exe'"`), the child exits
/// with code 3 and prints nothing, and the installer then reports "no running explorer.exe, topbar or rust-explorer"
/// although the shell is right there. `-EncodedCommand` takes the script as Base64 of UTF-16LE, so nothing is re-quoted.
fn ps(script: &str) -> Result<String, String> {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    out("powershell", &["-NoProfile", "-NonInteractive", "-EncodedCommand", &b64(&utf16)])
}

fn detect() -> Result<Session, String> {
    let (sid, token) = parse(&ps(FIND).map_err(|e| {
        tf!("не удалось определить пользователя сеанса (нет запущенного explorer.exe, topbar или rust-explorer): {e}", "could not determine the signed-in user (no running explorer.exe, topbar or rust-explorer): {e}", ja: "サインイン中のユーザーを特定できませんでした (実行中の explorer.exe、topbar、rust-explorer がありません): {e}", pt: "não foi possível identificar o usuário conectado (sem explorer.exe, topbar ou rust-explorer em execução): {e}", zh: "无法确定登录用户（没有正在运行的 explorer.exe、topbar 或 rust-explorer）：{e}")
    })?)?;
    if matches!(sid.as_str(), "S-1-5-18" | "S-1-5-19" | "S-1-5-20") {
        return Err(t!("оболочка сеанса запущена не от обычного пользователя (SYSTEM или служба)", "the session shell does not run as a normal user (SYSTEM or a service)", ja: "セッションのシェルが通常のユーザーとして実行されていません (SYSTEM またはサービス)", pt: "o shell da sessão não roda como um usuário comum (SYSTEM ou um serviço)", zh: "会话外壳未以普通用户身份运行（SYSTEM 或服务）").into());
    }
    Ok(Session { other: sid != token, sid })
}

/// Settles the target user once and, when it is not the elevated account, tells the children. An Err means: change nothing.
pub(crate) fn init() -> Result<&'static Session, String> {
    let s = SESSION.get_or_init(detect).as_ref().map_err(Clone::clone)?;
    if s.other {
        log(&format!("elevated as another account: per-user settings go to {}", s.sid));
        unsafe { env::set_var(i18n::reg::SID_ENV, &s.sid) };
    }
    Ok(s)
}

/// Uninstall must work even when the user cannot be told (a broken session is when it is needed most): then the elevated
/// account's own HKCU is used and the failure becomes a warning.
pub(crate) fn init_soft() {
    if let Err(e) = init() {
        soft(t!("пользователь сеанса не определён, настройки пользователя убираются у текущей учётной записи", "signed-in user unknown, per-user settings are removed from the current account", ja: "サインイン中のユーザーが不明なため、ユーザーごとの設定は現在のアカウントから削除されます", pt: "usuário conectado desconhecido; as configurações por usuário são removidas da conta atual", zh: "登录用户未知，将从当前账户移除用户设置"), Err(e));
    }
}

/// PowerShell: `$u` = the account scheduled tasks are registered for.
pub(crate) fn task_user() -> String {
    match env::var(i18n::reg::SID_ENV).ok().filter(|s| i18n::reg::valid_sid(s)) {
        Some(sid) => format!("$u=(New-Object Security.Principal.SecurityIdentifier '{sid}').Translate([Security.Principal.NTAccount]).Value;"),
        None => "$u=[Security.Principal.WindowsIdentity]::GetCurrent().Name;".into(),
    }
}

/// PowerShell: `$p` = this shell's CurrentUserCurrentHost profile of the target user (Documents may be redirected: the
/// User Shell Folders value of that user's hive says where).
/// Logon task (Limited) for the signed-in user: unlike a HKCU Run value it cannot land in an MSIX package's private registry copy.
pub(crate) fn logon_task(name: &str, exe: &Path) -> bool {
    let task = format!("{} Register-ScheduledTask -TaskName {name} -Force -Action (New-ScheduledTaskAction -Execute '{}') -Trigger (New-ScheduledTaskTrigger -AtLogOn -User $u) -Principal (New-ScheduledTaskPrincipal -UserId $u -LogonType Interactive -RunLevel Limited) -Settings (New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)) | Out-Null", task_user(), exe.display());
    ps(&task).is_ok()
}

pub(crate) fn profile_path() -> String {
    match env::var(i18n::reg::SID_ENV).ok().filter(|s| i18n::reg::valid_sid(s)) {
        Some(sid) => format!(
            r"$h=(Get-ItemProperty 'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}').ProfileImagePath; $d=(Get-ItemProperty 'Registry::HKEY_USERS\{sid}\Software\Microsoft\Windows\CurrentVersion\Explorer\User Shell Folders').Personal; $d=[Environment]::ExpandEnvironmentVariables($d.Replace('%USERPROFILE%',$h)); $p=Join-Path $d ($(if($PSVersionTable.PSEdition -eq 'Core'){{'PowerShell'}}else{{'WindowsPowerShell'}}) + '\Microsoft.PowerShell_profile.ps1');"
        ),
        None => "$p=$PROFILE;".into(),
    }
}
