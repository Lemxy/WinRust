//! Network adapters through GetAdaptersAddresses: names, addresses, gateways and DNS.

use std::{net::{Ipv4Addr, Ipv6Addr}, ptr::null};

use windows_sys::Win32::{
    Foundation::*,
    NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
    Networking::WinSock::*,
};

use crate::{
    model::{Action, BtnKind, Button, Row, Target, Tone},
    sys,
};

#[derive(Clone)]
struct Adapter {
    name: String,
    description: String,
    kind: String,
    mac: String,
    ipv4: String,
    ipv6: String,
    gateway: String,
    dns: String,
    up: bool,
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

fn sockaddr_text(addr: SOCKET_ADDRESS) -> Option<String> {
    unsafe {
        let sa = addr.lpSockaddr;
        if sa.is_null() || addr.iSockaddrLength <= 0 {
            return None;
        }
        match (*sa).sa_family {
            AF_INET if addr.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN>() => {
                let sin = &*(sa as *const SOCKADDR_IN);
                // S_addr keeps the four network-order bytes in memory, so native-order bytes rebuild the address.
                Some(Ipv4Addr::from(sin.sin_addr.S_un.S_addr.to_ne_bytes()).to_string())
            }
            AF_INET6 if addr.iSockaddrLength as usize >= std::mem::size_of::<SOCKADDR_IN6>() => {
                let sin6 = &*(sa as *const SOCKADDR_IN6);
                Some(Ipv6Addr::from(sin6.sin6_addr.u.Byte).to_string())
            }
            _ => None,
        }
    }
}

fn if_type_name(t: u32) -> String {
    match t {
        6 => "Ethernet".into(),
        71 => "Wi-Fi".into(),
        23 => "PPP".into(),
        24 => t!("Кольцевой интерфейс", "Loopback", ja: "ループバック", pt: "Loopback", zh: "环回").into(),
        131 => t!("Туннель", "Tunnel", ja: "トンネル", pt: "Túnel", zh: "隧道").into(),
        243 => "Wi-Fi".into(),
        _ => tf!("Тип {}", "Type {}", ja: "種類 {}", pt: "Tipo {}", zh: "类型 {}", t),
    }
}

fn join(first: *mut IP_ADAPTER_UNICAST_ADDRESS_LH, ipv6: bool) -> String {
    let mut out = vec![];
    unsafe {
        let mut p = first;
        while !p.is_null() && out.len() < 3 {
            if let Some(s) = sockaddr_text((*p).Address) {
                let is6 = s.contains(':');
                if is6 == ipv6 && out.len() < 3 {
                    out.push(s);
                }
            }
            p = (*p).Next;
        }
    }
    out.join(", ")
}

fn gateways(first: *mut IP_ADAPTER_GATEWAY_ADDRESS_LH) -> String {
    let mut out = vec![];
    unsafe {
        let mut p = first;
        while !p.is_null() && out.len() < 2 {
            if let Some(s) = sockaddr_text((*p).Address) {
                out.push(s);
            }
            p = (*p).Next;
        }
    }
    out.join(", ")
}

fn dns(first: *mut IP_ADAPTER_DNS_SERVER_ADDRESS_XP) -> String {
    let mut out = vec![];
    unsafe {
        let mut p = first;
        while !p.is_null() && out.len() < 3 {
            if let Some(s) = sockaddr_text((*p).Address) {
                out.push(s);
            }
            p = (*p).Next;
        }
    }
    out.join(", ")
}

fn adapters() -> Result<Vec<Adapter>, u32> {
    unsafe {
        let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_INCLUDE_PREFIX | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
        let mut size = 32 * 1024u32;
        let mut buf = vec![0u64; (size as usize + 7) / 8];
        loop {
            let rc = GetAdaptersAddresses(AF_UNSPEC as u32, flags, null(), buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH, &mut size);
            if rc == ERROR_BUFFER_OVERFLOW {
                buf.resize((size as usize + 7) / 8, 0);
                continue;
            }
            if rc != 0 {
                return Err(rc);
            }
            break;
        }
        let mut out = vec![];
        let mut p = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            let a = &*p;
            let friendly = pwstr(a.FriendlyName);
            let description = pwstr(a.Description);
            let name = if friendly.trim().is_empty() { description.clone() } else { friendly };
            if a.IfType != 24 && !name.trim().is_empty() {
                let len = (a.PhysicalAddressLength as usize).min(a.PhysicalAddress.len());
                let mac = if len == 0 {
                    String::new()
                } else {
                    a.PhysicalAddress[..len].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join("-")
                };
                out.push(Adapter {
                    ipv4: join(a.FirstUnicastAddress, false),
                    ipv6: join(a.FirstUnicastAddress, true),
                    gateway: gateways(a.FirstGatewayAddress),
                    dns: dns(a.FirstDnsServerAddress),
                    kind: if_type_name(a.IfType),
                    up: a.OperStatus == IfOperStatusUp,
                    description,
                    name,
                    mac,
                });
            }
            p = a.Next;
        }
        out.sort_by(|a, b| b.up.cmp(&a.up).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        Ok(out)
    }
}

pub fn rows() -> Vec<Row> {
    let mut rows = vec![Row::entry(
        t!("Открыть параметры сети", "Open network settings", ja: "ネットワーク設定を開く", pt: "Abrir configurações de rede", zh: "打开网络设置"),
        "ms-settings:network",
        "",
        t!("Открыть", "Open", ja: "開く", pt: "Abrir", zh: "打开"),
        Tone::Accent,
        Some(Action::Open(Target::Uri("ms-settings:network"))),
        vec![],
    )];
    match adapters() {
        Ok(list) if list.is_empty() => rows.push(Row::note(
            t!("Сетевые адаптеры не найдены.", "No network adapters were found.", ja: "ネットワーク アダプターが見つかりません。", pt: "Nenhum adaptador de rede foi encontrado.", zh: "未找到网络适配器。"),
            Tone::Warn,
        )),
        Ok(list) => {
            rows.push(Row::note(
                tf!("Адаптеров: {}", "Adapters: {}", ja: "アダプター: {}", pt: "Adaptadores: {}", zh: "适配器：{}", list.len()),
                Tone::Dim,
            ));
            for a in list {
                let mut detail = vec![];
                if !a.ipv4.is_empty() {
                    detail.push(format!("IPv4 {}", a.ipv4));
                }
                if !a.ipv6.is_empty() {
                    detail.push(format!("IPv6 {}", a.ipv6));
                }
                if !a.gateway.is_empty() {
                    detail.push(format!("GW {}", a.gateway));
                }
                if !a.dns.is_empty() {
                    detail.push(format!("DNS {}", a.dns));
                }
                let mut subtitle = a.description.clone();
                if !a.kind.is_empty() {
                    subtitle = format!("{} · {}", a.kind, subtitle);
                }
                if !a.mac.is_empty() {
                    subtitle = format!("{} · {}", subtitle, a.mac);
                }
                let buttons = vec![Button::new(
                    t!("Настройки", "Settings", ja: "設定", pt: "Configurações", zh: "设置"),
                    Action::Open(Target::Uri("ms-settings:network")),
                    BtnKind::Normal,
                )];
                rows.push(Row::entry(
                    a.name,
                    subtitle,
                    detail.join("  ·  "),
                    if a.up { t!("Подключено", "Connected", ja: "接続済み", pt: "Conectado", zh: "已连接").to_string() } else { t!("Отключено", "Disconnected", ja: "切断", pt: "Desconectado", zh: "已断开").to_string() },
                    if a.up { Tone::Good } else { Tone::Dim },
                    None,
                    buttons,
                ));
            }
        }
        Err(e) => rows.push(Row::note(
            tf!("Не удалось прочитать сетевые адаптеры: {}", "Could not read the network adapters: {}", ja: "ネットワーク アダプターを読み取れませんでした: {}", pt: "Não foi possível ler os adaptadores de rede: {}", zh: "无法读取网络适配器：{}", sys::error_text(e)),
            Tone::Bad,
        )),
    }
    rows
}
