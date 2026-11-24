//! System information, all of it read-only.

use windows_sys::Win32::{
    Foundation::*,
    System::{Registry::*, SystemInformation::*},
};

use crate::{
    model::{Row, Tone},
    sys,
};

const NT: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const CPU: &str = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";
const GPU_CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
const VIDEO: &str = r"SYSTEM\CurrentControlSet\Control\Video";
const ENV_SYSTEM: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";
const ENV_USER: &str = r"Environment";

fn computer_name() -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let mut n = buf.len() as u32;
        if GetComputerNameExW(ComputerNameDnsHostname, buf.as_mut_ptr(), &mut n) != 0 {
            let end = (n as usize).min(buf.len());
            String::from_utf16_lossy(&buf[..end]).trim_end_matches('\0').to_string()
        } else {
            std::env::var("COMPUTERNAME").unwrap_or_default()
        }
    }
}

fn architecture() -> String {
    unsafe {
        let mut info: SYSTEM_INFO = std::mem::zeroed();
        GetNativeSystemInfo(&mut info);
        match info.Anonymous.Anonymous.wProcessorArchitecture {
            PROCESSOR_ARCHITECTURE_AMD64 => "x64",
            PROCESSOR_ARCHITECTURE_ARM64 => "ARM64",
            PROCESSOR_ARCHITECTURE_INTEL => "x86",
            PROCESSOR_ARCHITECTURE_ARM => "ARM",
            _ => t!("неизвестно", "unknown", ja: "不明", pt: "desconhecido", zh: "未知"),
        }
        .into()
    }
}

fn memory() -> Option<(u64, u64, u32)> {
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut m) != 0).then_some((m.ullTotalPhys, m.ullAvailPhys, m.dwMemoryLoad))
    }
}

fn gpus() -> Vec<(String, String)> {
    let mut out = vec![];
    for sub in sys::subkeys(HKEY_LOCAL_MACHINE, GPU_CLASS) {
        let key = format!(r"{GPU_CLASS}\{sub}");
        if let Some(name) = sys::string_long(HKEY_LOCAL_MACHINE, &key, "DriverDesc") {
            if !name.trim().is_empty() {
                let version = sys::string_long(HKEY_LOCAL_MACHINE, &key, "DriverVersion").unwrap_or_default();
                out.push((name, version));
            }
        }
    }
    if out.is_empty() {
        // Fallback for drivers that do not register under the display class: Video\{adapter}\0000.
        for adapter in sys::subkeys(HKEY_LOCAL_MACHINE, VIDEO) {
            for instance in sys::subkeys(HKEY_LOCAL_MACHINE, &format!(r"{VIDEO}\{adapter}")) {
                let key = format!(r"{VIDEO}\{adapter}\{instance}");
                if let Some(name) = sys::string_long(HKEY_LOCAL_MACHINE, &key, "DriverDesc") {
                    if !name.trim().is_empty() && !out.iter().any(|(n, _)| *n == name) {
                        let version = sys::string_long(HKEY_LOCAL_MACHINE, &key, "DriverVersion").unwrap_or_default();
                        out.push((name, version));
                    }
                }
            }
        }
    }
    out
}

/// The user's variables first, then the machine's; a user value with the same name wins.
fn environment() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for (root, key) in [(HKEY_CURRENT_USER, ENV_USER), (HKEY_LOCAL_MACHINE, ENV_SYSTEM)] {
        for (name, value) in sys::values(root, key) {
            if !out.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) {
                out.push((name, value));
            }
        }
    }
    out.sort_by_key(|(n, _)| n.to_lowercase());
    out
}

fn os_name(product: &str, build: u32) -> String {
    if build >= 22000 && product.contains("Windows 10") {
        product.replace("Windows 10", "Windows 11")
    } else {
        product.to_string()
    }
}

pub fn rows() -> Vec<Row> {
    let reg = |name: &str| sys::string(HKEY_LOCAL_MACHINE, NT, name).unwrap_or_default();
    let build: u32 = reg("CurrentBuildNumber").parse().unwrap_or_else(|_| reg("CurrentBuild").parse().unwrap_or(0));
    let ubr = sys::dword(HKEY_LOCAL_MACHINE, NT, "UBR").unwrap_or(0);
    let product = reg("ProductName");
    let edition = os_name(&product, build);
    let build_text = if ubr > 0 { format!("{build}.{ubr}") } else { build.to_string() };
    let cpu = sys::string(HKEY_LOCAL_MACHINE, CPU, "ProcessorNameString").unwrap_or_else(|| t!("неизвестно", "unknown", ja: "不明", pt: "desconhecido", zh: "未知").into());
    let logical = unsafe {
        let mut info: SYSTEM_INFO = std::mem::zeroed();
        GetNativeSystemInfo(&mut info);
        info.dwNumberOfProcessors
    };
    let uptime = sys::duration_text(unsafe { GetTickCount64() / 1000 });
    let user = std::env::var("USERNAME").unwrap_or_default();
    let domain = std::env::var("USERDOMAIN").unwrap_or_default();
    let who = if domain.is_empty() { user } else { format!(r"{domain}\{user}") };

    let mut rows = vec![
        Row::section(t!("Компьютер", "Computer", ja: "コンピューター", pt: "Computador", zh: "计算机")),
        Row::info(t!("Имя компьютера", "Computer name", ja: "コンピューター名", pt: "Nome do computador", zh: "计算机名"), computer_name(), Tone::Accent),
        Row::info(t!("Пользователь", "Signed in as", ja: "サインインしているユーザー", pt: "Conectado como", zh: "登录身份"), who, Tone::Neutral),
        Row::section("Windows"),
        Row::info(t!("Издание", "Edition", ja: "エディション", pt: "Edição", zh: "版本"), if edition.is_empty() { "—".to_string() } else { edition }, Tone::Neutral),
        Row::info(t!("Версия", "Version", ja: "バージョン", pt: "Versão", zh: "版本"), {
            let v = reg("DisplayVersion");
            if v.is_empty() { "—".to_string() } else { v }
        }, Tone::Neutral),
        Row::info(t!("Сборка", "Build", ja: "ビルド", pt: "Compilação", zh: "内部版本"), build_text, Tone::Neutral),
        Row::info(t!("Архитектура", "Architecture", ja: "アーキテクチャ", pt: "Arquitetura", zh: "体系结构"), architecture(), Tone::Neutral),
        Row::info(t!("Время работы", "Uptime", ja: "稼働時間", pt: "Tempo de atividade", zh: "运行时间"), uptime, Tone::Neutral),
        Row::section(t!("Процессор и память", "Processor and memory", ja: "プロセッサとメモリ", pt: "Processador e memória", zh: "处理器和内存")),
        Row::info("CPU", cpu, Tone::Neutral),
        Row::info(t!("Логических процессоров", "Logical processors", ja: "論理プロセッサ", pt: "Processadores lógicos", zh: "逻辑处理器"), logical.to_string(), Tone::Neutral),
    ];
    match memory() {
        Some((total, free, load)) => {
            rows.push(Row::info(t!("Оперативная память", "Memory", ja: "メモリ", pt: "Memória", zh: "内存"), format!("{} · {}%", sys::format_size(total), load), Tone::Neutral));
            rows.push(Row::info(t!("Свободно", "Available", ja: "使用可能", pt: "Disponível", zh: "可用"), sys::format_size(free), Tone::Dim));
        }
        None => rows.push(Row::info(t!("Оперативная память", "Memory", ja: "メモリ", pt: "Memória", zh: "内存"), "—".to_string(), Tone::Dim)),
    }
    rows.push(Row::section(t!("Графика", "Graphics", ja: "グラフィックス", pt: "Gráficos", zh: "图形")));
    let gpus = gpus();
    if gpus.is_empty() {
        rows.push(Row::info("GPU", t!("не найдено", "not found", ja: "見つかりません", pt: "não encontrado", zh: "未找到"), Tone::Dim));
    } else {
        for (i, (name, version)) in gpus.into_iter().enumerate() {
            rows.push(Row::info(format!("GPU {}", i + 1), if version.is_empty() { name } else { format!("{name} · {version}") }, Tone::Neutral));
        }
    }
    rows.push(Row::section(t!("Переменные среды", "Environment variables", ja: "環境変数", pt: "Variáveis de ambiente", zh: "环境变量")));
    let vars = environment();
    for (name, value) in vars.iter().take(40) {
        rows.push(Row::info(name.clone(), value.clone(), Tone::Dim));
    }
    if vars.len() > 40 {
        rows.push(Row::note(tf!("Ещё {} переменных не показаны.", "{} more variables are not shown.", ja: "残り {} 個の変数は表示されていません。", pt: "Mais {} variáveis não são mostradas.", zh: "还有 {} 个变量未显示。", vars.len() - 40), Tone::Dim));
    }
    if vars.is_empty() {
        rows.push(Row::note(t!("Переменные среды недоступны.", "Environment variables are not available.", ja: "環境変数を読み取れません。", pt: "As variáveis de ambiente não estão disponíveis.", zh: "环境变量不可用。"), Tone::Warn));
    }
    rows
}
