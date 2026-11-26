//! The numbers behind the widget: CPU, GPU, RAM, disks, disk I/O, network, temperatures.
//!
//! Everything here runs on one sampler thread (plus a low-priority thread for the temperatures, see `temp_loop`), so the
//! window thread only ever paints what it finds in a finished `Sample`. No WMI on every tick, no helper processes:
//! CPU/RAM/network/disks come from `kernel32`/`iphlpapi`, I/O and GPU from PDH performance counters (the same data Task
//! Manager reads), temperatures from the ACPI thermal-zone counter when the machine exposes one.
#![allow(unsafe_op_in_unsafe_fn)]
use std::{
    ffi::c_void,
    ptr::null_mut,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI32, Ordering::Relaxed},
    },
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::FILETIME,
    NetworkManagement::{
        IpHelper::*,
        Ndis::IfOperStatusUp,
    },
    Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW},
    System::{
        Performance::*,
        SystemInformation::{GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX},
        Threading::{GetCurrentThread, GetSystemTimes, SetThreadPriority, THREAD_PRIORITY_LOWEST},
    },
};

use crate::config::{Config, TEMP};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn now() -> u64 {
    unsafe { GetTickCount64() }
}

// ---------- PDH ----------

const PDH_VALID: u32 = 0; // PDH_CSTATUS_VALID_DATA
const PDH_NEW: u32 = 1; // PDH_CSTATUS_NEW_DATA

unsafe fn pdh_add(q: PDH_HQUERY, path: &str) -> Option<PDH_HCOUNTER> {
    let p = wide(path);
    let mut h: PDH_HCOUNTER = null_mut();
    let s = PdhAddEnglishCounterW(q, p.as_ptr(), 0, &mut h);
    (s == 0 && !h.is_null()).then_some(h)
}

unsafe fn pdh_value(h: PDH_HCOUNTER) -> Option<f64> {
    let mut v: PDH_FMT_COUNTERVALUE = std::mem::zeroed();
    if PdhGetFormattedCounterValue(h, PDH_FMT_DOUBLE, null_mut(), &mut v) != 0 || (v.CStatus != PDH_VALID && v.CStatus != PDH_NEW) {
        return None;
    }
    Some(v.Anonymous.doubleValue)
}

/// The instances of a wildcard counter with their values. The buffer is 8-byte aligned (the item has pointers).
unsafe fn pdh_array(h: PDH_HCOUNTER) -> Vec<(String, f64)> {
    let (mut size, mut count) = (0u32, 0u32);
    let s = PdhGetFormattedCounterArrayW(h, PDH_FMT_DOUBLE, &mut size, &mut count, null_mut());
    if s != PDH_MORE_DATA || size == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let s = PdhGetFormattedCounterArrayW(h, PDH_FMT_DOUBLE, &mut size, &mut count, buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W);
    if s != 0 || count == 0 {
        return Vec::new();
    }
    let items = std::slice::from_raw_parts(buf.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W, count as usize);
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        if it.FmtValue.CStatus != PDH_VALID && it.FmtValue.CStatus != PDH_NEW {
            continue;
        }
        let mut n = 0usize;
        while n < 512 && *it.szName.add(n) != 0 {
            n += 1;
        }
        out.push((String::from_utf16_lossy(std::slice::from_raw_parts(it.szName, n)), it.FmtValue.Anonymous.doubleValue));
    }
    out
}

/// `\PhysicalDisk(_Total)\Disk Read Bytes/sec` + Write, the two PDH rates of the whole machine.
struct IoQuery {
    q: PDH_HQUERY,
    rd: PDH_HCOUNTER,
    wr: PDH_HCOUNTER,
}

impl Drop for IoQuery {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.q) };
    }
}

impl IoQuery {
    fn open() -> Option<IoQuery> {
        unsafe {
            let mut q: PDH_HQUERY = null_mut();
            if PdhOpenQueryW(std::ptr::null(), 0, &mut q) != 0 || q.is_null() {
                return None;
            }
            let rd = pdh_add(q, r"\PhysicalDisk(_Total)\Disk Read Bytes/sec");
            let wr = pdh_add(q, r"\PhysicalDisk(_Total)\Disk Write Bytes/sec");
            match (rd, wr) {
                (Some(rd), Some(wr)) => Some(IoQuery { q, rd, wr }),
                _ => {
                    PdhCloseQuery(q);
                    None
                }
            }
        }
    }

    /// MB/s. The first collect only primes the rates, so it reports nothing.
    fn read(&self, primed: bool) -> Option<(f64, f64)> {
        unsafe {
            if PdhCollectQueryData(self.q) != 0 {
                return None;
            }
            if !primed {
                return None;
            }
            let (r, w) = (pdh_value(self.rd)?, pdh_value(self.wr)?);
            (r >= 0.0 && w >= 0.0).then(|| (r / 1_048_576.0, w / 1_048_576.0))
        }
    }
}

/// GPU utilization and the dedicated VRAM in use. Windows has no single "GPU usage" counter: Task Manager groups the
/// per-engine instances by engine type and shows the busiest group, and so does this.
struct GpuQuery {
    q: PDH_HQUERY,
    util: PDH_HCOUNTER,
    mem: PDH_HCOUNTER,
    primed: bool,
}

impl Drop for GpuQuery {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.q) };
    }
}

impl GpuQuery {
    fn open() -> Option<GpuQuery> {
        unsafe {
            let mut q: PDH_HQUERY = null_mut();
            if PdhOpenQueryW(std::ptr::null(), 0, &mut q) != 0 || q.is_null() {
                return None;
            }
            let util = pdh_add(q, r"\GPU Engine(*)\Utilization Percentage");
            let mem = pdh_add(q, r"\GPU Adapter Memory(*)\Dedicated Usage");
            match (util, mem) {
                (Some(util), Some(mem)) => {
                    // prime the rate counters now: the first sample the widget asks for is a real one
                    PdhCollectQueryData(q);
                    Some(GpuQuery { q, util, mem, primed: true })
                }
                _ => {
                    PdhCloseQuery(q);
                    None
                }
            }
        }
    }

    /// (usage %, dedicated bytes in use). `None` when the driver exposes no counters.
    fn read(&mut self) -> Option<(f32, Option<u64>)> {
        unsafe {
            if PdhCollectQueryData(self.q) != 0 {
                return None;
            }
            if !self.primed {
                self.primed = true;
                return None; // the first collect is the baseline of the rate counters
            }
            let mut groups: [(&str, f64); 7] = [("3D", 0.0), ("Copy", 0.0), ("VideoDecode", 0.0), ("VideoEncode", 0.0), ("VideoProcessing", 0.0), ("Compute", 0.0), ("Other", 0.0)];
            let mut any = false;
            for (name, v) in pdh_array(self.util) {
                if v < 0.0 {
                    continue;
                }
                any = true;
                let ty = name.rsplit_once("engtype_").map(|(_, t)| t).unwrap_or("");
                let slot = groups.iter_mut().find(|(k, _)| *k == ty);
                match slot {
                    Some((_, sum)) => *sum += v,
                    None => groups[6].1 += v,
                }
            }
            if !any {
                return None;
            }
            let usage = groups.iter().map(|(_, v)| *v).fold(0.0, f64::max).clamp(0.0, 100.0);
            let mem: u64 = pdh_array(self.mem).into_iter().filter(|(_, v)| *v > 0.0).map(|(_, v)| v as u64).sum();
            Some((usage as f32, (mem > 0).then_some(mem)))
        }
    }
}

// ---------- the sample ----------

/// Set by the window on `WM_DEVICECHANGE`: the drive letters are re-read on the next tick.
pub static REFRESH_DISKS: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug, Default)]
pub struct DiskRow {
    pub letter: char,
    pub label: String,
    pub used: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub seq: u64,
    pub cpu: f32,
    pub cores: Vec<f32>,
    pub gpu: Option<f32>,
    pub gpu_mem: Option<u64>,
    pub ram_used: u64,
    pub ram_total: u64,
    pub disks: Vec<DiskRow>,
    /// read / write MB/s
    pub io: Option<(f64, f64)>,
    /// down / up MB/s
    pub net: Option<(f64, f64)>,
    pub temp_cpu: Option<f32>,
    pub temp_gpu: Option<f32>,
    pub uptime: u64,
}

/// A first sample without touching PDH: RAM, the disks and the uptime, so the window opens at its final size.
pub fn quick(cfg: &Config) -> Sample {
    let mut s = Sample { uptime: now() / 1000, ..Sample::default() };
    ram(&mut s);
    s.disks = cfg.disk_letters.iter().filter_map(|&l| disk_row(l, true)).collect();
    s
}

fn ram(s: &mut Sample) {
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
        s.ram_total = m.ullTotalPhys;
        s.ram_used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
    }
}

// ---------- CPU ----------
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CoreTimes {
    idle: i64,
    kernel: i64,
    user: i64,
    dpc: i64,
    interrupt: i64,
    count: u32,
    _pad: u32,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

const SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION: u32 = 8;

/// The per-core counters, if ntdll shares them (empty: no bars, the total keeps working).
fn core_times() -> Vec<CoreTimes> {
    let mut buf = vec![CoreTimes::default(); 64];
    let mut ret = 0u32;
    let (ptr, len) = (buf.as_mut_ptr() as *mut c_void, (buf.len() * std::mem::size_of::<CoreTimes>()) as u32);
    let st = unsafe { NtQuerySystemInformation(SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION, ptr, len, &mut ret) };
    if st < 0 {
        return Vec::new();
    }
    let n = (ret as usize / std::mem::size_of::<CoreTimes>()).min(buf.len());
    buf.truncate(n);
    buf
}

fn cpu_times() -> (u64, u64, u64) {
    let (mut i, mut k, mut u): (FILETIME, FILETIME, FILETIME) = unsafe { std::mem::zeroed() };
    unsafe { GetSystemTimes(&mut i, &mut k, &mut u) };
    let q = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    (q(i), q(k), q(u))
}

// ---------- the sampler ----------

pub struct Sampler {
    seq: u64,
    prev_cpu: Option<(u64, u64, u64)>,
    prev_cores: Vec<CoreTimes>,
    prev_net: Option<(u64, u64, u64)>,
    iq: Option<IoQuery>,
    io_tick: u64,
    io_tries: u32,
    gq: Option<GpuQuery>,
    gpu_tick: u64,
    gpu_tries: u32,
    last_gpu: Option<(f32, Option<u64>)>,
    disk_tick: u64,
    disk_label_tick: u64,
    disk_letters: Vec<char>,
    disks: Vec<DiskRow>,
}

impl Default for Sampler {
    fn default() -> Self {
        Sampler {
            seq: 0,
            prev_cpu: None,
            prev_cores: Vec::new(),
            prev_net: None,
            iq: None,
            io_tick: 0,
            io_tries: 0,
            gq: None,
            gpu_tick: 0,
            gpu_tries: 0,
            last_gpu: None,
            disk_tick: 0,
            disk_label_tick: 0,
            disk_letters: Vec::new(),
            disks: Vec::new(),
        }
    }
}

impl Sampler {
    pub fn collect(&mut self, cfg: &Config) -> Sample {
        self.seq += 1;
        let mut s = Sample { seq: self.seq, uptime: now() / 1000, ..Sample::default() };

        // CPU: kernel time includes idle, so usage = 1 - idle / (kernel + user)
        let (idle, kernel, user) = cpu_times();
        if let Some((pi, pk, pu)) = self.prev_cpu {
            let di = idle.saturating_sub(pi);
            let dt = kernel.saturating_sub(pk) + user.saturating_sub(pu);
            if dt > 0 {
                s.cpu = ((1.0 - di as f64 / dt as f64) * 100.0).clamp(0.0, 100.0) as f32;
            }
        }
        self.prev_cpu = Some((idle, kernel, user));

        let cores = core_times();
        if !cores.is_empty() {
            let mut v = Vec::with_capacity(cores.len());
            for (i, c) in cores.iter().enumerate() {
                let busy = match self.prev_cores.get(i) {
                    Some(p) => {
                        let di = c.idle.saturating_sub(p.idle) as f64;
                        let dt = (c.kernel.saturating_sub(p.kernel) + c.user.saturating_sub(p.user)) as f64;
                        if dt > 0.0 { ((1.0 - di / dt) * 100.0).clamp(0.0, 100.0) as f32 } else { 0.0 }
                    }
                    None => 0.0,
                };
                v.push(busy);
            }
            self.prev_cores = cores;
            s.cores = v;
        }

        // RAM
        ram(&mut s);

        // Disks: the configured letters, sizes every 5 s and labels every 60 s (cheap, but not once a second cheap)
        if cfg.on[crate::config::DISK] {
            let letters = cfg.disk_letters.clone();
            let t = now();
            if REFRESH_DISKS.swap(false, Relaxed) {
                self.disk_tick = 0; // WM_DEVICECHANGE: do not wait for the 5 s tick
            }
            if letters != self.disk_letters || self.disk_tick == 0 || t.saturating_sub(self.disk_tick) >= 5000 {
                self.disk_letters = letters;
                self.disk_tick = t.max(1);
                let labels = t.saturating_sub(self.disk_label_tick) >= 60_000;
                if labels {
                    self.disk_label_tick = t;
                }
                let old = std::mem::take(&mut self.disks);
                let mut rows: Vec<DiskRow> = self.disk_letters.iter().filter_map(|&l| disk_row(l, labels)).collect();
                if !labels {
                    // a size refresh must not forget the volume names, they change once in a blue moon
                    for r in &mut rows {
                        if r.label.is_empty() {
                            r.label = old.iter().find(|o| o.letter == r.letter).map(|o| o.label.clone()).unwrap_or_default();
                        }
                    }
                }
                self.disks = rows;
            }
        } else {
            self.disk_tick = 0;
        }
        s.disks = self.disks.clone();

        // Disk I/O: PDH rate counters; a machine without them just shows a dash
        if cfg.on[crate::config::IO] {
            if self.iq.is_none() && (self.io_tries < 2 || now().saturating_sub(self.io_tick) > 60_000) {
                self.io_tick = now();
                self.io_tries += 1;
                self.iq = IoQuery::open();
            }
            s.io = self.iq.as_ref().and_then(|q| q.read(self.seq > 3));
        }

        // GPU: the engine percentage is the expensive one, so every other tick is enough
        if cfg.on[crate::config::GPU] {
            if self.gq.is_none() && (self.gpu_tries < 2 || now().saturating_sub(self.gpu_tick) > 60_000) {
                self.gpu_tick = now();
                self.gpu_tries += 1;
                self.gq = GpuQuery::open();
            }
            // every 4th second: the wildcard GPU Engine counter is the one expensive read (a GPU % that ages a few
            // seconds is fine, and it keeps the whole widget inside its CPU budget)
            if self.seq % 4 == 0 {
                if let Some((u, mem)) = self.gq.as_mut().and_then(|q| q.read()) {
                    self.last_gpu = Some((u, mem));
                }
            }
            s.gpu = self.last_gpu.map(|(u, _)| u);
            s.gpu_mem = self.last_gpu.and_then(|(_, m)| m);
        }

        // Network: byte counters of the interfaces that are up, summed
        s.net = self.net(cfg);

        s.temp_cpu = temp(&TEMP_CPU);
        s.temp_gpu = temp(&TEMP_GPU);
        s
    }

    fn net(&mut self, cfg: &Config) -> Option<(f64, f64)> {
        let t = now();
        let (mut down, mut up) = (0u64, 0u64);
        let mut table: *mut MIB_IF_TABLE2 = null_mut();
        unsafe {
            if GetIfTable2(&mut table) != 0 || table.is_null() {
                return None;
            }
            let n = (*table).NumEntries as usize;
            let rows = std::ptr::addr_of!((*table).Table) as *const MIB_IF_ROW2;
            let want = cfg.net_iface.trim().to_ascii_lowercase();
            for i in 0..n {
                let r = &*rows.add(i);
                if r.OperStatus != IfOperStatusUp || r.Type == IF_TYPE_SOFTWARE_LOOPBACK {
                    continue;
                }
                // a real link has a hardware address; the loopback, Teredo and most VPN pseudo-adapters do not
                if r.PhysicalAddressLength == 0 {
                    continue;
                }
                if want != "auto" && !want.is_empty() {
                    let alias = String::from_utf16_lossy(&r.Alias).trim_end_matches('\0').to_lowercase();
                    let desc = String::from_utf16_lossy(&r.Description).trim_end_matches('\0').to_lowercase();
                    if !alias.contains(&want) && !desc.contains(&want) {
                        continue;
                    }
                }
                down = down.saturating_add(r.InOctets);
                up = up.saturating_add(r.OutOctets);
            }
            FreeMibTable(table as *const c_void);
        }
        match self.prev_net {
            // a counter that went backwards means an interface disappeared: start over rather than show a spike
            Some((pin, pout, pt)) if down >= pin && up >= pout && t > pt => {
                let dt = (t - pt) as f64 / 1000.0;
                self.prev_net = Some((down, up, t));
                if dt <= 0.0 { None } else { Some(((down - pin) as f64 / dt / 1_048_576.0, (up - pout) as f64 / dt / 1_048_576.0)) }
            }
            _ => {
                self.prev_net = Some((down, up, t));
                None
            }
        }
    }
}

/// One fixed drive: free-space numbers and, when `labels` is set, its volume name.
fn disk_row(letter: char, labels: bool) -> Option<DiskRow> {
    // the letter must still be one Windows lists: an unplugged drive keeps its old row in the cache otherwise
    let mask = unsafe { GetLogicalDrives() };
    let idx = (letter.to_ascii_uppercase() as u32).wrapping_sub('A' as u32);
    if idx >= 26 || mask & (1 << idx) == 0 {
        return None;
    }
    let root = format!("{letter}:\\");
    let w = wide(&root);
    unsafe {
        let kind = GetDriveTypeW(w.as_ptr());
        if kind == 1 {
            return None; // DRIVE_NO_ROOT_DIR
        }
        let (mut free, mut total, mut avail) = (0u64, 0u64, 0u64);
        if GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, &mut total, &mut free) == 0 || total == 0 {
            return None;
        }
        let mut label = String::new();
        if labels {
            let mut buf = [0u16; 64];
            if GetVolumeInformationW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32, null_mut(), null_mut(), null_mut(), null_mut(), 0) != 0 {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                label = String::from_utf16_lossy(&buf[..n]).trim().to_string();
            }
        }
        Some(DiskRow { letter, label, used: total.saturating_sub(free), total })
    }
}

// ---------- temperatures ----------

/// Tenths of a degree Celsius, `i32::MIN` when nothing is known.
pub static TEMP_CPU: AtomicI32 = AtomicI32::new(i32::MIN);
pub static TEMP_GPU: AtomicI32 = AtomicI32::new(i32::MIN);

fn temp(a: &AtomicI32) -> Option<f32> {
    let v = a.load(Relaxed);
    (v != i32::MIN).then(|| v as f32 / 10.0)
}

/// The ACPI thermal-zone counter, when the firmware exposes one. Its own lowest-priority thread, every 3 s, only while
/// the module is on: this is the one probe that may touch a slow firmware path, and it must never hold up a frame.
pub fn temp_loop(cfg: Arc<Mutex<Config>>) {
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST as i32);
    }
    let mut q: PDH_HQUERY = null_mut();
    let mut h: PDH_HCOUNTER = null_mut();
    let mut primed = false;
    loop {
        let on = cfg.lock().map(|c| c.on[TEMP]).unwrap_or(false);
        if !on {
            TEMP_CPU.store(i32::MIN, Relaxed);
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        unsafe {
            if q.is_null() {
                let mut nq: PDH_HQUERY = null_mut();
                if PdhOpenQueryW(std::ptr::null(), 0, &mut nq) == 0 && !nq.is_null() {
                    if let Some(nh) = pdh_add(nq, r"\Thermal Zone Information(*)\Temperature") {
                        q = nq;
                        h = nh;
                    } else {
                        PdhCloseQuery(nq);
                    }
                }
            }
            if !q.is_null() {
                if PdhCollectQueryData(q) == 0 {
                    if primed {
                        let mut best: Option<f64> = None;
                        for (_, v) in pdh_array(h) {
                            // some firmware reports tenths of Kelvin (3000 = 300.0 K), some tenths of Celsius,
                            // some whole degrees; keep whatever lands in a plausible range
                            let c = if v >= 2000.0 {
                                v / 10.0 - 273.15
                            } else if v >= 200.0 {
                                v - 273.15
                            } else {
                                v
                            };
                            if (-20.0..=125.0).contains(&c) && best.is_none_or(|b| c > b) {
                                best = Some(c);
                            }
                        }
                        TEMP_CPU.store(best.map_or(i32::MIN, |c| (c * 10.0).round() as i32), Relaxed);
                    }
                    primed = true;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

// Memoized last GPU reading: the sample every other tick repeats it, so the widget keeps a steady number.
