//! Services through the Service Control Manager, no sc.exe process. Start modes are spelled as `sc config` spells them.
use windows_sys::Win32::System::Services::*;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Opens `name` with `access`, runs `f` and closes both handles. None when the SCM or the service cannot be opened.
fn with<T>(name: &str, access: u32, f: impl FnOnce(SC_HANDLE) -> T) -> Option<T> {
    unsafe {
        let scm = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return None;
        }
        let s = OpenServiceW(scm, w(name).as_ptr(), access);
        let r = (!s.is_null()).then(|| f(s));
        if !s.is_null() {
            CloseServiceHandle(s);
        }
        CloseServiceHandle(scm);
        r
    }
}

/// "auto", "delayed-auto", "demand" or "disabled". None = no such service, or a boot/system driver (never touched).
pub fn start_mode(name: &str) -> Option<&'static str> {
    with(name, SERVICE_QUERY_CONFIG, |s| unsafe {
        let mut need = 0u32;
        QueryServiceConfigW(s, std::ptr::null_mut(), 0, &mut need);
        let mut buf = vec![0u64; need as usize / 8 + 1]; // u64: QUERY_SERVICE_CONFIGW holds pointers
        let cfg = buf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW;
        if QueryServiceConfigW(s, cfg, (buf.len() * 8) as u32, &mut need) == 0 {
            return None;
        }
        Some(match (*cfg).dwStartType {
            SERVICE_AUTO_START => {
                let mut d = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: 0 };
                let ok = QueryServiceConfig2W(s, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, (&mut d as *mut SERVICE_DELAYED_AUTO_START_INFO).cast(), size_of::<SERVICE_DELAYED_AUTO_START_INFO>() as u32, &mut need) != 0;
                if ok && d.fDelayedAutostart != 0 { "delayed-auto" } else { "auto" }
            }
            SERVICE_DEMAND_START => "demand",
            SERVICE_DISABLED => "disabled",
            _ => return None,
        })
    })
    .flatten()
}

/// Sets the start mode (`start_mode`'s words). true on success.
pub fn set_start(name: &str, mode: &str) -> bool {
    let (ty, delayed) = match mode {
        "auto" => (SERVICE_AUTO_START, Some(false)),
        "delayed-auto" => (SERVICE_AUTO_START, Some(true)),
        "demand" => (SERVICE_DEMAND_START, None),
        "disabled" => (SERVICE_DISABLED, None),
        _ => return false,
    };
    with(name, SERVICE_CHANGE_CONFIG, |s| unsafe {
        let n = std::ptr::null();
        let ok = ChangeServiceConfigW(s, SERVICE_NO_CHANGE, ty, SERVICE_NO_CHANGE, n, n, std::ptr::null_mut(), n, n, n, n) != 0;
        ok && delayed.is_none_or(|d| {
            let mut i = SERVICE_DELAYED_AUTO_START_INFO { fDelayedAutostart: d as i32 };
            ChangeServiceConfig2W(s, SERVICE_CONFIG_DELAYED_AUTO_START_INFO, (&mut i as *mut SERVICE_DELAYED_AUTO_START_INFO).cast()) != 0
        })
    })
    .unwrap_or(false)
}

/// Asks a running service to stop and does not wait. true when the request was accepted.
pub fn stop(name: &str) -> bool {
    with(name, SERVICE_STOP, |s| unsafe {
        let mut st: SERVICE_STATUS = std::mem::zeroed();
        ControlService(s, SERVICE_CONTROL_STOP, &mut st) != 0
    })
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_live_services() {
        // the RPC endpoint mapper is always there and always automatic
        assert_eq!(super::start_mode("RpcEptMapper"), Some("auto"));
        assert_eq!(super::start_mode("no-such-service-winrust"), None);
        assert!(!super::set_start("no-such-service-winrust", "demand"));
        assert!(!super::set_start("RpcEptMapper", "sometimes"), "unknown mode is refused before anything is opened");
    }
}
