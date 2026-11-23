//! The window: drawing (GDI+ shapes, GDI text), layout, dialogs and the message loop handler.
use crate::*;

// ---------- drawing ----------
//
// One back buffer per paint: GDI+ for the anti-aliased shapes, then GDI (ClearType) for text on top.

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// COLORREF (0x00BBGGRR) + alpha -> GDI+ ARGB.
pub(crate) const fn argb(c: u32, a: u8) -> u32 {
    (a as u32) << 24 | (c & 255) << 16 | (c >> 8 & 255) << 8 | (c >> 16 & 255)
}

pub(crate) fn lerp(a: u32, b: u32, t: f32) -> u32 {
    let m = |sh: u32| (((a >> sh & 255) as f32) * (1.0 - t) + ((b >> sh & 255) as f32) * t) as u32;
    m(0) | m(8) << 8 | m(16) << 16
}

pub(crate) type Box4 = [f32; 4]; // left, top, right, bottom in logical px

pub(crate) struct Ui {
    dc: HDC,
    g: *mut GpGraphics,
    s: f32,
}
impl Ui {
    fn px(&self, v: f32) -> i32 {
        (v * self.s).round() as i32
    }
    unsafe fn shape(&self, b: Box4, rad: f32, color: u32) {
        let (s, mut path) = (self.s, null_mut::<GpPath>());
        let (x, y, w, h) = (b[0] * s, b[1] * s, (b[2] - b[0]) * s, (b[3] - b[1]) * s);
        let d = (rad * 2.0 * s).min(h).min(w).max(0.01);
        GdipCreatePath(FillModeAlternate, &mut path);
        GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
        GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
        GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
        GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
        GdipClosePathFigure(path);
        let mut br = null_mut::<GpSolidFill>();
        GdipCreateSolidFill(color, &mut br);
        GdipFillPath(self.g, br as *mut GpBrush, path);
        GdipDeleteBrush(br as *mut GpBrush);
        GdipDeletePath(path);
    }
    /// Rounded box with a 1 px outline (`line` == `fill` for none).
    unsafe fn round(&self, b: Box4, rad: f32, fill: u32, line: u32) {
        if line != fill {
            self.shape(b, rad, argb(line, 255));
            let i = 1.0 / self.s;
            self.shape([b[0] + i, b[1] + i, b[2] - i, b[3] - i], rad - i, argb(fill, 255));
        } else {
            self.shape(b, rad, argb(fill, 255));
        }
    }
    unsafe fn font(&self, size: f32, weight: i32, face: &str) -> HFONT {
        CreateFontW(-self.px(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(face).as_ptr())
    }
    unsafe fn text_in(&self, s: &str, b: Box4, size: f32, weight: i32, color: u32, flags: u32, face: &str) {
        let f = self.font(size, weight, face);
        let of = SelectObject(self.dc, f);
        SetTextColor(self.dc, color);
        SetBkMode(self.dc, TRANSPARENT as i32);
        let mut r = RECT { left: self.px(b[0]), top: self.px(b[1]), right: self.px(b[2]), bottom: self.px(b[3]) };
        DrawTextW(self.dc, wide(s).as_ptr(), -1, &mut r, flags | DT_NOPREFIX);
        SelectObject(self.dc, of);
        DeleteObject(f);
    }
    unsafe fn text(&self, s: &str, b: Box4, size: f32, weight: i32, color: u32, flags: u32) {
        self.text_in(s, b, size, weight, color, flags, winrust_common::style::sans_face());
    }
    /// Width of a single line of text, logical px.
    unsafe fn width(&self, s: &str, size: f32, weight: i32) -> f32 {
        let f = self.font(size, weight, winrust_common::style::sans_face());
        let of = SelectObject(self.dc, f);
        let mut r: RECT = zeroed();
        DrawTextW(self.dc, wide(s).as_ptr(), -1, &mut r, DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(self.dc, of);
        DeleteObject(f);
        r.right as f32 / self.s
    }
}

// layout, logical px: one grouped list like the macOS settings, quiet text, the action at the bottom right
pub(crate) const WIN_W: f32 = 640.0;
pub(crate) const M: f32 = 32.0; // outer margin
pub(crate) const N_READY: usize = {
    let (mut n, mut i) = (0, 0);
    while i < PLUGINS.len() {
        if PLUGINS[i].actions.is_some() && PLUGINS[i].bulk {
            n += 1;
        }
        i += 1;
    }
    n
};
/// Tools that change system behaviour (debloat) or remove apps for good (purge): their own group, never ticked by default.
pub(crate) const N_EXTRA: usize = {
    let (mut n, mut i) = (0, 0);
    while i < PLUGINS.len() {
        if PLUGINS[i].actions.is_some() && !PLUGINS[i].bulk {
            n += 1;
        }
        i += 1;
    }
    n
};
const SEC_Y: f32 = 104.0; // the list's caption
pub(crate) const LIST_TOP: f32 = SEC_Y + 28.0;
pub(crate) const ROW_H: f32 = 56.0;
pub(crate) const LIST_H: f32 = N_READY as f32 * ROW_H;
const EXTRA_SEC_Y: f32 = LIST_TOP + LIST_H + 18.0; // the second group's caption
const EXTRA_TOP: f32 = EXTRA_SEC_Y + 28.0;
const LISTS_END: f32 = if N_EXTRA > 0 { EXTRA_TOP + N_EXTRA as f32 * ROW_H } else { LIST_TOP + LIST_H };
const NOTE_Y: f32 = LISTS_END + 12.0; // status / the hovered component's description
const NOTE_H: f32 = 40.0;
pub(crate) const BTN_H: f32 = 32.0;
const BTN_Y: f32 = NOTE_Y + NOTE_H + 16.0;
pub(crate) const WIN_H: f32 = BTN_Y + BTN_H + 28.0;
pub(crate) const BTN_W: f32 = 120.0;
const MAIN_W: f32 = 210.0;
// clickable things
pub(crate) const ID_MAIN: usize = 0;
pub(crate) const ID_ROLLBACK: usize = 1;
pub(crate) const ID_OK: usize = 2;
pub(crate) const ID_CANCEL: usize = 3;
pub(crate) const ID_SWITCH: usize = 4;
pub(crate) const ID_GEAR: usize = 5;
pub(crate) const ID_LANG: usize = 6; // EN / RU switch
pub(crate) const ID_SHELL: usize = 7; // settings panel: replace Explorer with rust-explorer
pub(crate) const ID_UPDATE: usize = 9; // "update available" link under the language switch
pub(crate) const ID_ALL: usize = 8; // "select all / none" next to the list caption
pub(crate) const ID_CHECK: usize = 100; // + plugin index: the whole row toggles it
pub(crate) const STARTING: usize = usize::MAX - 1; // JOB before apply() picks its first step

const GREEN: u32 = rgb(48, 209, 88); // the switch when on

/// Installable components, in install order (topbar comes before rust-explorer, which needs it).
pub(crate) fn ready() -> impl Iterator<Item = usize> {
    (0..PLUGINS.len()).filter(|&i| PLUGINS[i].actions.is_some() && PLUGINS[i].bulk)
}

/// The second group: on request only (command line `install debloat` or its switch here).
pub(crate) fn extras() -> impl Iterator<Item = usize> {
    (0..PLUGINS.len()).filter(|&i| PLUGINS[i].actions.is_some() && !PLUGINS[i].bulk)
}

/// Every row of the window: the components, then the extras.
pub(crate) fn rows() -> impl Iterator<Item = usize> {
    ready().chain(extras())
}

/// The rows whose switches are on, in install order (extras last).
pub(crate) fn chosen() -> impl Iterator<Item = usize> {
    rows().filter(|&i| SELECTED[i].load(Relaxed))
}

/// What the button would do now: (to remove, to install).
pub(crate) fn diff() -> (Vec<usize>, Vec<usize>) {
    plan(&rows().collect::<Vec<_>>(), |i| INSTALLED[i].load(Relaxed), |i| SELECTED[i].load(Relaxed))
}

/// The running job's steps, (plugin, install?) in order: the progress line and the "queued" words read it.
pub(crate) static STEPS: Mutex<Vec<(usize, bool)>> = Mutex::new(Vec::new());

/// The switch at the right end of row `k`, and the row itself (all of it flips the switch).
pub(crate) fn check_box(k: usize, w: f32) -> (Box4, Box4) {
    let r = row_box(k, w);
    let mid = (r[1] + r[3]) / 2.0;
    ([r[2] - 16.0 - 40.0, mid - 12.0, r[2] - 16.0, mid + 12.0], r)
}

/// Row `k` of rows(): the first N_READY in the components group, the rest in the extras group.
pub(crate) fn row_box(k: usize, w: f32) -> Box4 {
    let t = if k < N_READY { LIST_TOP + k as f32 * ROW_H } else { EXTRA_TOP + (k - N_READY) as f32 * ROW_H };
    [M, t, w - M, t + ROW_H]
}

/// Settings gear left of the switch on the row of the plugin that has settings (topbar): (plugin index, box).
pub(crate) fn gear_box(w: f32) -> Option<(usize, Box4)> {
    let (k, i) = ready().enumerate().find(|&(_, i)| PLUGINS[i].settings)?;
    let (sw, _) = check_box(k, w);
    Some((i, [sw[0] - 12.0 - 28.0, sw[1] - 2.0, sw[0] - 12.0, sw[3] + 2.0]))
}

/// "Select all / none" at the right end of the list caption.
pub(crate) fn all_box(w: f32) -> Box4 {
    [w - M - 160.0, SEC_Y, w - M - 4.0, SEC_Y + 22.0]
}

pub(crate) fn main_box(w: f32) -> Box4 {
    [w - M - MAIN_W, BTN_Y, w - M, BTN_Y + BTN_H]
}

/// On/off switch inside the settings panel's first row.
pub(crate) fn switch_box(w: f32) -> Box4 {
    let (d, _, _) = dialog_boxes(w);
    let mid = d[1] + 72.0 + 32.0;
    [d[2] - 24.0 - 16.0 - 40.0, mid - 12.0, d[2] - 24.0 - 16.0, mid + 12.0]
}

/// Second switch row of the settings panel: which shell to install.
pub(crate) fn shell_switch_box(w: f32) -> Box4 {
    let b = switch_box(w);
    [b[0], b[1] + 64.0, b[2], b[3] + 64.0]
}

/// Index of rust-explorer in PLUGINS.
fn explorer_idx() -> Option<usize> {
    PLUGINS.iter().position(|p| p.name == crate::selection::RUST_EXPLORER)
}

/// "Update available: v0.6.0 · Download" under the language link.
pub(crate) fn update_box(w: f32) -> Box4 {
    [w - M - 340.0, 64.0, w - M, 84.0]
}

/// The language drop-down in the top right corner, and one item of its list.
pub(crate) fn lang_box(w: f32) -> Box4 {
    [w - M - 132.0, 36.0, w - M, 62.0]
}
const LANG_ITEM_H: f32 = 26.0; // one item of the open list
const LANG_LIST: &str = "WinRustLangList";

/// "Remove all": plain red text at the bottom left.
pub(crate) fn rollback_box(_w: f32) -> Box4 {
    [M - 8.0, BTN_Y, M + 112.0, BTN_Y + BTN_H]
}

/// Dialog panel and its (cancel, ok) buttons.
pub(crate) fn dialog_boxes(w: f32) -> (Box4, Box4, Box4) {
    let (dw, dh) = (500.0, 440.0);
    let (l, t) = ((w - dw) / 2.0, (WIN_H - dh) / 2.0);
    let (bt, r) = (t + dh - 20.0 - BTN_H, l + dw - 20.0);
    ([l, t, l + dw, t + dh], [r - BTN_W - 8.0 - BTN_W, bt, r - BTN_W - 8.0, bt + BTN_H], [r - BTN_W, bt, r, bt + BTN_H])
}

/// Pixels per logical px: the screen's DPI, shrunk so the whole window fits the work area (small screens, big DPI).
pub(crate) fn scale() -> f32 {
    unsafe {
        let dc = GetDC(null_mut());
        let mut s = GetDeviceCaps(dc, LOGPIXELSX as i32) as f32 / 96.0;
        ReleaseDC(null_mut(), dc);
        let mut wa: RECT = zeroed();
        if SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut _, 0) != 0 {
            let frame = GetSystemMetrics(SM_CYCAPTION) + 2 * GetSystemMetrics(SM_CYFIXEDFRAME) + 8;
            s = s.min(((wa.bottom - wa.top - frame) as f32 / WIN_H).max(0.5));
        }
        s
    }
}

/// A button: `kind` 0 = primary (blue), 1 = secondary (grey), 2 = danger (red), 3 = disabled.
pub(crate) unsafe fn button(ui: &Ui, b: Box4, label: &str, kind: u8, hot: bool) {
    let (fill, fg) = match kind {
        0 => (ACCENT, rgb(255, 255, 255)),
        1 => (rgb(72, 72, 74), TEXT),
        2 => (BAD, rgb(255, 255, 255)),
        _ => (rgb(58, 58, 60), lerp(DIM, BG, 0.3)),
    };
    let fill = if hot && kind != 3 { lerp(fill, rgb(255, 255, 255), 0.10) } else { fill };
    ui.round(b, 8.0, fill, fill);
    ui.text(label, b, 13.0, 500, fg, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

/// The Apple switch: a pill track, green when on, with a white knob.
unsafe fn switch(ui: &Ui, b: Box4, on: bool, dim: bool) {
    let track = if on { GREEN } else { rgb(72, 72, 74) };
    let track = if dim { lerp(track, PANEL, 0.5) } else { track };
    let h = b[3] - b[1];
    ui.round(b, h / 2.0, track, track);
    let k = h - 4.0;
    let x = if on { b[2] - 2.0 - k } else { b[0] + 2.0 };
    ui.round([x, b[1] + 2.0, x + k, b[3] - 2.0], k / 2.0, rgb(255, 255, 255), rgb(255, 255, 255));
}

/// The coloured square behind a component's glyph (the system colours, one per component).
fn tile_color(name: &str) -> u32 {
    match name {
        "rust-search" => rgb(10, 132, 255),
        "rtaskmgr-win32" => rgb(48, 209, 88),
        "topbar" => rgb(94, 92, 230),
        "rust-explorer" => rgb(255, 159, 10),
        "wiztree" => rgb(191, 90, 242),
        "photoview" => rgb(255, 55, 95),
        "debloat" => rgb(100, 210, 255),
        "purge" => rgb(255, 69, 58),
        _ => rgb(142, 142, 147),
    }
}

pub(crate) unsafe fn paint(hwnd: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut rc: RECT = zeroed();
    GetClientRect(hwnd, &mut rc);
    let (cw, ch) = (rc.right, rc.bottom);
    let mem = CreateCompatibleDC(hdc);
    let bmp = CreateCompatibleBitmap(hdc, cw, ch);
    let ob = SelectObject(mem, bmp);
    let s = scale();
    let bgb = CreateSolidBrush(BG);
    FillRect(mem, &rc, bgb);
    DeleteObject(bgb);
    let mut g = null_mut::<GpGraphics>();
    GdipCreateFromHDC(mem, &mut g);
    GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
    let ui = Ui { dc: mem, g, s };
    let w = cw as f32 / s;
    let hover = HOVER.get();
    let dialog = DIALOG.with_borrow(|d| d.as_ref().map(|d| (d.title.clone(), d.text.clone(), d.ok.clone(), d.danger)));
    let hot = |id: usize| hover == Some(id);
    let busy = BUSY.load(Relaxed);
    let job = JOB.load(Relaxed);
    let n = PLUGINS.len();
    let n_inst = ready().filter(|&i| INSTALLED[i].load(Relaxed)).count();
    let icons = winrust_common::style::icon_face();
    let hair = 1.0 / s;

    // title
    ui.text("WinRust", [M, 30.0, w - M - 120.0, 64.0], 28.0, 700, TEXT, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
    let sub = if n_inst == 0 { t!("Оболочка Windows на Rust", "A Windows shell in Rust", ja: "Rust 製の Windows シェル", pt: "Um shell do Windows em Rust", zh: "用 Rust 编写的 Windows 外壳").to_string() } else { tf!("Установлено {n_inst} из {N_READY}", "{n_inst} of {N_READY} installed", ja: "{N_READY} 個中 {n_inst} 個をインストール済み", pt: "{n_inst} de {N_READY} instalados", zh: "已安装 {n_inst} 个，共 {N_READY} 个") };
    let sub = format!("{} · {sub}", update::current());
    ui.text(&sub, [M, 64.0, w - M, 84.0], 13.0, 400, DIM, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
    if let Some(v) = update::NEWER.lock().unwrap().clone() {
        let s = tf!("Доступно обновление {} · Скачать", "Update available: {} · Download", ja: "新しいバージョン {} があります · ダウンロード", pt: "Atualização disponível: {} · Baixar", zh: "有可用更新 {} · 下载", v);
        ui.text(&s, update_box(w), 13.0, 600, if hot(ID_UPDATE) { TEXT } else { ACCENT }, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
    }
    // the language drop-down: the current language and a caret; the list itself is a popup (open_lang_list)
    let lb = lang_box(w);
    let open = LANG_OPEN.get().is_some();
    ui.round(lb, 6.0, if hot(ID_LANG) || open { lerp(PANEL, TEXT, 0.08) } else { PANEL }, if open { ACCENT } else { BORDER });
    ui.text(i18n::lang().name(), [lb[0] + 10.0, lb[1], lb[2] - 26.0, lb[3]], 13.0, 400, TEXT, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
    ui.text_in("\u{E70D}", [lb[2] - 22.0, lb[1], lb[2] - 8.0, lb[3]], 9.0, 400, DIM, DT_RIGHT | DT_SINGLELINE | DT_VCENTER, winrust_common::style::icon_face());

    // list caption
    let n_sel_ready = chosen().filter(|&i| PLUGINS[i].bulk).count();
    ui.text(t!("Компоненты", "Components", ja: "コンポーネント", pt: "Componentes", zh: "组件"), [M + 4.0, SEC_Y, w - M, SEC_Y + 22.0], 13.0, 600, DIM, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
    if !busy {
        let label = if n_sel_ready == N_READY { t!("Снять все", "Select none", ja: "すべて選択解除", pt: "Desmarcar tudo", zh: "全不选") } else { t!("Выбрать все", "Select all", ja: "すべて選択", pt: "Selecionar tudo", zh: "全选") };
        ui.text(label, all_box(w), 13.0, 400, if hot(ID_ALL) { TEXT } else { ACCENT }, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
    }

    // the grouped lists
    ui.round([M, LIST_TOP, w - M, LIST_TOP + LIST_H], 12.0, PANEL, PANEL);
    if N_EXTRA > 0 {
        ui.text(t!("Дополнительно", "Extras", ja: "オプション", pt: "Extras", zh: "附加功能"), [M + 4.0, EXTRA_SEC_Y, w - M, EXTRA_SEC_Y + 22.0], 13.0, 600, DIM, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
        ui.text(t!("Только по вашему выбору", "Only when you choose", ja: "選択したときのみ", pt: "Somente quando você escolher", zh: "仅在手动选择时"), [M + 4.0, EXTRA_SEC_Y, w - M - 4.0, EXTRA_SEC_Y + 22.0], 12.0, 400, lerp(DIM, BG, 0.3), DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
        ui.round([M, EXTRA_TOP, w - M, LISTS_END], 12.0, PANEL, PANEL);
    }
    let steps = STEPS.lock().unwrap().clone();
    let pos = steps.iter().position(|&(i, _)| i == job); // where the job is now
    let (to_rm, to_add) = diff();
    let gear = gear_box(w);
    for (k, i) in rows().enumerate() {
        let p = &PLUGINS[i];
        let (sw, r) = check_box(k, w);
        let mid = (r[1] + r[3]) / 2.0;
        let installed = INSTALLED[i].load(Relaxed);
        let working = busy && (job == i || (job == n && installed));
        let step = steps.iter().position(|&(j, _)| j == i);
        let queued = busy && step.is_some_and(|q| pos.is_none_or(|c| q > c));
        let removing = steps.get(step.unwrap_or(usize::MAX)).is_some_and(|s| !s.1) || job == n;
        if hot(ID_CHECK + i) && !busy {
            // the row lights up a little, keeping the group's rounded ends
            let c = argb(lerp(PANEL, TEXT, 0.05), 255);
            let (first, last) = (k == 0 || k == N_READY, k + 1 == N_READY || k + 1 == N_READY + N_EXTRA);
            if first || last {
                ui.shape(r, 12.0, c);
            }
            if !(first && last) {
                let h = (r[3] - r[1]) / 2.0;
                ui.shape(if first { [r[0], r[1] + h, r[2], r[3]] } else if last { [r[0], r[1], r[2], r[3] - h] } else { r }, 0.0, c);
            }
        }
        let tile = [r[0] + 14.0, mid - 15.0, r[0] + 44.0, mid + 15.0];
        let tc = tile_color(p.name);
        ui.round(tile, 7.0, tc, tc);
        ui.text_in(p.glyph, tile, 15.0, 400, rgb(255, 255, 255), DT_CENTER | DT_VCENTER | DT_SINGLELINE, icons);
        if k > 0 && k != N_READY {
            ui.shape([tile[2] + 14.0, r[1], r[2], r[1] + hair], 0.0, argb(BORDER, 255));
        }

        // right side: status words, the gear, the switch
        let mut right = sw[0] - 12.0;
        if let Some((_, gb)) = gear.filter(|&(gi, _)| gi == i) {
            let gh = hot(ID_GEAR) && !busy;
            ui.text_in("\u{E713}", gb, 15.0, 400, if gh { TEXT } else { DIM }, DT_CENTER | DT_VCENTER | DT_SINGLELINE, icons);
            right = gb[0] - 8.0;
        }
        let status = if working {
            Some((if removing { t!("Удаление…", "Removing…", ja: "削除中…", pt: "Removendo…", zh: "正在移除…") } else { t!("Установка…", "Installing…", ja: "インストール中…", pt: "Instalando…", zh: "正在安装…") }, ACCENT))
        } else if queued {
            Some((t!("В очереди", "Queued", ja: "待機中", pt: "Na fila", zh: "已排队"), DIM))
        } else if !busy && to_rm.contains(&i) {
            Some((t!("Будет удалён", "Will be removed", ja: "削除されます", pt: "Será removido", zh: "将被移除"), BAD))
        } else if !busy && to_add.contains(&i) {
            Some((t!("Будет установлен", "Will be installed", ja: "インストールされます", pt: "Será instalado", zh: "将被安装"), ACCENT))
        } else if installed {
            Some((if p.bulk { t!("Установлен", "Installed", ja: "インストール済み", pt: "Instalado", zh: "已安装") } else { t!("Применён", "Applied", ja: "適用済み", pt: "Aplicado", zh: "已应用") }, DIM))
        } else {
            None
        };
        if let Some((st, c)) = status {
            let sw_ = ui.width(st, 13.0, 400);
            ui.text(st, [right - sw_ - 2.0, r[1], right, r[3]], 13.0, 400, c, DT_RIGHT | DT_SINGLELINE | DT_VCENTER);
            right -= sw_ + 14.0;
        }
        switch(&ui, sw, SELECTED[i].load(Relaxed), busy || locked(i));

        let nx = tile[2] + 14.0;
        ui.text(p.name, [nx, r[1] + 9.0, right, mid + 1.0], 14.0, 500, TEXT, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
        let desc = p.desc.get();
        let short = desc.split(". ").next().unwrap_or(desc).trim_end_matches('.');
        ui.text(short, [nx, mid + 1.0, right, r[3] - 9.0], 12.0, 400, DIM, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
    }

    // under the list: the hovered component's full description, else the status
    let (m, ok) = MSG.lock().unwrap().clone();
    let card = hover.filter(|&h| h >= ID_CHECK && !busy).map(|h| h - ID_CHECK);
    let (m, fg) = if let Some(i) = card {
        (PLUGINS[i].desc.get().to_string(), DIM)
    } else if busy {
        (t!("Не закрывайте окно до конца установки.", "Keep the window open until it finishes.", ja: "完了するまでウィンドウを開いたままにしてください。", pt: "Mantenha a janela aberta até terminar.", zh: "完成之前请不要关闭窗口。").to_string(), DIM)
    } else if m.is_empty() {
        (t!("Перед установкой создаётся точка восстановления.", "A restore point is created before installing.", ja: "インストールの前に復元ポイントを作成します。", pt: "Um ponto de restauração é criado antes de instalar.", zh: "安装前会创建还原点。").into(), DIM)
    } else if ok {
        (m, GOOD)
    } else {
        (m, BAD)
    };
    ui.text(&m, [M + 4.0, NOTE_Y, w - M - 4.0, NOTE_Y + NOTE_H], 12.0, 400, fg, DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS);

    // bottom row: remove-all on the left, the action on the right
    let rb = rollback_box(w);
    let can_rm = n_inst > 0 && !busy;
    ui.text(t!("Удалить всё", "Remove all", ja: "すべて削除", pt: "Remover tudo", zh: "全部移除"), [rb[0] + 8.0, rb[1], rb[2], rb[3]], 13.0, 400, if !can_rm { lerp(DIM, BG, 0.4) } else if hot(ID_ROLLBACK) { lerp(BAD, TEXT, 0.3) } else { BAD }, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
    let mb = main_box(w);
    if busy {
        let total = steps.len().max(1);
        let done = if job == n { 0.0 } else { pos.map_or(0.0, |q| (q as f32 + 0.5) / total as f32) };
        let label = if job == n {
            t!("Удаление…", "Removing…", ja: "削除中…", pt: "Removendo…", zh: "正在移除…").to_string()
        } else {
            pos.map_or(t!("Подготовка…", "Preparing…", ja: "準備中…", pt: "Preparando…", zh: "正在准备…").into(), |q| tf!("Шаг {} из {}…", "Step {} of {}…", ja: "ステップ {} / {}…", pt: "Etapa {} de {}…", zh: "第 {} 步，共 {} 步…", q + 1, total))
        };
        // a thin progress line over the bottom row
        let pl = [mb[0], mb[1] - 10.0, mb[2], mb[1] - 7.0];
        ui.round(pl, 1.5, rgb(58, 58, 60), rgb(58, 58, 60));
        if done > 0.0 {
            ui.round([pl[0], pl[1], pl[0] + (pl[2] - pl[0]) * done, pl[3]], 1.5, ACCENT, ACCENT);
        }
        button(&ui, mb, &label, 3, false);
    } else {
        // the switches say what should be installed: the button applies only the difference
        let (label, kind) = match (to_rm.len(), to_add.len()) {
            (0, 0) if n_inst == 0 => (t!("Ничего не выбрано", "Nothing selected", ja: "何も選択されていません", pt: "Nada selecionado", zh: "未选择任何内容").to_string(), 3),
            (0, 0) => (t!("Обновить", "Update", ja: "更新", pt: "Atualizar", zh: "更新").to_string(), 1),
            (0, a) if n_inst == 0 && a >= N_READY - 1 => (t!("Установить WinRust", "Install WinRust", ja: "WinRust をインストール", pt: "Instalar a WinRust", zh: "安装 WinRust").to_string(), 0),
            (0, a) => (tf!("Установить ({a})", "Install ({a})", ja: "インストール ({a})", pt: "Instalar ({a})", zh: "安装（{a}）"), 0),
            (r, 0) => (tf!("Удалить ({r})", "Remove ({r})", ja: "削除 ({r})", pt: "Remover ({r})", zh: "移除（{r}）"), 2),
            (r, a) => (tf!("Применить (+{a} −{r})", "Apply (+{a} −{r})", ja: "適用 (+{a} −{r})", pt: "Aplicar (+{a} −{r})", zh: "应用（+{a} −{r}）"), 0),
        };
        button(&ui, mb, &label, kind, hot(ID_MAIN));
    }

    // confirmation dialog over the dimmed window
    if let Some((title, text, ok_label, danger)) = dialog {
        ui.shape([0.0, 0.0, w, WIN_H + 1.0], 0.0, argb(0, 150));
        let (d, cancel, okb) = dialog_boxes(w);
        let card = rgb(44, 44, 46);
        ui.round(d, 14.0, card, rgb(64, 64, 66));
        ui.text(&title, [d[0] + 24.0, d[1] + 22.0, d[2] - 24.0, d[1] + 50.0], 16.0, 600, TEXT, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        ui.text(&text, [d[0] + 24.0, d[1] + 60.0, d[2] - 24.0, cancel[1] - 14.0], 12.5, 400, DIM, DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS);
        button(&ui, cancel, t!("Отмена", "Cancel", ja: "キャンセル", pt: "Cancelar", zh: "取消"), 1, hot(ID_CANCEL));
        button(&ui, okb, &ok_label, if danger { 2 } else { 0 }, hot(ID_OK));
    }

    // settings panel of a plugin (topbar: OneDrive switch, which shell)
    if let Some(i) = SETTINGS.get() {
        ui.shape([0.0, 0.0, w, WIN_H + 1.0], 0.0, argb(0, 150));
        let (d, _, closeb) = dialog_boxes(w);
        let card = rgb(44, 44, 46);
        ui.round(d, 14.0, card, rgb(64, 64, 66));
        ui.text(&tf!("Настройки {}", "{} settings", ja: "{} の設定", pt: "Configurações de {}", zh: "{} 设置", PLUGINS[i].name), [d[0] + 24.0, d[1] + 22.0, d[2] - 24.0, d[1] + 50.0], 16.0, 600, TEXT, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        let group = [d[0] + 24.0, d[1] + 72.0, d[2] - 24.0, d[1] + 72.0 + 128.0];
        let inner = rgb(58, 58, 60);
        ui.round(group, 10.0, inner, inner);
        ui.shape([group[0] + 16.0, group[1] + 64.0, group[2], group[1] + 64.0 + hair], 0.0, argb(rgb(72, 72, 74), 255));
        let rows = [
            (t!("Скрыть значок OneDrive", "Hide the OneDrive icon", ja: "OneDrive アイコンを隠す", pt: "Ocultar o ícone do OneDrive", zh: "隐藏 OneDrive 图标"), t!("Синхронизация продолжает работать", "Sync keeps working", ja: "同期は引き続き動作します", pt: "A sincronização continua funcionando", zh: "同步功能仍可正常使用"), od_hidden(), switch_box(w)),
            (t!("Заменить Проводник", "Replace Explorer", ja: "エクスプローラーを置き換える", pt: "Substituir o Explorer", zh: "替换资源管理器"), t!("Оболочкой становится rust-explorer", "rust-explorer becomes the shell", ja: "rust-explorer がシェルになります", pt: "O rust-explorer passa a ser o shell", zh: "rust-explorer 将成为外壳"), explorer_idx().is_some_and(|e| SELECTED[e].load(Relaxed)), shell_switch_box(w)),
        ];
        for (k, (name, note, on, sw)) in rows.into_iter().enumerate() {
            let t = group[1] + k as f32 * 64.0;
            ui.text(name, [group[0] + 16.0, t + 12.0, sw[0] - 12.0, t + 32.0], 14.0, 500, TEXT, DT_LEFT | DT_SINGLELINE | DT_VCENTER);
            ui.text(note, [group[0] + 16.0, t + 32.0, sw[0] - 12.0, t + 52.0], 12.0, 400, DIM, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
            switch(&ui, sw, on, false);
        }
        button(&ui, closeb, t!("Готово", "Done", ja: "完了", pt: "Concluído", zh: "完成"), 0, hot(ID_CANCEL));
    }

    GdipDeleteGraphics(g);
    BitBlt(hdc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
    SelectObject(mem, ob);
    DeleteObject(bmp);
    DeleteDC(mem);
    EndPaint(hwnd, &ps);
}

// ---------- window ----------

/// What a confirmed dialog runs.
#[derive(Clone, Copy)]
pub(crate) enum Act {
    InstallAll,
    Rollback,
}
pub(crate) struct Dialog {
    title: String,
    text: String,
    ok: String,
    danger: bool,
    act: Act,
}
thread_local! { pub(crate) static DIALOG: std::cell::RefCell<Option<Dialog>> = const { std::cell::RefCell::new(None) }; }

/// Clickable thing under the cursor (ID_*). While a panel is up only its buttons count; while a job runs nothing does.
pub(crate) unsafe fn hit(hwnd: HWND, lp: isize) -> Option<usize> {
    let (x, y) = ((lp & 0xFFFF) as i16 as f32, ((lp >> 16) & 0xFFFF) as i16 as f32);
    let mut rc: RECT = zeroed();
    let s = scale();
    GetClientRect(hwnd, &mut rc);
    let w = rc.right as f32 / s;
    let inside = |b: Box4| x >= b[0] * s && x <= b[2] * s && y >= b[1] * s && y <= b[3] * s;
    if SETTINGS.get().is_some() {
        let (_, _, closeb) = dialog_boxes(w);
        return if inside(switch_box(w)) { Some(ID_SWITCH) } else if inside(shell_switch_box(w)) { Some(ID_SHELL) } else if inside(closeb) { Some(ID_CANCEL) } else { None };
    }
    if DIALOG.with_borrow(|d| d.is_some()) {
        let (_, cancel, ok) = dialog_boxes(w);
        return if inside(ok) { Some(ID_OK) } else if inside(cancel) { Some(ID_CANCEL) } else { None };
    }
    if BUSY.load(Relaxed) {
        return None;
    }
    if gear_box(w).is_some_and(|(_, b)| inside(b)) {
        return Some(ID_GEAR);
    }
    if inside(all_box(w)) {
        return Some(ID_ALL);
    }
    if let Some(i) = rows().enumerate().find_map(|(k, i)| inside(check_box(k, w).1).then_some(i)) {
        return Some(ID_CHECK + i);
    }
    if inside(lang_box(w)) {
        return Some(ID_LANG);
    }
    if update::NEWER.lock().unwrap().is_some() && inside(update_box(w)) {
        return Some(ID_UPDATE);
    }
    if inside(main_box(w)) {
        return Some(ID_MAIN);
    }
    (INSTALLED.iter().any(|f| f.load(Relaxed)) && inside(rollback_box(w))).then_some(ID_ROLLBACK)
}

/// Runs `job` off the UI thread; its Ok/Err text becomes the status line. `which` = plugin index, n = rollback.
pub(crate) fn start(hwnd: HWND, which: usize, job: impl FnOnce() -> Result<String, String> + Send + 'static) {
    JOB.store(which, Relaxed);
    BUSY.store(true, Relaxed);
    HOVER.set(None);
    unsafe { InvalidateRect(hwnd, null(), 0) };
    let h = hwnd as isize;
    thread::spawn(move || {
        *MSG.lock().unwrap() = match job() {
            Ok(m) => (m, true),
            Err(e) => (tf!("Ошибка: {e}", "Error: {e}", ja: "エラー: {e}", pt: "Erro: {e}", zh: "错误：{e}"), false),
        };
        refresh();
        BUSY.store(false, Relaxed);
        unsafe { PostMessageW(h as HWND, WM_DONE, 0, 0) };
    });
}

/// The button's job: removes what was switched off, installs what was switched on, nothing else. With no change it
/// updates the installed components from this installer (not the extras: purge would run again). A failed step does not
/// stop the others; the failures are listed at the end. `h` = window to repaint between steps.
pub(crate) fn apply(h: isize) -> Result<String, String> {
    let (rm, add) = diff();
    let update = rm.is_empty() && add.is_empty();
    let steps: Vec<(usize, bool)> = if update {
        ready().filter(|&i| INSTALLED[i].load(Relaxed)).map(|i| (i, true)).collect()
    } else {
        rm.iter().map(|&i| (i, false)).chain(add.iter().map(|&i| (i, true))).collect()
    };
    *STEPS.lock().unwrap() = steps.clone();
    let (mut added, mut removed, mut failed, mut warned) = (Vec::new(), Vec::new(), Vec::new(), false);
    for (i, install) in steps {
        JOB.store(i, Relaxed);
        unsafe { InvalidateRect(h as HWND, null(), 0) };
        let r = if install { install_plugin(i) } else { uninstall_plugin(i) };
        match r {
            Ok(_) => {
                warned |= !WARN.lock().unwrap().is_empty(); // each step clears the list first
                if install { added.push(PLUGINS[i].name) } else { removed.push(PLUGINS[i].name) }
            }
            Err(e) => failed.push(format!("{}: {e}", PLUGINS[i].name)),
        }
        refresh();
    }
    STEPS.lock().unwrap().clear();
    let mut done = vec![];
    if !added.is_empty() {
        done.push(format!("{} {}", if update { t!("Обновлено:", "Updated:", ja: "更新:", pt: "Atualizados:", zh: "已更新：") } else { t!("Установлено:", "Installed:", ja: "インストール:", pt: "Instalados:", zh: "已安装：") }, added.join(", ")));
    }
    if !removed.is_empty() {
        done.push(format!("{} {}", t!("Удалено:", "Removed:", ja: "削除:", pt: "Removidos:", zh: "已移除："), removed.join(", ")));
    }
    let done = done.join(". ");
    if !failed.is_empty() {
        return Err(format!("{}.{}", failed.join("; "), if done.is_empty() { String::new() } else { format!(" {done}.") }));
    }
    let note = if warned { t!(" Есть предупреждения (см. %ProgramData%\\plugin-launcher\\install.log).", " There are warnings (see %ProgramData%\\plugin-launcher\\install.log).", ja: " 警告があります (%ProgramData%\\plugin-launcher\\install.log を参照)。", pt: " Há avisos (veja %ProgramData%\\plugin-launcher\\install.log).", zh: " 有警告（见 %ProgramData%\\plugin-launcher\\install.log）。") } else { "" };
    Ok(format!("{done}.{note}"))
}

/// A purge that has run stays on: switching it off would only delete purge.exe, the apps do not come back.
fn locked(i: usize) -> bool {
    PLUGINS[i].name == "purge" && INSTALLED[i].load(Relaxed)
}

pub(crate) unsafe fn ask(hwnd: HWND, d: Dialog) {
    DIALOG.set(Some(d));
    HOVER.set(None);
    InvalidateRect(hwnd, null(), 0);
}

pub(crate) unsafe fn close_dialog(hwnd: HWND, run: bool) {
    let Some(d) = DIALOG.take() else { return };
    HOVER.set(None);
    InvalidateRect(hwnd, null(), 0);
    if !run {
        return;
    }
    match d.act {
        Act::Rollback => start(hwnd, PLUGINS.len(), rollback),
        Act::InstallAll => {
            let h = hwnd as isize;
            start(hwnd, STARTING, move || apply(h))
        }
    }
}

pub(crate) unsafe fn click(hwnd: HWND, id: usize) {
    if SETTINGS.get().is_some() {
        if id == ID_SWITCH {
            if let Err(e) = set_od_hidden(!od_hidden()) {
                *MSG.lock().unwrap() = (tf!("Ошибка: {e}", "Error: {e}", ja: "エラー: {e}", pt: "Erro: {e}", zh: "错误：{e}"), false);
            }
        } else if id == ID_SHELL {
            if let Some(e) = explorer_idx() {
                SELECTED[e].fetch_xor(true, Relaxed);
            }
        } else if id == ID_CANCEL {
            SETTINGS.set(None);
        }
        HOVER.set(None);
        InvalidateRect(hwnd, null(), 0);
        return;
    }
    if id == ID_OK || id == ID_CANCEL {
        return close_dialog(hwnd, id == ID_OK);
    }
    if BUSY.load(Relaxed) {
        return;
    }
    match id {
        i if i >= ID_CHECK => {
            let i = i - ID_CHECK;
            if !locked(i) {
                let on = !SELECTED[i].fetch_xor(true, Relaxed);
                if let Some((other, v)) = linked(PLUGINS[i].name, on) {
                    PLUGINS.iter().position(|p| p.name == other).inspect(|&o| SELECTED[o].store(v, Relaxed));
                }
            }
            InvalidateRect(hwnd, null(), 0);
        }
        ID_MAIN if diff() == (vec![], vec![]) && !INSTALLED.iter().any(|f| f.load(Relaxed)) => {}
        ID_ALL => {
            let on = chosen().filter(|&i| PLUGINS[i].bulk).count() < N_READY;
            ready().for_each(|i| SELECTED[i].store(on, Relaxed));
            InvalidateRect(hwnd, null(), 0);
        }
        ID_UPDATE => {
            let _ = Command::new("explorer.exe").arg(format!("{}/latest", update::BASE)).spawn();
        }
        ID_LANG => {
            open_lang_list(hwnd);
            HOVER.set(None);
            InvalidateRect(hwnd, null(), 0);
        }
        ID_GEAR => {
            SETTINGS.set(PLUGINS.iter().position(|p| p.settings));
            HOVER.set(None);
            InvalidateRect(hwnd, null(), 0);
        }
        ID_ROLLBACK => ask(hwnd, Dialog {
            title: t!("Удалить WinRust?", "Remove WinRust?", ja: "WinRust を削除しますか?", pt: "Remover a WinRust?", zh: "移除 WinRust？").into(),
            text: t!("Все компоненты будут удалены. Вернутся поиск Windows, стандартный диспетчер задач, панель задач и обычный Проводник.", "All components will be removed. Windows Search, the stock Task Manager, the taskbar and regular Explorer come back.", ja: "すべてのコンポーネントが削除されます。Windows 検索、標準のタスク マネージャー、タスクバー、通常のエクスプローラーが元に戻ります。", pt: "Todos os componentes serão removidos. A Pesquisa do Windows, o Gerenciador de Tarefas padrão, a barra de tarefas e o Explorer comum voltam.", zh: "所有组件都将被移除。Windows 搜索、系统自带的任务管理器、任务栏和常规资源管理器都会恢复。").into(),
            ok: t!("Удалить всё", "Remove all", ja: "すべて削除", pt: "Remover tudo", zh: "全部移除").into(),
            danger: true,
            act: Act::Rollback,
        }),
        ID_MAIN if diff() == (vec![], vec![]) => ask(hwnd, Dialog {
            title: t!("Обновить WinRust?", "Update WinRust?", ja: "WinRust を更新しますか?", pt: "Atualizar a WinRust?", zh: "更新 WinRust？").into(),
            text: t!("Установленные компоненты будут переустановлены из этой версии установщика. Настройки сохранятся.", "The installed components will be reinstalled from this version of the installer. Settings are kept.", ja: "インストール済みのコンポーネントを、このバージョンのインストーラーから再インストールします。設定は保持されます。", pt: "Os componentes instalados serão reinstalados a partir desta versão do instalador. As configurações são mantidas.", zh: "已安装的组件将用此版本的安装程序重新安装。设置会被保留。").into(),
            ok: t!("Обновить", "Update", ja: "更新", pt: "Atualizar", zh: "更新").into(),
            danger: false,
            act: Act::InstallAll,
        }),
        ID_MAIN => {
            let (rm, add) = diff();
            let names = |v: &[usize]| v.iter().map(|&i| PLUGINS[i].name).collect::<Vec<_>>().join(", ");
            let mut text = String::new();
            if !add.is_empty() {
                text.push_str(&tf!("Будет установлено: {}.", "To be installed: {}.", ja: "インストールされるもの: {}。", pt: "Serão instalados: {}.", zh: "将安装：{}。", names(&add)));
            }
            if !rm.is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&tf!("Будет удалено: {} (вернутся стандартные части Windows).", "To be removed: {} (the stock Windows parts come back).", ja: "削除されるもの: {} (Windows 標準の部品が元に戻ります)。", pt: "Serão removidos: {} (as partes originais do Windows voltam).", zh: "将移除：{}（系统自带的 Windows 部件会恢复）。", names(&rm)));
            }
            // what the new ones change: the big ones get their own warning
            for &i in add.iter().filter(|&&i| PLUGINS[i].name == RUST_EXPLORER || !PLUGINS[i].bulk) {
                text.push_str(&format!("\n\n{}: {}", PLUGINS[i].name, PLUGINS[i].warn.get()));
            }
            if !add.is_empty() {
                text.push_str(t!("\n\nПеред установкой создаётся точка восстановления.", "\n\nA restore point is created before installing.", ja: "\n\nインストールの前に復元ポイントを作成します。", pt: "\n\nUm ponto de restauração é criado antes de instalar.", zh: "\n\n安装前会创建还原点。"));
            }
            let purge = add.iter().any(|&i| PLUGINS[i].name == "purge");
            let (title, ok, danger) = if purge {
                (t!("Удалить приложения навсегда?", "Remove apps for good?", ja: "アプリを完全に削除しますか?", pt: "Remover os apps de vez?", zh: "永久删除应用？"), t!("Удалить навсегда", "Remove for good", ja: "完全に削除", pt: "Remover de vez", zh: "永久删除"), true)
            } else if add.is_empty() {
                (t!("Удалить выбранное?", "Remove the selected?", ja: "選択したものを削除しますか?", pt: "Remover os selecionados?", zh: "移除所选项目？"), t!("Удалить", "Remove", ja: "削除", pt: "Remover", zh: "移除"), true)
            } else if rm.is_empty() {
                (t!("Установить выбранное?", "Install the selected?", ja: "選択したものをインストールしますか?", pt: "Instalar os selecionados?", zh: "安装所选项目？"), t!("Установить", "Install", ja: "インストール", pt: "Instalar", zh: "安装"), false)
            } else {
                (t!("Применить изменения?", "Apply the changes?", ja: "変更を適用しますか?", pt: "Aplicar as alterações?", zh: "应用更改？"), t!("Применить", "Apply", ja: "適用", pt: "Aplicar", zh: "应用"), false)
            };
            ask(hwnd, Dialog { title: title.into(), text, ok: ok.into(), danger, act: Act::InstallAll });
        }
        _ => {}
    }
}

thread_local! { static LANG_OPEN: Cell<Option<HWND>> = const { Cell::new(None) }; } // the open language list
thread_local! { static LANG_PARENT: Cell<Option<HWND>> = const { Cell::new(None) }; } // the window it belongs to
thread_local! { static LANG_HOVER: Cell<usize> = const { Cell::new(usize::MAX) }; }
pub(crate) const WM_LANG_DONE: u32 = WM_APP + 9; // a language was picked in the list: say that a restart is needed
const LANGS: [i18n::Lang; 5] = [i18n::Lang::En, i18n::Lang::Ru, i18n::Lang::Ja, i18n::Lang::Pt, i18n::Lang::Zh];

unsafe fn lang_scale() -> f32 {
    let dc = GetDC(null_mut());
    let s = GetDeviceCaps(dc, LOGPIXELSX as i32) as f32 / 96.0;
    ReleaseDC(null_mut(), dc);
    s
}

/// The item of the open list under `y` (device px), if any.
fn lang_item_at(y: i32, s: f32) -> Option<usize> {
    let pad = (4.0 * s) as i32;
    let h = (LANG_ITEM_H * s) as i32;
    let i = (y - pad) / h.max(1);
    (y >= pad && (i as usize) < LANGS.len()).then_some(i as usize)
}

unsafe fn paint_lang_list(h: HWND) {
    let mut ps: PAINTSTRUCT = zeroed();
    let dc = BeginPaint(h, &mut ps);
    let mut rc: RECT = zeroed();
    GetClientRect(h, &mut rc);
    let s = lang_scale();
    let bg = CreateSolidBrush(PANEL);
    FillRect(dc, &rc, bg);
    DeleteObject(bg);
    let border = CreateSolidBrush(BORDER);
    FillRect(dc, &RECT { right: rc.right, bottom: (s as i32).max(1), ..rc }, border);
    FillRect(dc, &RECT { left: rc.left, top: rc.bottom - (s as i32).max(1), right: rc.right, bottom: rc.bottom }, border);
    FillRect(dc, &RECT { left: rc.left, top: rc.top, right: (s as i32).max(1), bottom: rc.bottom }, border);
    FillRect(dc, &RECT { left: rc.right - (s as i32).max(1), top: rc.top, right: rc.right, bottom: rc.bottom }, border);
    DeleteObject(border);
    let pad = (4.0 * s) as i32;
    let ih = (LANG_ITEM_H * s) as i32;
    let hover = LANG_HOVER.get();
    let font = CreateFontW(-(13.0 * s) as i32, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide(winrust_common::style::sans_face()).as_ptr());
    for (i, l) in LANGS.iter().enumerate() {
        let r = RECT { left: (4.0 * s) as i32, top: pad + ih * i as i32, right: rc.right - (4.0 * s) as i32, bottom: pad + ih * (i as i32 + 1) };
        if i == hover {
            let b = CreateSolidBrush(lerp(PANEL, TEXT, 0.1));
            FillRect(dc, &r, b);
            DeleteObject(b);
        }
        let cur = *l == i18n::lang();
        let f = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, if cur { ACCENT } else { TEXT });
        let mut tr = r;
        DrawTextW(dc, wide(l.name()).as_ptr(), -1, &mut tr, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
        if cur {
            let icon = CreateFontW(-(12.0 * s) as i32, 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide(winrust_common::style::icon_face()).as_ptr());
            let fi = SelectObject(dc, icon);
            let mut tr = r;
            DrawTextW(dc, wide("\u{E73E}").as_ptr(), -1, &mut tr, DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            SelectObject(dc, fi);
            DeleteObject(icon);
        }
        SelectObject(dc, f);
    }
    DeleteObject(font);
    EndPaint(h, &mut ps);
}

unsafe extern "system" fn lang_list_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint_lang_list(h);
            0
        }
        WM_MOUSEMOVE => {
            let s = lang_scale();
            let y = (lp >> 16) as i16 as i32;
            let i = lang_item_at(y, s).unwrap_or(usize::MAX);
            if i != LANG_HOVER.get() {
                LANG_HOVER.set(i);
                InvalidateRect(h, null(), 0);
            }
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
            0
        }
        WM_MOUSELEAVE => {
            LANG_HOVER.set(usize::MAX);
            InvalidateRect(h, null(), 0);
            0
        }
        WM_LBUTTONUP => {
            let s = lang_scale();
            let y = (lp >> 16) as i16 as i32;
            if let Some(i) = lang_item_at(y, s) {
                let written = i18n::set_lang(LANGS[i]);
                // the installer repaints itself; the other programs read lang.txt when they start
                if let Some(parent) = LANG_PARENT.get() {
                    PostMessageW(parent, WM_LANG_DONE, written as usize, 0);
                }
            }
            DestroyWindow(h);
            0
        }
        WM_KEYDOWN if wp as u16 == VK_ESCAPE => {
            DestroyWindow(h);
            0
        }
        WM_KILLFOCUS => {
            DestroyWindow(h);
            0
        }
        WM_DESTROY => {
            LANG_OPEN.set(None);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

/// Opens the language list under the drop-down box in the top right corner.
pub(crate) unsafe fn open_lang_list(hwnd: HWND) {
    if LANG_OPEN.get().is_some() {
        return;
    }
    let s = lang_scale();
    let hi = GetModuleHandleW(null());
    let cls = wide(LANG_LIST);
    let wc = WNDCLASSW { lpfnWndProc: Some(lang_list_proc), hInstance: hi, hCursor: LoadCursorW(null_mut(), IDC_ARROW), lpszClassName: cls.as_ptr(), ..zeroed() };
    RegisterClassW(&wc);
    let b = lang_box(WIN_W);
    let mut pt = POINT { x: (b[0] * s) as i32, y: (b[3] * s) as i32 + 2 };
    ClientToScreen(hwnd, &mut pt);
    let w = ((b[2] - b[0]) * s) as i32;
    let h = ((LANG_ITEM_H * LANGS.len() as f32 + 8.0) * s) as i32;
    let list = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST, cls.as_ptr(), null(), WS_POPUP, pt.x, pt.y, w, h, null_mut(), null_mut(), hi, null());
    LANG_HOVER.set(usize::MAX);
    LANG_PARENT.set(Some(hwnd));
    LANG_OPEN.set(Some(list));
    ShowWindow(list, SW_SHOW);
    SetForegroundWindow(list);
}

/// The status line after a language switch: the other programs pick lang.txt up when they start. `written` is false
/// when lang.txt could not be written (the installer runs without administrator rights), which is said instead.
pub(crate) fn lang_changed(hwnd: HWND, written: bool) {
    let (text, ok) = if written {
        (
            t!(
                "Язык изменён. Перезапустите программы WinRust (или выйдите из системы), чтобы он применился везде.",
                "Language changed. Restart the WinRust programs (or sign out) to apply it everywhere.",
                ja: "言語を変更しました。すべてに反映するには WinRust の各プログラムを再起動 (またはサインアウト) してください。",
                pt: "Idioma alterado. Reinicie os programas da WinRust (ou saia da sessão) para aplicá-lo em todos.",
                zh: "语言已切换。请重启 WinRust 程序（或注销后重新登录），让所有程序都生效。"
            ),
            true,
        )
    } else {
        (
            t!(
                "Язык переключён только в этом окне: %ProgramData%\\WinRust\\lang.txt защищён от записи. Запустите установщик от имени администратора, чтобы он применился везде.",
                "Only this window switched: %ProgramData%\\WinRust\\lang.txt could not be written. Run the installer as administrator to apply the language everywhere.",
                ja: "このウィンドウだけが切り替わりました: %ProgramData%\\WinRust\\lang.txt に書き込めません。すべてに反映するにはインストーラーを管理者として実行してください。",
                pt: "Somente esta janela mudou: não foi possível gravar %ProgramData%\\WinRust\\lang.txt. Execute o instalador como administrador para aplicar o idioma em todos.",
                zh: "只有这个窗口切换了：无法写入 %ProgramData%\\WinRust\\lang.txt。请以管理员身份运行安装程序，才能让所有程序都生效。"
            ),
            false,
        )
    };
    *MSG.lock().unwrap() = (text.into(), ok);
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

pub(crate) unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => paint(hwnd),
        WM_ERASEBKGND => return 1,
        WM_MOUSEMOVE => {
            let h = hit(hwnd, lp);
            if h != HOVER.get() {
                HOVER.set(h);
                InvalidateRect(hwnd, null(), 0);
            }
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            TrackMouseEvent(&mut t);
        }
        WM_SETCURSOR if (lp & 0xFFFF) as u32 == HTCLIENT => {
            SetCursor(LoadCursorW(null_mut(), if HOVER.get().is_some() { IDC_HAND } else { IDC_ARROW }));
            return 1;
        }
        WM_MOUSELEAVE => {
            HOVER.set(None);
            InvalidateRect(hwnd, null(), 0);
        }
        WM_LBUTTONUP => {
            if let Some(i) = hit(hwnd, lp) {
                click(hwnd, i);
            }
        }
        WM_KEYDOWN if SETTINGS.get().is_some() && wp as u16 == VK_ESCAPE => {
            SETTINGS.set(None);
            InvalidateRect(hwnd, null(), 0);
        }
        WM_KEYDOWN if DIALOG.with_borrow(|d| d.is_some()) => match wp as u16 {
            VK_ESCAPE => close_dialog(hwnd, false),
            VK_RETURN if DIALOG.with_borrow(|d| d.as_ref().is_some_and(|d| !d.danger)) => close_dialog(hwnd, true), // a dangerous one needs the click

            _ => {}
        },
        WM_LANG_DONE => lang_changed(hwnd, wp != 0),
        WM_DONE => {
            InvalidateRect(hwnd, null(), 0);
        }
        WM_CLOSE if BUSY.load(Relaxed) => {} // closing mid-install would leave the system half-changed
        WM_DESTROY => {
            LANG_OPEN.with(|o| {
                if let Some(h) = o.take() {
                    DestroyWindow(h);
                }
            });
            PostQuitMessage(0)
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    0
}
