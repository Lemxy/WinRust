//! Date, time and time zone.

use windows_sys::Win32::{
    Foundation::*,
    System::{Registry::*, SystemInformation::*},
};

use crate::{
    model::{Action, Row, Target, Tone},
    sys,
};

const TZ: &str = r"SYSTEM\CurrentControlSet\Control\TimeZoneInformation";
const W32TIME: &str = r"SYSTEM\CurrentControlSet\Services\W32Time\Parameters";

fn weekday(d: u16) -> String {
    match d {
        0 => t!("воскресенье", "Sunday", ja: "日曜日", pt: "domingo", zh: "星期日").into(),
        1 => t!("понедельник", "Monday", ja: "月曜日", pt: "segunda-feira", zh: "星期一").into(),
        2 => t!("вторник", "Tuesday", ja: "火曜日", pt: "terça-feira", zh: "星期二").into(),
        3 => t!("среда", "Wednesday", ja: "水曜日", pt: "quarta-feira", zh: "星期三").into(),
        4 => t!("четверг", "Thursday", ja: "木曜日", pt: "quinta-feira", zh: "星期四").into(),
        5 => t!("пятница", "Friday", ja: "金曜日", pt: "sexta-feira", zh: "星期五").into(),
        _ => t!("суббота", "Saturday", ja: "土曜日", pt: "sábado", zh: "星期六").into(),
    }
}

fn stamp(st: &SYSTEMTIME) -> String {
    // The date order comes from sys::format_date; the time is always 24 h.
    let date = sys::format_date(&format!("{:04}{:02}{:02}", st.wYear, st.wMonth, st.wDay));
    format!("{date}  {:02}:{:02}:{:02}", st.wHour, st.wMinute, st.wSecond)
}

fn utc_offset() -> String {
    let bias = sys::dword(HKEY_LOCAL_MACHINE, TZ, "Bias").unwrap_or(0) as i32;
    let minutes = -bias;
    let sign = if minutes < 0 { '-' } else { '+' };
    format!("UTC{}{:02}:{:02}", sign, minutes.abs() / 60, minutes.abs() % 60)
}

pub fn rows() -> Vec<Row> {
    let (mut local, mut utc): (SYSTEMTIME, SYSTEMTIME) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    unsafe {
        GetLocalTime(&mut local);
        GetSystemTime(&mut utc);
    }
    let zone = sys::string_long(HKEY_LOCAL_MACHINE, TZ, "TimeZoneKeyName").unwrap_or_else(|| t!("неизвестно", "unknown", ja: "不明", pt: "desconhecido", zh: "未知").into());
    let standard = sys::string_long(HKEY_LOCAL_MACHINE, TZ, "StandardName").unwrap_or_default();
    let daylight = sys::string_long(HKEY_LOCAL_MACHINE, TZ, "DaylightName").unwrap_or_default();
    let ntp = sys::string_long(HKEY_LOCAL_MACHINE, W32TIME, "NtpServer").unwrap_or_default();
    let sync = sys::string_long(HKEY_LOCAL_MACHINE, W32TIME, "Type").unwrap_or_default();

    let mut rows = vec![
        Row::section(t!("Дата и время", "Date and Time", ja: "日付と時刻", pt: "Data e hora", zh: "日期和时间")),
        Row::info(t!("Местное время", "Local time", ja: "現地時刻", pt: "Hora local", zh: "本地时间"), stamp(&local), Tone::Accent),
        Row::info(t!("UTC", "UTC", ja: "UTC", pt: "UTC", zh: "UTC"), stamp(&utc), Tone::Dim),
        Row::info(t!("День недели", "Day of the week", ja: "曜日", pt: "Dia da semana", zh: "星期"), weekday(local.wDayOfWeek), Tone::Neutral),
        Row::section(t!("Часовой пояс", "Time zone", ja: "タイム ゾーン", pt: "Fuso horário", zh: "时区")),
        Row::info(t!("Часовой пояс", "Time zone", ja: "タイム ゾーン", pt: "Fuso horário", zh: "时区"), zone, Tone::Neutral),
        Row::info(t!("Смещение", "Offset", ja: "オフセット", pt: "Deslocamento", zh: "偏移"), utc_offset(), Tone::Dim),
    ];
    if !standard.is_empty() {
        rows.push(Row::info(t!("Стандартное время", "Standard time", ja: "標準時刻", pt: "Horário padrão", zh: "标准时间"), standard, Tone::Dim));
    }
    if !daylight.is_empty() {
        rows.push(Row::info(t!("Летнее время", "Daylight time", ja: "夏時間", pt: "Horário de verão", zh: "夏令时"), daylight, Tone::Dim));
    }
    rows.push(Row::section(t!("Синхронизация", "Synchronisation", ja: "同期", pt: "Sincronização", zh: "同步")));
    rows.push(Row::info(t!("Тип", "Type", ja: "種類", pt: "Tipo", zh: "类型"), if sync.is_empty() { "—".into() } else { sync }, Tone::Dim));
    rows.push(Row::info(t!("Сервер времени", "Time server", ja: "タイム サーバー", pt: "Servidor de hora", zh: "时间服务器"), if ntp.is_empty() { "—".into() } else { ntp }, Tone::Dim));
    rows.push(Row::entry(
        t!("Изменить дату и время", "Change date and time", ja: "日付と時刻の変更", pt: "Alterar data e hora", zh: "更改日期和时间"),
        "ms-settings:dateandtime",
        "",
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
        Tone::Accent,
        Some(Action::Open(Target::Uri("ms-settings:dateandtime"))),
        vec![],
    ));
    rows.push(Row::entry(
        t!("Классические дата и время", "Classic Date and Time", ja: "従来の日付と時刻", pt: "Data e hora clássicas", zh: "经典日期和时间"),
        "timedate.cpl",
        "",
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
        Tone::Neutral,
        Some(Action::Open(Target::File("timedate.cpl"))),
        vec![],
    ));
    rows
}
