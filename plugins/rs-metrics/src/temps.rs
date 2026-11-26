//! Temperatures per component, on one lowest-priority thread, cached for the renderer.
//!
//! Windows has no CPU-die sensor without a kernel driver. What this module tries, in order of usefulness:
//!   - GPU: NVML from the NVIDIA driver (`nvml.dll`), when there is one;
//!   - disks: SMART temperature over `IOCTL_STORAGE_QUERY_PROPERTY` (needs the privilege to open the physical drive,
//!     so it may be "—" when the widget runs unelevated);
//!   - CPU and board/VRM: LibreHardwareMonitor's WMI namespace, only if it is installed and running;
//!   - the ACPI thermal zone (chassis, not the CPU), only when `temp-acpi=1`.
//! Nothing here runs on the paint thread and nothing waits on a slow sensor: the last good values are cached.
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    ffi::c_void,
    ptr::null_mut,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        Ioctl::{PropertyStandardQuery, STORAGE_PROPERTY_QUERY, STORAGE_TEMPERATURE_DATA_DESCRIPTOR, StorageDeviceTemperatureProperty},
        IO::DeviceIoControl,
        LibraryLoader::{GetProcAddress, LoadLibraryW},
        Performance::*,
        SystemInformation::GetTickCount64,
        Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST},
    },
};

use crate::config::{Config, TEMP};

const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const GENERIC_READ: u32 = 0x8000_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn now() -> u64 {
    unsafe { GetTickCount64() }
}

/// What the temp module shows, one entry per component.
#[derive(Clone, Debug, Default)]
pub struct Temps {
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    pub board: Option<f32>,
    pub acpi: Option<f32>,
    /// (volume letter, temperature), one per configured drive
    pub disks: Vec<(char, Option<f32>)>,
    /// NVML's dedicated VRAM (used, total) when an NVIDIA driver answers
    pub vram: Option<(u64, u64)>,
}

static TEMPS: OnceLock<Mutex<Temps>> = OnceLock::new();

fn slot() -> &'static Mutex<Temps> {
    TEMPS.get_or_init(|| Mutex::new(Temps::default()))
}

pub fn snapshot() -> Temps {
    slot().lock().map(|t| t.clone()).unwrap_or_else(|e| e.into_inner().clone())
}

fn store(t: Temps) {
    if let Ok(mut s) = slot().lock() {
        *s = t;
    }
}

// ---------- NVML (NVIDIA) ----------

type NvmlInit = unsafe extern "system" fn() -> i32;
type NvmlGetHandle = unsafe extern "system" fn(u32, *mut *mut c_void) -> i32;
type NvmlGetTemp = unsafe extern "system" fn(*mut c_void, i32, *mut u32) -> i32;
type NvmlGetMemory = unsafe extern "system" fn(*mut c_void, *mut NvmlMemory) -> i32;

#[repr(C)]
#[derive(Default)]
struct NvmlMemory {
    total: u64,
    free: u64,
    used: u64,
}

struct Nvml {
    dev: *mut c_void,
    get_temp: NvmlGetTemp,
    get_memory: Option<NvmlGetMemory>,
}

impl Nvml {
    /// Loads `nvml.dll` and grabs device 0. `None` when the driver is not there (AMD, Intel, no driver).
    unsafe fn open() -> Option<Nvml> {
        let mut lib = LoadLibraryW(wide("nvml.dll").as_ptr());
        if lib.is_null() {
            lib = LoadLibraryW(wide(r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll").as_ptr());
        }
        if lib.is_null() {
            return None;
        }
        let p = |name: &str| {
            let mut n = name.as_bytes().to_vec();
            n.push(0);
            GetProcAddress(lib, n.as_ptr())
        };
        let init: NvmlInit = std::mem::transmute(p("nvmlInit_v2")?);
        let handle: NvmlGetHandle = std::mem::transmute(p("nvmlDeviceGetHandleByIndex_v2")?);
        let get_temp: NvmlGetTemp = std::mem::transmute(p("nvmlDeviceGetTemperature")?);
        let get_memory = p("nvmlDeviceGetMemoryInfo").map(|f| std::mem::transmute::<_, NvmlGetMemory>(f));
        if init() != 0 {
            return None;
        }
        let mut dev: *mut c_void = null_mut();
        if handle(0, &mut dev) != 0 || dev.is_null() {
            return None;
        }
        Some(Nvml { dev, get_temp, get_memory })
    }

    fn read(&self) -> (Option<f32>, Option<(u64, u64)>) {
        unsafe {
            let mut t = 0u32;
            let temp = ((self.get_temp)(self.dev, 0, &mut t) == 0 && t < 150).then_some(t as f32);
            let mem = self.get_memory.and_then(|f| {
                let mut m = NvmlMemory::default();
                (f(self.dev, &mut m) == 0 && m.total > 0).then_some((m.used, m.total))
            });
            (temp, mem)
        }
    }
}

// ---------- AMD ADL (Radeon) ----------

type AdlCreate = unsafe extern "system" fn(AdlMalloc, i32) -> i32;
type AdlNumAdapters = unsafe extern "system" fn(*mut i32) -> i32;
type AdlOd5Temp = unsafe extern "system" fn(i32, i32, *mut AdlTemperature) -> i32;

/// `ADL_MAIN_MALLOC_CALLBACK`: ADL allocates its tables with this, so it has to be the CRT's malloc.
type AdlMalloc = unsafe extern "system" fn(i32) -> *mut c_void;

#[repr(C)]
#[derive(Default)]
struct AdlTemperature {
    size: i32,
    /// millidegrees Celsius
    temperature: i32,
}

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
}

unsafe extern "system" fn adl_malloc(size: i32) -> *mut c_void {
    malloc(size.max(1) as usize)
}

struct Adl {
    count: i32,
    get_temp: AdlOd5Temp,
    logged: bool,
}

impl Adl {
    /// `atiadlxx.dll` + Overdrive5 temperature. `None` when the library is missing or refuses to start.
    unsafe fn open() -> Option<Adl> {
        let lib = LoadLibraryW(wide("atiadlxx.dll").as_ptr());
        if lib.is_null() {
            log("adl: atiadlxx.dll not loaded");
            return None;
        }
        let p = |name: &str| {
            let mut n = name.as_bytes().to_vec();
            n.push(0);
            GetProcAddress(lib, n.as_ptr())
        };
        let (Some(create), Some(num), Some(get_temp)) = (
            p("ADL_Main_Control_Create"),
            p("ADL_Adapter_NumberOfAdapters_Get"),
            p("ADL_Overdrive5_Temperature_Get"),
        ) else {
            log("adl: exports missing");
            return None;
        };
        let create: AdlCreate = std::mem::transmute(create);
        let num: AdlNumAdapters = std::mem::transmute(num);
        let get_temp: AdlOd5Temp = std::mem::transmute(get_temp);
        let rc = create(adl_malloc, 1);
        let mut count = 0i32;
        let nrc = if rc == 0 { num(&mut count) } else { -1 };
        log(&format!("adl: create rc={rc} num rc={nrc} adapters={count}"));
        if rc != 0 || nrc != 0 || count <= 0 {
            return None;
        }
        Some(Adl { count: count.min(16), get_temp, logged: false })
    }

    /// The first adapter whose Overdrive5 sensor reports a plausible temperature.
    fn read(&mut self) -> Option<f32> {
        for i in 0..self.count {
            let mut t = AdlTemperature { size: std::mem::size_of::<AdlTemperature>() as i32, temperature: 0 };
            let rc = unsafe { (self.get_temp)(i, 0, &mut t) };
            if !self.logged {
                log(&format!("adl: adapter {i} rc={rc} raw={}", t.temperature));
                self.logged = true;
            }
            if rc != 0 {
                continue;
            }
            let c = t.temperature as f32 / 1000.0;
            if (0.0..=120.0).contains(&c) && c > 0.0 {
                return Some(c);
            }
        }
        None
    }
}

/// Same log as the window's (`%LOCALAPPDATA%\rs-shell\metrics.log`), for the probes that only run on this thread.
fn log(msg: &str) {
    use std::io::Write;
    let Ok(base) = std::env::var("LOCALAPPDATA") else { return };
    let dir = std::path::PathBuf::from(base).join("rs-shell");
    let _ = std::fs::create_dir_all(&dir);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("metrics.log")) {
        let _ = writeln!(f, "[{t}] {msg}");
    }
}

// ---------- disks (SMART temperature) ----------

#[repr(C)]
#[derive(Default)]
struct DiskExtent {
    disk: u32,
    _pad: u32,
    start: i64,
    len: i64,
}

#[repr(C)]
#[derive(Default)]
struct VolumeExtents {
    count: u32,
    _pad: u32,
    first: DiskExtent,
}

unsafe fn open_device(path: &str, access: u32) -> windows_sys::Win32::Foundation::HANDLE {
    CreateFileW(
        wide(path).as_ptr(),
        access,
        FILE_SHARE_READ | FILE_SHARE_WRITE,
        std::ptr::null(),
        OPEN_EXISTING,
        0,
        null_mut(),
    )
}

/// The physical disk number behind `C:` (0 = unknown).
unsafe fn disk_of_volume(letter: char) -> u32 {
    let mut h = open_device(&format!(r"\\.\{letter}:"), 0);
    if h == INVALID_HANDLE_VALUE {
        h = open_device(&format!(r"\\.\{letter}:"), GENERIC_READ);
    }
    if h == INVALID_HANDLE_VALUE {
        return 0;
    }
    let mut out = VolumeExtents::default();
    let mut got = 0u32;
    let ok = DeviceIoControl(
        h,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        std::ptr::null(),
        0,
        &mut out as *mut _ as *mut c_void,
        std::mem::size_of::<VolumeExtents>() as u32,
        &mut got,
        null_mut(),
    );
    windows_sys::Win32::Foundation::CloseHandle(h);
    if ok == 0 || out.count == 0 {
        return 0;
    }
    out.first.disk
}

/// Degrees Celsius of one physical drive, when the driver answers the storage temperature property.
unsafe fn disk_temp(disk: u32) -> Option<f32> {
    let mut h = open_device(&format!(r"\\.\PhysicalDrive{disk}"), 0);
    if h == INVALID_HANDLE_VALUE {
        h = open_device(&format!(r"\\.\PhysicalDrive{disk}"), GENERIC_READ);
    }
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    let q = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceTemperatureProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    // 8-byte aligned: the descriptor starts with two u32 and then shorts, and the IOCTL writes a whole struct
    let mut buf = vec![0u64; 32];
    let mut got = 0u32;
    let ok = DeviceIoControl(
        h,
        IOCTL_STORAGE_QUERY_PROPERTY,
        &q as *const _ as *const c_void,
        std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
        buf.as_mut_ptr() as *mut c_void,
        256,
        &mut got,
        null_mut(),
    );
    let _ = got;
    windows_sys::Win32::Foundation::CloseHandle(h);
    if ok == 0 {
        return None;
    }
    let d = &*(buf.as_ptr() as *const STORAGE_TEMPERATURE_DATA_DESCRIPTOR);
    if d.InfoCount == 0 {
        return None;
    }
    let t = d.TemperatureInfo[0].Temperature as f32;
    // most drivers report Celsius; some report Kelvin (whole or tenths)
    let c = if t >= 2000.0 {
        t / 10.0 - 273.15
    } else if t > 150.0 {
        t - 273.15
    } else {
        t
    };
    (-20.0..=125.0).contains(&c).then_some(c)
}

// ---------- LibreHardwareMonitor over WMI ----------

mod lhm {
    use windows::core::{BSTR, PCWSTR};
    use windows::Win32::System::{
        Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoInitializeEx, COINIT_MULTITHREADED},
        Variant::{VARIANT, VAR_CHANGE_FLAGS, VT_BSTR, VT_R8, VariantChangeType, VariantClear},
        Wmi::{IWbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE, WbemLocator},
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    unsafe fn as_f64(v: &VARIANT) -> Option<f64> {
        let mut d = VARIANT::default();
        VariantChangeType(&mut d, v, VAR_CHANGE_FLAGS(0), VT_R8).ok()?;
        let x = d.Anonymous.Anonymous.Anonymous.dblVal;
        let _ = VariantClear(&mut d);
        Some(x)
    }

    unsafe fn as_string(v: &VARIANT) -> Option<String> {
        let mut d = VARIANT::default();
        VariantChangeType(&mut d, v, VAR_CHANGE_FLAGS(0), VT_BSTR).ok()?;
        let s = d.Anonymous.Anonymous.Anonymous.bstrVal.to_string();
        let _ = VariantClear(&mut d);
        Some(s)
    }

    /// (CPU package, board/VRM, GPU core) in degrees Celsius. `None` when the namespace is not there.
    pub unsafe fn read() -> Option<(Option<f32>, Option<f32>, Option<f32>)> {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let loc: IWbemLocator = CoCreateInstance(&WbemLocator, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
        let ns = BSTR::from("ROOT\\LibreHardwareMonitor");
        let empty = BSTR::new();
        let svc = loc.ConnectServer(&ns, &empty, &empty, &empty, 0, &empty, None).ok()?;
        let en = svc
            .ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from("SELECT Name, SensorType, Value FROM Sensor"),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            )
            .ok()?;
        let mut cpu: Option<f32> = None;
        let mut core_max: Option<f32> = None;
        let mut board: Option<f32> = None;
        let mut gpu: Option<f32> = None;
        loop {
            let mut objs: [Option<windows::Win32::System::Wmi::IWbemClassObject>; 8] = Default::default();
            let mut got = 0u32;
            let hr = en.Next(WBEM_INFINITE, &mut objs, &mut got);
            if hr.is_err() || got == 0 {
                break;
            }
            for o in objs.iter_mut().take(got as usize) {
                let Some(o) = o.take() else { continue };
                let mut name = VARIANT::default();
                let mut kind = VARIANT::default();
                let mut val = VARIANT::default();
                let get = |n: &str, v: *mut VARIANT| o.Get(PCWSTR(wide(n).as_ptr()), 0, v, None, None).is_ok();
                let (ok_name, ok_kind, ok_val) = (get("Name", &mut name), get("SensorType", &mut kind), get("Value", &mut val));
                let (n, k, v) = if ok_name && ok_kind && ok_val {
                    (as_string(&name), as_string(&kind), as_f64(&val))
                } else {
                    (None, None, None)
                };
                let _ = VariantClear(&mut name);
                let _ = VariantClear(&mut kind);
                let _ = VariantClear(&mut val);
                let (Some(n), Some(k), Some(v)) = (n, k, v) else { continue };
                if !k.eq_ignore_ascii_case("Temperature") || !(-20.0..=125.0).contains(&v) {
                    continue;
                }
                let v = v as f32;
                if n.contains("CPU Package") || n.contains("Core (Tctl/Tdie)") || n.contains("CPU Die") {
                    cpu = Some(cpu.map_or(v, |c: f32| c.max(v)));
                } else if n.starts_with("CPU Core") {
                    core_max = Some(core_max.map_or(v, |c| c.max(v)));
                } else if n.contains("Motherboard") || n.contains("VRM") || n.contains("Chipset") {
                    board = Some(board.map_or(v, |c| c.max(v)));
                } else if n.contains("GPU") {
                    // "GPU Core", "GPU Diode", ... (the SensorType filter already drops the fan)
                    gpu = Some(gpu.map_or(v, |c: f32| c.max(v)));
                }
            }
        }
        Some((cpu.or(core_max), board, gpu))
    }
}

// ---------- ACPI zone (the old PDH counter) ----------

struct Acpi {
    q: PDH_HQUERY,
    h: PDH_HCOUNTER,
    primed: bool,
}

const PDH_VALID: u32 = 0;
const PDH_NEW: u32 = 1;

unsafe fn pdh_add(q: PDH_HQUERY, path: &str) -> Option<PDH_HCOUNTER> {
    let p = wide(path);
    let mut h: PDH_HCOUNTER = null_mut();
    (PdhAddEnglishCounterW(q, p.as_ptr(), 0, &mut h) == 0 && !h.is_null()).then_some(h)
}

impl Acpi {
    unsafe fn open() -> Option<Acpi> {
        let mut q: PDH_HQUERY = null_mut();
        if PdhOpenQueryW(std::ptr::null(), 0, &mut q) != 0 || q.is_null() {
            return None;
        }
        match pdh_add(q, r"\Thermal Zone Information(*)\Temperature") {
            Some(h) => Some(Acpi { q, h, primed: false }),
            None => {
                PdhCloseQuery(q);
                None
            }
        }
    }

    unsafe fn read(&mut self) -> Option<f32> {
        if PdhCollectQueryData(self.q) != 0 {
            return None;
        }
        if !self.primed {
            self.primed = true;
            return None;
        }
        let (mut size, mut count) = (0u32, 0u32);
        let s = PdhGetFormattedCounterArrayW(self.h, PDH_FMT_DOUBLE, &mut size, &mut count, null_mut());
        if s != PDH_MORE_DATA || size == 0 {
            return None;
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let s = PdhGetFormattedCounterArrayW(self.h, PDH_FMT_DOUBLE, &mut size, &mut count, buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W);
        if s != 0 || count == 0 {
            return None;
        }
        let items = std::slice::from_raw_parts(buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count as usize);
        let mut best: Option<f32> = None;
        for it in items {
            if it.FmtValue.CStatus != PDH_VALID && it.FmtValue.CStatus != PDH_NEW {
                continue;
            }
            let v = it.FmtValue.Anonymous.doubleValue;
            let c = if v >= 2000.0 {
                v / 10.0 - 273.15
            } else if v >= 200.0 {
                v - 273.15
            } else {
                v
            };
            if (-20.0..=125.0).contains(&c) && best.is_none_or(|b| c as f32 > b) {
                best = Some(c as f32);
            }
        }
        best
    }
}

impl Drop for Acpi {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.q) };
    }
}

// ---------- the thread ----------

/// Set by the window on `WM_DEVICECHANGE`: the disk list is re-read on the next tick.
pub static REFRESH_DISKS: AtomicBool = AtomicBool::new(false);

struct Probe {
    nvml: Option<Nvml>,
    nvml_retry: u64,
    adl: Option<Adl>,
    adl_retry: u64,
    lhm: Option<(Option<f32>, Option<f32>, Option<f32>)>,
    lhm_retry: u64,
    acpi: Option<Acpi>,
    acpi_retry: u64,
    disk_at: u64,
    disk_map: Vec<(char, u32)>,
    disks: Vec<(char, Option<f32>)>,
}

pub fn temp_loop(cfg: Arc<Mutex<Config>>) {
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST as i32) };
    let mut p = Probe {
        nvml: None,
        nvml_retry: 0,
        adl: None,
        adl_retry: 0,
        lhm: None,
        lhm_retry: 0,
        acpi: None,
        acpi_retry: 0,
        disk_at: 0,
        disk_map: Vec::new(),
        disks: Vec::new(),
    };
    loop {
        let c = cfg.lock().map(|c| c.clone()).unwrap_or_else(|e| e.into_inner().clone());
        let mut out = Temps::default();
        let t = now();

        // LibreHardwareMonitor first (CPU, board and GPU if it is running), every 6 s; a missing namespace is
        // retried once a minute
        if c.on[TEMP] && c.temp_lhm {
            if p.lhm.is_none() && (p.lhm_retry == 0 || t.saturating_sub(p.lhm_retry) > 60_000) {
                p.lhm_retry = t.max(1);
                p.lhm = unsafe { lhm::read() };
            }
            if let Some((cpu, board, gpu)) = p.lhm {
                out.cpu = cpu;
                out.board = board;
                out.gpu = gpu;
            }
        }

        // GPU temperature, priority LHM > AMD ADL > NVML; the VRAM numbers come from NVML when it is there
        if c.on[TEMP] || c.on[crate::config::GPU] {
            if out.gpu.is_none() {
                if p.adl.is_none() && (p.adl_retry == 0 || t.saturating_sub(p.adl_retry) > 60_000) {
                    p.adl_retry = t.max(1);
                    p.adl = unsafe { Adl::open() };
                }
                if let Some(a) = p.adl.as_mut() {
                    out.gpu = a.read();
                }
            }
            if p.nvml.is_none() && (p.nvml_retry == 0 || t.saturating_sub(p.nvml_retry) > 60_000) {
                p.nvml_retry = t.max(1);
                p.nvml = unsafe { Nvml::open() };
            }
            if let Some(n) = &p.nvml {
                let (temp, vram) = n.read();
                out.vram = vram;
                if out.gpu.is_none() {
                    out.gpu = temp;
                }
            }
        }

        // disks: every 5 s (SMART is not free), only while the temp module is on
        if c.on[TEMP] && c.temp_disks {
            if p.disk_at == 0 || REFRESH_DISKS.swap(false, Relaxed) || t.saturating_sub(p.disk_at) >= 5000 {
                p.disk_at = t.max(1);
                let letters = c.disk_letters.clone();
                if p.disk_map.iter().map(|(l, _)| *l).collect::<Vec<_>>() != letters {
                    p.disk_map = letters.iter().map(|&l| (l, unsafe { disk_of_volume(l) })).collect();
                }
                p.disks = p
                    .disk_map
                    .iter()
                    .map(|&(l, d)| (l, if d == 0 { None } else { unsafe { disk_temp(d) } }))
                    .collect();
            }
            out.disks = p.disks.clone();
        }

        // LibreHardwareMonitor already ran at the top of the tick (CPU, board and GPU)

        // ACPI: only when asked, every 5 s, its own PDH query
        if c.on[TEMP] && c.temp_acpi {
            if p.acpi.is_none() && (p.acpi_retry == 0 || t.saturating_sub(p.acpi_retry) > 60_000) {
                p.acpi_retry = t.max(1);
                p.acpi = unsafe { Acpi::open() };
            }
            if let Some(a) = p.acpi.as_mut() {
                out.acpi = unsafe { a.read() };
            }
        }

        store(out);
        std::thread::sleep(Duration::from_secs(3));
    }
}

