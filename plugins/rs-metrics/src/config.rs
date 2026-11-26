//! `%ProgramData%\WinRust\metrics.txt`: the widget's settings, one `key=value` per line, like `look.txt`.
//!
//! The file is read at start, re-read by the window when its timestamp changes (once a second) and rewritten by the
//! right-click menu. Unknown keys are kept, so a hand edit of a key this version does not know yet survives a menu click.
use std::{
    collections::HashMap,
    path::PathBuf,
};

use winrust_common::style;

/// The modules, in the order they appear in the file and by default on screen.
pub const MODS: [&str; 8] = ["cpu", "gpu", "ram", "disk", "io", "net", "temp", "uptime"];
pub const CPU: usize = 0;
pub const GPU: usize = 1;
pub const RAM: usize = 2;
pub const DISK: usize = 3;
pub const IO: usize = 4;
pub const NET: usize = 5;
pub const TEMP: usize = 6;
pub const UPTIME: usize = 7;

/// How a module is drawn. One match arm in `render::draw_one` per variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Bar,
    Ring,
    Text,
    Spark,
    Pill,
}

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        match s.trim().to_ascii_lowercase().as_str() {
            "bar" => Some(Style::Bar),
            "ring" => Some(Style::Ring),
            "text" => Some(Style::Text),
            "spark" => Some(Style::Spark),
            "pill" => Some(Style::Pill),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Style::Bar => "bar",
            Style::Ring => "ring",
            Style::Text => "text",
            Style::Spark => "spark",
            Style::Pill => "pill",
        }
    }

    pub fn all() -> [Style; 5] {
        [Style::Bar, Style::Ring, Style::Text, Style::Spark, Style::Pill]
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub on: [bool; 8],
    pub style: [Style; 8],
    pub label: [String; 8],
    pub x: i32,
    pub y: i32,
    /// Content width of one column, in 96 dpi pixels.
    pub width: i32,
    pub cols: i32,
    pub gap: i32,
    /// Inner padding of the card.
    pub pad: i32,
    /// 0..=255, the alpha of the card itself.
    pub opacity: u8,
    pub accent: u32, // COLORREF
    pub decimals: usize,
    pub units: bool,
    pub disk_letters: Vec<char>,
    pub net_iface: String,
    pub cores: bool,
    pub snap: bool,
    pub shadow: bool,
    /// 0 = live on the desktop (child of the WinRust desktop window), 1 = stay above every window.
    pub topmost: bool,
    /// The Widgets page's on/off: hidden but still running when false, so turning it back on is instant.
    pub enabled: bool,
    /// Every pair read, so a save keeps keys this version does not know.
    map: HashMap<String, String>,
}

pub fn file() -> Option<PathBuf> {
    std::env::var_os("ProgramData").map(|p| PathBuf::from(p).join("WinRust").join("metrics.txt"))
}

fn parse(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string()))
        .collect()
}

/// "C,D" -> ['C', 'D']; anything that is not a letter is skipped.
fn letters(v: &str) -> Vec<char> {
    let mut l: Vec<char> = v
        .split([',', ';', ' '])
        .filter_map(|s| s.trim().chars().next())
        .map(|c| c.to_ascii_uppercase())
        .filter(|c| c.is_ascii_alphabetic())
        .collect();
    l.dedup();
    l
}

pub fn color_hex(c: u32) -> String {
    format!("#{:02X}{:02X}{:02X}", c & 255, (c >> 8) & 255, (c >> 16) & 255)
}

fn color(v: &str, default: u32) -> u32 {
    let t = v.trim().trim_start_matches('#').trim_start_matches("0x").trim_start_matches("0X");
    if t.len() != 6 {
        return default;
    }
    match u32::from_str_radix(t, 16) {
        Ok(n) if n <= 0xFFFFFF => (n & 255) << 16 | (n & 0xFF00) | (n >> 16) & 255,
        _ => default,
    }
}

fn flag(map: &HashMap<String, String>, key: &str, default: bool) -> bool {
    map.get(key).map_or(default, |v| v.parse::<i32>().map(|n| n != 0).unwrap_or(default))
}

fn num(map: &HashMap<String, String>, key: &str, default: i32, min: i32, max: i32) -> i32 {
    map.get(key).and_then(|v| v.parse::<i32>().ok()).map_or(default, |v| v.clamp(min, max))
}

/// The labels a fresh file gets: short, and translated where the language has a word for it.
fn default_label(i: usize) -> String {
    match i {
        CPU => "CPU".into(),
        GPU => "GPU".into(),
        RAM => "RAM".into(),
        DISK => t!("Диск", "Disk").into(),
        IO => t!("Диск", "Disk IO").into(),
        NET => t!("Сеть", "Network").into(),
        TEMP => t!("Темп", "Temp").into(),
        _ => t!("Аптайм", "Uptime").into(),
    }
}

fn defaults() -> Config {
    Config {
        on: [true; 8],
        style: [
            Style::Ring,
            Style::Bar,
            Style::Bar,
            Style::Bar,
            Style::Bar,
            Style::Bar,
            Style::Text,
            Style::Text,
        ],
        label: std::array::from_fn(default_label),
        x: 60,
        y: 60,
        width: 230,
        cols: 1,
        gap: 8,
        pad: 14,
        opacity: 222,
        accent: style::pal().accent,
        decimals: 0,
        units: true,
        disk_letters: vec!['C', 'D'],
        net_iface: "auto".into(),
        cores: false,
        snap: true,
        shadow: true,
        topmost: false,
        enabled: true,
        map: HashMap::new(),
    }
}

/// Reads the file (missing or unreadable: the defaults). A missing file is created so "Open config" always has one.
pub fn read() -> Config {
    let Some(f) = file() else { return defaults() };
    let text = std::fs::read_to_string(&f).unwrap_or_default();
    let mut c = defaults();
    if text.trim().is_empty() {
        c.save();
        return c;
    }
    c.map = parse(&text);
    let m = &c.map;
    for i in 0..8 {
        c.on[i] = flag(m, MODS[i], c.on[i]);
        c.style[i] = m.get(&format!("style-{}", MODS[i])).and_then(|v| Style::parse(v)).unwrap_or(c.style[i]);
        if let Some(v) = m.get(&format!("label-{}", MODS[i])).filter(|v| !v.trim().is_empty()) {
            c.label[i] = v.chars().take(24).collect();
        }
    }
    c.x = num(m, "x", c.x, -20000, 20000);
    c.y = num(m, "y", c.y, -20000, 20000);
    c.width = num(m, "width", c.width, 150, 460);
    c.cols = num(m, "cols", c.cols, 1, 2);
    c.gap = num(m, "gap", c.gap, 0, 32);
    c.pad = num(m, "pad", c.pad, 6, 32);
    c.opacity = num(m, "opacity", c.opacity as i32, 40, 255) as u8;
    c.accent = m.get("accent").map_or(c.accent, |v| color(v, c.accent));
    c.decimals = num(m, "decimals", c.decimals as i32, 0, 2) as usize;
    c.units = flag(m, "units", c.units);
    c.disk_letters = m.get("disk-letters").map(|v| letters(v)).filter(|l| !l.is_empty()).unwrap_or(c.disk_letters);
    c.net_iface = m.get("net-iface").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or(c.net_iface);
    c.cores = flag(m, "cores", c.cores);
    c.snap = flag(m, "snap", c.snap);
    c.shadow = flag(m, "shadow", c.shadow);
    c.topmost = flag(m, "topmost", c.topmost);
    c.enabled = flag(m, "enabled", c.enabled);
    c
}

impl Config {
    fn pairs(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = Vec::with_capacity(40);
        for i in 0..8 {
            v.push((MODS[i].to_string(), i32::from(self.on[i]).to_string()));
            v.push((format!("style-{}", MODS[i]), self.style[i].key().to_string()));
            v.push((format!("label-{}", MODS[i]), self.label[i].clone()));
        }
        for (k, val) in [
            ("x", self.x.to_string()),
            ("y", self.y.to_string()),
            ("width", self.width.to_string()),
            ("cols", self.cols.to_string()),
            ("gap", self.gap.to_string()),
            ("pad", self.pad.to_string()),
            ("opacity", self.opacity.to_string()),
            ("accent", color_hex(self.accent)),
            ("decimals", self.decimals.to_string()),
            ("units", i32::from(self.units).to_string()),
            ("disk-letters", self.disk_letters.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(",")),
            ("net-iface", self.net_iface.clone()),
            ("cores", i32::from(self.cores).to_string()),
            ("snap", i32::from(self.snap).to_string()),
            ("shadow", i32::from(self.shadow).to_string()),
            ("topmost", i32::from(self.topmost).to_string()),
        ("enabled", i32::from(self.enabled).to_string()),
        ] {
            v.push((k.to_string(), val));
        }
        v
    }

    /// Writes every key back, unknown ones included, sorted.
    pub fn save(&mut self) {
        let Some(f) = file() else { return };
        for (k, v) in self.pairs() {
            self.map.insert(k, v);
        }
        let mut keys: Vec<&String> = self.map.keys().collect();
        keys.sort();
        let mut text = String::from(
            "# rs-metrics. key=value per line, applied live (the widget checks the file once a second).\n\
             # modules: cpu gpu ram disk io net temp uptime  (1 = on, 0 = off)\n\
             # styles: bar ring text spark pill   (style-cpu=ring)\n\
             # edit a label with label-cpu=CPU, the accent with accent=#0A84FF\n\
             # topmost=0 lives on the desktop (behind the app windows); topmost=1 stays above them\n\
             # enabled=1 shows the widget; enabled=0 hides it (it keeps running, so it comes back instantly)\n",
        );
        for k in keys {
            text.push_str(k);
            text.push('=');
            text.push_str(&self.map[k]);
            text.push('\n');
        }
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        // write-then-rename: the widget itself may be reading the file at this very moment
        let tmp = f.with_extension("txt.tmp");
        let _ = std::fs::write(&tmp, text);
        let _ = std::fs::rename(&tmp, &f);
    }

    /// One single value (a menu click), saved. The caller's `Config` is this one, so the fields are updated too.
    pub fn set(&mut self, key: &str, value: &str) {
        self.map.insert(key.to_string(), value.to_string());
        for i in 0..8 {
            if key == MODS[i] {
                self.on[i] = value != "0";
            } else if key == format!("style-{}", MODS[i]) {
                if let Some(s) = Style::parse(value) {
                    self.style[i] = s;
                }
            } else if key == format!("label-{}", MODS[i]) {
                self.label[i] = value.chars().take(24).collect();
            }
        }
        match key {
            "x" | "y" | "width" | "cols" | "gap" | "pad" | "opacity" | "decimals" => {
                let n = value.parse::<i32>().unwrap_or(0);
                match key {
                    "x" => self.x = n,
                    "y" => self.y = n,
                    "width" => self.width = n.clamp(150, 460),
                    "cols" => self.cols = n.clamp(1, 2),
                    "gap" => self.gap = n.clamp(0, 32),
                    "pad" => self.pad = n.clamp(6, 32),
                    "opacity" => self.opacity = n.clamp(40, 255) as u8,
                    _ => self.decimals = n.clamp(0, 2) as usize,
                }
            }
            "accent" => self.accent = color(value, self.accent),
            "snap" => self.snap = value != "0",
            "shadow" => self.shadow = value != "0",
            "units" => self.units = value != "0",
            "cores" => self.cores = value != "0",
            "topmost" => self.topmost = value != "0",
            "net-iface" => self.net_iface = value.to_string(),
            "disk-letters" => {
                let l = letters(value);
                if !l.is_empty() {
                    self.disk_letters = l;
                }
            }
            _ => {}
        }
        self.save();
    }
}
