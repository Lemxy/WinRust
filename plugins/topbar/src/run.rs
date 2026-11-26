//! topbar: run (split out of main.rs; shares its items through `super::*`).
//!
//! The WinRust "Run" box (Win+R). It is our own small frost popup with a single-line field, its history and two buttons,
//! instead of Windows' 1995 dialog. Commands go through `winrust_common::launch`, i.e. with the user's normal token: the
//! box never elevates by itself.
//!
//! Win+R itself is taken by the low-level keyboard hook below (`kb_hook`), which is what makes the box work while
//! Explorer is not the shell. `main` additionally registers the same key with `RegisterHotKey` as a backup: if Windows
//! drops the hook (it does that when the hook answers too slowly) the registered hotkey still opens the box.
use super::*;
use std::sync::atomic::AtomicIsize;
use winrust_common::frost;

const RUN_CLASS: &str = "topbar_run";
const RUN_W: i32 = 420;
const RUN_H: i32 = 150;
const RUN_EDIT: i32 = 101;
const WM_RUN_KEY: u32 = WM_APP + 18; // posted by the field: Enter, Esc, Up, Down (wParam = the key)
const WM_RUN_FOCUS: u32 = WM_APP + 19; // a second Win+R: put this dialog in front again
const EM_SETSEL: u32 = 0xB1;
const EM_SETMARGINS: u32 = 0xD3;
const EM_LIMITTEXT: u32 = 0xC5;
const DLGC_WANTALLKEYS: usize = 0x0004;
const DLGC_WANTARROWS: usize = 0x0001;
const DLGC_WANTCHARS: usize = 0x0080;

/// One Win+R press already opening a box: the hook can fire again before the window exists.
static RUN_BUSY: AtomicBool = AtomicBool::new(false);
/// The field's own procedure, needed to call the original one back.
static RUN_EDIT_OLD: AtomicIsize = AtomicIsize::new(0);

struct RunBox {
    edit: HWND,
    font: HFONT,
    small: HFONT,  // the hint line and the button labels
    brush: HBRUSH, // the field's background, returned on WM_CTLCOLOREDIT
    px: Vec<u32>,  // the screen behind the popup, captured before it is shown
    history: Vec<String>, // newest first
    idx: usize,           // 0 = what the user typed, 1 = newest entry, ...
    typed: String,        // the text to come back to on the way down
    hover: u8,            // 0 none, 1 OK, 2 Cancel
}

thread_local! {
    static RUN: RefCell<Option<RunBox>> = const { RefCell::new(None) };
}

// ---------- history: %APPDATA%\WinRust\run-history.txt ----------

fn history_file() -> PathBuf {
    appdata_file(r"WinRust\run-history.txt")
}

fn load_history() -> Vec<String> {
    std::fs::read_to_string(history_file())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(50)
        .map(str::to_string)
        .collect()
}

/// Remembers `line` as the newest command: no duplicates (ignoring case, like Win+R's own list), 50 at most.
fn save_history(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let mut v = load_history();
    v.retain(|l| !l.eq_ignore_ascii_case(line));
    v.insert(0, line.to_string());
    v.truncate(50);
    let p = history_file();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, v.join("\n") + "\n");
}

/// Pushes the field's text into the visible history: `idx` 0 is the text the user was typing.
fn show_entry(delta: i32) {
    // compute under the borrow, then set the text outside it: SetWindowText may ask the parent to colour the field
    let target = RUN.with_borrow_mut(|s| {
        let rb = s.as_mut()?;
        if delta < 0 {
            if rb.idx == 0 {
                rb.typed = edit_text(rb.edit);
            }
            if rb.idx < rb.history.len() {
                rb.idx += 1;
            }
        } else if rb.idx > 1 {
            rb.idx -= 1;
        } else if rb.idx == 1 {
            rb.idx = 0;
            return Some((rb.edit, rb.typed.clone()));
        } else {
            return None;
        }
        Some((rb.edit, if rb.idx == 0 { rb.typed.clone() } else { rb.history[rb.idx - 1].clone() }))
    });
    if let Some((ed, text)) = target {
        set_edit_text(ed, &text);
    }
}

fn edit_text(ed: HWND) -> String {
    unsafe {
        let mut buf = vec![0u16; 1024];
        let n = GetWindowTextW(ed, buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

fn set_edit_text(ed: HWND, text: &str) {
    unsafe {
        SetWindowTextW(ed, wide(text).as_ptr());
        // put the caret after the last character, the way the Windows box does when arrowing through its history
        let n = text.encode_utf16().count() as isize;
        SendMessageW(ed, EM_SETSEL as u32, n as usize, n);
    }
}

// ---------- the field: Enter / Esc / arrows are the window's ----------

unsafe extern "system" fn edit_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_GETDLGCODE => (DLGC_WANTALLKEYS | DLGC_WANTARROWS | DLGC_WANTCHARS) as LRESULT,
        WM_KEYDOWN if matches!(wp as u16, VK_RETURN | VK_ESCAPE | VK_UP | VK_DOWN) => {
            PostMessageW(GetParent(h), WM_RUN_KEY, wp, 0);
            0
        }
        WM_CHAR if wp == 13 || wp == 27 => 0,
        WM_CHAR | WM_KEYDOWN => {
            // typing leaves history mode; the next Up starts from the newest entry again
            RUN.with_borrow_mut(|s| {
                if let Some(rb) = s.as_mut() {
                    rb.idx = 0;
                }
            });
            CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(RUN_EDIT_OLD.load(Relaxed)), h, msg, wp, lp)
        }
        _ => CallWindowProcW(std::mem::transmute::<isize, WNDPROC>(RUN_EDIT_OLD.load(Relaxed)), h, msg, wp, lp),
    }
}

// ---------- layout and painting ----------

struct Rect {
    field: RECT,
    edit: RECT,
    ok: RECT,
    cancel: RECT,
}

fn run_layout(w: i32, h: i32) -> Rect {
    let p = sc;
    let (l, r) = (p(16), w - p(16));
    let field = RECT { left: l, top: p(56), right: r, bottom: p(96) };
    let edit = RECT { left: field.left + p(8), top: field.top + p(5), right: field.right - p(8), bottom: field.bottom - p(5) };
    let (bw, bh) = (p(88), p(30));
    let top = h - p(44);
    let cancel = RECT { left: r - bw, top, right: r, bottom: top + bh };
    let ok = RECT { left: cancel.left - p(8) - bw, top, right: cancel.left - p(8), bottom: top + bh };
    Rect { field, edit, ok, cancel }
}

fn inside(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

unsafe fn button(dc: HDC, r: RECT, label: &str, primary: bool, font: HFONT) {
    let (bg, fg) = if primary { (pal().accent, on_accent()) } else { (pal().hover, pal().text) };
    fill(dc, r, bg, sc(8));
    let of = SelectObject(dc, font);
    text(dc, label, r, fg, DT_CENTER);
    SelectObject(dc, of);
}

// ---------- the window ----------

unsafe extern "system" fn run_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_RUN_FOCUS => {
            winrust_common::fg::force(hwnd);
            RUN.with_borrow(|s| {
                if let Some(rb) = s.as_ref() {
                    SetFocus(rb.edit);
                }
            });
            0
        }
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut cr: RECT = zeroed();
            GetClientRect(hwnd, &mut cr);
            RUN.with_borrow(|s| {
                let Some(rb) = s.as_ref() else { return };
                // the frost the flyouts use; a light style would make the captured dark glass unreadable, so it gets the
                // solid background instead
                let frosted = !rb.px.is_empty() && !style().light() && frost::paint_capture(dc, &rb.px, cr, pal().bg, frost::tint_alpha());
                if !frosted {
                    fill(dc, cr, pal().bg, 0);
                }
                // title and the line that says what the box takes
                let title = t!("Выполнить", "Run", ja: "ファイル名を指定して実行", pt: "Executar", zh: "运行");
                let hint = t!(
                    "Введите имя программы, папки, документа или интернет-адреса",
                    "Type the name of a program, folder, document or web address",
                    ja: "プログラム、フォルダー、ドキュメント、または Web アドレスの名前を入力してください",
                    pt: "Digite o nome de um programa, pasta, documento ou endereço da Web",
                    zh: "键入程序、文件夹、文档或 Internet 地址的名称"
                );
                let of = SelectObject(dc, rb.font);
                text(dc, title, RECT { left: sc(16), top: sc(12), right: cr.right - sc(16), bottom: sc(34) }, pal().text, DT_LEFT);
                SelectObject(dc, rb.small);
                text(dc, hint, RECT { left: sc(16), top: sc(34), right: cr.right - sc(16), bottom: sc(54) }, pal().dim, DT_LEFT);
                SelectObject(dc, of);
                // the field's plate: the EDIT sits inset on it, so its corners look rounded too
                let r = run_layout(cr.right, cr.bottom);
                fill(dc, r.field, card(), sc(10));
                let ok = if lang!() == winrust_common::Lang::Es {
                    "Aceptar" // `common/es.tsv` is maintained elsewhere: keep the exact Spanish word here
                } else {
                    t!("ОК", "OK", ja: "OK", pt: "OK", zh: "确定")
                };
                let cancel = t!("Отмена", "Cancel", ja: "キャンセル", pt: "Cancelar", zh: "取消");
                button(dc, r.ok, ok, true, rb.small);
                button(dc, r.cancel, cancel, false, rb.small);
            });
            EndPaint(hwnd, &ps);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLOREDIT => {
            let dc = wp as HDC;
            SetTextColor(dc, pal().text);
            SetBkColor(dc, card());
            RUN.with_borrow(|s| s.as_ref().map_or(0, |rb| rb.brush as LRESULT))
        }
        WM_MOUSEMOVE => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let mut cr: RECT = zeroed();
            GetClientRect(hwnd, &mut cr);
            let r = run_layout(cr.right, cr.bottom);
            let h = if inside(r.ok, x, y) { 1 } else if inside(r.cancel, x, y) { 2 } else { 0 };
            let changed = RUN.with_borrow_mut(|s| s.as_mut().is_some_and(|rb| std::mem::replace(&mut rb.hover, h) != h));
            if changed {
                InvalidateRect(hwnd, null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
            let mut cr: RECT = zeroed();
            GetClientRect(hwnd, &mut cr);
            let r = run_layout(cr.right, cr.bottom);
            if inside(r.ok, x, y) {
                submit(hwnd);
            } else if inside(r.cancel, x, y) {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_RUN_KEY => {
            match wp as u16 {
                VK_RETURN => submit(hwnd),
                VK_ESCAPE => { DestroyWindow(hwnd); }
                VK_UP => show_entry(-1),
                VK_DOWN => show_entry(1),
                _ => {}
            }
            0
        }
        WM_ACTIVATE if (wp & 0xffff) != 0 => {
            // clicking the popup (not the field) must not lose the caret
            RUN.with_borrow(|s| {
                if let Some(rb) = s.as_ref() {
                    SetFocus(rb.edit);
                }
            });
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            RUN.with(|s| {
                if let Ok(mut b) = s.try_borrow_mut() {
                    if let Some(rb) = b.take() {
                        DeleteObject(rb.font);
                        DeleteObject(rb.small);
                        DeleteObject(rb.brush);
                    }
                }
            });
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Runs the field's text: expands %VARS% and hands it to the shared launcher. Off the UI thread: a slow target must not
/// freeze the popup (and the popup is about to be destroyed anyway).
unsafe fn submit(hwnd: HWND) {
    let text = RUN.with_borrow(|s| s.as_ref().map(|rb| edit_text(rb.edit)));
    let Some(mut text) = text else { return };
    text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    save_history(&text);
    DestroyWindow(hwnd);
    std::thread::spawn(move || {
        let expanded = expand_env(&text);
        let expanded = expanded.trim();
        if expanded.is_empty() {
            return;
        }
        if expanded.to_ascii_lowercase().starts_with("ms-settings:") {
            // our settings hub when it is installed, Windows' own page otherwise
            winrust_common::settings::open(expanded);
            return;
        }
        let (program, args) = split_command(expanded);
        if args.is_empty() {
            winrust_common::launch::program(&program);
        } else {
            winrust_common::launch::line(&program, &args);
        }
    });
}

/// Splits a Run command into the program and its arguments: a quoted program, an existing path (which may contain
/// spaces), else the first word. The shell gets the two parts separately, so no quoting rules are guessed here.
fn split_command(s: &str) -> (String, String) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(i) = rest.find('"') {
            return (rest[..i].to_string(), rest[i + 1..].trim().to_string());
        }
    }
    if std::path::Path::new(s).exists() {
        return (s.to_string(), String::new());
    }
    match s.find(char::is_whitespace) {
        Some(i) => (s[..i].to_string(), s[i..].trim().to_string()),
        None => (s.to_string(), String::new()),
    }
}

/// The Windows "Run" box: the fallback when our own window cannot be created. `rundll32 shell32.dll,#61` cannot do it
/// (ordinal 61 is RunFileDlg(hwnd, icon, dir, title, prompt, flags) and rundll32 passes other arguments).
fn windows_run_dialog() {
    unsafe {
        let sh = LoadLibraryW(wide("shell32.dll").as_ptr());
        if let Some(f) = GetProcAddress(sh, 61 as *const u8) {
            type RunFileDlg = unsafe extern "system" fn(HWND, HICON, *const u16, *const u16, *const u16, u32);
            std::mem::transmute::<_, RunFileDlg>(f)(null_mut(), null_mut(), null(), null(), null(), 0);
        }
    }
}

/// Opens the WinRust Run box. Called by the keyboard hook (Win+R) and by Start's "Run"; never blocks the caller.
pub(crate) fn run_box() {
    unsafe {
        let h = FindWindowW(wide(RUN_CLASS).as_ptr(), null());
        if !h.is_null() {
            PostMessageW(h, WM_RUN_FOCUS, 0, 0);
            return;
        }
    }
    if RUN_BUSY.swap(true, Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        run_dialog();
        RUN_BUSY.store(false, Relaxed);
    });
}

/// Win key on its own opens our Start. Runs on its own thread so a busy bar can never stall the system keyboard.
pub(crate) unsafe extern "system" fn kb_hook(n: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if n >= 0 {
        let k = &*(l as *const KBDLLHOOKSTRUCT);
        let down0 = w == WM_KEYDOWN as usize || w == WM_SYSKEYDOWN as usize;
        // without explorer nobody handles the volume keys, and Surface's power + volume-up arrives as an injected Win+PrtSc
        // (the injected events are skipped below, so these two are taken before that check)
        if !EXPLORER_UP.load(Relaxed) {
            let vol = [(VK_VOLUME_UP, 0), (VK_VOLUME_DOWN, 1), (VK_VOLUME_MUTE, 2)].into_iter().find(|&(vk, _)| k.vkCode == vk as u32);
            if let Some((_, v)) = vol {
                if down0 {
                    PostMessageW(BAR.load(Relaxed) as HWND, VOLKEY, v, 0);
                }
                return 1;
            }
            if k.vkCode == VK_SNAPSHOT as u32 && k.flags & LLKHF_INJECTED != 0 {
                if down0 {
                    PostMessageW(BAR.load(Relaxed) as HWND, SNIP, 1, 0);
                }
                return 1;
            }
        }
        if k.flags & LLKHF_INJECTED == 0 {
            // Win+Shift+S is our own region clipper. PrtSc is taken too when rs-shot is installed (even under Explorer,
            // where Windows' own snip may own the key); without rs-shot PrtSc is taken only when nobody else handles it.
            let held = |vk: u16| GetAsyncKeyState(vk as i32) < 0;
            let snip_key = k.vkCode == VK_SNAPSHOT as u32 && (!EXPLORER_UP.load(Relaxed) || super::shot::rs_shot().is_some());
            let win_shift_s = k.vkCode == b'S' as u32 && down0 && (held(VK_LWIN) || held(VK_RWIN)) && held(VK_SHIFT);
            if snip_key || win_shift_s {
                if down0 {
                    WIN_ALONE.store(false, Relaxed);
                    if win_shift_s {
                        // a key between Win down and up, so Windows does not open its Start menu on the release
                        keybd_event(0xE8, 0, 0, 0);
                        keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
                    }
                    // rs-shot when installed (default bar: Photo + Region); otherwise the bar's own clipper, as before
                    if !rs_shot_start(&[]) {
                        PostMessageW(BAR.load(Relaxed) as HWND, SNIP, snip_key as usize, 0); // wp 1: PrtSc, 0: Win+Shift+S
                    }
                }
                return 1;
            }
            // Win+Alt+R: rs-shot records the region (the Game Bar shortcut is only taken when rs-shot exists)
            if k.vkCode == b'R' as u32 && down0 && (held(VK_LWIN) || held(VK_RWIN)) && held(VK_MENU) && !held(VK_SHIFT) && !held(VK_CONTROL) && rs_shot_start(&["--record"]) {
                WIN_ALONE.store(false, Relaxed);
                keybd_event(0xE8, 0, 0, 0);
                keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
                return 1;
            }
            // Win+1..9: the n-th app button of the bar (Windows 11 taskbar behaviour)
            if (0x31..=0x39).contains(&k.vkCode) && (held(VK_LWIN) || held(VK_RWIN)) && !held(VK_CONTROL) && !held(VK_MENU) && !held(VK_SHIFT) {
                if down0 {
                    WIN_ALONE.store(false, Relaxed);
                    keybd_event(0xE8, 0, 0, 0); // a key between Win down and up: no Start menu on release
                    keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
                    PostMessageW(BAR.load(Relaxed) as HWND, WIN_NUM, k.vkCode as usize - 0x31, 0);
                }
                return 1;
            }
            // Win+Space / Win+Shift+Space: without explorer nobody switches the keyboard layout (Alt+Shift still works, the
            // system handles that one itself). Our own Start in front: the system switcher's request never reaches the elevated
            // bar (UIPI drops it), so it is done here too (#105).
            if (!EXPLORER_UP.load(Relaxed) || own_foreground()) && k.vkCode == VK_SPACE as u32 && (held(VK_LWIN) || held(VK_RWIN)) && !held(VK_CONTROL) && !held(VK_MENU) {
                if down0 {
                    WIN_ALONE.store(false, Relaxed);
                    keybd_event(0xE8, 0, 0, 0); // a key between Win down and up: no Start menu on release
                    keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
                    PostMessageW(BAR.load(Relaxed) as HWND, NEXT_LANG, held(VK_SHIFT) as usize, 0);
                }
                return 1;
            }
            // Win+R: always our Run box, also while Explorer runs (lent to Settings, or not replaced): its own box is the plain one
            if k.vkCode == b'R' as u32 && (held(VK_LWIN) || held(VK_RWIN)) && !held(VK_CONTROL) && !held(VK_MENU) && !held(VK_SHIFT) {
                if down0 {
                    WIN_ALONE.store(false, Relaxed);
                    keybd_event(0xE8, 0, 0, 0); // a key between Win down and up: no Start menu on release
                    keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
                    run_box();
                }
                return 1;
            }
            // Win+X: our power-user menu (the one Start's right click opens); Explorer's needs its own taskbar, which is hidden
            if k.vkCode == b'X' as u32 && (held(VK_LWIN) || held(VK_RWIN)) && !held(VK_CONTROL) && !held(VK_MENU) && !held(VK_SHIFT) {
                if down0 {
                    WIN_ALONE.store(false, Relaxed);
                    keybd_event(0xE8, 0, 0, 0); // a key between Win down and up: no Start menu on release
                    keybd_event(0xE8, 0, KEYEVENTF_KEYUP, 0);
                    PostMessageW(BAR.load(Relaxed) as HWND, WIN_X, 0, 0);
                }
                return 1;
            }
            let down = w == WM_KEYDOWN as usize || w == WM_SYSKEYDOWN as usize;
            let bar = BAR.load(Relaxed) as HWND;
            // Alt+Tab: our own switcher. Without explorer Windows only flips to the next window blindly, and a fullscreen
            // game often takes the focus straight back.
            if k.vkCode == VK_TAB as u32 && k.flags & LLKHF_ALTDOWN != 0 && GetAsyncKeyState(VK_CONTROL as i32) >= 0 {
                if down {
                    SWITCHING.store(true, Relaxed);
                    PostMessageW(bar, ALT_TAB, (GetAsyncKeyState(VK_SHIFT as i32) < 0) as usize, 0);
                }
                return 1;
            }
            if SWITCHING.load(Relaxed) {
                if k.vkCode == VK_ESCAPE as u32 && down {
                    SWITCHING.store(false, Relaxed);
                    PostMessageW(bar, ALT_UP, 1, 0);
                    return 1;
                }
                // the switcher owns these keys while open (they must not reach the app behind it, #92)
                if matches!(k.vkCode as u16, VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_DELETE | VK_RETURN) {
                    if down {
                        match k.vkCode as u16 {
                            VK_LEFT | VK_UP => { PostMessageW(bar, ALT_TAB, 1, 0); }
                            VK_RIGHT | VK_DOWN => { PostMessageW(bar, ALT_TAB, 0, 0); }
                            VK_DELETE => { PostMessageW(bar, ALT_UP, 2, 0); }
                            _ => {
                                // Enter: switch now; the Alt release afterwards must not open the menu bar
                                SWITCHING.store(false, Relaxed);
                                PostMessageW(bar, ALT_UP, 0, 0);
                                keys(&[0xE8]);
                            }
                        }
                    }
                    return 1;
                }
                if matches!(k.vkCode as u16, VK_LMENU | VK_RMENU | VK_MENU) && !down {
                    SWITCHING.store(false, Relaxed);
                    keys(&[0xE8]); // the target app must not take the lone Alt release as "open the menu bar"
                    PostMessageW(bar, ALT_UP, 0, 0);
                }
            }
            if k.vkCode == VK_LWIN as u32 || k.vkCode == VK_RWIN as u32 {
                if down {
                    WIN_ALONE.store(true, Relaxed);
                } else if WIN_ALONE.swap(false, Relaxed) {
                    // A lone Win *release* is what opens Windows' own Start. An E8 sent from here used to be queued behind
                    // the release already being dispatched, so the shell sometimes saw no other key in time and opened its
                    // Start on top of ours. Swallow the real release and hand the shell an E8 and then a synthetic Win-up:
                    // it sees a key between down and up (so no Start), and the synthetic up still clears the Win key for
                    // everything else (our own hook skips it: injected). A Win+key chord never reaches here: the key down
                    // clears WIN_ALONE before the release.
                    keys(&[0xE8]);
                    crate::key_up(k.vkCode as u16);
                    PostMessageW(BAR.load(Relaxed) as HWND, WIN_START, 0, 0);
                }
            } else if down {
                WIN_ALONE.store(false, Relaxed); // Win+something else: a shortcut, not a Start press
            }
        }
    }
    CallNextHookEx(null_mut(), n, w, l)
}

/// The box on this thread, blocking until it is closed. `topbar --run` uses it; Win+R goes through `run_box`.
pub(crate) fn run_dialog() {
    unsafe {
        let h = FindWindowW(wide(RUN_CLASS).as_ptr(), null());
        if !h.is_null() {
            PostMessageW(h, WM_RUN_FOCUS, 0, 0);
            return;
        }
        let class = wide(RUN_CLASS);
        let hi = GetModuleHandleW(null());
        let wc = WNDCLASSW { lpfnWndProc: Some(run_proc), hInstance: hi, hCursor: LoadCursorW(null_mut(), IDC_ARROW), lpszClassName: class.as_ptr(), ..zeroed() };
        RegisterClassW(&wc);
        let (w, h) = (sc(RUN_W), sc(RUN_H));
        let mut pt: POINT = zeroed();
        GetCursorPos(&mut pt);
        let mon = winrust_common::monitors::at(pt.x, pt.y).rc;
        let x = mon.left + ((mon.right - mon.left) - w) / 2;
        let y = mon.top + ((mon.bottom - mon.top) - h) / 2;
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            wide(t!("Выполнить", "Run", ja: "ファイル名を指定して実行", pt: "Executar", zh: "运行")).as_ptr(),
            WS_POPUP,
            x,
            y,
            w,
            h,
            null_mut(),
            null_mut(),
            hi,
            null(),
        );
        if hwnd.is_null() {
            windows_run_dialog();
            return;
        }
        // frozen before the first ShowWindow: the window itself is not in the capture
        let px = frost::capture_rect(RECT { left: x, top: y, right: x + w, bottom: y + h }, frost::radius_for(w, h));
        let font = CreateFontW(-sc(14), 0, 0, 0, 500, 0, 0, 0, 1, 0, 0, 5, 0, wide(ui_face()).as_ptr());
        let small = CreateFontW(-sc(12), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, wide(ui_face()).as_ptr());
        let brush = CreateSolidBrush(card());
        RUN.with_borrow_mut(|s| {
            *s = Some(RunBox { edit: null_mut(), font, small, brush, px, history: load_history(), idx: 0, typed: String::new(), hover: 0 })
        });
        let r = run_layout(w, h);
        let ed = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            null(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
            r.edit.left,
            r.edit.top,
            r.edit.right - r.edit.left,
            r.edit.bottom - r.edit.top,
            hwnd,
            RUN_EDIT as isize as *mut std::ffi::c_void,
            hi,
            null(),
        );
        if ed.is_null() {
            DestroyWindow(hwnd);
            windows_run_dialog();
            return;
        }
        SendMessageW(ed, WM_SETFONT, font as usize, 1);
        SendMessageW(ed, EM_LIMITTEXT, 1024, 0);
        SendMessageW(ed, EM_SETMARGINS, 3, (sc(6) * 0x10001) as isize); // left and right
        RUN_EDIT_OLD.store(SetWindowLongPtrW(ed, GWLP_WNDPROC, edit_proc as *const () as isize), Relaxed);
        RUN.with_borrow_mut(|s| {
            if let Some(rb) = s.as_mut() {
                rb.edit = ed;
            }
        });
        SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, sc(18), sc(18)), 1);
        ShowWindow(hwnd, SW_SHOW);
        winrust_common::fg::force(hwnd);
        SetFocus(ed);
        let mut m: MSG = zeroed();
        while GetMessageW(&mut m, null_mut(), 0, 0) > 0 {
            TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
}
