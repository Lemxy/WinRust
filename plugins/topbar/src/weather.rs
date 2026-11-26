//! Weather + local news for the bar: the item right of Start and its own flyout.
//!
//! # Where the data comes from / privacy
//!
//! * **Location**: `Windows.Devices.Geolocation` (WinRT) first. If the user denies the permission, the location service
//!   is off, or the call fails, the public IP is looked up (`https://ipapi.co/json/`, then `http://ip-api.com/json/`).
//!   A manual override in `weather.txt` (`weather-city`, `weather-lat`/`weather-lon`) wins over both.
//! * **Weather**: Open-Meteo (`https://api.open-meteo.com/v1/forecast`), no key.
//! * **News**: Google News RSS (the country feed, its BUSINESS section, and a search for the city/region), no key.
//! * Only the coordinates are sent to Open-Meteo, and only the headlines are requested from Google News. When the IP
//!   lookup is used the provider sees the request that any web page would make; no other data is attached. There are no
//!   accounts, no API keys and no analytics. The result is cached in `%LOCALAPPDATA%\rs-shell\weather.json` so the bar
//!   can start with the last values; deleting that file only costs one refresh.
//!
//! # Configuration (`%ProgramData%\WinRust\weather.txt`, `key=value`, re-read every second)
//!
//! ```text
//! weather=1                         # the item on the bar; 0 hides it without stopping the bar
//! weather-unit=c                    # c or f
//! weather-city=                     # manual place name (geocoded through Open-Meteo's geocoding API)
//! weather-lat=                      # manual coordinates; win over city/geolocation when both are set
//! weather-lon=
//! weather-refresh=15                # minutes between refreshes (5..180)
//! weather-news=local,country,market # the three sections; leave one out to hide it
//! weather-browser=default           # default browser (reserved; links use ShellExecute)
//! ```
//!
//! All network work happens on one worker thread; the UI only reads the shared snapshot. A single request runs at a
//! time, the refresh interval is 15 minutes by default, and the flyout asks for an early refresh only when its data is
//! older than five minutes.
use super::*;
use std::{
    collections::HashMap,
    ffi::c_void,
    future::IntoFuture,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
        mpsc, Arc, Mutex, OnceLock,
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};
use windows::{
    Devices::Geolocation::{GeolocationAccessStatus, Geolocator},
    Foundation::TimeSpan,
    Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
};
use windows_sys::Win32::{
    Networking::WinHttp::*,
    System::Time::SystemTimeToTzSpecificLocalTime,
};
use crate::fly::surface;

// ---------- configuration ----------

#[derive(Clone, PartialEq)]
struct Cfg {
    on: bool,
    f: bool,
    city: String,
    lat: Option<f64>,
    lon: Option<f64>,
    refresh: u32, // minutes
    news: [bool; 3],
    browser: String,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { on: true, f: false, city: String::new(), lat: None, lon: None, refresh: 15, news: [true; 3], browser: "default".into() }
    }
}

impl Cfg {
    fn news_on(&self, cat: usize) -> bool {
        self.news.get(cat).copied().unwrap_or(false)
    }
}

pub(crate) fn config_path() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_default()).join(r"WinRust\weather.txt")
}

fn cache_path() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join(r"rs-shell\weather.json")
}

fn defaults() -> &'static str {
    "weather=1\nweather-unit=c\nweather-city=\nweather-lat=\nweather-lon=\nweather-refresh=15\nweather-news=local,country,market\nweather-browser=default\n"
}

/// Creates the file with every key the first time it is opened from the menu or the settings window, so the user sees
/// what can be changed instead of an empty file.
pub(crate) fn ensure_config() {
    let f = config_path();
    if f.is_file() {
        return;
    }
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if std::fs::write(&f, defaults()).is_err() {
        crate::tlog("weather: could not create weather.txt");
    }
}

fn parse_cfg(text: &str) -> Cfg {
    let mut map: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let get = |k: &str| map.get(k).map(String::as_str);
    let flag = |k: &str, d: bool| get(k).and_then(|v| v.parse::<i32>().ok()).map_or(d, |v| v != 0);
    let num = |k: &str| get(k).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
    let mut news = [false; 3];
    match get("weather-news") {
        None => news = [true; 3],
        Some(v) => {
            for part in v.split(',') {
                match part.trim().to_ascii_lowercase().as_str() {
                    "local" | "zona" | "area" | "city" => news[0] = true,
                    "country" | "pais" | "país" | "national" => news[1] = true,
                    "market" | "mercado" | "business" | "economy" => news[2] = true,
                    _ => {}
                }
            }
        }
    }
    let unit = get("weather-unit").unwrap_or("c").to_ascii_lowercase();
    Cfg {
        on: flag("weather", true),
        f: unit == "f" || unit == "fahrenheit" || unit == "°f",
        city: get("weather-city").unwrap_or("").trim().to_string(),
        lat: num("weather-lat"),
        lon: num("weather-lon"),
        refresh: get("weather-refresh").and_then(|v| v.parse::<u32>().ok()).map_or(15, |v| v.clamp(5, 180)),
        news,
        browser: get("weather-browser").unwrap_or("default").to_string(),
    }
}

struct CfgCache {
    at: u64,
    cfg: Cfg,
}

static CFG_CACHE: Mutex<Option<CfgCache>> = Mutex::new(None);

fn cfg() -> Cfg {
    let now = unsafe { crate::now_ms() };
    if let Ok(g) = CFG_CACHE.lock() {
        if let Some(c) = g.as_ref() {
            if now.saturating_sub(c.at) < 1000 {
                return c.cfg.clone();
            }
        }
    }
    let c = std::fs::read_to_string(config_path()).ok().map(|s| parse_cfg(&s)).unwrap_or_default();
    if let Ok(mut g) = CFG_CACHE.lock() {
        *g = Some(CfgCache { at: now, cfg: c.clone() });
    }
    c
}

fn invalidate_cfg() {
    if let Ok(mut g) = CFG_CACHE.lock() {
        *g = None;
    }
}

static LAST_CFG: Mutex<Option<Cfg>> = Mutex::new(None);
static LAST_LANG: Mutex<Option<&'static str>> = Mutex::new(None);

/// Once a second from the bar's status timer: re-reads `weather.txt` and asks the worker for work when the file (or the
/// UI language, which chooses the Google News edition) changed. No polling inside the worker: it sleeps on its deadline.
pub(crate) fn poll() {
    let c = cfg();
    let lang = winrust_common::lang::get().code();
    let mut changed = false;
    let mut first = false;
    let mut fetch_diff = false;
    if let Ok(mut g) = LAST_CFG.lock() {
        match g.as_ref() {
            None => first = true,
            Some(prev) => {
                fetch_diff = prev.on != c.on || prev.city != c.city || prev.lat != c.lat || prev.lon != c.lon || prev.news != c.news || prev.refresh != c.refresh;
            }
        }
        if g.as_ref() != Some(&c) {
            *g = Some(c.clone());
            changed = true;
        }
    }
    let mut lang_changed = false;
    if let Ok(mut g) = LAST_LANG.lock() {
        if *g != Some(lang) {
            *g = Some(lang);
            changed = true;
            lang_changed = true;
        }
    }
    if changed {
        GEN.fetch_add(1, Relaxed); // unit, on/off and the labels must show at once
        if first {
            return; // the worker already does its first fetch at start-up
        }
        if !c.on {
            send(Req::Config); // wake the worker so it drops the pending deadline while the item is off
        } else if fetch_diff || lang_changed || !c.on {
            request_now();
        }
    }
}

/// The item is on the bar. Cheap: the file is parsed at most once a second and this is called by every paint.
pub(crate) fn enabled() -> bool {
    cfg().on
}

/// Writes one key, keeping the others (the settings window has its own copy of this small writer).
pub(crate) fn set_key(key: &str, value: &str) {
    ensure_config();
    let f = config_path();
    let old = std::fs::read_to_string(&f).unwrap_or_default();
    let prefix = format!("{key}=");
    let mut out = String::with_capacity(old.len() + value.len() + 8);
    let mut found = false;
    for line in old.lines() {
        if line.trim_start().to_ascii_lowercase().starts_with(&prefix) {
            out.push_str(&format!("{key}={value}\n"));
            found = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !found {
        out.push_str(&format!("{key}={value}\n"));
    }
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, out);
    invalidate_cfg();
    GEN.fetch_add(1, Relaxed);
}

// ---------- shared state ----------

#[derive(Clone)]
struct Loc {
    lat: f64,
    lon: f64,
    city: String,
    region: String,
    country: String,
    cc: String,
}

#[derive(Clone)]
struct Wx {
    temp: f64, // °C
    code: i32,
    max: [f64; 3],
    min: [f64; 3],
    codes: [i32; 3],
    dates: [String; 3],
}

#[derive(Clone)]
struct Item {
    title: String,
    link: String,
    source: String,
    time: String,
}

#[derive(Clone, Default)]
struct Snap {
    loc: Option<Loc>,
    wx: Option<Wx>,
    news: [Vec<Item>; 3],
    news_err: [bool; 3],
    loc_err: bool,
    wx_err: bool,
    busy: bool,
    updated: u64,
    hhmm: String,
}

#[derive(Default)]
struct State {
    loc: Option<Loc>,
    loc_at: u64,
    wx: Option<Wx>,
    updated: u64,
    hhmm: String,
    news: [Vec<Item>; 3],
    news_err: [bool; 3],
    loc_err: bool,
    wx_err: bool,
    busy: bool,
}

static ST: Mutex<Option<State>> = Mutex::new(None);
static GEN: AtomicU64 = AtomicU64::new(1);
static BUSY: AtomicBool = AtomicBool::new(false);

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut g = ST.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(State::default))
}

fn snap() -> Snap {
    with_state(|s| Snap {
        loc: s.loc.clone(),
        wx: s.wx.clone(),
        news: s.news.clone(),
        news_err: s.news_err,
        loc_err: s.loc_err,
        wx_err: s.wx_err,
        busy: s.busy,
        updated: s.updated,
        hhmm: s.hhmm.clone(),
    })
}

/// Changes since the bar last painted; `view_sig` includes it.
pub(crate) fn sig() -> u64 {
    GEN.load(Relaxed)
}

fn epoch() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn local_hhmm() -> String {
    let mut t: SYSTEMTIME = unsafe { zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:02}:{:02}", t.wHour, t.wMinute)
}

// ---------- cache ----------

fn jesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Saves the location and the weather (not the headlines: those are cheap to ask for again and must not go stale).
fn save_cache() {
    let (loc, wx, loc_at, updated, hhmm) = with_state(|s| (s.loc.clone(), s.wx.clone(), s.loc_at, s.updated, s.hhmm.clone()));
    if loc.is_none() && wx.is_none() {
        return;
    }
    let mut out = String::from("{\"v\":1");
    out.push_str(&format!(",\"locAt\":{loc_at},\"at\":{updated},\"hhmm\":\"{}\"", jesc(&hhmm)));
    if let Some(l) = loc {
        out.push_str(&format!(
            ",\"lat\":{:.5},\"lon\":{:.5},\"city\":\"{}\",\"region\":\"{}\",\"country\":\"{}\",\"cc\":\"{}\"",
            l.lat,
            l.lon,
            jesc(&l.city),
            jesc(&l.region),
            jesc(&l.country),
            jesc(&l.cc)
        ));
    }
    if let Some(w) = wx {
        out.push_str(&format!(",\"temp\":{:.2},\"code\":{},", w.temp, w.code));
        out.push_str(&format!("\"max\":[{:.2},{:.2},{:.2}],", w.max[0], w.max[1], w.max[2]));
        out.push_str(&format!("\"min\":[{:.2},{:.2},{:.2}],", w.min[0], w.min[1], w.min[2]));
        out.push_str(&format!("\"codes\":[{},{},{}],", w.codes[0], w.codes[1], w.codes[2]));
        out.push_str(&format!("\"dates\":[\"{}\",\"{}\",\"{}\"]", jesc(&w.dates[0]), jesc(&w.dates[1]), jesc(&w.dates[2])));
    }
    out.push('}');
    let p = cache_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(&p, out);
}

fn load_cache() {
    let Ok(text) = std::fs::read_to_string(cache_path()) else { return };
    let loc_at = jnum(&text, "locAt").unwrap_or(0.0) as u64;
    let updated = jnum(&text, "at").unwrap_or(0.0) as u64;
    let hhmm = jstr(&text, "hhmm").unwrap_or_default();
    let loc = match (jnum(&text, "lat"), jnum(&text, "lon")) {
        (Some(lat), Some(lon)) => Some(Loc {
            lat,
            lon,
            city: jstr(&text, "city").unwrap_or_default(),
            region: jstr(&text, "region").unwrap_or_default(),
            country: jstr(&text, "country").unwrap_or_default(),
            cc: jstr(&text, "cc").unwrap_or_default(),
        }),
        _ => None,
    };
    let wx = match jnum(&text, "temp") {
        Some(temp) => {
            let max = jarr_num(&text, "max").unwrap_or_default();
            let min = jarr_num(&text, "min").unwrap_or_default();
            let codes = jarr_num(&text, "codes").unwrap_or_default();
            let dates = jarr_str(&text, "dates").unwrap_or_default();
            let at = |v: &Vec<f64>, i: usize| v.get(i).copied().unwrap_or(0.0);
            let ai = |v: &Vec<f64>, i: usize| v.get(i).copied().unwrap_or(0.0) as i32;
            let ds = |i: usize| dates.get(i).cloned().unwrap_or_default();
            Some(Wx { temp, code: jnum(&text, "code").unwrap_or(0.0) as i32, max: [at(&max, 0), at(&max, 1), at(&max, 2)], min: [at(&min, 0), at(&min, 1), at(&min, 2)], codes: [ai(&codes, 0), ai(&codes, 1), ai(&codes, 2)], dates: [ds(0), ds(1), ds(2)] })
        }
        None => None,
    };
    with_state(|s| {
        s.loc = loc;
        s.loc_at = loc_at;
        s.wx = wx;
        s.updated = updated;
        s.hhmm = hhmm;
    });
    GEN.fetch_add(1, Relaxed);
}

/// Minimal JSON readers for our own cache and for the two APIs: the bodies are small and the shapes are flat.
fn find_key<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\"");
    let i = s.find(&pat)? + pat.len();
    let rest = s[i..].trim_start();
    rest.strip_prefix(':').map(|r| r.trim_start())
}

fn jnum(s: &str, key: &str) -> Option<f64> {
    let r = find_key(s, key)?;
    let end = r.find(|c: char| !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))).unwrap_or(r.len());
    r[..end].parse().ok()
}

fn jstr(s: &str, key: &str) -> Option<String> {
    let r = find_key(s, key)?;
    let mut it = r.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => match it.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let hex: String = it.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

fn jobj<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let r = find_key(s, key)?.strip_prefix('{')?;
    let (mut depth, mut instr, mut esc) = (1i32, false, false);
    for (i, c) in r.char_indices() {
        if instr {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                instr = false;
            }
            continue;
        }
        match c {
            '"' => instr = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&r[..i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn jarr<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let r = find_key(s, key)?.strip_prefix('[')?;
    let end = r.find(']')?;
    Some(&r[..end])
}

fn jarr_num(s: &str, key: &str) -> Option<Vec<f64>> {
    let a = jarr(s, key)?;
    Some(a.split(',').filter_map(|v| v.trim().parse::<f64>().ok()).collect())
}

fn jarr_str(s: &str, key: &str) -> Option<Vec<String>> {
    let a = jarr(s, key)?;
    let mut out = Vec::new();
    let mut rest = a;
    while let Some(i) = rest.find('"') {
        rest = &rest[i + 1..];
        let mut v = String::new();
        let mut esc = false;
        for (j, c) in rest.char_indices() {
            if esc {
                v.push(match c {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    c => c,
                });
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                out.push(v);
                rest = &rest[j + 1..];
                break;
            } else {
                v.push(c);
            }
        }
    }
    Some(out)
}

// ---------- HTTP (WinHTTP: system TLS and proxy settings, no extra crates) ----------

struct Handle(*mut c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn split_url(url: &str) -> Option<(bool, String, String)> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return None;
    };
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if host.is_empty() {
        return None;
    }
    Some((secure, host.to_string(), path.to_string()))
}

fn http_get(url: &str, accept: &str) -> Result<String, String> {
    let (secure, host, path) = split_url(url).ok_or_else(|| "bad url".to_string())?;
    unsafe {
        let session = Handle(WinHttpOpen(wide("WinRust/0.1 (Windows; weather widget)").as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            return Err("winhttp open".into());
        }
        WinHttpSetTimeouts(session.0, 3000, 4000, 4000, 8000);
        let conn = Handle(WinHttpConnect(session.0, wide(&host).as_ptr(), if secure { 443 } else { 80 }, 0));
        if conn.0.is_null() {
            return Err("winhttp connect".into());
        }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { 0 };
        let req = Handle(WinHttpOpenRequest(conn.0, wide("GET").as_ptr(), wide(&path).as_ptr(), null(), null(), null(), flags | WINHTTP_FLAG_REFRESH));
        if req.0.is_null() {
            return Err("winhttp request".into());
        }
        let redir = WINHTTP_OPTION_REDIRECT_POLICY_ALWAYS;
        WinHttpSetOption(req.0, WINHTTP_OPTION_REDIRECT_POLICY, &redir as *const _ as *const c_void, size_of::<u32>() as u32);
        let tls = WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2 | WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;
        WinHttpSetOption(req.0, WINHTTP_OPTION_SECURE_PROTOCOLS, &tls as *const _ as *const c_void, size_of::<u32>() as u32);
        let headers = format!("Accept: {accept}\r\nAccept-Language: es-ES,es;q=0.9,en;q=0.8\r\nCache-Control: no-cache\r\n");
        WinHttpAddRequestHeaders(req.0, wide(&headers).as_ptr(), u32::MAX, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE);
        if WinHttpSendRequest(req.0, null(), 0, null(), 0, 0, 0) == 0 {
            return Err("winhttp send".into());
        }
        if WinHttpReceiveResponse(req.0, null_mut()) == 0 {
            return Err("winhttp receive".into());
        }
        let mut code: u32 = 0;
        let mut len = size_of::<u32>() as u32;
        if WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, null(), &mut code as *mut _ as *mut c_void, &mut len, null_mut()) == 0 {
            return Err("winhttp status".into());
        }
        if code != 200 {
            return Err(format!("http {code}"));
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let mut avail: u32 = 0;
            if WinHttpQueryDataAvailable(req.0, &mut avail) == 0 || avail == 0 {
                break;
            }
            let n = avail.min(64 * 1024);
            let mut buf = vec![0u8; n as usize];
            let mut read: u32 = 0;
            if WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, n, &mut read) == 0 {
                return Err("winhttp read".into());
            }
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read as usize]);
            if out.len() > 2 << 20 {
                break; // three Google feeds are under 300 KB each; a runaway answer is not worth buffering
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

// ---------- location ----------

struct Noop;
impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}

/// Blocks a WinRT `IAsyncOperation` without pulling in an async runtime. Only the weather worker calls this.
fn block_on<F: std::future::Future>(fut: F, timeout: Duration) -> Option<F::Output> {
    let waker = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    let start = Instant::now();
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return Some(v),
            Poll::Pending => {
                if start.elapsed() >= timeout {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

fn win_location() -> Option<(f64, f64)> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let access = Geolocator::RequestAccessAsync().ok()?;
        let status = block_on(access.into_future(), Duration::from_secs(12))?.ok()?;
        if status != GeolocationAccessStatus::Allowed {
            crate::tlog("weather: Windows location not allowed, trying the IP lookup");
            return None;
        }
        let g = Geolocator::new().ok()?;
        let op = g.GetGeopositionAsyncWithAgeAndTimeout(TimeSpan { Duration: 30 * 60 * 10_000_000 }, TimeSpan { Duration: 8 * 10_000_000 }).ok()?;
        let pos = block_on(op.into_future(), Duration::from_secs(10))?.ok()?;
        let c = pos.Coordinate().ok()?;
        let (lat, lon) = (c.Latitude().ok()?, c.Longitude().ok()?);
        if lat.is_finite() && lon.is_finite() && !(lat == 0.0 && lon == 0.0) {
            Some((lat, lon))
        } else {
            None
        }
    }
}

/// One IP geolocation answer in the shape both providers can fill.
fn parse_ip_json(text: &str) -> Option<Loc> {
    let lat = jnum(text, "latitude").or_else(|| jnum(text, "lat"))?;
    let lon = jnum(text, "longitude").or_else(|| jnum(text, "lon"))?;
    let city = jstr(text, "city").unwrap_or_default();
    let region = jstr(text, "region").or_else(|| jstr(text, "regionName")).unwrap_or_default();
    let country = jstr(text, "country_name").or_else(|| jstr(text, "country")).unwrap_or_default();
    let cc = jstr(text, "country_code").or_else(|| jstr(text, "countryCode")).unwrap_or_default();
    if !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    Some(Loc { lat, lon, city, region, country, cc: cc.to_ascii_uppercase() })
}

fn ip_location() -> Option<Loc> {
    for url in ["https://ipapi.co/json/", "http://ip-api.com/json/?fields=status,message,country,countryCode,regionName,city,lat,lon"] {
        match http_get(url, "application/json") {
            Ok(t) => {
                if let Some(l) = parse_ip_json(&t) {
                    crate::tlog(&format!("weather: IP location {}{} ({:.3},{:.3})", l.city, if l.cc.is_empty() { String::new() } else { format!(", {}", l.cc) }, l.lat, l.lon));
                    return Some(l);
                }
                crate::tlog(&format!("weather: IP answer from {url} had no usable position"));
            }
            Err(e) => crate::tlog(&format!("weather: IP lookup {url}: {e}")),
        }
    }
    None
}

fn geocode(city: &str) -> Option<Loc> {
    let lang = winrust_common::lang::get().code();
    let url = format!("https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language={lang}&format=json", url_encode(city));
    let text = http_get(&url, "application/json").ok()?;
    let start = find_key(&text, "results")?;
    let obj = start.strip_prefix('[')?.trim_start().strip_prefix('{')?;
    let obj = &obj[..obj.find('}')?];
    let lat = jnum(obj, "latitude")?;
    let lon = jnum(obj, "longitude")?;
    Some(Loc {
        lat,
        lon,
        city: jstr(obj, "name").unwrap_or_else(|| city.to_string()),
        region: jstr(obj, "admin1").unwrap_or_default(),
        country: jstr(obj, "country").unwrap_or_default(),
        cc: jstr(obj, "country_code").unwrap_or_default().to_ascii_uppercase(),
    })
}

/// The place name for the coordinates Windows gave us. The IP answer is only used for its labels: its own lat/lon is
/// ignored, so the coordinates never travel anywhere except Open-Meteo.
fn label_for(cached: Option<&Loc>) -> Option<Loc> {
    if let Some(l) = cached {
        if !l.city.is_empty() || !l.country.is_empty() {
            return Some(l.clone());
        }
    }
    ip_location()
}

fn resolve_location(c: &Cfg, s: &State) -> Option<Loc> {
    // manual coordinates: the label comes from weather-city or the cached place
    if let (Some(lat), Some(lon)) = (c.lat, c.lon) {
        let old = s.loc.as_ref().filter(|l| (l.lat - lat).abs() < 0.5 && (l.lon - lon).abs() < 0.5);
        if let Some(old) = old {
            return Some(Loc { lat, lon, city: if c.city.is_empty() { old.city.clone() } else { c.city.clone() }, region: old.region.clone(), country: old.country.clone(), cc: old.cc.clone() });
        }
        return Some(Loc { lat, lon, city: c.city.clone(), region: String::new(), country: String::new(), cc: String::new() });
    }
    // a place name from the config: Open-Meteo's geocoding API turns it into coordinates (once; the cache keeps it)
    if !c.city.is_empty() {
        if let Some(old) = s.loc.as_ref().filter(|l| l.city.eq_ignore_ascii_case(&c.city) && l.lat.is_finite()) {
            return Some(old.clone());
        }
        if let Some(l) = geocode(&c.city) {
            crate::tlog(&format!("weather: geocoded {} to {:.3},{:.3}", c.city, l.lat, l.lon));
            return Some(l);
        }
        crate::tlog(&format!("weather: could not geocode {:?}, using the automatic location", c.city));
    }
    // cached for at most six hours
    if let Some(l) = &s.loc {
        if epoch().saturating_sub(s.loc_at) < 6 * 3600 {
            return Some(l.clone());
        }
    }
    if let Some((lat, lon)) = win_location() {
        let label = label_for(s.loc.as_ref());
        let l = Loc {
            lat,
            lon,
            city: label.as_ref().map_or(String::new(), |l| l.city.clone()),
            region: label.as_ref().map_or(String::new(), |l| l.region.clone()),
            country: label.as_ref().map_or(String::new(), |l| l.country.clone()),
            cc: label.as_ref().map_or(String::new(), |l| l.cc.clone()),
        };
        crate::tlog(&format!("weather: Windows location ({:.3},{:.3}) label {:?}", l.lat, l.lon, l.city));
        return Some(l);
    }
    if let Some(l) = ip_location() {
        return Some(l);
    }
    // last resort: a stale cached location still beats an empty panel
    s.loc.clone()
}

// ---------- weather ----------

fn weather_code_text(code: i32) -> &'static str {
    match code {
        0 => t!("Ясно", "Clear sky", ja: "快晴", pt: "Céu limpo", zh: "晴"),
        1 => t!("В основном ясно", "Mostly clear", ja: "おおむね晴れ", pt: "Predomínio de sol", zh: "大部晴朗"),
        2 => t!("Переменная облачность", "Partly cloudy", ja: "晴れ時々曇り", pt: "Parcialmente nublado", zh: "局部多云"),
        3 => t!("Пасмурно", "Overcast", ja: "曇り", pt: "Nublado", zh: "阴"),
        45 | 48 => t!("Туман", "Fog", ja: "霧", pt: "Neblina", zh: "雾"),
        51 | 53 | 55 | 56 | 57 => t!("Морось", "Drizzle", ja: "霧雨", pt: "Garoa", zh: "毛毛雨"),
        61 | 63 | 65 | 66 | 67 => t!("Дождь", "Rain", ja: "雨", pt: "Chuva", zh: "雨"),
        71 | 73 | 75 | 77 => t!("Снег", "Snow", ja: "雪", pt: "Neve", zh: "雪"),
        80 | 81 | 82 => t!("Ливень", "Showers", ja: "にわか雨", pt: "Aguaceiros", zh: "阵雨"),
        85 | 86 => t!("Снегопад", "Snow showers", ja: "にわか雪", pt: "Aguaceiros de neve", zh: "阵雪"),
        95 => t!("Гроза", "Thunderstorm", ja: "雷雨", pt: "Trovoada", zh: "雷暴"),
        96 | 99 => t!("Гроза с градом", "Thunderstorm with hail", ja: "雷雨とひょう", pt: "Trovoada com granizo", zh: "雷暴伴冰雹"),
        _ => t!("Неизвестно", "Unknown", ja: "不明", pt: "Desconhecido", zh: "未知"),
    }
}

/// A glyph that Segoe UI (with Windows' font linking) draws everywhere, including the fallback `☁`.
fn weather_glyph(code: i32) -> &'static str {
    match code {
        0 => "☀",
        1 | 2 => "⛅",
        3 => "☁",
        45 | 48 => "🌫",
        51 | 53 | 55 | 56 | 57 => "☂",
        61 | 63 | 65 | 66 | 67 | 80 | 81 | 82 => "🌧",
        71 | 73 | 75 | 77 | 85 | 86 => "❄",
        95 | 96 | 99 => "⛈",
        _ => "☁",
    }
}

fn parse_weather(text: &str) -> Option<Wx> {
    let cur = jobj(text, "current")?;
    let daily = jobj(text, "daily")?;
    let temp = jnum(cur, "temperature_2m")?;
    let code = jnum(cur, "weather_code").unwrap_or(-1.0) as i32;
    let max = jarr_num(daily, "temperature_2m_max").unwrap_or_default();
    let min = jarr_num(daily, "temperature_2m_min").unwrap_or_default();
    let codes = jarr_num(daily, "weather_code").unwrap_or_default();
    let dates = jarr_str(daily, "time").unwrap_or_default();
    let at = |v: &Vec<f64>, i: usize| v.get(i).copied().unwrap_or(0.0);
    let ai = |v: &Vec<f64>, i: usize| v.get(i).copied().unwrap_or(-1.0) as i32;
    let ds = |i: usize| dates.get(i).cloned().unwrap_or_default();
    Some(Wx { temp, code, max: [at(&max, 0), at(&max, 1), at(&max, 2)], min: [at(&min, 0), at(&min, 1), at(&min, 2)], codes: [ai(&codes, 0), ai(&codes, 1), ai(&codes, 2)], dates: [ds(0), ds(1), ds(2)] })
}

fn fetch_weather(l: &Loc) -> Result<Wx, String> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}&current=temperature_2m,weather_code&daily=temperature_2m_max,temperature_2m_min,weather_code&forecast_days=3&timezone=auto",
        l.lat, l.lon
    );
    let text = http_get(&url, "application/json")?;
    parse_weather(&text).ok_or_else(|| "unexpected weather answer".into())
}

// ---------- news ----------

struct NewsCodes {
    hl: String,
    cc: String,
}

fn news_codes(loc: Option<&Loc>) -> NewsCodes {
    let hl = match winrust_common::lang::get() {
        winrust_common::Lang::Ru => "ru",
        winrust_common::Lang::Ja => "ja",
        winrust_common::Lang::Pt => "pt",
        winrust_common::Lang::Zh => "zh",
        winrust_common::Lang::Es => "es",
        winrust_common::Lang::En => "en",
    }
    .to_string();
    let cc = loc
        .map(|l| l.cc.trim().to_ascii_uppercase())
        .filter(|s| s.len() == 2 && s.chars().all(|c| c.is_ascii_alphabetic()))
        .unwrap_or_else(|| {
            match winrust_common::lang::get() {
                winrust_common::Lang::Ru => "RU",
                winrust_common::Lang::Ja => "JP",
                winrust_common::Lang::Pt => "BR",
                winrust_common::Lang::Zh => "CN",
                winrust_common::Lang::Es => "AR",
                winrust_common::Lang::En => "US",
            }
            .to_string()
        })
        .to_ascii_uppercase();
    NewsCodes { hl, cc }
}

fn xml_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        let mut ent = String::new();
        let mut closed = false;
        for c in it.by_ref() {
            if c == ';' {
                closed = true;
                break;
            }
            if ent.len() > 10 {
                break;
            }
            ent.push(c);
        }
        if !closed {
            out.push('&');
            out.push_str(&ent);
            continue;
        }
        let v = match ent.as_str() {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match v {
            Some(ch) => out.push(ch),
            None => {
                out.push('&');
                out.push_str(&ent);
                out.push(';');
            }
        }
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut inside = false;
    for c in s.chars() {
        match c {
            '<' => inside = true,
            '>' => inside = false,
            c if !inside => out.push(c),
            _ => {}
        }
    }
    out
}

fn clean_xml(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("<![CDATA[").and_then(|x| x.strip_suffix("]]>")).unwrap_or(s);
    xml_decode(&strip_tags(s)).trim().to_string()
}

fn xml_tag(block: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let start = block.find(&open)?;
    let gt = block[start..].find('>')? + start;
    let close = format!("</{tag}>");
    let end = block[gt..].find(&close)? + gt;
    Some(clean_xml(&block[gt + 1..end]))
}

fn xml_link(block: &str) -> String {
    if let Some(v) = xml_tag(block, "link") {
        if !v.is_empty() {
            return v;
        }
    }
    // Atom (and some RSS variants) carry the address in the attribute
    let start = match block.find("<link") {
        Some(i) => i,
        None => return String::new(),
    };
    let end = match block[start..].find('>') {
        Some(i) => start + i,
        None => return String::new(),
    };
    let tag = &block[start..end];
    for attr in ["href=\"", "url=\""] {
        if let Some(i) = tag.find(attr) {
            let rest = &tag[i + attr.len()..];
            if let Some(j) = rest.find('"') {
                return xml_decode(&rest[..j]);
            }
        }
    }
    String::new()
}

fn month_num(s: &str) -> Option<u32> {
    let m = s.to_ascii_lowercase();
    Some(match &m[..m.len().min(3)] {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    })
}

/// `Thu, 26 Nov 2026 11:28:24 GMT` -> "08:28" in the user's time zone (the feeds are always GMT).
fn rss_time(s: &str) -> String {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let mi = match parts.iter().position(|p| month_num(p).is_some()) {
        Some(i) if i > 0 && i + 2 < parts.len() => i,
        _ => return String::new(),
    };
    let day = parts[mi - 1].trim_end_matches(',');
    let year = parts[mi + 1];
    let hm: Vec<&str> = parts[mi + 2].split(':').collect();
    let (Ok(d), Some(month), Ok(y)) = (day.parse::<u16>(), month_num(parts[mi]), year.parse::<u16>()) else { return String::new() };
    let (Ok(h), Ok(min)) = (hm.first().unwrap_or(&"0").parse::<u16>(), hm.get(1).unwrap_or(&"0").parse::<u16>()) else { return String::new() };
    let utc = SYSTEMTIME { wYear: y, wMonth: month as u16, wDayOfWeek: 0, wDay: d, wHour: h, wMinute: min, wSecond: 0, wMilliseconds: 0 };
    let mut local: SYSTEMTIME = unsafe { zeroed() };
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(null(), &utc, &mut local) } != 0;
    if ok {
        format!("{:02}:{:02}", local.wHour, local.wMinute)
    } else {
        format!("{h:02}:{min:02}")
    }
}

fn parse_rss(xml: &str, max: usize) -> Vec<Item> {
    let mut out = Vec::new();
    for chunk in xml.split("<item").skip(1) {
        if out.len() >= max {
            break;
        }
        let body = match chunk.find("</item>") {
            Some(i) => &chunk[..i],
            None => continue,
        };
        let title = xml_tag(body, "title").unwrap_or_default();
        let link = xml_link(body);
        if title.is_empty() || link.is_empty() {
            continue;
        }
        let source = xml_tag(body, "source").unwrap_or_default();
        let mut title = title;
        // Google News appends " - Source" to every title; the source has a column of its own
        if !source.is_empty() {
            let suffix = format!(" - {source}");
            if let Some(t) = title.strip_suffix(&suffix) {
                title = t.trim_end().to_string();
            }
        }
        let time = xml_tag(body, "pubDate").map(|s| rss_time(&s)).unwrap_or_default();
        out.push(Item { title, link, source, time });
    }
    out
}

fn news_url(cat: usize, loc: Option<&Loc>, c: &NewsCodes) -> Option<String> {
    match cat {
        0 => {
            let l = loc?;
            let q = format!("{} {}", l.city, l.region).trim().to_string();
            if q.is_empty() {
                return None;
            }
            Some(format!("https://news.google.com/rss/search?q={}&hl={}&gl={}&ceid={}:{}", url_encode(&q), c.hl, c.cc, c.cc, c.hl))
        }
        1 => Some(format!("https://news.google.com/rss?hl={}&gl={}&ceid={}:{}", c.hl, c.cc, c.cc, c.hl)),
        _ => Some(format!("https://news.google.com/rss/headlines/section/topic/BUSINESS?hl={}&gl={}&ceid={}:{}", c.hl, c.cc, c.cc, c.hl)),
    }
}

fn fetch_news(cat: usize, loc: Option<&Loc>) -> Result<Vec<Item>, String> {
    let codes = news_codes(loc);
    let url = news_url(cat, loc, &codes).ok_or_else(|| "no city".to_string())?;
    let xml = http_get(&url, "application/rss+xml, application/xml;q=0.9, text/xml;q=0.8")?;
    let items = parse_rss(&xml, 3);
    if items.is_empty() {
        return Err("empty feed".into());
    }
    Ok(items)
}

// ---------- worker ----------

#[derive(Clone, Copy)]
enum Req {
    Now,
    IfStale(u64),
    Config,
}

static TX: OnceLock<mpsc::Sender<Req>> = OnceLock::new();
static STARTED: OnceLock<()> = OnceLock::new();

pub(crate) fn init() {
    if STARTED.set(()).is_err() {
        return;
    }
    load_cache();
    let (tx, rx) = mpsc::channel();
    let _ = TX.set(tx);
    let _ = std::thread::Builder::new().name("weather".into()).spawn(move || worker(rx));
}

fn send(r: Req) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(r);
    }
}

pub(crate) fn request_now() {
    send(Req::Now);
}

/// The flyout asks for a refresh only when what it shows is older than `secs`.
pub(crate) fn request_if_stale(secs: u64) {
    send(Req::IfStale(secs));
}

fn worker(rx: mpsc::Receiver<Req>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED); // WinRT and WinHTTP both want an apartment
    }
    let mut next = Instant::now();
    let mut last_fetch: u64 = 0;
    loop {
        let wait = next.saturating_duration_since(Instant::now());
        let first = if wait.is_zero() {
            None
        } else {
            match rx.recv_timeout(wait) {
                Ok(r) => Some(r),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        };
        let mut force = wait.is_zero();
        let mut stale: Option<u64> = None;
        let mut config = false;
        let mut absorb = |r: Req| match r {
            Req::Now => force = true,
            Req::IfStale(s) => stale = Some(stale.map_or(s, |x| x.max(s))),
            Req::Config => {
                force = true;
                config = true;
            }
        };
        if let Some(r) = first {
            absorb(r);
        }
        while let Ok(r) = rx.try_recv() {
            absorb(r);
        }
        let c = cfg();
        if !c.on {
            // The bar's poll() sends a Config when the file changes, so this long wait is woken as soon as it is on again.
            next = Instant::now() + Duration::from_secs(3600);
            continue;
        }
        let now = epoch();
        let due = force || wait.is_zero();
        let stale = stale.is_some_and(|s| now.saturating_sub(last_fetch) >= s);
        if !due && !stale {
            continue; // an IfStale that is not stale yet: the pending deadline still stands
        }
        // A second "refresh" that arrived while the fetch above was running (the user clicked twice, or the first
        // paint and the first poll raced at start-up) is not a new fetch.
        if force && !config && last_fetch > 0 && now.saturating_sub(last_fetch) < 5 {
            continue;
        }
        refresh(&c);
        last_fetch = epoch();
        next = Instant::now() + Duration::from_secs(c.refresh as u64 * 60);
    }
}

fn refresh(c: &Cfg) {
    BUSY.store(true, Relaxed);
    with_state(|s| s.busy = true);
    GEN.fetch_add(1, Relaxed);

    let (old_loc, old_loc_at) = with_state(|s| (s.loc.clone(), s.loc_at));
    let was_fresh = old_loc.is_some() && epoch().saturating_sub(old_loc_at) < 6 * 3600;
    let manual = c.lat.is_some() || !c.city.is_empty();
    let loc = resolve_location(c, &State { loc: old_loc.clone(), loc_at: old_loc_at, ..State::default() });
    let changed = loc.as_ref().is_some_and(|l| old_loc.as_ref().is_none_or(|o| (o.lat - l.lat).abs() > 1e-6 || (o.lon - l.lon).abs() > 1e-6 || o.city != l.city));
    with_state(|s| {
        s.loc_err = loc.is_none();
        if let Some(l) = &loc {
            s.loc = Some(l.clone());
            if !was_fresh || manual || changed {
                s.loc_at = epoch(); // a successful (re)resolution starts the six-hour cache window
            }
        }
    });
    if loc.is_some() && (!was_fresh || manual || changed) {
        save_cache();
    }
    GEN.fetch_add(1, Relaxed);

    if let Some(l) = &loc {
        match fetch_weather(l) {
            Ok(w) => {
                crate::tlog(&format!("weather: {:.1} °C, code {} for {}", w.temp, w.code, l.city));
                with_state(|s| {
                    s.wx = Some(w);
                    s.wx_err = false;
                    s.updated = epoch();
                    s.hhmm = local_hhmm();
                });
                save_cache();
            }
            Err(e) => {
                crate::tlog(&format!("weather: forecast failed: {e}"));
                with_state(|s| s.wx_err = true);
            }
        }
    } else {
        with_state(|s| s.wx_err = true);
    }
    GEN.fetch_add(1, Relaxed);

    // One section at a time: a broken feed leaves the other two (and their previous headlines) alone. A short pause
    // between the three Google News requests keeps the free RSS endpoint from answering 429.
    let mut first_news = true;
    for cat in 0..3 {
        if !c.news_on(cat) {
            continue;
        }
        if !first_news {
            std::thread::sleep(Duration::from_millis(1200));
        }
        first_news = false;
        let mut result = fetch_news(cat, loc.as_ref());
        if result.as_ref().is_err_and(|e| e.contains("429")) {
            std::thread::sleep(Duration::from_millis(2500));
            result = fetch_news(cat, loc.as_ref());
        }
        match result {
            Ok(items) => {
                crate::tlog(&format!("weather: news {cat}: {} items", items.len()));
                with_state(|s| {
                    s.news[cat] = items;
                    s.news_err[cat] = false;
                });
            }
            Err(e) => {
                crate::tlog(&format!("weather: news {cat} failed: {e}"));
                with_state(|s| s.news_err[cat] = true);
            }
        }
        GEN.fetch_add(1, Relaxed);
    }

    BUSY.store(false, Relaxed);
    with_state(|s| {
        s.busy = false;
        if s.updated == 0 && s.wx.is_some() {
            s.updated = epoch();
            s.hhmm = local_hhmm();
        }
    });
    GEN.fetch_add(1, Relaxed);
}

// ---------- the bar item ----------

fn degree(v: f64, f: bool) -> i32 {
    let v = if f { v * 9.0 / 5.0 + 32.0 } else { v };
    v.round() as i32
}

fn temp_text(v: f64, f: bool) -> String {
    format!("{}°", degree(v, f))
}

/// What the bar paints: the glyph and temperature, or `☁` and `—` until there is data. `None` when the item is off.
pub(crate) fn bar_item() -> Option<(String, String, bool)> {
    let c = cfg();
    if !c.on {
        return None;
    }
    let wx = with_state(|s| s.wx.clone());
    Some(match wx {
        Some(w) => (weather_glyph(w.code).to_string(), temp_text(w.temp, c.f), true),
        None => ("☁".to_string(), "—".to_string(), false),
    })
}

thread_local! {
    /// Segoe UI Variable Text has no weather symbols and font linking picks a plain ring for `☀`; the symbol face has
    /// all of them. One cached font per UI thread and size (the bar repaints many times a second).
    static SYMFONT: Cell<(i32, HFONT)> = const { Cell::new((0, null_mut())) };
}

unsafe fn sym_font(px: i32) -> HFONT {
    let want = sc(px);
    SYMFONT.with(|c| {
        let (have, old) = c.get();
        if have == want && !old.is_null() {
            return old;
        }
        if !old.is_null() {
            DeleteObject(old);
        }
        let face = wide("Segoe UI Symbol");
        let f = CreateFontW(-want, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, face.as_ptr());
        c.set((want, f));
        f
    })
}

/// The bar's weather cell: a hover glow, the symbol and the temperature.
pub(crate) unsafe fn paint_bar(dc: HDC, rc: RECT, hot: bool, font: HFONT) {
    glow(dc, crate::WEATHER, hot, rc, pal().bg, pal().hover, sc(8));
    let Some((glyph, temp, data)) = bar_item() else { return };
    let col = if data { pal().text } else { mix(pal().dim, pal().text, 0.5) };
    let split = rc.left + sc(24);
    SelectObject(dc, sym_font(16));
    text(dc, &glyph, RECT { right: split, ..rc }, col, DT_CENTER);
    SelectObject(dc, font);
    text(dc, &temp, RECT { left: split - sc(2), ..rc }, col, DT_CENTER);
}

/// Right-click menu of the bar item.
pub(crate) unsafe fn bar_menu(hwnd: HWND) {
    let c = cfg();
    let items = vec![
        fly::MItem::new(t!("Обновить", "Refresh", ja: "更新", pt: "Atualizar", zh: "刷新")),
        fly::MItem::new("°C").on(!c.f),
        fly::MItem::new("°F").on(c.f),
        fly::MItem::new(t!("Открыть настройки", "Open settings", ja: "設定を開く", pt: "Abrir configurações", zh: "打开设置")).hint("weather.txt").sep(),
        fly::MItem::new(t!("Скрыть", "Hide", ja: "非表示", pt: "Ocultar", zh: "隐藏")),
    ];
    let cx = crate::tip_cx(&crate::bar_layout(hwnd), crate::WEATHER);
    fly::menu(cx, None, items, move |i| match i {
        0 => request_now(),
        1 => set_key("weather-unit", "c"),
        2 => set_key("weather-unit", "f"),
        3 => open_config(),
        4 => set_key("weather", "0"),
        _ => {}
    });
}

pub(crate) fn open_config() {
    ensure_config();
    let p = config_path();
    unsafe { crate::open(&wide(&p.to_string_lossy())) };
}

pub(crate) fn open_news(code: u8) {
    let cat = (code / 4) as usize;
    let idx = (code % 4) as usize;
    let link = with_state(|s| s.news.get(cat).and_then(|v| v.get(idx)).map(|i| i.link.clone()));
    if let Some(link) = link {
        unsafe { crate::open(&wide(&link)) };
    }
}

// ---------- the flyout ----------

const FLY_W: i32 = 420;

fn scale() -> f32 {
    crate::sc(1000) as f32 / 1000.0
}

fn max_logical_height() -> i32 {
    let screen = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    let px = (screen - crate::bar_h() - crate::sc(24)).max(320);
    (px as f32 / scale()) as i32
}

fn items_per_section() -> usize {
    let h = max_logical_height();
    if h >= 650 {
        3
    } else if h >= 548 {
        2
    } else {
        1
    }
}

fn news_title(cat: usize) -> String {
    match cat {
        0 => t!("Район", "Local", ja: "地域", pt: "Local", zh: "本地").to_string(),
        1 => t!("Страна", "Country", ja: "国", pt: "País", zh: "国家").to_string(),
        _ => t!("Рынок", "Market", ja: "マーケット", pt: "Mercado", zh: "市场").to_string(),
    }
}

/// Logical size of the weather flyout. Each enabled section reserves room for `items_per_section()` rows even when the
/// feed has not arrived yet, so the panel does not jump while the worker fills it.
pub(crate) fn fly_size() -> (i32, i32) {
    let c = cfg();
    let per = items_per_section() as i32;
    let mut h = 14 + 94 + 76 + 8 + 50;
    for cat in 0..3 {
        if c.news_on(cat) {
            h += 28 + 34 * per + 6;
        }
    }
    (FLY_W, h.clamp(300, max_logical_height()))
}

/// Called when the user opens the panel: asks for data only when it is older than five minutes.
pub(crate) fn opened() {
    request_if_stale(5 * 60);
}

struct RowLayout {
    cat: usize,
    idx: usize,
    rc: RECT,
}

struct SecLayout {
    cat: usize,
    title: RECT,
    rows: Vec<RowLayout>,
}

struct Geo {
    head: RECT,
    forecast: RECT,
    footer: i32,
    sections: Vec<SecLayout>,
}

fn geometry(w: i32, h: i32) -> Geo {
    let pad = sc(16);
    let head = RECT { left: pad, top: sc(12), right: w - pad, bottom: sc(12 + 86) };
    let ftop = sc(106);
    let forecast = RECT { left: pad, top: ftop, right: w - pad, bottom: ftop + sc(66) };
    let footer = h - sc(50);
    let c = cfg();
    let per = items_per_section();
    let mut y = forecast.bottom + sc(10);
    let mut sections = Vec::new();
    for cat in 0..3 {
        if !c.news_on(cat) {
            continue;
        }
        let need = sc(32 + 34 * per as i32);
        if y + need > footer - sc(4) {
            break;
        }
        let title = RECT { left: pad + sc(4), top: y, right: w - pad, bottom: y + sc(24) };
        y += sc(26);
        let mut rows = Vec::new();
        for idx in 0..per {
            let rc = RECT { left: pad, top: y, right: w - pad, bottom: y + sc(34) };
            rows.push(RowLayout { cat, idx, rc });
            y += sc(34);
        }
        y += sc(6);
        sections.push(SecLayout { cat, title, rows });
    }
    Geo { head, forecast, footer, sections }
}

pub(crate) fn rects(w: i32, h: i32) -> Vec<(RECT, fly::Act)> {
    let g = geometry(w, h);
    let mut out = Vec::new();
    for s in g.sections {
        for r in s.rows {
            out.push((r.rc, fly::Act::WeatherNews((r.cat * 4 + r.idx) as u8)));
        }
    }
    let by = h - sc(46);
    out.push((RECT { left: sc(16), top: by, right: sc(136), bottom: by + sc(36) }, fly::Act::WeatherRefresh));
    out.push((RECT { left: sc(144), top: by, right: sc(274), bottom: by + sc(36) }, fly::Act::WeatherSettings));
    out
}

fn day_label(i: usize, date: &str) -> String {
    match i {
        0 => t!("Сегодня", "Today", ja: "今日", pt: "Hoje", zh: "今天").to_string(),
        1 => t!("Завтра", "Tomorrow", ja: "明日", pt: "Amanhã", zh: "明天").to_string(),
        _ if date.len() >= 10 => format!("{}/{}", &date[8..10], &date[5..7]),
        _ => String::new(),
    }
}

unsafe fn pill(dc: HDC, rc: RECT, glyph: &str, label: &str, font: HFONT, gf: HFONT, active: bool) {
    surface(dc, rc, card(), sc(18));
    let col = if active { pal().text } else { mix(pal().dim, pal().text, 0.45) };
    SelectObject(dc, gf);
    text(dc, glyph, RECT { left: rc.left + sc(10), ..rc }, col, DT_LEFT);
    SelectObject(dc, font);
    text(dc, label, RECT { left: rc.left + sc(34), right: rc.right - sc(8), ..rc }, col, DT_LEFT);
}

pub(crate) unsafe fn paint(dc: HDC, cr: RECT, fonts: [HFONT; 7], card: u32, over: Option<fly::Act>) {
    let [f_txt, f_small, f_glyph, f_title, f_big, f_semi, _f_label] = fonts;
    let w = cr.right;
    let h = cr.bottom;
    let g = geometry(w, h);
    let s = snap();
    let c = cfg();
    let hot = |a: fly::Act| over == Some(a);
    let hl = |a: fly::Act, rc: RECT| glow(dc, rc.top * 4096 + rc.left + 1_000_000, hot(a), rc, pal().bg, pal().hover, sc(8));

    // ---- header: place, current condition, big temperature, today's range ----
    let place = s
        .loc
        .as_ref()
        .map(|l| {
            let mut v = l.city.clone();
            if v.is_empty() {
                v = t!("Моё местоположение", "My location", ja: "現在地", pt: "Minha localização", zh: "我的位置").to_string();
            }
            if !l.region.is_empty() {
                v.push_str(", ");
                v.push_str(&l.region);
            } else if !l.country.is_empty() {
                v.push_str(", ");
                v.push_str(&l.country);
            }
            v
        })
        .unwrap_or_else(|| t!("Нет местоположения", "No location", ja: "位置情報なし", pt: "Sem localização", zh: "无位置").to_string());
    SelectObject(dc, f_title);
    text(dc, &place, RECT { right: g.head.right, bottom: g.head.top + sc(26), ..g.head }, pal().text, DT_LEFT);
    if s.updated > 0 && !s.hhmm.is_empty() {
        SelectObject(dc, f_small);
        text(dc, &tf!("Обновлено {}", "Updated {}", ja: "更新 {}", pt: "Atualizado {}", zh: "更新于 {}", s.hhmm), RECT { left: g.head.left, right: g.head.right, top: g.head.top, bottom: g.head.top + sc(24) }, pal().dim, DT_RIGHT);
    }

    let (big, cond, range) = match (&s.wx, s.loc.is_some()) {
        (Some(wxd), _) => {
            let range = tf!(
                "Макс {}° · Мин {}°",
                "High {}° · Low {}°",
                ja: "最高 {}° · 最低 {}°",
                pt: "Máx {}° · Mín {}°",
                zh: "最高 {}° · 最低 {}°",
                degree(wxd.max[0], c.f),
                degree(wxd.min[0], c.f)
            );
            (temp_text(wxd.temp, c.f), weather_code_text(wxd.code).to_string(), range)
        }
        (None, true) => (
            "—".to_string(),
            if s.busy {
                t!("Загрузка…", "Loading…", ja: "読み込み中…", pt: "Carregando…", zh: "加载中…").to_string()
            } else if s.wx_err {
                t!("Не удалось загрузить погоду", "Could not load the weather", ja: "天気を取得できませんでした", pt: "Não foi possível carregar o tempo", zh: "无法加载天气").to_string()
            } else {
                t!("Нет данных о погоде", "No weather data", ja: "天気データなし", pt: "Sem dados do tempo", zh: "无天气数据").to_string()
            },
            String::new(),
        ),
        (None, false) => (
            "—".to_string(),
            if s.busy {
                t!("Поиск местоположения…", "Finding the location…", ja: "位置を取得中…", pt: "Localizando…", zh: "正在定位…").to_string()
            } else {
                t!(
                    "Разрешите доступ к геолокации или задайте weather-city в weather.txt",
                    "Allow location access or set weather-city in weather.txt",
                    ja: "位置情報を許可するか weather.txt に weather-city を設定してください",
                    pt: "Permita o acesso à localização ou defina weather-city no weather.txt",
                    zh: "请允许位置访问，或在 weather.txt 中设置 weather-city"
                )
                .to_string()
            },
            String::new(),
        ),
    };
    SelectObject(dc, f_big);
    text(dc, &big, RECT { left: g.head.left, right: g.head.left + sc(128), top: g.head.top + sc(24), bottom: g.head.top + sc(80) }, if s.wx.is_some() { pal().text } else { pal().dim }, DT_LEFT);
    SelectObject(dc, f_small);
    text(dc, &cond, RECT { left: g.head.left + sc(126), right: g.head.right, top: g.head.top + sc(38), bottom: g.head.top + sc(62) }, mix(pal().dim, pal().text, 0.45), DT_LEFT);
    if !range.is_empty() {
        text(dc, &range, RECT { left: g.head.left + sc(126), right: g.head.right, top: g.head.top + sc(60), bottom: g.head.top + sc(82) }, pal().dim, DT_LEFT);
    }

    // ---- three-day forecast ----
    surface(dc, g.forecast, card, sc(14));
    let third = (g.forecast.right - g.forecast.left) / 3;
    for i in 0..3 {
        let left = g.forecast.left + third * i as i32;
        let right = if i == 2 { g.forecast.right } else { left + third };
        if i > 0 {
            fill(dc, RECT { left, top: g.forecast.top + sc(10), right: left + 1, bottom: g.forecast.bottom - sc(10) }, pal().line, 0);
        }
        let rc = RECT { left, top: g.forecast.top, right, bottom: g.forecast.bottom };
        if let Some(wxd) = &s.wx {
            SelectObject(dc, f_small);
            text(dc, &day_label(i, &wxd.dates[i]), RECT { top: rc.top + sc(6), bottom: rc.top + sc(24), ..rc }, pal().dim, DT_CENTER);
            SelectObject(dc, sym_font(18));
            text(dc, weather_glyph(wxd.codes[i]), RECT { top: rc.top + sc(18), bottom: rc.top + sc(42), ..rc }, pal().text, DT_CENTER);
            SelectObject(dc, f_small);
            let mx = format!("{}°", degree(wxd.max[i], c.f));
            let mn = format!("{}°", degree(wxd.min[i], c.f));
            text(dc, &mx, RECT { left: rc.left, right: (rc.left + rc.right) / 2 + sc(2), top: rc.top + sc(42), bottom: rc.bottom - sc(2) }, pal().text, DT_RIGHT);
            text(dc, &mn, RECT { left: (rc.left + rc.right) / 2 + sc(2) + sc(2), right: rc.right, top: rc.top + sc(42), bottom: rc.bottom - sc(2) }, pal().dim, DT_LEFT);
        }
    }
    if s.wx.is_none() {
        SelectObject(dc, f_small);
        text(dc, t!("Нет прогноза", "No forecast", ja: "予報なし", pt: "Sem previsão", zh: "无预报"), g.forecast, pal().dim, DT_CENTER);
    }

    // ---- news sections: local, country, market ----
    for sec in &g.sections {
        SelectObject(dc, f_semi);
        text(dc, &news_title(sec.cat), sec.title, mix(pal().dim, pal().text, 0.35), DT_LEFT);
        let items = &s.news[sec.cat];
        for row in &sec.rows {
            let Some(it) = items.get(row.idx) else { continue };
            let a = fly::Act::WeatherNews((row.cat * 4 + row.idx) as u8);
            hl(a, row.rc);
            SelectObject(dc, f_txt);
            text(dc, &it.title, RECT { left: row.rc.left + sc(6), right: row.rc.right - sc(6), top: row.rc.top + sc(1), bottom: row.rc.top + sc(19) }, pal().text, DT_LEFT);
            SelectObject(dc, f_small);
            let meta = match (it.source.is_empty(), it.time.is_empty()) {
                (false, false) => format!("{} · {}", it.source, it.time),
                (false, true) => it.source.clone(),
                (true, false) => it.time.clone(),
                (true, true) => String::new(),
            };
            if !meta.is_empty() {
                text(dc, &meta, RECT { left: row.rc.left + sc(6), right: row.rc.right - sc(6), top: row.rc.top + sc(17), bottom: row.rc.bottom - sc(1) }, pal().dim, DT_LEFT);
            }
        }
        if items.is_empty() {
            let msg = if sec.cat == 0 && s.loc.as_ref().is_none_or(|l| l.city.is_empty() && c.city.is_empty()) {
                t!("Нет города для местных новостей", "No city for local news", ja: "地域ニュースの都市がありません", pt: "Sem cidade para notícias locais", zh: "没有本地新闻的城市").to_string()
            } else if s.busy {
                t!("Загрузка…", "Loading…", ja: "読み込み中…", pt: "Carregando…", zh: "加载中…").to_string()
            } else if s.news_err[sec.cat] {
                t!("Не удалось загрузить новости", "Could not load the news", ja: "ニュースを取得できませんでした", pt: "Não foi possível carregar as notícias", zh: "无法加载新闻").to_string()
            } else {
                t!("Нет заголовков", "No headlines", ja: "見出しなし", pt: "Sem manchetes", zh: "无标题").to_string()
            };
            SelectObject(dc, f_small);
            if let Some(row) = sec.rows.first() {
                text(dc, &msg, RECT { left: row.rc.left + sc(6), right: row.rc.right - sc(6), ..row.rc }, pal().dim, DT_LEFT);
            }
        }
    }

    // ---- footer: refresh, settings, when the data is from ----
    let by = h - sc(46);
    let refresh = RECT { left: sc(16), top: by, right: sc(136), bottom: by + sc(36) };
    let settings = RECT { left: sc(144), top: by, right: sc(274), bottom: by + sc(36) };
    let label = if s.busy {
        t!("Обновление…", "Updating…", ja: "更新中…", pt: "Atualizando…", zh: "更新中…")
    } else {
        t!("Обновить", "Refresh", ja: "更新", pt: "Atualizar", zh: "刷新")
    };
    hl(fly::Act::WeatherRefresh, refresh);
    pill(dc, refresh, "\u{E72C}", label, f_txt, f_glyph, true);
    hl(fly::Act::WeatherSettings, settings);
    pill(dc, settings, "\u{E713}", t!("Настройки", "Settings", ja: "設定", pt: "Configurações", zh: "设置"), f_txt, f_glyph, true);
    if s.updated > 0 && !s.hhmm.is_empty() {
        SelectObject(dc, f_small);
        text(dc, &s.hhmm, RECT { left: sc(280), right: w - sc(16), top: by, bottom: by + sc(36) }, pal().dim, DT_RIGHT);
    }
}
