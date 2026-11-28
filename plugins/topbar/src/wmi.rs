// WMI over COM: what used to be a PowerShell `Get-CimInstance` (~0.5 s and a new process per call).
use windows::{
    Win32::System::{
        Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE},
        Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE},
        Variant::{VARIANT, VT_I4, VT_UI1, VariantClear, VariantToInt32, VariantToStringAlloc},
        Wmi::{IEnumWbemClassObject, IWbemClassObject, IWbemLocator, IWbemServices, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE, WbemLocator},
    },
    core::{BSTR, HSTRING},
};

/// Connects to a namespace (`root\WMI`, `root\cimv2`). The calling thread gets COM (MTA) if it has none yet.
fn connect(ns: &str) -> Option<IWbemServices> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED); // already initialised (any model) is fine
        let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
        let svc = loc.ConnectServer(&BSTR::from(ns), &BSTR::new(), &BSTR::new(), &BSTR::new(), 0, &BSTR::new(), None).ok()?;
        CoSetProxyBlanket(&svc, RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE, None, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, None, EOAC_NONE).ok()?;
        Some(svc)
    }
}

fn first(svc: &IWbemServices, wql: &str) -> Option<IWbemClassObject> {
    unsafe {
        let e: IEnumWbemClassObject = svc.ExecQuery(&BSTR::from("WQL"), &BSTR::from(wql), WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY, None).ok()?;
        let mut objs = [None];
        let mut n = 0;
        let _ = e.Next(WBEM_INFINITE, &mut objs, &mut n);
        objs[0].take()
    }
}

fn get(o: &IWbemClassObject, prop: &str) -> Option<VARIANT> {
    let mut v = VARIANT::default();
    unsafe { o.Get(&HSTRING::from(prop), 0, &mut v, None, None).ok().map(|_| v) }
}

/// One integer property of the first object the query returns.
pub fn int(ns: &str, wql: &str, prop: &str) -> Option<i32> {
    let mut v = get(&first(&connect(ns)?, wql)?, prop)?;
    let r = unsafe { VariantToInt32(&v).ok() };
    unsafe { let _ = VariantClear(&mut v); }
    r
}

/// One string property of the first object the query returns (None when it is NULL, e.g. no access).
pub fn string(ns: &str, wql: &str, prop: &str) -> Option<String> {
    let mut v = get(&first(&connect(ns)?, wql)?, prop)?;
    let r = unsafe { VariantToStringAlloc(&v).ok().map(|p| { let s = p.to_string().unwrap_or_default(); windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _)); s }) };
    unsafe { let _ = VariantClear(&mut v); }
    r.filter(|s| !s.is_empty())
}

fn put(o: &IWbemClassObject, name: &str, vt: windows::Win32::System::Variant::VARENUM, val: i32) -> Option<()> {
    let mut v = VARIANT::default();
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = vt;
        if vt == VT_UI1 { inner.Anonymous.bVal = val as u8 } else { inner.Anonymous.lVal = val }
        o.Put(&HSTRING::from(name), 0, &v, 0).ok()
    }
}

/// WmiMonitorBrightnessMethods.WmiSetBrightness on the first panel. false = no such panel.
pub fn set_brightness(level: u8) -> bool {
    (|| unsafe {
        let svc = connect(r"root\WMI")?;
        let inst = first(&svc, "SELECT * FROM WmiMonitorBrightnessMethods")?;
        let mut path = get(&inst, "__PATH")?;
        let p = VariantToStringAlloc(&path).ok()?;
        let path_s = p.to_string().unwrap_or_default();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
        let _ = VariantClear(&mut path);
        let mut class = None;
        svc.GetObject(&BSTR::from("WmiMonitorBrightnessMethods"), Default::default(), None, Some(&mut class), None).ok()?;
        let mut sig = None;
        class?.GetMethod(&HSTRING::from("WmiSetBrightness"), 0, &mut sig, std::ptr::null_mut()).ok()?;
        let args = sig?.SpawnInstance(0).ok()?;
        put(&args, "Timeout", VT_I4, 1)?;
        put(&args, "Brightness", VT_UI1, level as i32)?;
        svc.ExecMethod(&BSTR::from(path_s), &BSTR::from("WmiSetBrightness"), Default::default(), None, &args, None, None).ok()
    })()
    .is_some()
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_live_wmi() {
        assert!(super::int(r"root\cimv2", "SELECT NumberOfLogicalProcessors FROM Win32_ComputerSystem", "NumberOfLogicalProcessors").unwrap() > 0);
        let me = std::process::id();
        let cl = super::string(r"root\cimv2", &format!("SELECT CommandLine FROM Win32_Process WHERE ProcessId={me}"), "CommandLine").unwrap();
        assert!(cl.to_lowercase().contains("topbar"), "{cl}");
        assert_eq!(super::string(r"root\cimv2", "SELECT CommandLine FROM Win32_Process WHERE ProcessId=4294967295", "CommandLine"), None);
    }
}
