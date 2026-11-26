//! What the widget shows: one row per module, laid out in one or two columns and drawn in the style the config picked.
//!
//! The renderer only composes strings and rectangles; `metrics` collects the numbers on its own thread. Adding a style is
//! one variant in `config::Style` and one arm in `draw_row`.
use crate::{
    canvas::{Align, Canvas, Fonts},
    config::{Config, Style, CPU, DISK, GPU, IO, NET, RAM, TEMP, UPTIME},
    metrics::Sample,
};
use winrust_common::style::pal;

const DASH: &str = "—";
const MAX_POINTS: usize = 60;

/// A small rolling history per module, and the decaying peak that scales the auto-ranged series (I/O, network).
pub struct Series {
    buf: [[f32; MAX_POINTS]; 8],
    head: usize,
    len: usize,
    peak: [f32; 8],
}

impl Default for Series {
    fn default() -> Self {
        Series { buf: [[0.0; MAX_POINTS]; 8], head: 0, len: 0, peak: [1.0; 8] }
    }
}

impl Series {
    pub fn push(&mut self, s: &Sample) {
        self.head = (self.head + 1) % MAX_POINTS;
        self.len = (self.len + 1).min(MAX_POINTS);
        self.buf[CPU][self.head] = s.cpu;
        self.buf[GPU][self.head] = s.gpu.unwrap_or(0.0);
        self.buf[RAM][self.head] = ram_frac(s) * 100.0;
        self.buf[IO][self.head] = s.io.map_or(0.0, |(r, w)| (r + w) as f32);
        self.buf[NET][self.head] = s.net.map_or(0.0, |(d, u)| (d + u) as f32);
        self.buf[TEMP][self.head] = s.temp_cpu.unwrap_or(0.0);
        for i in [IO, NET] {
            // no data still pushes 0: a month of silence must not keep the bar tiny
            self.peak[i] = (self.peak[i] * 0.93).max(self.buf[i][self.head]).max(0.5);
        }
    }

    /// The last `MAX_POINTS` values, oldest first.
    pub fn copy(&self, module: usize) -> ([f32; MAX_POINTS], usize) {
        let mut out = [0.0f32; MAX_POINTS];
        let n = self.len.min(MAX_POINTS);
        for k in 0..n {
            out[k] = self.buf[module][(self.head + MAX_POINTS + 1 - n + k) % MAX_POINTS];
        }
        (out, n)
    }

    pub fn max(&self, module: usize) -> f32 {
        match module {
            IO | NET => self.peak[module],
            _ => 100.0,
        }
    }
}

fn ram_frac(s: &Sample) -> f32 {
    if s.ram_total == 0 { 0.0 } else { (s.ram_used as f64 / s.ram_total as f64) as f32 }
}

fn num(v: f64, dec: usize) -> String {
    let s = format!("{:.*}", dec, v);
    if dot!() { s } else { s.replace('.', ",") }
}

fn pct(v: f32, cfg: &Config) -> String {
    format!("{}%", num(v as f64, cfg.decimals))
}

fn size(b: u64) -> String {
    let gb = b as f64 / 1_073_741_824.0;
    if gb >= 100.0 { format!("{} GB", num(gb, 0)) } else { format!("{} GB", num(gb, 1)) }
}

/// `R 1,2  W 0,3 MB/s`: one unit for the pair, and the order is readable without a second line.
fn speed_pair(a: f64, b: f64, cfg: &Config, al: &str, bl: &str) -> String {
    let dec = if a.max(b) >= 100.0 { 0 } else { cfg.decimals.max(1) };
    let s = format!("{al} {}  {bl} {}", num(a, dec), num(b, dec));
    if cfg.units { format!("{s} MB/s") } else { s }
}

fn temp_value(c: f32, cfg: &Config) -> String {
    let n = num(c as f64, cfg.decimals);
    if cfg.units { format!("{n} °C") } else { n }
}

fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    if d > 0 {
        tf!("{} д {} ч", "{}d {}h", d, h)
    } else if h > 0 {
        tf!("{} ч {} м", "{}h {}m", h, m)
    } else {
        tf!("{} мин", "{}m", m)
    }
}

/// One line of the widget.
#[derive(Clone)]
pub struct Row {
    pub module: usize,
    pub style: Style,
    pub label: String,
    pub value: String,
    /// A small dim text after the label (a disk's numbers, "down / up", ...).
    pub sub: Option<String>,
    /// A second dim line under a bar (the disks' used / free numbers).
    pub foot: Option<String>,
    /// The magnitude: 0..1 for percentages, MB/s for the auto-ranged I/O and network rows.
    pub frac: Option<f32>,
    pub auto: bool,
    /// A strip of one mini bar per core instead of the module's own row.
    pub cores: bool,
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 16 { format!("{}…", s.chars().take(15).collect::<String>()) } else { s.to_string() }
}

fn row(module: usize, style: Style, label: String, value: String) -> Row {
    Row { module, style, label, value, sub: None, foot: None, frac: None, auto: false, cores: false }
}

/// The rows the current settings and numbers call for, before they are placed.
fn rows(cfg: &Config, s: &Sample) -> Vec<Row> {
    let mut v = Vec::with_capacity(10);
    if cfg.on[CPU] {
        let mut r = row(CPU, cfg.style[CPU], cfg.label[CPU].clone(), pct(s.cpu, cfg));
        r.frac = Some((s.cpu / 100.0).clamp(0.0, 1.0));
        v.push(r);
        if cfg.cores && !s.cores.is_empty() {
            let avg = s.cores.iter().sum::<f32>() / s.cores.len().max(1) as f32;
            let mut c = row(CPU, Style::Bar, t!("Ядра", "Cores", ja: "コア", pt: "Núcleos", zh: "核心").to_string(), String::new());
            c.cores = true;
            c.frac = Some((avg / 100.0).clamp(0.0, 1.0));
            v.push(c);
        }
    }
    if cfg.on[GPU] {
        let mut r = row(GPU, cfg.style[GPU], cfg.label[GPU].clone(), DASH.to_string());
        if let Some(u) = s.gpu {
            r.value = pct(u, cfg);
            r.frac = Some((u / 100.0).clamp(0.0, 1.0));
            r.sub = s.gpu_mem.map(|m| format!("VRAM {}", size(m)));
        } else {
            r.frac = Some(0.0);
        }
        v.push(r);
    }
    if cfg.on[RAM] {
        let f = ram_frac(s);
        let mut r = row(RAM, cfg.style[RAM], cfg.label[RAM].clone(), pct(f * 100.0, cfg));
        r.sub = Some(format!("{} / {}", size(s.ram_used), size(s.ram_total)));
        r.frac = Some(f);
        v.push(r);
    }
    if cfg.on[DISK] {
        for d in &s.disks {
            let f = if d.total == 0 { 0.0 } else { (d.used as f64 / d.total as f64) as f32 };
            let name = if d.label.is_empty() { format!("{}:", d.letter) } else { format!("{}: {}", d.letter, short(&d.label)) };
            let detail = format!("{} / {} · {} {}", size(d.used), size(d.total), size(d.total.saturating_sub(d.used)), t!("свободно", "free"));
            let mut r = row(DISK, cfg.style[DISK], name, pct(f * 100.0, cfg));
            if cfg.style[DISK] == Style::Bar {
                r.foot = Some(detail);
            } else {
                r.sub = Some(detail);
            }
            r.frac = Some(f);
            v.push(r);
        }
    }
    if cfg.on[IO] {
        let (value, total) = match s.io {
            Some((r, w)) => (speed_pair(r, w, cfg, "R", "W"), (r + w) as f32),
            None => (format!("{DASH} / {DASH}"), 0.0),
        };
        let mut r = row(IO, cfg.style[IO], cfg.label[IO].clone(), value);
        r.sub = None;
        r.frac = Some(total);
        r.auto = true;
        v.push(r);
    }
    if cfg.on[NET] {
        let (value, total) = match s.net {
            Some((d, u)) => (speed_pair(d, u, cfg, "↓", "↑"), (d + u) as f32),
            None => (format!("{DASH} / {DASH}"), 0.0),
        };
        let mut r = row(NET, cfg.style[NET], cfg.label[NET].clone(), value);
        r.sub = None;
        r.frac = Some(total);
        r.auto = true;
        v.push(r);
    }
    if cfg.on[TEMP] {
        let mut r = row(TEMP, cfg.style[TEMP], cfg.label[TEMP].clone(), DASH.to_string());
        match s.temp_cpu {
            Some(c) => {
                r.value = temp_value(c, cfg);
                r.frac = Some((c / 100.0).clamp(0.0, 1.0));
            }
            None => r.frac = Some(0.0),
        }
        r.sub = s.temp_gpu.map(|g| format!("GPU {}", temp_value(g, cfg)));
        v.push(r);
    }
    if cfg.on[UPTIME] {
        let mut r = row(UPTIME, cfg.style[UPTIME], cfg.label[UPTIME].clone(), uptime(s.uptime));
        // no magnitude: whatever style was picked, this reads best as plain text
        r.style = Style::Text;
        v.push(r);
    }
    v
}

fn sc(v: f32, scale: f32) -> i32 {
    (v * scale).round() as i32
}

/// The effective style: a bar, ring or spark with nothing to measure (uptime, a missing sensor) reads as text.
fn style_of(r: &Row) -> Style {
    if r.cores {
        return Style::Bar;
    }
    if r.frac.is_none() && matches!(r.style, Style::Bar | Style::Ring | Style::Spark) { Style::Text } else { r.style }
}

fn design_height(r: &Row) -> f32 {
    match style_of(r) {
        Style::Bar => if r.foot.is_some() { 48.0 } else { 36.0 },
        Style::Ring => 54.0,
        Style::Text => 24.0,
        Style::Spark => 40.0,
        Style::Pill => 26.0,
    }
}

fn height(r: &Row, scale: f32) -> i32 {
    if r.cores { sc(22.0, scale) } else { sc(design_height(r), scale) }
}

pub struct Item {
    pub row: Row,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

pub struct Plan {
    pub w: i32,
    pub h: i32,
    pub shadow: i32,
    pub pad: i32,
    pub radius: f32,
    pub items: Vec<Item>,
}

/// Measures the widget for these settings: the window is exactly this size, shadow margin included.
pub fn build(cfg: &Config, s: &Sample, scale: f32) -> Plan {
    let rows = rows(cfg, s);
    let cols = cfg.cols.clamp(1, 2) as usize;
    let pad = sc(cfg.pad as f32, scale);
    let gap = sc(cfg.gap as f32, scale);
    let shadow = if cfg.shadow { sc(10.0, scale) } else { 0 };
    let col_w = sc(cfg.width as f32, scale);
    let content_w = col_w * cols as i32 + gap * (cols as i32 - 1);
    let nrows = rows.len().div_ceil(cols).max(1);
    let mut rh = vec![0i32; nrows];
    for (i, r) in rows.iter().enumerate() {
        let k = i / cols;
        rh[k] = rh[k].max(height(r, scale));
    }
    let content_h: i32 = if rows.is_empty() { sc(26.0, scale) } else { rh.iter().sum::<i32>() + gap * (nrows as i32 - 1) };
    let w = content_w + pad * 2 + shadow * 2;
    let h = content_h + pad * 2 + shadow * 2;
    let mut items = Vec::with_capacity(rows.len());
    let mut top = shadow + pad;
    for (k, &row_h) in rh.iter().enumerate() {
        for (j, r) in rows.iter().enumerate().filter(|(j, _)| j / cols == k) {
            let col = j % cols;
            let x = shadow + pad + col as i32 * (col_w + gap);
            items.push(Item { row: r.clone(), x, y: top, w: col_w, h: height(r, scale) });
        }
        top += row_h + gap;
    }
    Plan { w, h, shadow, pad, radius: (14.0 * scale).round(), items }
}

fn frac_of(r: &Row, series: &Series) -> f32 {
    let raw = r.frac.unwrap_or(0.0);
    if r.auto { (raw / series.max(r.module)).clamp(0.0, 1.0) } else { raw.clamp(0.0, 1.0) }
}

/// Paints the card itself (shadow, rim, body) into `cv`. Cached by the window: it only changes with the size or the style.
pub fn draw_bg(cv: &mut Canvas, plan: &Plan, cfg: &Config) {
    let p = pal();
    cv.clear();
    let (sw, sh) = (plan.w as f32, plan.h as f32);
    let m = plan.shadow as f32;
    let (cx, cy, cw, ch) = (m, m, sw - 2.0 * m, sh - 2.0 * m);
    if plan.shadow > 0 {
        // a cheap soft shadow: four concentric rounded rectangles, the outer ones fainter
        for (i, a) in [16, 20, 24, 28].into_iter().enumerate() {
            let e = (4 - i) as f32 * (plan.shadow as f32 / 5.0);
            cv.round_rect(cx - e + 1.0, cy - e + 2.0, cw + 2.0 * e, ch + 2.0 * e, plan.radius + e, 0x000000, a);
        }
    }
    // the card: a hairline rim of the style's line colour, then the translucent body
    cv.round_rect(cx, cy, cw, ch, plan.radius, p.line, cfg.opacity as u32);
    cv.round_rect(cx + 1.0, cy + 1.0, cw - 2.0, ch - 2.0, (plan.radius - 1.0).max(0.0), p.bg, cfg.opacity as u32);
}

/// Paints the rows (and the "no modules" hint) over the cached background.
pub fn draw_content(cv: &mut Canvas, plan: &Plan, cfg: &Config, s: &Sample, series: &Series, fonts: &Fonts) {
    let p = pal();
    if plan.items.is_empty() {
        cv.text(
            (plan.pad as i32 + plan.shadow, plan.pad as i32 + plan.shadow, plan.w - plan.pad * 2 - plan.shadow * 2, plan.h - plan.pad * 2 - plan.shadow * 2),
            t!("Нет модулей", "No modules", ja: "モジュールなし", pt: "Sem módulos", zh: "无模块"),
            fonts.label,
            p.dim,
            220,
            Align::Center,
        );
        return;
    }
    for item in &plan.items {
        draw_row(cv, item, cfg, s, series, fonts);
    }
}

fn draw_row(cv: &mut Canvas, it: &Item, cfg: &Config, s: &Sample, series: &Series, fonts: &Fonts) {
    let p = pal();
    let (x, y, w, h) = (it.x, it.y, it.w.max(8), it.h.max(8));
    let r = &it.row;
    if r.cores {
        draw_cores(cv, x, y, w, h, s, fonts, cfg);
        return;
    }
    let style = style_of(r);
    let accent = cfg.accent;
    let (text, dim) = (p.text, p.dim);
    let u = h as f32 / design_height(r);
    let f = frac_of(r, series);
    match style {
        Style::Bar => {
            let band = (24.0 * u) as i32;
            let vw = cv.measure(fonts.value, &r.value);
            let value_w = (vw + (8.0 * u) as i32).clamp((w as f32 * 0.30) as i32, (w as f32 * 0.68) as i32);
            let vfont = if vw > value_w - 4 { fonts.mini } else { fonts.value };
            cv.text((x, y, w - value_w, band), &r.label, fonts.label, dim, 205, Align::Left);
            if let Some(sub) = &r.sub {
                let lw = cv.measure(fonts.label, &r.label);
                let sx = x + lw + (7.0 * u) as i32;
                let avail = x + w - value_w - sx - (4.0 * u) as i32;
                if avail > cv.measure(fonts.small, sub) {
                    cv.text((sx, y, avail, band), sub, fonts.small, dim, 150, Align::Left);
                }
            }
            cv.text((x + w - value_w, y, value_w, band), &r.value, vfont, text, 250, Align::Right);
            let ty = y + (28.0 * u) as i32;
            let th = (6.0 * u).max(3.0) as i32;
            let rr = th as f32 / 2.0;
            cv.round_rect(x as f32, ty as f32, w as f32, th as f32, rr, dim, 46);
            if f > 0.001 {
                let fw = (w as f32 * f).max(th as f32).min(w as f32);
                cv.round_rect(x as f32, ty as f32, fw, th as f32, rr.min(fw / 2.0), accent, 255);
            }
            if let Some(foot) = &r.foot {
                cv.text((x, y + (36.0 * u) as i32, w, (12.0 * u) as i32), foot, fonts.small, dim, 165, Align::Left);
            }
        }
        Style::Text => {
            let vw = cv.measure(fonts.value, &r.value);
            let value_w = (vw + 8).clamp((w as f32 * 0.30) as i32, (w as f32 * 0.68) as i32);
            let vfont = if vw > value_w - 4 { fonts.mini } else { fonts.value };
            cv.text((x, y, w - value_w - 6, h), &r.label, fonts.label, dim, 205, Align::Left);
            if let Some(sub) = &r.sub {
                let lw = cv.measure(fonts.label, &r.label);
                let sx = x + lw + 7;
                if sx + cv.measure(fonts.small, sub) < x + w - value_w {
                    cv.text((sx, y, x + w - value_w - sx - 4, h), sub, fonts.small, dim, 150, Align::Left);
                }
            }
            cv.text((x + w - value_w, y, value_w, h), &r.value, vfont, text, 250, Align::Right);
        }
        Style::Pill => {
            cv.round_rect(x as f32, y as f32, w as f32, h as f32, h as f32 / 2.0, text, 24);
            let vu = h as f32 / 26.0;
            let dot = (3.5 * vu) as i32;
            let cyd = y + h / 2;
            cv.round_rect(x as f32 + 12.0 * vu - dot as f32, (cyd - dot) as f32, (dot * 2) as f32, (dot * 2) as f32, dot as f32, accent, 255);
            let value_w = (w as f32 * 0.55) as i32;
            cv.text((x + (24.0 * vu) as i32, y, w - value_w - (30.0 * vu) as i32, h), &r.label, fonts.label, dim, 210, Align::Left);
            cv.text((x + w - value_w - (10.0 * vu) as i32, y, value_w, h), &r.value, fonts.mini, text, 250, Align::Right);
        }
        Style::Spark => {
            let band = (20.0 * u) as i32;
            cv.text((x, y, w / 2, band), &r.label, fonts.label, dim, 205, Align::Left);
            cv.text((x + w / 2, y, w / 2, band), &r.value, fonts.mini, text, 250, Align::Right);
            let (mut vals, n) = series.copy(r.module);
            if n >= 2 {
                let top = y + (22.0 * u) as i32;
                let sh = y + h - top;
                let max = series.max(r.module);
                cv.spark(x as f32, top as f32, w as f32, sh as f32, &mut vals[..n], max, accent);
            }
        }
        Style::Ring => {
            let ru = h as f32 / 54.0;
            let rad = (17.0 * ru).max(6.0);
            let thick = (4.0 * ru).max(1.5);
            let ccx = x as f32 + rad + thick + 1.0;
            let ccy = y as f32 + h as f32 / 2.0;
            cv.ring(ccx, ccy, rad, thick, f, accent, p.line);
            let inner = (rad * 2.0) as i32;
            let number = if r.value == DASH { DASH.to_string() } else { num((f * 100.0) as f64, 0) };
            cv.text((ccx as i32 - inner / 2 - 2, ccy as i32 - (9.0 * ru) as i32, inner + 4, (18.0 * ru) as i32), &number, fonts.mini, text, 255, Align::Center);
            let tx = ccx as i32 + inner / 2 + (9.0 * ru) as i32;
            let tw = (x + w - tx).max(0);
            cv.text((tx, y + (4.0 * ru) as i32, tw, (16.0 * ru) as i32), &r.label, fonts.label, dim, 205, Align::Left);
            cv.text((tx, y + (18.0 * ru) as i32, tw, (24.0 * ru) as i32), &r.value, fonts.value, text, 250, Align::Left);
            if let Some(sub) = &r.sub {
                cv.text((tx, y + (40.0 * ru) as i32, tw, (13.0 * ru) as i32), sub, fonts.small, dim, 160, Align::Left);
            }
        }
    }
}

fn draw_cores(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32, s: &Sample, fonts: &Fonts, cfg: &Config) {
    let p = pal();
    let u = h as f32 / 22.0;
    cv.text((x, y, w, (13.0 * u) as i32), t!("Ядра", "Cores", ja: "コア", pt: "Núcleos", zh: "核心"), fonts.small, p.dim, 170, Align::Left);
    let n = s.cores.len();
    if n == 0 {
        return;
    }
    let gap = (2.0 * u) as i32;
    let top = y + (14.0 * u) as i32;
    let bh = (6.0 * u).max(2.0) as i32;
    let seg = ((w - gap * (n as i32 - 1)) / n as i32).max(1);
    for (i, v) in s.cores.iter().enumerate() {
        let sx = x + i as i32 * (seg + gap);
        let ww = seg.min(x + w - sx).max(1);
        let rr = (bh as f32 / 2.0).min(ww as f32 / 2.0);
        cv.round_rect(sx as f32, top as f32, ww as f32, bh as f32, rr, p.dim, 40);
        let f = (v / 100.0).clamp(0.0, 1.0);
        if f > 0.005 {
            let fw = (ww as f32 * f).max(bh as f32).min(ww as f32);
            cv.round_rect(sx as f32, top as f32, fw, bh as f32, rr.min(fw / 2.0), cfg.accent, 255);
        }
    }
}
