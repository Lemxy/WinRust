//! Layout and painting: one rounded, frosted pill per module, stacked in one or two columns; every style is one arm in
//! `draw_row`. The window itself is transparent — only the pills have a background — and a detached module is the same
//! pill in a window of its own.
use crate::{
    canvas::{Align, Canvas, Fonts},
    config::{Config, Style, CPU, DISK, GPU, IO, NET, RAM, TEMP, UPTIME},
    frost,
    metrics::Sample,
    temps::Temps,
};
use winrust_common::style::pal;

const DASH: &str = "—";
const MAX_POINTS: usize = 60;
/// Pill padding, at 96 dpi.
const PX: f32 = 12.0;
const PT: f32 = 10.0;
const PB: f32 = 10.0;
const ROW_GAP: f32 = 4.0;
const MARGIN: f32 = 6.0;

// ---------- history ----------

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
        self.buf[TEMP][self.head] = s.cpu; // unused by the list, kept for the spark style of a future CPU row
        for i in [IO, NET] {
            self.peak[i] = (self.peak[i] * 0.93).max(self.buf[i][self.head]).max(0.5);
        }
    }

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

// ---------- strings ----------

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

// ---------- rows ----------

#[derive(Clone)]
pub struct Row {
    pub module: usize,
    pub style: Style,
    pub label: String,
    pub value: String,
    pub sub: Option<String>,
    pub foot: Option<String>,
    /// 0..1 for percentages, MB/s for the auto-ranged I/O and network rows.
    pub frac: Option<f32>,
    pub auto: bool,
    pub cores: bool,
    /// One line of a list (the temperatures): smaller fonts, 22 px.
    pub compact: bool,
}

fn row(module: usize, style: Style, label: String, value: String) -> Row {
    Row { module, style, label, value, sub: None, foot: None, frac: None, auto: false, cores: false, compact: false }
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 16 { format!("{}…", s.chars().take(15).collect::<String>()) } else { s.to_string() }
}

/// The rows of one module (one pill). The temperature module is a list of components.
fn module_rows(m: usize, cfg: &Config, s: &Sample, t: &Temps) -> Vec<Row> {
    let mut v = Vec::with_capacity(3);
    match m {
        CPU => {
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
        GPU => {
            let mut r = row(GPU, cfg.style[GPU], cfg.label[GPU].clone(), DASH.to_string());
            if let Some(u) = s.gpu {
                r.value = pct(u, cfg);
                r.frac = Some((u / 100.0).clamp(0.0, 1.0));
                r.sub = match (t.vram, s.gpu_mem) {
                    (Some((used, total)), _) => Some(format!("VRAM {} / {}", size(used), size(total))),
                    (None, Some(used)) => Some(format!("VRAM {}", size(used))),
                    _ => None,
                };
            } else {
                r.frac = Some(0.0);
            }
            v.push(r);
        }
        RAM => {
            let f = ram_frac(s);
            let mut r = row(RAM, cfg.style[RAM], cfg.label[RAM].clone(), pct(f * 100.0, cfg));
            r.sub = Some(format!("{} / {}", size(s.ram_used), size(s.ram_total)));
            r.frac = Some(f);
            v.push(r);
        }
        DISK => {
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
            if v.is_empty() {
                v.push(row(DISK, Style::Text, cfg.label[DISK].clone(), DASH.to_string()));
            }
        }
        IO => {
            let (value, total) = match s.io {
                Some((r, w)) => (speed_pair(r, w, cfg, "R", "W"), (r + w) as f32),
                None => (format!("{DASH} / {DASH}"), 0.0),
            };
            let mut r = row(IO, cfg.style[IO], cfg.label[IO].clone(), value);
            r.frac = Some(total);
            r.auto = true;
            v.push(r);
        }
        NET => {
            let (value, total) = match s.net {
                Some((d, u)) => (speed_pair(d, u, cfg, "↓", "↑"), (d + u) as f32),
                None => (format!("{DASH} / {DASH}"), 0.0),
            };
            let mut r = row(NET, cfg.style[NET], cfg.label[NET].clone(), value);
            r.frac = Some(total);
            r.auto = true;
            v.push(r);
        }
        TEMP => {
            let mut add = |label: String, value: Option<f32>| {
                let mut r = row(TEMP, Style::Text, label, value.map_or(DASH.to_string(), |c| temp_value(c, cfg)));
                r.compact = true;
                v.push(r);
            };
            add(t!("ЦП", "CPU", ja: "CPU", pt: "CPU", zh: "CPU").to_string(), t.cpu);
            add(t!("ГП", "GPU", ja: "GPU", pt: "GPU", zh: "GPU").to_string(), t.gpu);
            if cfg.temp_disks {
                // only the drives that exist right now: a letter that was unplugged gets no "—" row
                for d in &s.disks {
                    let temp = t.disks.iter().find(|(l, _)| *l == d.letter).and_then(|(_, v)| *v);
                    add(format!("{}:", d.letter), temp);
                }
            }
            if cfg.temp_lhm {
                add(t!("Плата", "Board", ja: "マザー", pt: "Placa-mãe", zh: "主板").to_string(), t.board);
            }
            if cfg.temp_acpi {
                add("ACPI".to_string(), t.acpi);
            }
        }
        UPTIME => {
            v.push(row(UPTIME, Style::Text, cfg.label[UPTIME].clone(), uptime(s.uptime)));
        }
        _ => {}
    }
    v
}

// ---------- layout ----------

fn sc(v: f32, scale: f32) -> i32 {
    (v * scale).round() as i32
}

fn style_of(r: &Row) -> Style {
    if r.cores {
        return Style::Bar;
    }
    if r.frac.is_none() && matches!(r.style, Style::Bar | Style::Ring | Style::Spark) { Style::Text } else { r.style }
}

fn design_height(r: &Row) -> f32 {
    if r.compact {
        return 22.0;
    }
    match style_of(r) {
        Style::Bar => if r.foot.is_some() { 48.0 } else { 36.0 },
        Style::Ring => 54.0,
        Style::Text => 24.0,
        Style::Spark => 40.0,
        Style::Pill => 26.0,
    }
}

fn height(r: &Row, scale: f32) -> i32 {
    sc(design_height(r), scale)
}

pub struct Pill {
    pub module: usize,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub rows: Vec<Row>,
}

pub struct Plan {
    pub w: i32,
    pub h: i32,
    pub scale: f32,
    pub pills: Vec<Pill>,
}

/// `only`: a detached module, alone in its window.
pub fn build(cfg: &Config, s: &Sample, t: &Temps, scale: f32, only: Option<usize>) -> Plan {
    let px = sc(PX, scale);
    let pt = sc(PT, scale);
    let pb = sc(PB, scale);
    let gap = sc(ROW_GAP, scale);
    let margin = sc(MARGIN, scale);
    let col_w = sc(cfg.width as f32, scale) + px * 2;
    let mut pills: Vec<Pill> = Vec::with_capacity(9);
    for &m in cfg.order.iter() {
        if only.map_or(true, |o| o == m) && cfg.on[m] && (only.is_some() || !cfg.detached[m]) {
            let rows = module_rows(m, cfg, s, t);
            if rows.is_empty() {
                continue;
            }
            let content: i32 = rows.iter().map(|r| height(r, scale)).sum::<i32>() + gap * (rows.len() as i32 - 1);
            pills.push(Pill { module: m, x: 0, y: 0, w: col_w, h: pt + content + pb, rows });
        }
    }
    if only.is_some() {
        for p in &mut pills {
            p.x = margin;
            p.y = margin;
        }
        let w = pills.first().map_or(col_w + margin * 2, |p| p.w + margin * 2);
        let h = pills.first().map_or(sc(26.0, scale) + margin * 2, |p| p.h + margin * 2);
        return Plan { w, h, scale, pills };
    }
    // one or two columns, row-major; a visual row is as tall as its tallest pill
    let cols = cfg.cols.clamp(1, 2) as usize;
    let vgap = sc(cfg.gap as f32, scale);
    let nrows = pills.len().div_ceil(cols).max(1);
    let mut rh = vec![0i32; nrows];
    for (i, p) in pills.iter().enumerate() {
        rh[i / cols] = rh[i / cols].max(p.h);
    }
    let content_w = col_w * cols as i32 + vgap * (cols as i32 - 1);
    let content_h: i32 = if pills.is_empty() { sc(26.0, scale) } else { rh.iter().sum::<i32>() + vgap * (nrows as i32 - 1) };
    let mut top = margin;
    for (k, &row_h) in rh.iter().enumerate() {
        for (j, p) in pills.iter_mut().enumerate().filter(|(j, _)| j / cols == k) {
            p.x = margin + (j % cols) as i32 * (col_w + vgap);
            p.y = top;
        }
        top += row_h + vgap;
    }
    Plan { w: content_w + margin * 2, h: content_h + margin * 2, scale, pills }
}


/// A cheap hash of everything the cached background depends on.
pub fn bg_key(plan: &Plan, cfg: &Config, origin: (i32, i32)) -> u64 {
    let mut h: u64 = 1469598103934665603;
    let mut put = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(1099511628211);
    };
    put(plan.w as u64);
    put(plan.h as u64);
    put(origin.0 as u32 as u64);
    put(origin.1 as u32 as u64);
    put(cfg.opacity as u64);
    put(cfg.blur as u64);
    put(frost::generation());
    put(pal().bg as u64);
    put(pal().line as u64);
    for p in &plan.pills {
        put(p.module as u64);
        put(p.x as u64);
        put(p.y as u64);
        put(p.w as u64);
        put(p.h as u64);
    }
    h
}

// ---------- painting ----------

/// The pills' backgrounds (frost + rim + a soft shadow). Cached by the window: it changes with the size or the position.
pub fn draw_bg(cv: &mut Canvas, plan: &Plan, cfg: &Config, origin: (i32, i32)) {
    cv.clear();
    let p = pal();
    let scale = plan.scale;
    for pill in &plan.pills {
        let (x, y, w, h) = (pill.x, pill.y, pill.w, pill.h);
        let r = (14.0 * scale).round().max(4.0);
        if cfg.shadow {
            for (i, a) in [14u32, 18, 22].into_iter().enumerate() {
                let e = (3 - i) as f32 * 1.5 * scale;
                cv.round_rect(x as f32 - e, y as f32 - e + 2.0, w as f32 + 2.0 * e, h as f32 + 2.0 * e, r + e, 0, a);
            }
        }
        // the rim, then the frosted body one pixel inside it
        cv.round_rect(x as f32, y as f32, w as f32, h as f32, r, p.line, cfg.opacity as u32);
        let (bx, by, bw, bh) = (x as f32 + 1.0, y as f32 + 1.0, w as f32 - 2.0, h as f32 - 2.0);
        let (br, bg, bb) = ((p.bg & 255) as f32, ((p.bg >> 8) & 255) as f32, ((p.bg >> 16) & 255) as f32);
        let blur = cfg.blur;
        cv.round_rect_with(bx, by, bw, bh, (r - 1.0).max(0.0), cfg.opacity as u32, |px, py| {
            let c = if blur { frost::sample(origin.0 + px, origin.1 + py) } else { None };
            match c {
                // 78 % of the card colour: the wallpaper still shows through, the text stays readable
                Some((wr, wg, wb)) => {
                    let r = (br * 0.78 + wr as f32 * 0.22) as u32;
                    let g = (bg * 0.78 + wg as f32 * 0.22) as u32;
                    let b = (bb * 0.78 + wb as f32 * 0.22) as u32;
                    r | g << 8 | b << 16
                }
                None => p.bg,
            }
        });
    }
}

/// The rows, over the (cached) background.
pub fn draw_content(cv: &mut Canvas, plan: &Plan, cfg: &Config, s: &Sample, series: &Series, fonts: &Fonts) {
    let p = pal();
    if plan.pills.is_empty() {
        let (x, y) = (sc(MARGIN, plan.scale), sc(MARGIN, plan.scale));
        cv.text((x, y, plan.w - x * 2, plan.h - y * 2), t!("Нет модулей", "No modules", ja: "モジュールなし", pt: "Sem módulos", zh: "无模块"), fonts.label, p.dim, 220, Align::Center);
        return;
    }
    for pill in &plan.pills {
        let x = pill.x + sc(PX, plan.scale);
        let w = pill.w - sc(PX, plan.scale) * 2;
        let mut y = pill.y + sc(PT, plan.scale);
        for r in &pill.rows {
            let h = height(r, plan.scale);
            draw_row(cv, r, x, y, w, h, cfg, s, series, fonts);
            y += h + sc(ROW_GAP, plan.scale);
        }
    }
}

fn frac_of(r: &Row, series: &Series) -> f32 {
    let raw = r.frac.unwrap_or(0.0);
    if r.auto { (raw / series.max(r.module)).clamp(0.0, 1.0) } else { raw.clamp(0.0, 1.0) }
}

#[allow(clippy::too_many_arguments)]
fn draw_row(cv: &mut Canvas, r: &Row, x: i32, y: i32, w: i32, h: i32, cfg: &Config, s: &Sample, series: &Series, fonts: &Fonts) {
    let p = pal();
    let (w, h) = (w.max(8), h.max(8));
    if r.cores {
        draw_cores(cv, x, y, w, h, s, fonts, cfg);
        return;
    }
    if r.compact {
        // a list line: small label left, small bold value right
        let vw = cv.measure(fonts.mini, &r.value) + 8;
        let value_w = vw.clamp((w as f32 * 0.3) as i32, (w as f32 * 0.62) as i32);
        cv.text((x, y, w - value_w - 6, h), &r.label, fonts.label, p.dim, 210, Align::Left);
        cv.text((x + w - value_w, y, value_w, h), &r.value, fonts.mini, p.text, 250, Align::Right);
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
