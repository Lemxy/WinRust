//! Windows services through the service control manager: enumeration, start/stop and start type.

use std::ptr::{null, null_mut};

use windows_sys::Win32::{
    Foundation::*,
    System::Services::*,
};

use crate::{
    model::{Action, BtnKind, Button, Row, Tone},
    sys,
};

const SCM_ACCESS: u32 = SC_MANAGER_CONNECT | SC_MANAGER_ENUMERATE_SERVICE;

#[derive(Clone)]
struct Service {
    name: String,
    display: String,
    state: u32,
    start: u32,
}

fn state_text(state: u32) -> String {
    match state {
        SERVICE_STOPPED => t!("Остановлена", "Stopped", ja: "停止", pt: "Parado", zh: "已停止").into(),
        SERVICE_START_PENDING => t!("Запускается", "Starting", ja: "開始中", pt: "Iniciando", zh: "正在启动").into(),
        SERVICE_STOP_PENDING => t!("Останавливается", "Stopping", ja: "停止中", pt: "Parando", zh: "正在停止").into(),
        SERVICE_RUNNING => t!("Работает", "Running", ja: "実行中", pt: "Em execução", zh: "正在运行").into(),
        5 => t!("Продолжается", "Continuing", ja: "再開中", pt: "Continuando", zh: "正在继续").into(),
        6 => t!("Приостанавливается", "Pausing", ja: "一時停止中", pt: "Pausando", zh: "正在暂停").into(),
        SERVICE_PAUSED => t!("Приостановлена", "Paused", ja: "一時停止", pt: "Pausado", zh: "已暂停").into(),
        _ => t!("Неизвестно", "Unknown", ja: "不明", pt: "Desconhecido", zh: "未知").into(),
    }
}

fn state_tone(state: u32) -> Tone {
    match state {
        SERVICE_RUNNING => Tone::Good,
        SERVICE_STOPPED => Tone::Dim,
        SERVICE_START_PENDING | SERVICE_STOP_PENDING | 5 | 6 => Tone::Warn,
        SERVICE_PAUSED => Tone::Warn,
        _ => Tone::Neutral,
    }
}

pub fn start_type_text(start: u32) -> String {
    match start {
        0 => t!("Загрузка", "Boot", ja: "ブート", pt: "Inicialização", zh: "引导").into(),
        1 => t!("Система", "System", ja: "システム", pt: "Sistema", zh: "系统").into(),
        SERVICE_AUTO_START => t!("Автоматически", "Automatic", ja: "自動", pt: "Automático", zh: "自动").into(),
        SERVICE_DEMAND_START => t!("Вручную", "Manual", ja: "手動", pt: "Manual", zh: "手动").into(),
        SERVICE_DISABLED => t!("Отключена", "Disabled", ja: "無効", pt: "Desativado", zh: "已禁用").into(),
        _ => t!("Неизвестно", "Unknown", ja: "不明", pt: "Desconhecido", zh: "未知").into(),
    }
}

fn query_start(scm: SC_HANDLE, name: &str) -> u32 {
    unsafe {
        let h = OpenServiceW(scm, sys::wide(name).as_ptr(), SERVICE_QUERY_CONFIG);
        if h.is_null() {
            return u32::MAX;
        }
        let mut buf = vec![0u64; 512]; // 4 KiB: enough for long binary paths and dependency lists
        let mut needed = 0u32;
        let ok = QueryServiceConfigW(h, buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW, (buf.len() * 8) as u32, &mut needed);
        let start = if ok != 0 { unsafe { (*(buf.as_ptr() as *const QUERY_SERVICE_CONFIGW)).dwStartType } } else { u32::MAX };
        CloseServiceHandle(h);
        start
    }
}

fn services() -> Result<Vec<Service>, u32> {
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SCM_ACCESS);
        if scm.is_null() {
            return Err(GetLastError());
        }
        let mut buf = vec![0u64; 64 * 1024]; // 512 KiB, aligned for ENUM_SERVICE_STATUS_PROCESSW
        let mut needed = 0u32;
        let mut returned = 0u32;
        let mut resume = 0u32;
        let mut ok = EnumServicesStatusExW(
            scm,
            SC_ENUM_PROCESS_INFO,
            SERVICE_WIN32,
            SERVICE_STATE_ALL,
            buf.as_mut_ptr() as *mut u8,
            (buf.len() * 8) as u32,
            &mut needed,
            &mut returned,
            &mut resume,
            null(),
        );
        if ok == 0 {
            let e = GetLastError();
            if e == ERROR_MORE_DATA && needed > 0 {
                let words = (needed as usize + 7) / 8;
                buf.resize(words + 16, 0);
                resume = 0;
                ok = EnumServicesStatusExW(
                    scm,
                    SC_ENUM_PROCESS_INFO,
                    SERVICE_WIN32,
                    SERVICE_STATE_ALL,
                    buf.as_mut_ptr() as *mut u8,
                    (buf.len() * 8) as u32,
                    &mut needed,
                    &mut returned,
                    &mut resume,
                    null(),
                );
            }
            if ok == 0 {
                let e = GetLastError();
                CloseServiceHandle(scm);
                return Err(e);
            }
        }
        let mut out = vec![];
        if returned > 0 {
            let list = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, returned as usize);
            for item in list {
                let name = pwstr(item.lpServiceName);
                let display = pwstr(item.lpDisplayName);
                let state = item.ServiceStatusProcess.dwCurrentState;
                out.push(Service {
                    start: query_start(scm, &name),
                    display: if display.is_empty() { name.clone() } else { display },
                    name,
                    state,
                });
            }
        }
        CloseServiceHandle(scm);
        out.sort_by_key(|s| s.display.to_lowercase());
        Ok(out)
    }
}

fn pwstr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let mut n = 0usize;
        while *p.add(n) != 0 && n < 4096 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

pub fn rows() -> Vec<Row> {
    let mut rows = vec![];
    match services() {
        Ok(list) if list.is_empty() => {
            rows.push(Row::note(
                t!("Службы Windows не найдены.", "No Windows services were found.", ja: "Windows サービスが見つかりません。", pt: "Nenhum serviço do Windows foi encontrado.", zh: "未找到 Windows 服务。"),
                Tone::Warn,
            ));
            return rows;
        }
        Ok(list) => {
            rows.push(Row::note(
                tf!("Служб: {} · для изменения нужны права администратора", "Services: {} · changing one needs administrator rights", ja: "サービス: {} · 変更には管理者権限が必要です", pt: "Serviços: {} · alterar exige direitos de administrador", zh: "服务：{} · 更改需要管理员权限", list.len()),
                Tone::Dim,
            ));
            for s in list {
                let mut buttons = vec![];
                if s.state == SERVICE_RUNNING {
                    buttons.push(Button::new(
                        t!("Остановить", "Stop", ja: "停止", pt: "Parar", zh: "停止"),
                        Action::ServiceStop { name: s.name.clone(), label: s.display.clone() },
                        BtnKind::Danger,
                    ));
                } else if s.state == SERVICE_STOPPED {
                    buttons.push(Button::new(
                        t!("Запустить", "Start", ja: "開始", pt: "Iniciar", zh: "启动"),
                        Action::ServiceStart { name: s.name.clone(), label: s.display.clone() },
                        BtnKind::Accent,
                    ));
                }
                buttons.push(Button::new(
                    tf!("Запуск: {}", "Startup: {}", ja: "スタートアップ: {}", pt: "Inicialização: {}", zh: "启动类型：{}", start_type_text(s.start)),
                    Action::ServiceStartupMenu { name: s.name.clone(), label: s.display.clone(), current: s.start },
                    BtnKind::Normal,
                ));
                let subtitle = format!("{} · {}", s.name, start_type_text(s.start));
                rows.push(Row::entry(s.display, subtitle, "", state_text(s.state), state_tone(s.state), None, buttons));
            }
        }
        Err(e) => rows.push(Row::note(
            tf!("Не удалось открыть диспетчер служб: {}", "Could not open the service manager: {}", ja: "サービス マネージャーを開けませんでした: {}", pt: "Não foi possível abrir o gerenciador de serviços: {}", zh: "无法打开服务管理器：{}", sys::error_text(e)),
            Tone::Bad,
        )),
    }
    rows
}

fn scm() -> Result<SC_HANDLE, u32> {
    let h = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    if h.is_null() {
        Err(unsafe { GetLastError() })
    } else {
        Ok(h)
    }
}

pub fn start(name: &str) -> Result<(), u32> {
    unsafe {
        let scm = scm()?;
        let h = OpenServiceW(scm, sys::wide(name).as_ptr(), SERVICE_START | SERVICE_QUERY_STATUS);
        if h.is_null() {
            let e = GetLastError();
            CloseServiceHandle(scm);
            return Err(e);
        }
        let ok = StartServiceW(h, 0, null());
        let e = if ok == 0 { GetLastError() } else { 0 };
        CloseServiceHandle(h);
        CloseServiceHandle(scm);
        if e == 0 { Ok(()) } else { Err(e) }
    }
}

pub fn stop(name: &str) -> Result<(), u32> {
    unsafe {
        let scm = scm()?;
        let h = OpenServiceW(scm, sys::wide(name).as_ptr(), SERVICE_STOP | SERVICE_QUERY_STATUS);
        if h.is_null() {
            let e = GetLastError();
            CloseServiceHandle(scm);
            return Err(e);
        }
        let mut status: SERVICE_STATUS = std::mem::zeroed();
        let ok = ControlService(h, SERVICE_CONTROL_STOP, &mut status);
        let e = if ok == 0 { GetLastError() } else { 0 };
        CloseServiceHandle(h);
        CloseServiceHandle(scm);
        if e == 0 { Ok(()) } else { Err(e) }
    }
}

/// `start` is one of SERVICE_AUTO_START, SERVICE_DEMAND_START or SERVICE_DISABLED.
pub fn set_start(name: &str, start: u32) -> Result<(), u32> {
    unsafe {
        let scm = scm()?;
        let h = OpenServiceW(scm, sys::wide(name).as_ptr(), SERVICE_CHANGE_CONFIG);
        if h.is_null() {
            let e = GetLastError();
            CloseServiceHandle(scm);
            return Err(e);
        }
        let no_change = 0xFFFF_FFFFu32;
        let ok = ChangeServiceConfigW(h, no_change, start, no_change, null(), null(), null_mut(), null(), null(), null(), null());
        let e = if ok == 0 { GetLastError() } else { 0 };
        CloseServiceHandle(h);
        CloseServiceHandle(scm);
        if e == 0 { Ok(()) } else { Err(e) }
    }
}

/// Current state of one service, for the Security page.
pub fn state(name: &str) -> Option<u32> {
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return None;
        }
        let h = OpenServiceW(scm, sys::wide(name).as_ptr(), SERVICE_QUERY_STATUS);
        if h.is_null() {
            CloseServiceHandle(scm);
            return None;
        }
        let mut st: SERVICE_STATUS = std::mem::zeroed();
        let ok = QueryServiceStatus(h, &mut st);
        CloseServiceHandle(h);
        CloseServiceHandle(scm);
        (ok != 0).then_some(st.dwCurrentState)
    }
}
