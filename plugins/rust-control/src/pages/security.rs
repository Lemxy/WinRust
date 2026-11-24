//! Defender and firewall status. Defender is read from the registry and the WinDefend service;
//! the firewall profiles are parsed from `netsh advfirewall show allprofiles`, as requested.

use std::os::windows::process::CommandExt;
use std::process::Command;

use windows_sys::Win32::System::Registry::HKEY_LOCAL_MACHINE;

use crate::{
    model::{Action, Row, Target, Tone},
    pages::services,
    sys,
};

const DEFENDER: &str = r"SOFTWARE\Microsoft\Windows Defender";
const RTP: &str = r"SOFTWARE\Microsoft\Windows Defender\Real-Time Protection";
const FEATURES: &str = r"SOFTWARE\Microsoft\Windows Defender\Features";

fn protection(disable: Option<u32>) -> (String, Tone) {
    match disable {
        Some(1) => (t!("Выключена", "Off", ja: "オフ", pt: "Desativada", zh: "关闭").into(), Tone::Bad),
        Some(0) => (t!("Включена", "On", ja: "オン", pt: "Ativada", zh: "开启").into(), Tone::Good),
        Some(_) => (t!("Неизвестно", "Unknown", ja: "不明", pt: "Desconhecido", zh: "未知").into(), Tone::Warn),
        None => (t!("Недоступно", "Not readable", ja: "読み取れません", pt: "Não legível", zh: "无法读取").into(), Tone::Dim),
    }
}

fn service_line() -> (String, Tone) {
    match services::state("WinDefend") {
        Some(4) => (t!("Работает", "Running", ja: "実行中", pt: "Em execução", zh: "正在运行").into(), Tone::Good),
        Some(1) => (t!("Остановлена", "Stopped", ja: "停止", pt: "Parado", zh: "已停止").into(), Tone::Warn),
        Some(_) => (t!("Неизвестно", "Unknown", ja: "不明", pt: "Desconhecido", zh: "未知").into(), Tone::Warn),
        None => (t!("Служба не найдена", "Service not found", ja: "サービスが見つかりません", pt: "Serviço não encontrado", zh: "未找到服务").into(), Tone::Dim),
    }
}

fn is_off(word: &str) -> bool {
    let w = word.trim().to_ascii_lowercase();
    matches!(w.as_str(), "off" | "disabled" | "desactivar" | "desactivado" | "no")
}

fn is_on(word: &str) -> bool {
    let w = word.trim().to_ascii_lowercase();
    matches!(w.as_str(), "on" | "enabled" | "activar" | "activado" | "si" | "sí")
}

/// The three profile states in the order netsh prints them: domain, private, public.
fn firewall() -> Result<Vec<(String, bool)>, String> {
    let out = Command::new("netsh")
        .args(["advfirewall", "show", "allprofiles"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(tf!("netsh завершился с кодом {}", "netsh exited with {}", ja: "netsh が終了しました（コード {}）", pt: "netsh terminou com o código {}", zh: "netsh 已退出，代码 {}", out.status.code().unwrap_or(-1)));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut states = vec![];
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(first) = trimmed.split_whitespace().next() else { continue };
        if !first.eq_ignore_ascii_case("state") && !first.eq_ignore_ascii_case("estado") {
            continue;
        }
        let Some(last) = trimmed.split_whitespace().last() else { continue };
        if is_off(last) {
            states.push(false);
        } else if is_on(last) {
            states.push(true);
        }
    }
    if states.len() < 3 {
        return Err(t!("не удалось разобрать вывод netsh", "could not parse the netsh output", ja: "netsh の出力を解析できませんでした", pt: "não foi possível analisar a saída do netsh", zh: "无法解析 netsh 输出").into());
    }
    let names = [
        t!("Домен", "Domain", ja: "ドメイン", pt: "Domínio", zh: "域"),
        t!("Частный", "Private", ja: "プライベート", pt: "Privado", zh: "专用"),
        t!("Общий", "Public", ja: "パブリック", pt: "Público", zh: "公用"),
    ];
    Ok(states.into_iter().take(3).enumerate().map(|(i, on)| (names[i].to_string(), on)).collect())
}

pub fn rows() -> Vec<Row> {
    let anti = sys::dword(HKEY_LOCAL_MACHINE, DEFENDER, "DisableAntiSpyware");
    let rtp = protection(sys::dword(HKEY_LOCAL_MACHINE, RTP, "DisableRealtimeMonitoring"));
    let behavior = protection(sys::dword(HKEY_LOCAL_MACHINE, RTP, "DisableBehaviorMonitoring"));
    let script = protection(sys::dword(HKEY_LOCAL_MACHINE, RTP, "DisableScriptScanning"));
    let tamper_raw = sys::dword(HKEY_LOCAL_MACHINE, FEATURES, "TamperProtection").or_else(|| sys::dword(HKEY_LOCAL_MACHINE, DEFENDER, "TamperProtection"));
    let tamper = match tamper_raw {
        Some(0) => (t!("Выключена", "Off", ja: "オフ", pt: "Desativada", zh: "关闭").to_string(), Tone::Warn),
        Some(_) => (t!("Включена", "On", ja: "オン", pt: "Ativada", zh: "开启").to_string(), Tone::Good),
        None => (t!("Недоступно", "Not readable", ja: "読み取れません", pt: "Não legível", zh: "无法读取").to_string(), Tone::Dim),
    };
    let anti_text = match anti {
        Some(1) => (t!("Антивирус отключён", "Antivirus disabled", ja: "ウイルス対策が無効", pt: "Antivírus desativado", zh: "防病毒已禁用").to_string(), Tone::Bad),
        Some(0) => (t!("Включён", "Enabled", ja: "有効", pt: "Ativado", zh: "已启用").to_string(), Tone::Good),
        _ => (t!("Недоступно", "Not readable", ja: "読み取れません", pt: "Não legível", zh: "无法读取").to_string(), Tone::Dim),
    };
    let svc = service_line();
    let (anti_s, anti_t) = anti_text;
    let (rtp_s, rtp_t) = rtp;
    let (behavior_s, behavior_t) = behavior;
    let (script_s, script_t) = script;
    let (tamper_s, tamper_t) = tamper;
    let (svc_s, svc_t) = svc;
    let mut rows = vec![
        Row::section("Microsoft Defender"),
        Row::info(t!("Антивирусная защита", "Antivirus protection", ja: "ウイルス対策", pt: "Proteção antivírus", zh: "防病毒保护"), anti_s, anti_t),
        Row::info(t!("Защита в реальном времени", "Real-time protection", ja: "リアルタイム保護", pt: "Proteção em tempo real", zh: "实时保护"), rtp_s, rtp_t),
        Row::info(t!("Мониторинг поведения", "Behaviour monitoring", ja: "動作の監視", pt: "Monitoramento de comportamento", zh: "行为监控"), behavior_s, behavior_t),
        Row::info(t!("Проверка скриптов", "Script scanning", ja: "スクリプト スキャン", pt: "Verificação de scripts", zh: "脚本扫描"), script_s, script_t),
        Row::info(t!("Защита от подделки", "Tamper protection", ja: "改ざん防止", pt: "Proteção contra adulteração", zh: "篡改防护"), tamper_s, tamper_t),
        Row::info(t!("Служба WinDefend", "WinDefend service", ja: "WinDefend サービス", pt: "Serviço WinDefend", zh: "WinDefend 服务"), svc_s, svc_t),
    ];
    if rtp_t == Tone::Dim && svc_t == Tone::Dim {
        rows.push(Row::note(
            t!("Значения Защитника закрыты (защита от подделки). Откройте Безопасность Windows для точного состояния.", "The Defender values are protected (tamper protection). Open Windows Security for the exact state.", ja: "Defender の値は保護されています (改ざん防止)。正確な状態は Windows セキュリティで確認してください。", pt: "Os valores do Defender estão protegidos (proteção contra adulteração). Abra a Segurança do Windows para o estado exato.", zh: "Defender 的值受保护（篡改防护）。请打开 Windows 安全中心查看准确状态。"),
            Tone::Warn,
        ));
    }
    rows.push(Row::section(t!("Брандмауэр", "Firewall", ja: "ファイアウォール", pt: "Firewall", zh: "防火墙")));
    match firewall() {
        Ok(list) => {
            for (name, on) in list {
                rows.push(Row::info(tf!("Профиль: {}", "Profile: {}", ja: "プロファイル: {}", pt: "Perfil: {}", zh: "配置文件：{}", name), if on { t!("Включён", "On", ja: "オン", pt: "Ativado", zh: "开启") } else { t!("Выключен", "Off", ja: "オフ", pt: "Desativado", zh: "关闭") }.to_string(), if on { Tone::Good } else { Tone::Bad }));
            }
        }
        Err(e) => rows.push(Row::note(tf!("Не удалось прочитать состояние брандмауэра: {}", "Could not read the firewall state: {}", ja: "ファイアウォールの状態を読み取れませんでした: {}", pt: "Não foi possível ler o estado do firewall: {}", zh: "无法读取防火墙状态：{}", e), Tone::Warn)),
    }
    rows.push(Row::entry(
        t!("Открыть Безопасность Windows", "Open Windows Security", ja: "Windows セキュリティを開く", pt: "Abrir a Segurança do Windows", zh: "打开 Windows 安全中心"),
        "windowsdefender:",
        "",
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
        Tone::Accent,
        Some(Action::Open(Target::Uri("windowsdefender:"))),
        vec![],
    ));
    rows.push(Row::entry(
        t!("Брандмауэр Защитника Windows", "Windows Defender Firewall", ja: "Windows Defender ファイアウォール", pt: "Firewall do Windows Defender", zh: "Windows Defender 防火墙"),
        "control.exe /name Microsoft.WindowsFirewall",
        "",
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
        Tone::Neutral,
        Some(Action::Open(Target::Control("Microsoft.WindowsFirewall"))),
        vec![],
    ));
    if !sys::is_admin() {
        rows.push(Row::note(
            t!("Для изменения параметров безопасности нужны права администратора.", "Changing security settings needs administrator rights.", ja: "セキュリティ設定を変更するには管理者権限が必要です。", pt: "Alterar as configurações de segurança requer direitos de administrador.", zh: "更改安全设置需要管理员权限。"),
            Tone::Dim,
        ));
    }
    rows
}
