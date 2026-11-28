//! Mobile hotspot through Windows.Networking.NetworkOperators (no Settings app): on/off, the network name and the password
//! of the shared Wi-Fi. The WinRT calls wait, so they run in a thread and leave their result in `STATE` for the next refresh.
use std::sync::Mutex;
use windows::{
    Networking::{
        Connectivity::NetworkInformation,
        NetworkOperators::{NetworkOperatorTetheringManager, TetheringOperationStatus, TetheringOperationalState},
    },
    core::HSTRING,
};

#[derive(Default, Clone)]
pub struct State {
    pub on: Option<bool>, // None: no internet connection to share (or no Wi-Fi adapter that can host one)
    pub ssid: String,
    pub pass: String,
    pub note: String, // why the last change failed
}

static STATE: Mutex<State> = Mutex::new(State { on: None, ssid: String::new(), pass: String::new(), note: String::new() });

pub fn state() -> State {
    STATE.lock().map(|s| s.clone()).unwrap_or_default()
}

fn manager() -> Option<NetworkOperatorTetheringManager> {
    unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED); }
    let p = NetworkInformation::GetInternetConnectionProfile().ok()?;
    NetworkOperatorTetheringManager::CreateFromConnectionProfile(&p).ok()
}

fn read(note: String) {
    let m = manager();
    let on = m.as_ref().and_then(|m| m.TetheringOperationalState().ok()).and_then(|s| match s {
        TetheringOperationalState::On | TetheringOperationalState::InTransition => Some(true),
        TetheringOperationalState::Off => Some(false),
        _ => None,
    });
    let cfg = m.as_ref().and_then(|m| m.GetCurrentAccessPointConfiguration().ok());
    let get = |f: &dyn Fn() -> windows::core::Result<HSTRING>| f().map(|s| s.to_string()).unwrap_or_default();
    let (ssid, pass) = cfg.map_or_else(Default::default, |c| (get(&|| c.Ssid()), get(&|| c.Passphrase())));
    if let Ok(mut s) = STATE.lock() {
        *s = State { on, ssid, pass, note };
    }
}

/// Reads the state again in a thread, then calls `done`.
pub fn refresh(done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        read(String::new());
        done();
    });
}

pub fn set(on: bool, done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let note = match manager() {
            Some(m) => {
                let r = if on { m.StartTetheringAsync() } else { m.StopTetheringAsync() }.and_then(|a| a.join());
                match r {
                    Ok(r) if r.Status().is_ok_and(|s| s == TetheringOperationStatus::Success) => String::new(),
                    Ok(r) => r.AdditionalErrorMessage().map(|s| s.to_string()).ok().filter(|s| !s.is_empty()).unwrap_or_else(|| tf!("ошибка {}", "error {}", ja: "エラー {}", pt: "erro {}", zh: "错误 {}", r.Status().map_or(-1, |s| s.0))),
                    Err(e) => e.message(),
                }
            }
            None => t!("Нет подключения к интернету для раздачи", "No internet connection to share", ja: "共有できるインターネット接続がありません", pt: "Sem conexão com a internet para compartilhar", zh: "没有可共享的互联网连接").into(),
        };
        read(note);
        done();
    });
}

/// Network name: 1 to 32 bytes.
pub fn valid_ssid(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 32
}

/// WPA2 passphrase: 8 to 63 printable ASCII characters.
pub fn valid_pass(s: &str) -> bool {
    (8..=63).contains(&s.len()) && s.bytes().all(|b| (0x20..0x7F).contains(&b))
}

/// Saves a new name (`pass` None) or password (`ssid` None) of the shared network; Windows applies it the next time it starts.
pub fn configure(ssid: Option<String>, pass: Option<String>, done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let r = manager().ok_or_else(|| t!("Нет подключения к интернету для раздачи", "No internet connection to share", ja: "共有できるインターネット接続がありません", pt: "Sem conexão com a internet para compartilhar", zh: "没有可共享的互联网连接").to_string()).and_then(|m| {
            let c = m.GetCurrentAccessPointConfiguration().map_err(|e| e.message())?;
            if let Some(s) = &ssid {
                c.SetSsid(&HSTRING::from(s)).map_err(|e| e.message())?;
            }
            if let Some(p) = &pass {
                c.SetPassphrase(&HSTRING::from(p)).map_err(|e| e.message())?;
            }
            m.ConfigureAccessPointAsync(&c).and_then(|a| a.join()).map_err(|e| e.message())
        });
        read(r.err().unwrap_or_default());
        done();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits() {
        assert!(valid_ssid("Мой ПК") && !valid_ssid("  ") && !valid_ssid(&"x".repeat(33)));
        assert!(valid_pass("12345678") && !valid_pass("1234567") && !valid_pass(&"x".repeat(64)) && !valid_pass("пароль12"));
    }
}
