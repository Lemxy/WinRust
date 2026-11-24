//! Present PnP devices through CfgMgr32 (the same device tree Device Manager shows).

use windows::core::PCWSTR;
use windows::Win32::{
    Devices::{
        DeviceAndDriverInstallation::*,
        Properties::{DEVPKEY_Device_Class, DEVPKEY_Device_DeviceDesc, DEVPKEY_Device_FriendlyName, DEVPKEY_Device_Manufacturer},
    },
    Foundation::DEVPROPKEY,
};

use crate::model::{col, Action, Row, Target, Tone};

#[derive(Clone)]
struct Device {
    name: String,
    class: String,
    manufacturer: String,
    status: String,
    tone: Tone,
}

fn prop_string(inst: u32, key: &DEVPROPKEY) -> Option<String> {
    unsafe {
        let mut ty = windows::Win32::Devices::Properties::DEVPROPTYPE(0);
        let mut size = 0u32;
        if CM_Get_DevNode_PropertyW(inst, key, &mut ty, None, &mut size, 0) != CR_SUCCESS || size < 2 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        if CM_Get_DevNode_PropertyW(inst, key, &mut ty, Some(buf.as_mut_ptr()), &mut size, 0) != CR_SUCCESS {
            return None;
        }
        // Only direct UTF-16 strings are used; an indirect string would need SHLoadIndirectString.
        let units: Vec<u16> = buf[..size as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let n = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let s = String::from_utf16_lossy(&units[..n]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

fn ids() -> Result<Vec<String>, u32> {
    unsafe {
        let mut chars = 0u32;
        if CM_Get_Device_ID_List_SizeW(&mut chars, PCWSTR::null(), CM_GETIDLIST_FILTER_PRESENT) != CR_SUCCESS || chars == 0 {
            return Err(1);
        }
        let mut buf = vec![0u16; chars as usize + 2];
        if CM_Get_Device_ID_ListW(PCWSTR::null(), &mut buf, CM_GETIDLIST_FILTER_PRESENT) != CR_SUCCESS {
            return Err(2);
        }
        let mut out = vec![];
        let mut start = 0usize;
        while start < buf.len() {
            if buf[start] == 0 {
                break;
            }
            let Some(end) = buf[start..].iter().position(|&c| c == 0).map(|n| start + n) else {
                break;
            };
            out.push(String::from_utf16_lossy(&buf[start..end]));
            start = end + 1;
        }
        Ok(out)
    }
}

fn devices() -> Result<Vec<Device>, u32> {
    let list = ids()?;
    let mut out = vec![];
    for id in list {
        unsafe {
            let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
            let mut inst = 0u32;
            if CM_Locate_DevNodeW(&mut inst, PCWSTR(wide.as_ptr()), CM_LOCATE_DEVNODE_NORMAL) != CR_SUCCESS {
                continue;
            }
            let mut status = CM_DEVNODE_STATUS_FLAGS(0);
            let mut problem = CM_PROB(0);
            let have_status = CM_Get_DevNode_Status(&mut status, &mut problem, inst, 0) == CR_SUCCESS;
            let (status_text, tone) = if have_status && problem.0 != 0 {
                if problem.0 == CM_PROB_DISABLED.0 {
                    (t!("Отключено", "Disabled", ja: "無効", pt: "Desativado", zh: "已禁用").to_string(), Tone::Warn)
                } else {
                    (format!("{} {}", t!("Проблема", "Problem", ja: "問題", pt: "Problema", zh: "问题"), problem.0), Tone::Bad)
                }
            } else if have_status && status.contains(DN_STARTED) {
                (t!("Работает", "Running", ja: "実行中", pt: "Em execução", zh: "正在运行").to_string(), Tone::Good)
            } else if have_status && status.contains(DN_DRIVER_LOADED) {
                (t!("Драйвер загружен", "Driver loaded", ja: "ドライバーが読み込まれました", pt: "Driver carregado", zh: "驱动程序已加载").to_string(), Tone::Warn)
            } else {
                (t!("Не запущено", "Not started", ja: "未開始", pt: "Não iniciado", zh: "未启动").to_string(), Tone::Dim)
            };
            let class = prop_string(inst, &DEVPKEY_Device_Class).unwrap_or_default();
            let name = prop_string(inst, &DEVPKEY_Device_FriendlyName)
                .or_else(|| prop_string(inst, &DEVPKEY_Device_DeviceDesc))
                .unwrap_or_else(|| id.clone());
            let manufacturer = prop_string(inst, &DEVPKEY_Device_Manufacturer).unwrap_or_default();
            out.push(Device { name, class, manufacturer, status: status_text, tone });
        }
    }
    out.sort_by(|a, b| a.class.to_lowercase().cmp(&b.class.to_lowercase()).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

pub fn rows() -> Vec<Row> {
    let mut rows = vec![Row::entry(
        t!("Открыть диспетчер устройств", "Open Device Manager", ja: "デバイス マネージャーを開く", pt: "Abrir o Gerenciador de Dispositivos", zh: "打开设备管理器"),
        "devmgmt.msc",
        "",
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
        Tone::Accent,
        Some(Action::Open(Target::File("devmgmt.msc"))),
        vec![],
    )];
    match devices() {
        Ok(list) if list.is_empty() => rows.push(Row::note(
            t!("Подключённые устройства не найдены.", "No present devices were found.", ja: "現在のデバイスが見つかりません。", pt: "Nenhum dispositivo presente foi encontrado.", zh: "未找到当前设备。"),
            Tone::Warn,
        )),
        Ok(list) => {
            rows.push(Row::note(
                tf!("Устройств: {}", "Devices: {}", ja: "デバイス: {}", pt: "Dispositivos: {}", zh: "设备：{}", list.len()),
                Tone::Dim,
            ));
            rows.push(Row::head(
                vec![
                    col(t!("Устройство", "Device", ja: "デバイス", pt: "Dispositivo", zh: "设备"), 5, Tone::Dim),
                    col(t!("Класс", "Class", ja: "クラス", pt: "Classe", zh: "类别"), 3, Tone::Dim),
                    col(t!("Состояние", "Status", ja: "状態", pt: "Status", zh: "状态"), 2, Tone::Dim),
                ],
                0,
            ));
            for d in list {
                let class = if d.manufacturer.is_empty() { d.class } else { format!("{} · {}", d.class, d.manufacturer) };
                rows.push(Row::table(
                    vec![col(d.name, 5, Tone::Neutral), col(if class.is_empty() { "—".into() } else { class }, 3, Tone::Dim), col(d.status, 2, d.tone)],
                    Tone::Neutral,
                    None,
                    vec![],
                    0,
                ));
            }
        }
        Err(_) => rows.push(Row::note(
            t!("Не удалось перечислить устройства через CfgMgr32.", "Could not enumerate devices through CfgMgr32.", ja: "CfgMgr32 でデバイスを列挙できませんでした。", pt: "Não foi possível enumerar dispositivos via CfgMgr32.", zh: "无法通过 CfgMgr32 枚举设备。"),
            Tone::Bad,
        )),
    }
    rows
}
