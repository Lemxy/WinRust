//! Bluetooth and airplane mode through Windows.Devices.Radios / Devices.Enumeration (no admin, no Settings app): the radio switch,
//! paired devices with their connection state, pairing a nearby device and removing a paired one. Classic (BluetoothDevice) and
//! Bluetooth LE (BluetoothLEDevice) devices are both listed, as modern pads only exist on the LE side. The nearby list comes from
//! a `DeviceWatcher` (a real discovery: a device in pairing mode has never been seen and is not in any cached list). The WinRT
//! calls wait, so they run in a thread and leave their result in `STATE`, which the window reads on its next refresh.
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};
use windows::{
    Devices::{
        Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice},
        Enumeration::{DeviceInformation, DeviceInformationUpdate, DevicePairingKinds, DevicePairingResultStatus, DeviceUnpairingResultStatus},
        Radios::{Radio, RadioKind, RadioState},
    },
    Foundation::TypedEventHandler,
    core::{HSTRING, Result},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Dev {
    pub id: String,
    pub name: String,
    pub connected: bool,
}

#[derive(Default, Clone)]
pub struct State {
    pub bt: Option<bool>,        // a Bluetooth radio: on or off; None without one
    pub air: Option<bool>,       // every radio is off
    pub paired: Vec<Dev>,
    pub nearby: Vec<Dev>,
    pub scanning: bool,
    pub note: String,            // the outcome of the last pairing / removal
}

static STATE: Mutex<State> = Mutex::new(State { bt: None, air: None, paired: vec![], nearby: vec![], scanning: false, note: String::new() });
static BUSY: AtomicBool = AtomicBool::new(false);

pub fn state() -> State {
    STATE.lock().map(|s| s.clone()).unwrap_or_default()
}

fn with<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE.lock().ok().map(|mut s| f(&mut s))
}

fn radios() -> Vec<Radio> {
    let _ = Radio::RequestAccessAsync().and_then(|a| a.join());
    Radio::GetRadiosAsync().and_then(|a| a.join()).map(|v| v.into_iter().collect()).unwrap_or_default()
}

/// Radio and device lists, read again in a thread (one at a time); `done` runs afterwards (to repaint).
pub fn refresh(done: impl FnOnce() + Send + 'static) {
    if BUSY.swap(true, Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        let rs = radios();
        let bt: Vec<_> = rs.iter().filter(|r| r.Kind().is_ok_and(|k| k == RadioKind::Bluetooth)).collect();
        let bt_on = (!bt.is_empty()).then(|| bt.iter().any(|r| r.State().is_ok_and(|s| s == RadioState::On)));
        let air = (!rs.is_empty()).then(|| rs.iter().all(|r| r.State().is_ok_and(|s| s != RadioState::On)));
        let paired = if bt_on == Some(true) { list(true) } else { vec![] };
        with(|s| (s.bt, s.air, s.paired) = (bt_on, air, paired));
        BUSY.store(false, Relaxed);
        done();
    });
}

/// True for an id of the Bluetooth LE stack (`BluetoothLE#BluetoothLE...`), false for the classic one
/// (`Bluetooth#Bluetooth...`). The prefix says which API can open the device.
fn is_le(id: &str) -> bool {
    id.starts_with("BluetoothLE#")
}

/// Whether the device behind that interface id is connected right now. The id's prefix says which stack owns it:
/// `BluetoothDevice` cannot open an LE id and `BluetoothLEDevice` knows nothing about a classic one, so a device that
/// does not answer is just not connected — never an error that would hide the row.
fn is_connected(id: &str) -> bool {
    let h = HSTRING::from(id);
    if is_le(id) {
        // the LE object holds the device open until it is closed, and a closed reference lets a controller sleep again
        let dev = BluetoothLEDevice::FromIdAsync(&h).and_then(|a| a.join());
        let connected = dev.as_ref().ok().is_some_and(|d| d.ConnectionStatus().is_ok_and(|c| c == BluetoothConnectionStatus::Connected));
        if let Ok(d) = &dev {
            let _ = d.Close();
        }
        return connected;
    }
    BluetoothDevice::FromIdAsync(&h).and_then(|a| a.join()).and_then(|d| d.ConnectionStatus()).is_ok_and(|c| c == BluetoothConnectionStatus::Connected)
}

/// The same device can be listed by both stacks (a DualSense is classic and LE at once) under different ids: one row
/// per device, matched by id, and by name across the two stacks.
fn same_device(dev: &Dev, id: &str, name: &str) -> bool {
    dev.id == id || (is_le(&dev.id) != is_le(id) && dev.name.eq_ignore_ascii_case(name))
}

/// The order the page shows: connected first, then by name (both lists and the live discovery use it).
fn sort_rows(rows: &mut [Dev]) {
    rows.sort_by(|a, b| b.connected.cmp(&a.connected).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
}

fn list(paired: bool) -> Vec<Dev> {
    // Both selectors: headsets, car kits and older pads are classic, while modern pads and most accessories advertise
    // over LE (and Windows' own Settings shows them from the LE list), so neither list alone has everything.
    let selectors: [Result<HSTRING>; 2] = [
        BluetoothDevice::GetDeviceSelectorFromPairingState(paired),
        BluetoothLEDevice::GetDeviceSelectorFromPairingState(paired),
    ];
    let mut out: Vec<Dev> = Vec::new();
    for sel in selectors.into_iter().flatten() {
        let Ok(all) = DeviceInformation::FindAllAsyncAqsFilter(&sel).and_then(|a| a.join()) else { continue };
        for d in all.into_iter() {
            let (Ok(id), Ok(name)) = (d.Id(), d.Name()) else { continue };
            let (id, name) = (id.to_string(), name.to_string());
            if name.trim().is_empty() {
                continue;
            }
            let connected = paired && is_connected(&id);
            match out.iter_mut().find(|o| same_device(o, &id, &name)) {
                Some(o) => o.connected |= connected, // the row already exists from the other stack
                None => out.push(Dev { id, name, connected }),
            }
        }
    }
    sort_rows(&mut out);
    out
}

/// How long the watchers are left running: enough for a pad in pairing mode to answer on one of its advertising bursts,
/// short enough for the page not to feel stuck.
const SCAN: Duration = Duration::from_secs(8);

/// The displayed name of a device, asked again by id: a watcher can announce a device before Windows has resolved its
/// name, and the update that carries the name may come later (or never, for a device that has just answered).
fn name_of(id: &str) -> String {
    DeviceInformation::CreateFromIdAsync(&HSTRING::from(id))
        .and_then(|a| a.join())
        .and_then(|d| d.Name())
        .map(|n| n.to_string())
        .unwrap_or_default()
}

/// Repaint hook for `scan`: it is called from the watchers' own threads and more than once, so it is shared and behind a
/// `Mutex` (the caller only promises `Fn() + Send`, which is not necessarily `Sync`). The window coalesces the nudges
/// itself, so nudging once per change is cheap.
#[derive(Clone)]
struct Notify(Arc<Mutex<Box<dyn Fn() + Send>>>);

impl Notify {
    fn new(f: impl Fn() + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Box::new(f))))
    }

    fn ping(&self) {
        if let Ok(f) = self.0.lock() {
            (*f)()
        }
    }
}

/// Adds one discovered device to `nearby` (in place, connected first and sorted by name), unless it is already there or is
/// paired: the same device can be announced by both stacks under different ids, and a paired one is not offered again.
/// Returns whether the list changed, which is exactly when the window has to be nudged.
fn publish(dev: Dev) -> bool {
    with(|s| {
        if s.paired.iter().any(|p| same_device(p, &dev.id, &dev.name)) {
            return false;
        }
        let changed;
        if let Some(d) = s.nearby.iter_mut().find(|d| same_device(d, &dev.id, &dev.name)) {
            changed = dev.connected && !d.connected; // the row exists already from the other stack
            d.connected |= dev.connected;
        } else {
            s.nearby.push(dev);
            changed = true;
        }
        if changed {
            sort_rows(&mut s.nearby);
        }
        changed
    })
    .unwrap_or(false)
}

/// Active discovery: a `DeviceWatcher` on the unpaired classic and LE selectors reports every device that answers while
/// it runs, including one in pairing mode that Windows has never seen (which `FindAllAsyncAqsFilter` cannot return: that
/// one only lists known devices). Each device goes to `nearby` from its own callback, so it shows up as it answers; the
/// wait only bounds how long the watchers look. `false` when no watcher could be created or started, which leaves `scan`
/// with the cached list.
fn watch(notify: &Notify) -> bool {
    let selectors: [Result<HSTRING>; 2] = [
        BluetoothDevice::GetDeviceSelectorFromPairingState(false),
        BluetoothLEDevice::GetDeviceSelectorFromPairingState(false),
    ];
    let seen: Arc<Mutex<HashMap<String, String>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut watchers = Vec::new();
    for sel in selectors.into_iter().flatten() {
        let Ok(w) = DeviceInformation::CreateWatcherAqsFilter(&sel) else { continue };
        let added_seen = Arc::clone(&seen);
        let added_notify = notify.clone();
        if w.Added(&TypedEventHandler::new(move |_, d: windows::core::Ref<DeviceInformation>| {
            if let Some(d) = d.as_ref() {
                if let Ok(id) = d.Id() {
                    let (id, name) = (id.to_string(), d.Name().map(|n| n.to_string()).unwrap_or_default());
                    if let Ok(mut m) = added_seen.lock() {
                        m.insert(id.clone(), name.clone());
                    }
                    // an empty name is not shown yet: `Updated` (or the sweep after the wait) brings it
                    if !name.trim().is_empty() && publish(Dev { id, name, connected: false }) {
                        added_notify.ping();
                    }
                }
            }
            Ok(())
        }))
        .is_err()
        {
            continue;
        }
        let updated_seen = Arc::clone(&seen);
        let updated_notify = notify.clone();
        let _ = w.Updated(&TypedEventHandler::new(move |_, u: windows::core::Ref<DeviceInformationUpdate>| {
            if let Some(u) = u.as_ref() {
                if let Ok(id) = u.Id() {
                    let id = id.to_string();
                    // a device announced before Windows resolved its name: ask again, then show it
                    let missing = updated_seen.lock().ok().is_some_and(|m| m.get(&id).is_some_and(|n| n.trim().is_empty()));
                    if missing {
                        let name = name_of(&id);
                        if !name.trim().is_empty() {
                            if let Ok(mut m) = updated_seen.lock() {
                                m.insert(id.clone(), name.clone());
                            }
                            if publish(Dev { id, name, connected: false }) {
                                updated_notify.ping();
                            }
                        }
                    }
                }
            }
            Ok(())
        }));
        if w.Start().is_ok() {
            watchers.push(w);
        }
    }
    if watchers.is_empty() {
        return false;
    }
    // the watchers (and their handlers) have to stay alive while they look: dropping them here would stop the discovery
    std::thread::sleep(SCAN);
    for w in &watchers {
        let _ = w.Stop();
    }
    drop(watchers);
    // devices whose name never arrived: ask for it once the enumeration has settled, then show them
    let pending: Vec<String> = seen
        .lock()
        .map(|m| m.iter().filter(|(_, n)| n.trim().is_empty()).map(|(id, _)| id.clone()).collect())
        .unwrap_or_default();
    for id in pending {
        let name = name_of(&id);
        if !name.trim().is_empty() && publish(Dev { id, name, connected: false }) {
            notify.ping();
        }
    }
    true
}

/// Switches every radio of that kind (`None`: all radios, which is airplane mode) and re-reads the state.
pub fn set(kind: Option<RadioKind>, on: bool, done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        for r in radios().into_iter().filter(|r| kind.is_none_or(|k| r.Kind().is_ok_and(|rk| rk == k))) {
            let _ = r.SetStateAsync(if on { RadioState::On } else { RadioState::Off }).and_then(|a| a.join());
        }
        BUSY.store(false, Relaxed);
        refresh(done);
    });
}

pub fn set_bluetooth(on: bool, done: impl FnOnce() + Send + 'static) {
    set(Some(RadioKind::Bluetooth), on, done);
}

pub fn set_airplane(on: bool, done: impl FnOnce() + Send + 'static) {
    // airplane mode: everything off; back from it: Wi-Fi and Bluetooth on (the radios that have a switch of their own)
    std::thread::spawn(move || {
        for r in radios() {
            let kind = r.Kind().ok();
            if on || matches!(kind, Some(RadioKind::WiFi | RadioKind::Bluetooth)) {
                let _ = r.SetStateAsync(if on { RadioState::Off } else { RadioState::On }).and_then(|a| a.join());
            }
        }
        BUSY.store(false, Relaxed);
        refresh(done);
    });
}

/// Looks for devices in range: the cached list is shown right away, then about eight seconds of active discovery (a pad in
/// pairing mode answers on one of its advertising bursts, which is why it has to be watched for a while). Each device is
/// published from its watcher callback and `notify` is called on every change, so it appears as soon as it is seen; the
/// `scanning` flag stays on until the watchers stop.
pub fn scan(notify: impl Fn() + Send + 'static) {
    if with(|s| std::mem::replace(&mut s.scanning, true)) != Some(false) {
        return;
    }
    std::thread::spawn(move || {
        let notify = Notify::new(notify);
        with(|s| s.note = t!("Ищем устройства…", "Searching…", ja: "検索中…", pt: "Procurando…", zh: "正在搜索…").into());
        notify.ping(); // the page shows "searching…" while the cached list is read
        // what Windows already knows, on screen before the discovery starts (the same filter the page always had)
        let cached = list(false);
        with(|s| {
            s.nearby = cached.into_iter().filter(|d| !s.paired.iter().any(|p| same_device(p, &d.id, &d.name))).collect();
            sort_rows(&mut s.nearby);
        });
        notify.ping();
        // live discovery: every callback publishes its device and nudges the window at once; without a watcher the cached
        // list is all there is, and the tail below still ends the scan
        let _ = watch(&notify);
        with(|s| {
            let count = s.nearby.len();
            s.scanning = false;
            s.note = tf!("Найдено устройств: {}", "{} devices found", ja: "{} 台のデバイスが見つかりました", pt: "{} dispositivos encontrados", zh: "找到 {} 个设备", count);
        });
        notify.ping();
    });
}

fn pairing_text(code: i32) -> String {
    match DevicePairingResultStatus(code) {
        DevicePairingResultStatus::Paired => t!("Устройство подключено", "Device paired", ja: "デバイスをペアリングしました", pt: "Dispositivo pareado", zh: "设备已配对").into(),
        DevicePairingResultStatus::AlreadyPaired => t!("Уже сопряжено", "Already paired", ja: "すでにペアリング済みです", pt: "Já pareado", zh: "已配对").into(),
        DevicePairingResultStatus::AuthenticationFailure | DevicePairingResultStatus::AuthenticationNotAllowed => t!("Сопряжение отклонено устройством", "The device refused to pair", ja: "デバイスがペアリングを拒否しました", pt: "O dispositivo recusou o pareamento", zh: "设备拒绝配对").into(),
        DevicePairingResultStatus::RequiredHandlerNotRegistered | DevicePairingResultStatus::RejectedByHandler => t!("Для этого устройства нужен ввод PIN: сопрягите его в параметрах Windows", "This device needs a PIN: pair it in Windows' own settings", ja: "このデバイスには PIN が必要です。Windows の設定でペアリングしてください", pt: "Este dispositivo precisa de um PIN: pareie-o nas configurações do próprio Windows", zh: "此设备需要 PIN：请在 Windows 设置中配对").into(),
        _ => tf!("Не удалось сопрячь (код {})", "Could not pair (code {})", ja: "ペアリングできませんでした (コード {})", pt: "Não foi possível parear (código {})", zh: "无法配对（代码 {}）", code),
    }
}

/// Pairs a device from `nearby`: confirmation and a displayed PIN are accepted here (the PIN is shown in the note so it can be
/// compared with the device); a device that wants a PIN typed in is reported.
pub fn pair(id: String, done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let msg = (|| -> windows::core::Result<String> {
            let info = DeviceInformation::CreateFromIdAsync(&HSTRING::from(&id))?.join()?;
            let custom = info.Pairing()?.Custom()?;
            custom.PairingRequested(&TypedEventHandler::new(|_, args: windows::core::Ref<windows::Devices::Enumeration::DevicePairingRequestedEventArgs>| {
                if let Some(a) = args.as_ref() {
                    if a.PairingKind().is_ok_and(|k| k == DevicePairingKinds::ConfirmPinMatch || k == DevicePairingKinds::DisplayPin) {
                        let pin = a.Pin().map(|p| p.to_string()).unwrap_or_default();
                        with(|s| s.note = tf!("PIN: {}  (сравните с устройством)", "PIN: {}  (compare with the device)", ja: "PIN: {}  (デバイスと照合してください)", pt: "PIN: {}  (compare com o dispositivo)", zh: "PIN：{}（与设备比对）", pin));
                    }
                    a.Accept()?;
                }
                Ok(())
            }))?;
            let kinds = DevicePairingKinds::ConfirmOnly | DevicePairingKinds::DisplayPin | DevicePairingKinds::ConfirmPinMatch;
            let r = custom.PairAsync(kinds)?.join()?;
            Ok(pairing_text(r.Status()?.0))
        })()
        .unwrap_or_else(|e| tf!("Не удалось сопрячь: {}", "Could not pair: {}", ja: "ペアリングできませんでした: {}", pt: "Não foi possível parear: {}", zh: "无法配对：{}", e.message()));
        with(|s| s.note = msg);
        BUSY.store(false, Relaxed);
        refresh(|| {});
        done();
    });
}

/// Removes a paired device.
pub fn unpair(id: String, done: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let msg = (|| -> windows::core::Result<String> {
            let info = DeviceInformation::CreateFromIdAsync(&HSTRING::from(&id))?.join()?;
            let r = info.Pairing()?.UnpairAsync()?.join()?;
            Ok(if r.Status()? == DeviceUnpairingResultStatus::Unpaired { t!("Устройство удалено", "Device removed", ja: "デバイスを削除しました", pt: "Dispositivo removido", zh: "设备已移除").into() } else { tf!("Не удалось удалить (код {})", "Could not remove (code {})", ja: "削除できませんでした (コード {})", pt: "Não foi possível remover (código {})", zh: "无法移除（代码 {}）", r.Status()?.0) })
        })()
        .unwrap_or_else(|e| tf!("Не удалось удалить: {}", "Could not remove: {}", ja: "削除できませんでした: {}", pt: "Não foi possível remover: {}", zh: "无法移除：{}", e.message()));
        with(|s| s.note = msg);
        BUSY.store(false, Relaxed);
        refresh(|| {});
        done();
    });
}

pub fn take_note() -> Option<String> {
    with(|s| std::mem::take(&mut s.note)).filter(|n| !n.is_empty())
}
