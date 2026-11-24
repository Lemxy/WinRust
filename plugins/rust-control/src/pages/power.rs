//! Power plans and battery status.

use std::ptr::{null, null_mut};

use windows_sys::{
    core::GUID,
    Win32::{Foundation::*, System::Power::*},
};

use crate::{
    model::{Action, BtnKind, Button, Row, Tone},
    sys,
};

fn same(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn friendly_name(g: &GUID) -> Option<String> {
    unsafe {
        let mut size = 0u32;
        let rc = PowerReadFriendlyName(null_mut(), g, null(), null(), null_mut(), &mut size);
        if rc != ERROR_SUCCESS || size < 2 {
            return None;
        }
        let mut buf = vec![0u8; size as usize + 2];
        let rc = PowerReadFriendlyName(null_mut(), g, null(), null(), buf.as_mut_ptr(), &mut size);
        if rc != ERROR_SUCCESS {
            return None;
        }
        let units: Vec<u16> = buf[..size as usize].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let n = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        let s = String::from_utf16_lossy(&units[..n]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

fn active() -> Option<GUID> {
    unsafe {
        let mut p: *mut GUID = null_mut();
        if PowerGetActiveScheme(null_mut(), &mut p) != ERROR_SUCCESS || p.is_null() {
            return None;
        }
        let g = *p;
        LocalFree(p as *mut core::ffi::c_void);
        Some(g)
    }
}

pub fn plans() -> Result<(Vec<(GUID, String)>, Option<GUID>), u32> {
    let active = active();
    let mut out = vec![];
    unsafe {
        let mut i = 0u32;
        loop {
            let mut g = GUID::default();
            let mut size = std::mem::size_of::<GUID>() as u32;
            let rc = PowerEnumerate(null_mut(), null(), null(), ACCESS_SCHEME, i, &mut g as *mut GUID as *mut u8, &mut size);
            if rc == ERROR_NO_MORE_ITEMS {
                break;
            }
            if rc != ERROR_SUCCESS {
                if i == 0 {
                    return Err(rc);
                }
                break;
            }
            let name = friendly_name(&g).unwrap_or_else(|| sys::guid_text(&g));
            out.push((g, name));
            i += 1;
        }
    }
    out.sort_by_key(|(_, n)| n.to_lowercase());
    Ok((out, active))
}

pub fn set_plan(g: GUID) -> Result<(), u32> {
    let rc = unsafe { PowerSetActiveScheme(null_mut(), &g) };
    if rc == ERROR_SUCCESS { Ok(()) } else { Err(rc) }
}

fn battery() -> Option<SYSTEM_POWER_STATUS> {
    unsafe {
        let mut s: SYSTEM_POWER_STATUS = std::mem::zeroed();
        (GetSystemPowerStatus(&mut s) != 0).then_some(s)
    }
}

pub fn rows() -> Vec<Row> {
    let mut rows = vec![Row::section(t!("Батарея", "Battery", ja: "バッテリー", pt: "Bateria", zh: "电池"))];
    match battery() {
        Some(s) if s.BatteryFlag != 128 && s.BatteryFlag != 255 => {
            let percent = if s.BatteryLifePercent == 255 { t!("неизвестно", "unknown", ja: "不明", pt: "desconhecido", zh: "未知").to_string() } else { format!("{}%", s.BatteryLifePercent) };
            let line = match s.ACLineStatus {
                1 => t!("Питание от сети", "Plugged in", ja: "電源に接続", pt: "Conectado à energia", zh: "已接通电源").to_string(),
                0 => t!("Работа от батареи", "On battery", ja: "バッテリー駆動", pt: "Na bateria", zh: "使用电池").to_string(),
                _ => t!("Питание неизвестно", "Power source unknown", ja: "電源が不明", pt: "Fonte de energia desconhecida", zh: "电源未知").to_string(),
            };
            let charging = (s.BatteryFlag & 8) != 0;
            rows.push(Row::info(t!("Источник питания", "Power source", ja: "電源", pt: "Fonte de energia", zh: "电源"), line, Tone::Neutral));
            rows.push(Row::info(t!("Заряд", "Charge", ja: "充電", pt: "Carga", zh: "电量"), format!("{percent}{}", if charging { format!(" · {}", t!("заряжается", "charging", ja: "充電中", pt: "carregando", zh: "正在充电")) } else { String::new() }), if s.BatteryLifePercent != 255 && s.BatteryLifePercent <= 20 { Tone::Warn } else { Tone::Good }));
            if s.BatteryLifeTime != u32::MAX {
                rows.push(Row::info(t!("Осталось", "Time left", ja: "残り時間", pt: "Tempo restante", zh: "剩余时间"), sys::duration_text(s.BatteryLifeTime as u64), Tone::Dim));
            }
        }
        Some(_) => rows.push(Row::info(t!("Батарея", "Battery", ja: "バッテリー", pt: "Bateria", zh: "电池"), t!("нет батареи (настольный компьютер)", "no battery (desktop computer)", ja: "バッテリーなし (デスクトップ)", pt: "sem bateria (computador desktop)", zh: "无电池（台式机）"), Tone::Dim)),
        None => rows.push(Row::note(t!("Не удалось прочитать состояние батареи.", "Could not read the battery status.", ja: "バッテリーの状態を読み取れませんでした。", pt: "Não foi possível ler o status da bateria.", zh: "无法读取电池状态。"), Tone::Warn)),
    }
    rows.push(Row::section(t!("Схемы питания", "Power plans", ja: "電源プラン", pt: "Planos de energia", zh: "电源计划")));
    match plans() {
        Ok((list, active)) if list.is_empty() => rows.push(Row::note(t!("Схемы питания не найдены.", "No power plans were found.", ja: "電源プランが見つかりません。", pt: "Nenhum plano de energia foi encontrado.", zh: "未找到电源计划。"), Tone::Warn)),
        Ok((list, active)) => {
            for (guid, name) in list {
                let is_active = active.as_ref().is_some_and(|a| same(a, &guid));
                let buttons = if is_active {
                    vec![]
                } else {
                    vec![Button::new(
                        t!("Включить", "Activate", ja: "有効化", pt: "Ativar", zh: "启用"),
                        Action::SetPlan { guid, label: name.clone() },
                        BtnKind::Normal,
                    )]
                };
                rows.push(Row::entry(
                    name,
                    sys::guid_text(&guid),
                    "",
                    if is_active { t!("Активна", "Active", ja: "使用中", pt: "Ativo", zh: "活动").to_string() } else { String::new() },
                    if is_active { Tone::Accent } else { Tone::Dim },
                    None,
                    buttons,
                ));
            }
        }
        Err(e) => rows.push(Row::note(
            tf!("Не удалось прочитать схемы питания: {}", "Could not read the power plans: {}", ja: "電源プランを読み取れませんでした: {}", pt: "Não foi possível ler os planos de energia: {}", zh: "无法读取电源计划：{}", sys::error_text(e)),
            Tone::Bad,
        )),
    }
    rows
}
