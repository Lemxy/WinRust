#![windows_subsystem = "windows"]
#![allow(unsafe_op_in_unsafe_fn)]
// rs-calc [expression]   The WinRust calculator: a small always-focused standard/scientific calculator with memory,
// a 20-entry history and a command line that evaluates an expression and prints the result (exit code 0/1).
//   rs-calc                 the window
//   rs-calc "2+2*3"         evaluate and print (scripts: `start /wait rs-calc "2+2"`)
//   rs-calc --rad "sin(pi/2)"
// The window is frameless with the project's own chrome (rounded corners, style palette, optional frost), resizable
// from 320x480, and follows the style/language files of the other WinRust programs. No dependencies beyond the
// shared crate and windows-sys; the maths live in calc.rs.
#[macro_use]
extern crate winrust_common;
mod calc;

use calc::Angle;
use std::{
    cell::RefCell,
    path::PathBuf,
    ptr::{null, null_mut},
};
use winrust_common::{frost, style as st};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{
        Dwm::DwmSetWindowAttribute,
        Gdi::*,
        GdiPlus::{GdiplusStartup, GdiplusStartupInput},
    },
    Storage::FileSystem::WriteFile,
    System::{
        Console::{ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_OUTPUT_HANDLE},
        DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, SetClipboardData},
        LibraryLoader::GetModuleHandleW,
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::CF_UNICODETEXT,
    },
    UI::{
        HiDpi::{GetDpiForSystem, GetDpiForWindow},
        Input::KeyboardAndMouse::*,
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "WinRustCalc";
const W: i32 = 360;
const H: i32 = 560;
const MIN_W: i32 = 320;
const MIN_H: i32 = 480;
const HEADER: i32 = 44;
const DISPLAY_H: i32 = 92;
const WM_MOUSELEAVE: u32 = 0x02A3; // not in windows-sys' enabled modules, as in rust-settings

fn title() -> &'static str {
    t!("Калькулятор", "Calculator", ja: "電卓", pt: "Calculadora", zh: "计算器")
}

fn history_title() -> &'static str {
    t!("История", "History", ja: "履歴", pt: "Histórico", zh: "历史记录")
}

fn clear_title() -> &'static str {
    t!("Очистить", "Clear", ja: "消去", pt: "Limpar", zh: "清除")
}

fn empty_title() -> &'static str {
    t!("Пока пусто", "Nothing yet", ja: "まだありません", pt: "Ainda vazio", zh: "暂无记录")
}

fn error_title() -> &'static str {
    t!("Ошибка", "Error", ja: "エラー", pt: "Erro", zh: "错误")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Digit,
    Op,
    Fn,
    Eq,
    Danger,
    Mem,
    Hist,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Act {
    Digit(u8),
    Dot,
    Op(char),
    Paren(char),
    Eq,
    Clear,
    ClearEntry,
    Back,
    Sign,
    Percent,
    Inv,
    Sqr,
    Sqrt,
    Fact,
    Exp,
    Const(&'static str),
    Func(&'static str),
    MemClear,
    MemRecall,
    MemAdd,
    MemSub,
    MemStore,
    ToggleSci,
    ToggleAngle,
    ToggleHistory,
    ClearHistory,
    History(usize),
    Close,
}

struct Key {
    rect: RECT,
    label: String,
    sub: String, // the expression of a history row
    act: Act,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Hot {
    #[default]
    None,
    Close,
    History,
    Sci,
    Angle,
    ClearHist,
    Mem(usize),
    Key(usize),
}

struct App {
    dpi: i32,
    input: String,       // the expression as typed, always with '.' as the decimal separator
    fresh: bool,         // the next digit starts a new value (the last '=' or function set this)
    last: Option<(String, f64)>, // the expression a '=' committed: shown dim above the result
    err: bool,
    memory: f64,
    angle: Angle,
    sci: bool,
    hist_open: bool,
    hist_scroll: i32,
    history: Vec<(String, f64)>, // newest first, at most 20
    hover: Hot,
    pressed: Hot,
    focused: bool,
    fonts: [isize; 7],
    font_dpi: i32,
    mono: bool,
}

impl Default for App {
    fn default() -> Self {
        App {
            dpi: 96,
            input: String::new(),
            fresh: false,
            last: None,
            err: false,
            memory: 0.0,
            angle: Angle::Deg,
            sci: false,
            hist_open: false,
            hist_scroll: 0,
            history: Vec::new(),
            hover: Hot::None,
            pressed: Hot::None,
            focused: false,
            fonts: [0; 7],
            font_dpi: 0,
            mono: false,
        }
    }
}

thread_local! { static APP: RefCell<App> = RefCell::new(App::default()) }

/// The wallpaper behind the window, blurred and cached between paints (optional: look.txt `win-blur=1`).
struct Glass {
    rect: RECT,
    dpi: i32,
    on: bool,
    px: Vec<u32>,
}
thread_local! { static GLASS: RefCell<Option<Glass>> = const { RefCell::new(None) }; }

fn glass_drop() {
    GLASS.with_borrow_mut(|g| *g = None);
}

fn px(v: i32, dpi: i32) -> i32 {
    (v as f64 * dpi as f64 / 96.0).round() as i32
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn sep_label() -> &'static str {
    if dot!() { "." } else { "," }
}

/// The internal '.' separator as the language writes it.
fn show(s: &str) -> String {
    if dot!() { s.to_string() } else { s.replace('.', ",") }
}

/// A number as it goes into `input` (always '.'): the command line and the persisted history stay machine-readable.
fn value_text(v: f64) -> String {
    calc::format(v, '.').unwrap_or_else(|| "0".into())
}

fn tail_op(s: &str) -> bool {
    s.ends_with('+') || s.ends_with('-') || s.ends_with('*') || s.ends_with('/') || s.ends_with('^')
}

fn ends_value(s: &str) -> bool {
    s.ends_with(')') || s.ends_with('!')
}

/// True when the text ends with a constant (`pi`, `e`) rather than the `e` of an exponent.
fn ends_constant(s: &str) -> bool {
    let b = s.as_bytes();
    match b.last() {
        Some(c) if c.is_ascii_alphabetic() => !(*c == b'e' && b.len() >= 2 && b[b.len() - 2].is_ascii_digit()),
        _ => false,
    }
}

/// Where the last numeric literal of `s` starts (including an exponent): `2+3.5` -> 2, `sin(30)` -> 4.
fn trailing_number_start(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut found: Option<(usize, usize)> = None; // (start, end of the literal)
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() || (b[i] == b'.' && i + 1 < b.len() && b[i + 1].is_ascii_digit()) {
            let st = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                i += 1;
            }
            if i < b.len() && b[i] == b'e' {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            found = Some((st, i));
        } else {
            i += 1;
        }
    }
    // Only a literal at the end counts (closing parentheses may follow): `sin(30)` has one, `2+pi` has none.
    found.and_then(|(st, end)| b[end..].iter().all(|&c| c == b')').then_some(st))
}

/// The last top-level binary operator: `2*3+4` -> the '+', `-5` and `2*-3` -> none for the first sign.
fn last_top_op(s: &str) -> Option<(usize, char)> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut found = None;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'+' | b'-' | b'*' | b'/' if depth == 0 => {
                let unary = i == 0 || matches!(b[i - 1], b'+' | b'-' | b'*' | b'/' | b'^' | b'(' | b'e');
                if !unary {
                    found = Some((i, c as char));
                }
            }
            _ => {}
        }
    }
    found
}

/// Flips the sign of the value being typed without evaluating the whole expression: `5-3` -> `5--3`, `-5` -> `5`.
fn toggle_sign_text(input: &str) -> String {
    if input.is_empty() {
        return "-0".into();
    }
    let Some(st) = trailing_number_start(input) else {
        return format!("-({input})");
    };
    let b = input.as_bytes();
    if st > 0 && b[st - 1] == b'-' && (st == 1 || matches!(b[st - 2], b'+' | b'-' | b'*' | b'/' | b'^' | b'(')) {
        let mut out = String::with_capacity(input.len() - 1);
        out.push_str(&input[..st - 1]);
        out.push_str(&input[st..]);
        return out;
    }
    let mut out = String::with_capacity(input.len() + 1);
    out.push_str(&input[..st]);
    out.push('-');
    out.push_str(&input[st..]);
    out
}

fn current(a: &App) -> Result<f64, calc::Error> {
    if a.input.is_empty() {
        return Ok(0.0);
    }
    calc::eval(&a.input, a.angle)
}

fn reset_edit(a: &mut App) {
    a.err = false;
    a.fresh = false;
    a.last = None;
}

fn push_digit(a: &mut App, d: u8) {
    if a.err || a.fresh {
        a.input.clear();
        reset_edit(a);
    }
    if a.input.len() > 160 {
        return;
    }
    if a.input.is_empty() {
        a.input.push((b'0' + d) as char);
        return;
    }
    if a.input == "-0" {
        a.input.pop();
        a.input.push((b'0' + d) as char);
        return;
    }
    if ends_value(&a.input) || ends_constant(&a.input) {
        a.input.push('*');
    }
    if let Some(st) = trailing_number_start(&a.input) {
        if &a.input[st..] == "0" {
            a.input.truncate(st);
        }
    }
    a.input.push((b'0' + d) as char);
}

fn push_dot(a: &mut App) {
    if a.err || a.fresh {
        a.input.clear();
        reset_edit(a);
    }
    if a.input.len() > 160 {
        return;
    }
    if let Some(st) = trailing_number_start(&a.input) {
        let lit = &a.input[st..];
        if lit.contains('.') || lit.contains('e') {
            return;
        }
        if lit.is_empty() {
            a.input.push_str("0.");
        } else {
            a.input.push('.');
        }
        return;
    }
    if a.input.is_empty() || tail_op(&a.input) || a.input.ends_with('(') {
        a.input.push_str("0.");
    } else if ends_value(&a.input) {
        a.input.push_str("*0.");
    } else if ends_constant(&a.input) {
        a.input.push_str("*0.");
    }
}

fn push_op(a: &mut App, c: char) {
    if a.err {
        return;
    }
    if a.fresh {
        a.fresh = false;
        a.last = None;
    }
    if a.input.is_empty() {
        a.input.push('0');
    }
    // after 'e', '^' or '(' a minus is a sign, not a binary operator
    if c == '-' && (a.input.ends_with('e') || a.input.ends_with('^') || a.input.ends_with('(')) {
        a.input.push('-');
        return;
    }
    while tail_op(&a.input) {
        let b = a.input.as_bytes();
        let n = b.len();
        let last = b[n - 1];
        if n == 1 || (last == b'-' && (b[n - 2] == b'e' || b[n - 2] == b'^' || b[n - 2] == b'(')) {
            break;
        }
        if b[n - 2] == b'e' {
            break; // the exponent's own sign
        }
        a.input.pop();
    }
    if a.input.is_empty() {
        a.input.push('0');
    }
    a.input.push(c);
}

fn push_paren(a: &mut App, c: char) {
    if a.err || a.fresh {
        a.input.clear();
        reset_edit(a);
    }
    if c == '(' {
        if a.input.ends_with(')') || a.input.ends_with('!') || a.input.chars().last().is_some_and(|x| x.is_ascii_digit()) || ends_constant(&a.input) {
            a.input.push('*');
        }
        a.input.push('(');
        return;
    }
    let open = a.input.matches('(').count() as i32 - a.input.matches(')').count() as i32;
    if open > 0 && !tail_op(&a.input) && !a.input.ends_with('(') {
        a.input.push(')');
    }
}

fn push_const(a: &mut App, name: &'static str) {
    if a.err || a.fresh {
        a.input.clear();
        reset_edit(a);
    }
    if a.input.is_empty() || tail_op(&a.input) || a.input.ends_with('(') {
        a.input.push_str(name);
    } else if a.input.ends_with(')') || a.input.ends_with('!') || a.input.chars().last().is_some_and(|c| c.is_ascii_digit() || c == '.') || ends_constant(&a.input) {
        a.input.push('*');
        a.input.push_str(name);
    } else {
        a.input.push_str(name);
    }
}

fn push_exp(a: &mut App) {
    if a.err || a.fresh {
        a.input.clear();
        reset_edit(a);
    }
    if a.input.len() > 160 {
        return;
    }
    if a.input.is_empty() {
        a.input.push_str("1e");
    } else if a.input.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        if trailing_number_start(&a.input).is_none_or(|st| !a.input[st..].contains('e')) {
            a.input.push('e');
        }
    }
}

fn clear_entry(a: &mut App) {
    reset_edit(a);
    if let Some(st) = trailing_number_start(&a.input) {
        a.input.truncate(st);
        // a minus we added as the sign of the entry goes with it
        while a.input.ends_with('-') {
            let b = a.input.as_bytes();
            let n = b.len();
            if n == 1 || matches!(b[n - 2], b'+' | b'-' | b'*' | b'/' | b'^' | b'(') {
                a.input.pop();
            } else {
                break;
            }
        }
    }
}

fn backspace(a: &mut App) {
    a.err = false;
    if a.fresh {
        a.input.clear();
        a.fresh = false;
        a.last = None;
        return;
    }
    a.input.pop();
    if a.input.is_empty() {
        a.last = None;
    }
}

fn apply_value(a: &mut App, v: f64) {
    if let Some(s) = calc::format(v, '.') {
        a.input = s;
        a.fresh = true;
        a.last = None;
        a.err = false;
    } else {
        a.err = true;
    }
}

fn apply_unary(a: &mut App, f: impl FnOnce(f64) -> Result<f64, calc::Error>) {
    match current(a).and_then(f) {
        Ok(v) => apply_value(a, v),
        Err(_) => a.err = true,
    }
}

fn apply_func(a: &mut App, name: &str) {
    match current(a).and_then(|v| calc::apply(name, v, a.angle)) {
        Ok(v) => apply_value(a, v),
        Err(_) => a.err = true,
    }
}

fn toggle_sign(a: &mut App) {
    if a.err {
        return;
    }
    if a.input.ends_with(')') || a.input.ends_with('!') || a.fresh {
        if let Ok(v) = current(a) {
            apply_value(a, -v);
        }
        return;
    }
    a.input = toggle_sign_text(&a.input);
    a.last = None;
}

fn percent(a: &mut App) {
    if a.err {
        return;
    }
    let Some((i, op)) = last_top_op(&a.input) else {
        if let Ok(v) = current(a) {
            apply_value(a, v / 100.0);
        }
        return;
    };
    let (lhs, rhs) = (&a.input[..i], &a.input[i + 1..]);
    if lhs.is_empty() || rhs.is_empty() {
        return;
    }
    let Ok(l) = calc::eval(lhs, a.angle) else { return };
    let Ok(r) = calc::eval(rhs, a.angle) else { return };
    let r = if op == '+' || op == '-' { l * r / 100.0 } else { r / 100.0 };
    if !r.is_finite() {
        a.err = true;
        return;
    }
    a.input = format!("{lhs}{op}{}", value_text(r));
    a.fresh = false;
    a.last = None;
}

fn equals(a: &mut App) {
    if a.fresh && a.last.is_some() {
        return; // '=' twice does not push the same operation again
    }
    let expr = if a.input.is_empty() { "0".to_string() } else { a.input.clone() };
    match calc::eval(&expr, a.angle) {
        Ok(v) => {
            if let Some(s) = calc::format(v, '.') {
                a.history.insert(0, (expr.clone(), v));
                a.history.truncate(20);
                a.input = s;
                a.fresh = true;
                a.last = Some((expr, v));
                a.err = false;
                a.hist_scroll = 0;
                save(a);
            } else {
                a.err = true;
            }
        }
        Err(_) => {
            a.err = true;
            a.fresh = false;
        }
    }
}

fn memory(a: &mut App, act: Act) {
    if act == Act::MemClear {
        a.memory = 0.0;
        save(a);
        return;
    }
    if act == Act::MemRecall {
        a.input = value_text(a.memory);
        a.fresh = true;
        a.last = None;
        a.err = false;
        return;
    }
    match current(a) {
        Ok(v) if v.is_finite() => {
            match act {
                Act::MemAdd => a.memory += v,
                Act::MemSub => a.memory -= v,
                _ => a.memory = v,
            }
            save(a);
        }
        _ => a.err = true,
    }
}

/// Runs one key. False when the window has to close.
fn do_act(a: &mut App, act: Act) -> bool {
    match act {
        Act::Digit(d) => push_digit(a, d),
        Act::Dot => push_dot(a),
        Act::Op(c) => push_op(a, c),
        Act::Paren(c) => push_paren(a, c),
        Act::Eq => equals(a),
        Act::Clear => {
            a.input.clear();
            reset_edit(a);
        }
        Act::ClearEntry => clear_entry(a),
        Act::Back => backspace(a),
        Act::Sign => toggle_sign(a),
        Act::Percent => percent(a),
        Act::Inv => apply_unary(a, |v| if v == 0.0 { Err(calc::Error::Domain) } else { Ok(1.0 / v) }),
        Act::Sqr => apply_unary(a, |v| Ok(v * v)),
        Act::Sqrt => apply_unary(a, |v| calc::apply("sqrt", v, Angle::Rad)),
        Act::Fact => apply_unary(a, calc::factorial),
        Act::Exp => push_exp(a),
        Act::Const(n) => push_const(a, n),
        Act::Func(n) => apply_func(a, n),
        Act::MemClear | Act::MemRecall | Act::MemAdd | Act::MemSub | Act::MemStore => memory(a, act),
        Act::ToggleSci => {
            a.sci = !a.sci;
            save(a);
        }
        Act::ToggleAngle => {
            a.angle = if a.angle == Angle::Deg { Angle::Rad } else { Angle::Deg };
            save(a);
        }
        Act::ToggleHistory => {
            a.hist_open = !a.hist_open;
            a.hist_scroll = 0;
        }
        Act::ClearHistory => {
            a.history.clear();
            a.hist_scroll = 0;
            save(a);
        }
        Act::History(i) => {
            if let Some(&(_, v)) = a.history.get(i) {
                a.input = value_text(v);
                a.fresh = true;
                a.last = None;
                a.err = false;
                a.hist_open = false;
            }
        }
        Act::Close => return false,
    }
    true
}

// ---------- persistence ----------

fn store_file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("WinRust").join("rs-calc.txt"))
}

fn save(a: &App) {
    let Some(f) = store_file() else { return };
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let mut s = String::new();
    s.push_str(&format!("angle={}\n", if a.angle == Angle::Deg { "deg" } else { "rad" }));
    s.push_str(&format!("sci={}\n", i32::from(a.sci)));
    s.push_str(&format!("memory={}\n", a.memory));
    for (expr, v) in &a.history {
        s.push_str(&format!("h={}\t{}\n", expr.replace(['\t', '\r', '\n'], " "), v));
    }
    let _ = std::fs::write(f, s);
}

fn load(a: &mut App) {
    let Some(text) = store_file().and_then(|f| std::fs::read_to_string(f).ok()) else { return };
    for line in text.trim_start_matches('\u{feff}').lines() {
        if let Some(v) = line.strip_prefix("angle=") {
            a.angle = if v.trim() == "rad" { Angle::Rad } else { Angle::Deg };
        } else if let Some(v) = line.strip_prefix("sci=") {
            a.sci = v.trim() == "1";
        } else if let Some(v) = line.strip_prefix("memory=") {
            if let Ok(v) = v.trim().parse::<f64>() {
                if v.is_finite() {
                    a.memory = v;
                }
            }
        } else if let Some(v) = line.strip_prefix("h=") {
            if let Some((expr, n)) = v.rsplit_once('\t') {
                if let Ok(n) = n.trim().parse::<f64>() {
                    if !expr.is_empty() && n.is_finite() {
                        a.history.push((expr.to_string(), n));
                    }
                }
            }
        }
    }
    a.history.truncate(20);
}

// ---------- geometry ----------

/// Everything the painter and the hit test need: the same rectangles for both, so they cannot disagree.
struct Layout {
    close: RECT,
    hist: RECT,
    sci: RECT,
    display: RECT,
    angle: RECT,
    mem: Vec<Key>,
    keys: Vec<Key>,
    hist_title: RECT,
    hist_clear: RECT,
    list: RECT,
    rows: usize,
}

const STD: [[(&str, Act, Kind); 5]; 4] = [
    [
        ("C", Act::Clear, Kind::Danger),
        ("CE", Act::ClearEntry, Kind::Fn),
        ("⌫", Act::Back, Kind::Fn),
        ("%", Act::Percent, Kind::Fn),
        ("÷", Act::Op('/'), Kind::Op),
    ],
    [
        ("7", Act::Digit(7), Kind::Digit),
        ("8", Act::Digit(8), Kind::Digit),
        ("9", Act::Digit(9), Kind::Digit),
        ("±", Act::Sign, Kind::Fn),
        ("×", Act::Op('*'), Kind::Op),
    ],
    [
        ("4", Act::Digit(4), Kind::Digit),
        ("5", Act::Digit(5), Kind::Digit),
        ("6", Act::Digit(6), Kind::Digit),
        ("√", Act::Sqrt, Kind::Fn),
        ("−", Act::Op('-'), Kind::Op),
    ],
    [
        ("1", Act::Digit(1), Kind::Digit),
        ("2", Act::Digit(2), Kind::Digit),
        ("3", Act::Digit(3), Kind::Digit),
        ("x²", Act::Sqr, Kind::Fn),
        ("+", Act::Op('+'), Kind::Op),
    ],
];

const LAST_ROW: [(&str, Act, Kind, i32); 4] = [
    ("0", Act::Digit(0), Kind::Digit, 1),
    ("", Act::Dot, Kind::Digit, 1),
    ("1/x", Act::Inv, Kind::Fn, 1),
    ("=", Act::Eq, Kind::Eq, 2),
];

const SCI: [[(&str, Act, Kind); 5]; 3] = [
    [
        ("sin", Act::Func("sin"), Kind::Fn),
        ("cos", Act::Func("cos"), Kind::Fn),
        ("tan", Act::Func("tan"), Kind::Fn),
        ("ln", Act::Func("ln"), Kind::Fn),
        ("log", Act::Func("log"), Kind::Fn),
    ],
    [
        ("asin", Act::Func("asin"), Kind::Fn),
        ("acos", Act::Func("acos"), Kind::Fn),
        ("atan", Act::Func("atan"), Kind::Fn),
        ("xʸ", Act::Op('^'), Kind::Op),
        ("n!", Act::Fact, Kind::Fn),
    ],
    [
        ("π", Act::Const("pi"), Kind::Fn),
        ("e", Act::Const("e"), Kind::Fn),
        ("EXP", Act::Exp, Kind::Fn),
        ("(", Act::Paren('('), Kind::Fn),
        (")", Act::Paren(')'), Kind::Fn),
    ],
];

fn layout(a: &App, cw: i32, ch: i32) -> Layout {
    let s = |v: i32| px(v, a.dpi);
    let m = s(12);
    let display = RECT { left: m, top: s(48), right: cw - m, bottom: s(48) + s(DISPLAY_H) };
    let bh = s(30);
    let close = RECT { left: cw - m - bh, top: s(7), right: cw - m, bottom: s(7) + bh };
    let hist = RECT { left: close.left - s(4) - bh, top: close.top, right: close.left - s(4), bottom: close.bottom };
    let sci = RECT { left: hist.left - s(4) - s(40), top: close.top, right: hist.left - s(4), bottom: close.bottom };
    let angle = if a.sci {
        RECT { left: display.left + s(10), top: display.top + s(8), right: display.left + s(10) + s(48), bottom: display.top + s(8) + s(20) }
    } else {
        RECT { left: 0, top: 0, right: 0, bottom: 0 }
    };
    let mt = display.bottom + s(8);
    let mh = s(24);
    let mem_gap = s(6);
    let mut mem = Vec::with_capacity(5);
    for (i, (label, act)) in [
        ("MC", Act::MemClear),
        ("MR", Act::MemRecall),
        ("M+", Act::MemAdd),
        ("M−", Act::MemSub),
        ("MS", Act::MemStore),
    ]
    .into_iter()
    .enumerate()
    {
        let i = i as i32;
        let l = m + (i * (cw - 2 * m + mem_gap)) / 5;
        let r = m + ((i + 1) * (cw - 2 * m + mem_gap)) / 5 - mem_gap;
        mem.push(Key { rect: RECT { left: l, top: mt, right: r, bottom: mt + mh }, label: label.into(), sub: String::new(), act, kind: Kind::Mem });
    }
    let top = mt + mh + s(8);
    let bottom = ch - s(8);
    let mut keys = Vec::new();
    let mut hist_title = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut hist_clear = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut list = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut rows = 0;
    if a.hist_open {
        hist_title = RECT { left: m, top, right: cw - m - s(72), bottom: top + s(26) };
        hist_clear = RECT { left: cw - m - s(64), top, right: cw - m, bottom: top + s(26) };
        list = RECT { left: m, top: top + s(32), right: cw - m, bottom };
        let rh = s(32);
        rows = (((list.bottom - list.top + s(2)) / (rh + s(2))).max(0)) as usize;
        let start = (a.hist_scroll.max(0) as usize).min(a.history.len().saturating_sub(rows));
        for (k, idx) in (start..a.history.len()).take(rows).enumerate() {
            let y = list.top + k as i32 * (rh + s(2));
            let (expr, v) = &a.history[idx];
            keys.push(Key {
                rect: RECT { left: list.left, top: y, right: list.right, bottom: y + rh },
                label: value_text(*v),
                sub: show(expr),
                act: Act::History(idx),
                kind: Kind::Hist,
            });
        }
    } else {
        let gap = s(6);
        let n = if a.sci { 8 } else { 5 };
        let avail = (bottom - top - gap * (n - 1)).max(1);
        let unit = avail as f64 / if a.sci { 7.4 } else { 5.0 };
        let std_h = (unit as i32).max(1);
        let sci_h = ((unit * 0.8) as i32).max(1);
        let span = cw - 2 * m + gap;
        let col = |i: i32| (m + (i * span) / 5, m + ((i + 1) * span) / 5 - gap);
        let mut y = top;
        if a.sci {
            for row in SCI.iter() {
                for (c, &(label, act, kind)) in row.iter().enumerate() {
                    let (l, r) = col(c as i32);
                    keys.push(Key { rect: RECT { left: l, top: y, right: r, bottom: y + sci_h }, label: label.into(), sub: String::new(), act, kind });
                }
                y += sci_h + gap;
            }
        }
        for row in STD.iter() {
            for (c, &(label, act, kind)) in row.iter().enumerate() {
                let (l, r) = col(c as i32);
                let label = if label.is_empty() { "" } else { label }; // the π key of the last standard row
                keys.push(Key { rect: RECT { left: l, top: y, right: r, bottom: y + std_h }, label: label.into(), sub: String::new(), act, kind });
            }
            y += std_h + gap;
        }
        let mut cx = 0i32;
        for &(label, act, kind, width) in LAST_ROW.iter() {
            let (l, _) = col(cx);
            let (_, r) = col(cx + width - 1);
            let label = if label.is_empty() { sep_label() } else { label };
            keys.push(Key { rect: RECT { left: l, top: y, right: r, bottom: y + std_h }, label: label.into(), sub: String::new(), act, kind });
            cx += width;
        }
    }
    Layout { close, hist, sci, display, angle, mem, keys, hist_title, hist_clear, list, rows }
}

fn hot_at(a: &App, cw: i32, ch: i32, x: i32, y: i32) -> Hot {
    let l = layout(a, cw, ch);
    let inside = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
    if inside(&l.close) {
        return Hot::Close;
    }
    if inside(&l.hist) {
        return Hot::History;
    }
    if inside(&l.sci) {
        return Hot::Sci;
    }
    if a.sci && inside(&l.angle) {
        return Hot::Angle;
    }
    if a.hist_open && inside(&l.hist_clear) {
        return Hot::ClearHist;
    }
    for (i, k) in l.mem.iter().enumerate() {
        if inside(&k.rect) {
            return Hot::Mem(i);
        }
    }
    for (i, k) in l.keys.iter().enumerate() {
        if inside(&k.rect) {
            return Hot::Key(i);
        }
    }
    Hot::None
}

fn act_at(a: &App, cw: i32, ch: i32, hot: Hot) -> Option<Act> {
    let l = layout(a, cw, ch);
    match hot {
        Hot::Close => Some(Act::Close),
        Hot::History => Some(Act::ToggleHistory),
        Hot::Sci => Some(Act::ToggleSci),
        Hot::Angle => Some(Act::ToggleAngle),
        Hot::ClearHist => Some(Act::ClearHistory),
        Hot::Mem(i) => l.mem.get(i).map(|k| k.act),
        Hot::Key(i) => l.keys.get(i).map(|k| k.act),
        Hot::None => None,
    }
}

// ---------- painting ----------

/// 0 body, 1 display (large), 2 display (medium), 3 title, 4 key, 5 small, 6 icons.
fn refresh_fonts(a: &mut App) {
    let mono = st::style().mono();
    if a.fonts[0] != 0 && a.mono == mono && a.font_dpi == a.dpi {
        return;
    }
    let face = st::ui_face();
    let icons = st::icon_face();
    let dpi = a.dpi;
    let f = |size: f32, weight: i32, face: &str| unsafe {
        CreateFontW(-((size * dpi as f32 / 96.0).round() as i32), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, wide(face).as_ptr()) as isize
    };
    let new = [f(31.0, 600, face), f(22.0, 600, face), f(14.5, 600, face), f(15.0, 400, face), f(11.0, 400, face), f(15.0, 400, icons), f(12.0, 400, face)];
    for h in a.fonts.into_iter().filter(|&h| h != 0) {
        unsafe { DeleteObject(h as _) };
    }
    a.fonts = new;
    a.font_dpi = dpi;
    a.mono = mono;
}

fn client_on_screen(hwnd: HWND) -> RECT {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let mut pt = POINT { x: 0, y: 0 };
        ClientToScreen(hwnd, &mut pt);
        RECT { left: pt.x, top: pt.y, right: pt.x + rc.right, bottom: pt.y + rc.bottom }
    }
}

fn glass_capture(hwnd: HWND) {
    let r = client_on_screen(hwnd);
    let on = winrust_common::look::win_blur();
    let dpi = APP.with_borrow(|a| a.dpi);
    let (w, h) = ((r.right - r.left).max(0), (r.bottom - r.top).max(0));
    let stale = GLASS.with_borrow(|g| match g {
        None => true,
        Some(g) => {
            g.on != on
                || g.dpi != dpi
                || g.rect.left != r.left
                || g.rect.top != r.top
                || g.rect.right != r.right
                || g.rect.bottom != r.bottom
                || (on && g.px.len() != (w * h) as usize)
        }
    });
    if !stale {
        return;
    }
    let px = if on && w > 0 && h > 0 { unsafe { frost::capture_desktop(r, frost::radius_for(w, h)) } } else { Vec::new() };
    GLASS.with_borrow_mut(|g| *g = Some(Glass { rect: r, dpi, on, px }));
}

unsafe fn round(dc: HDC, r: RECT, color: u32, rad: i32) {
    let br = CreateSolidBrush(color);
    if rad > 0 {
        let ob = SelectObject(dc, br);
        let op = SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, rad * 2, rad * 2);
        SelectObject(dc, ob);
        SelectObject(dc, op);
    } else {
        FillRect(dc, &r, br);
    }
    DeleteObject(br);
}

unsafe fn outline(dc: HDC, r: RECT, color: u32, rad: i32) {
    let pen = CreatePen(PS_SOLID, 1, color);
    let op = SelectObject(dc, pen);
    let ob = SelectObject(dc, GetStockObject(NULL_BRUSH));
    RoundRect(dc, r.left, r.top, r.right, r.bottom, rad * 2, rad * 2);
    SelectObject(dc, ob);
    SelectObject(dc, op);
    DeleteObject(pen);
}

unsafe fn text(dc: HDC, font: HFONT, s: &str, mut r: RECT, color: u32, flags: u32) {
    if s.is_empty() {
        return;
    }
    SelectObject(dc, font);
    SetTextColor(dc, color);
    DrawTextW(dc, wide(s).as_ptr(), -1, &mut r, flags);
}

fn inset(r: RECT, n: i32) -> RECT {
    RECT { left: r.left + n, top: r.top + n, right: r.right - n, bottom: r.bottom - n }
}

/// The live preview: only worth a line when the input is more than a plain number.
fn preview(a: &App) -> Option<String> {
    if a.input.is_empty() || a.fresh || a.err {
        return None;
    }
    let mut chars = a.input.char_indices();
    let _ = chars.next();
    let complex = a.input.chars().any(|c| c.is_ascii_alphabetic())
        || chars.any(|(_, c)| matches!(c, '+' | '-' | '*' | '/' | '%' | '^' | '(' | ')' | '!'));
    if !complex {
        return None;
    }
    calc::eval(&a.input, a.angle).ok().and_then(|v| calc::format(v, '.'))
}

unsafe fn draw_all(dc: HDC, a: &App, cw: i32, ch: i32) {
    let p = st::pal();
    let s = |v: i32| px(v, a.dpi);
    let l = layout(a, cw, ch);
    let rc = RECT { left: 0, top: 0, right: cw, bottom: ch };
    let rad = if st::style().square() { 0 } else { s(10) };
    let rad12 = if st::style().square() { 0 } else { s(12) };
    let rad6 = if st::style().square() { 0 } else { s(6) };
    let [big, mid, title_f, key_f, small, icon, hist_f] = [a.fonts[0] as HFONT, a.fonts[1] as HFONT, a.fonts[2] as HFONT, a.fonts[3] as HFONT, a.fonts[4] as HFONT, a.fonts[5] as HFONT, a.fonts[6] as HFONT];
    SetBkMode(dc, TRANSPARENT as i32);

    // header: the title, the history and scientific chips, the close button
    text(dc, title_f, title(), RECT { left: s(16), top: 0, right: l.sci.left - s(6), bottom: s(HEADER) }, p.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    if a.hover == Hot::History {
        round(dc, inset(l.hist, s(2)), p.hover, rad6);
    }
    text(dc, icon, "\u{E81C}", l.hist, if a.hist_open { p.accent } else { p.text }, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    let on_sci = a.sci;
    round(dc, l.sci, if on_sci { p.accent } else if a.hover == Hot::Sci { p.hover } else { st::mix(p.bg, p.text, 0.06) }, rad6);
    text(dc, small, "fx", l.sci, if on_sci { st::on_accent() } else { p.dim }, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    if a.hover == Hot::Close {
        round(dc, inset(l.close, s(2)), p.hover, rad6);
    }
    text(dc, key_f, "\u{2715}", l.close, p.text, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

    // the display card
    round(dc, l.display, st::card(), rad12);
    if a.focused {
        outline(dc, inset(l.display, s(1)), st::mix(st::card(), p.accent, 0.55), rad12);
    }
    if a.sci {
        if a.hover == Hot::Angle {
            round(dc, l.angle, p.hover, rad6);
        }
        text(dc, small, if a.angle == Angle::Deg { "DEG" } else { "RAD" }, l.angle, p.accent, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
    if a.memory != 0.0 {
        let mx = if a.sci { l.angle.right + s(8) } else { l.display.left + s(12) };
        text(dc, small, "M", RECT { left: mx, top: l.display.top + s(8), right: mx + s(20), bottom: l.display.top + s(28) }, p.dim, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
    let (line1, line1_color, line2) = if a.err {
        (error_title().to_string(), st::BAD, show(&a.input))
    } else if a.fresh {
        let l1 = a.last.as_ref().map(|(e, _)| show(e)).unwrap_or_default();
        (l1, p.dim, show(&a.input))
    } else if let Some(v) = preview(a) {
        (format!("= {}", show(&v)), p.dim, show(&a.input))
    } else {
        (String::new(), p.dim, show(&a.input))
    };
    text(dc, small, &line1, RECT { left: l.display.left + s(64), top: l.display.top + s(7), right: l.display.right - s(12), bottom: l.display.top + s(27) }, line1_color, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
    let line2 = if line2.is_empty() { "0".to_string() } else { line2 };
    let n = line2.chars().count();
    let f = if n <= 14 { big } else if n <= 20 { mid } else { key_f };
    text(dc, f, &line2, RECT { left: l.display.left + s(14), top: l.display.top + s(28), right: l.display.right - s(14), bottom: l.display.bottom - s(8) }, if a.err { st::BAD } else { p.text }, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);

    // memory row: flat text buttons
    for (i, k) in l.mem.iter().enumerate() {
        if a.hover == Hot::Mem(i) {
            round(dc, inset(k.rect, s(2)), p.hover, rad6);
        }
        let on = a.memory != 0.0 && matches!(k.act, Act::MemClear | Act::MemRecall);
        text(dc, small, &k.label, k.rect, if a.hover == Hot::Mem(i) || on { p.text } else { p.dim }, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }

    if a.hist_open {
        text(dc, title_f, history_title(), l.hist_title, p.text, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        if a.hover == Hot::ClearHist {
            round(dc, l.hist_clear, p.hover, rad6);
        }
        text(dc, small, clear_title(), l.hist_clear, p.text, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        if a.history.is_empty() {
            text(dc, small, empty_title(), l.list, p.dim, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        }
        for (i, k) in l.keys.iter().enumerate() {
            if a.hover == Hot::Key(i) {
                round(dc, inset(k.rect, s(1)), p.hover, rad6);
            }
            text(dc, hist_f, &k.sub, RECT { left: k.rect.left + s(8), right: k.rect.right - s(96), ..k.rect }, p.dim, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX | DT_END_ELLIPSIS);
            text(dc, key_f, &k.label, RECT { left: k.rect.right - s(94), right: k.rect.right - s(8), ..k.rect }, p.text, DT_RIGHT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        }
    } else {
        for (i, k) in l.keys.iter().enumerate() {
            let hot = a.hover == Hot::Key(i);
            let pressed = a.pressed == Hot::Key(i);
            let base = match k.kind {
                Kind::Digit => st::card(),
                Kind::Op => st::mix(st::card(), p.accent, 0.18),
                Kind::Fn => st::mix(p.bg, p.text, 0.07),
                Kind::Eq => p.accent,
                Kind::Danger => st::mix(st::card(), st::BAD, 0.18),
                _ => p.bg,
            };
            let bg = if pressed {
                st::mix(base, p.text, 0.18)
            } else if hot {
                st::mix(base, p.text, 0.08)
            } else {
                base
            };
            round(dc, k.rect, bg, rad);
            let fg = match k.kind {
                Kind::Digit => p.text,
                Kind::Op => p.accent,
                Kind::Fn => st::mix(p.text, p.dim, 0.25),
                Kind::Eq => st::on_accent(),
                Kind::Danger => st::BAD,
                _ => p.text,
            };
            text(dc, key_f, &k.label, k.rect, fg, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
        }
    }

    // a hairline around the whole window, clipped to the rounded region
    let edge = CreateSolidBrush(st::mix(p.bg, p.text, 0.14));
    FrameRect(dc, &rc, edge);
    DeleteObject(edge);
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let wdc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (cw, ch) = (rc.right, rc.bottom);
        if cw <= 0 || ch <= 0 {
            EndPaint(hwnd, &ps);
            return;
        }
        let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
        APP.with_borrow_mut(|a| {
            if a.dpi != dpi {
                a.dpi = dpi;
                refresh_fonts(a);
            }
        });
        glass_capture(hwnd);
        let dc = CreateCompatibleDC(wdc);
        let bmp = CreateCompatibleBitmap(wdc, cw, ch);
        let old = SelectObject(dc, bmp);
        let p = st::pal();
        let frosted = GLASS.with_borrow(|g| match g.as_ref() {
            Some(g) if g.on && g.px.len() == (cw * ch) as usize => frost::paint_capture(dc, &g.px, rc, p.bg, frost::tint_alpha()),
            _ => false,
        });
        if !frosted {
            let br = CreateSolidBrush(p.bg);
            FillRect(dc, &rc, br);
            DeleteObject(br);
            st::backdrop(dc, rc, dpi as f32 / 96.0);
        }
        APP.with_borrow(|a| draw_all(dc, a, cw, ch));
        BitBlt(wdc, 0, 0, cw, ch, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
    }
}

// ---------- the clipboard ----------

fn copy_display(hwnd: HWND) {
    // what the big display line shows: the expression being typed, or the result after '='
    let s = APP.with_borrow(|a| if a.input.is_empty() { "0".to_string() } else { a.input.clone() });
    let mut utf16: Vec<u16> = s.encode_utf16().collect();
    utf16.push(0);
    unsafe {
        if OpenClipboard(hwnd) == 0 {
            return;
        }
        EmptyClipboard();
        let bytes = utf16.len() * 2;
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !h.is_null() {
            let p = GlobalLock(h);
            if !p.is_null() {
                std::ptr::copy_nonoverlapping(utf16.as_ptr() as *const u8, p as *mut u8, bytes);
                GlobalUnlock(h);
            }
            SetClipboardData(CF_UNICODETEXT as u32, h);
        }
        CloseClipboard();
    }
}

fn paste_display(hwnd: HWND) {
    let s = unsafe {
        if OpenClipboard(hwnd) == 0 {
            return;
        }
        let mut out = String::new();
        if IsClipboardFormatAvailable(CF_UNICODETEXT as u32) != 0 {
            let h = GetClipboardData(CF_UNICODETEXT as u32);
            if !h.is_null() {
                let p = GlobalLock(h);
                if !p.is_null() {
                    let ptr = p as *const u16;
                    let mut n = 0usize;
                    while n < 4096 && *ptr.add(n) != 0 {
                        n += 1;
                    }
                    out = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, n));
                    GlobalUnlock(h);
                }
            }
        }
        CloseClipboard();
        out
    };
    // an expression copied as "2+2 = 4" keeps its right-hand side; commas from a localized display are decimals
    let s = s.rsplit('=').next().unwrap_or("").trim();
    if s.is_empty() {
        return;
    }
    let s = s.replace(',', ".").replace(['\r', '\n', '\t'], " ");
    let s: String = s.chars().take(200).collect();
    APP.with_borrow_mut(|a| {
        a.input = s;
        a.fresh = false;
        a.last = None;
        a.err = false;
    });
}

// ---------- the window ----------

unsafe fn hit_test(hwnd: HWND, lp: LPARAM) -> LRESULT {
    unsafe {
        let mut pt = POINT { x: (lp & 0xffff) as i16 as i32, y: ((lp >> 16) & 0xffff) as i16 as i32 };
        ScreenToClient(hwnd, &mut pt);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
        let b = px(6, dpi);
        let (left, top, right, bottom) = (pt.x < b, pt.y < b, pt.x >= rc.right - b, pt.y >= rc.bottom - b);
        if top && left {
            return HTTOPLEFT as LRESULT;
        }
        if top && right {
            return HTTOPRIGHT as LRESULT;
        }
        if bottom && left {
            return HTBOTTOMLEFT as LRESULT;
        }
        if bottom && right {
            return HTBOTTOMRIGHT as LRESULT;
        }
        if top {
            return HTTOP as LRESULT;
        }
        if bottom {
            return HTBOTTOM as LRESULT;
        }
        if left {
            return HTLEFT as LRESULT;
        }
        if right {
            return HTRIGHT as LRESULT;
        }
        if pt.y < px(HEADER, dpi) {
            let hot = APP.with_borrow(|a| hot_at(a, rc.right, rc.bottom, pt.x, pt.y));
            if hot == Hot::None {
                return HTCAPTION as LRESULT;
            }
        }
        HTCLIENT as LRESULT
    }
}

unsafe fn round_window(hwnd: HWND) {
    unsafe {
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
        let r = px(18, dpi);
        let hrgn = if st::style().square() { CreateRectRgn(0, 0, rc.right + 1, rc.bottom + 1) } else { CreateRoundRectRgn(0, 0, rc.right + 1, rc.bottom + 1, r * 2, r * 2) };
        SetWindowRgn(hwnd, hrgn, 1);
    }
}

/// The title bar and border colours for Windows 11; older Windows ignores these calls.
unsafe fn frame(hwnd: HWND) {
    let p = st::pal();
    let set = |attr: u32, v: u32| unsafe { DwmSetWindowAttribute(hwnd, attr, &v as *const u32 as _, 4) };
    set(20, !st::style().light() as u32);
    set(35, p.bg);
    set(36, p.text);
    set(34, p.line);
    set(33, if st::style().square() { 1 } else { 2 });
}

fn key_shortcut(hwnd: HWND, code: u16) -> bool {
    let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
    match code {
        VK_ESCAPE => {
            let close_panel = APP.with_borrow_mut(|a| {
                if a.hist_open {
                    a.hist_open = false;
                    true
                } else {
                    false
                }
            });
            if close_panel {
                unsafe { InvalidateRect(hwnd, null(), 0) };
            } else {
                unsafe { DestroyWindow(hwnd) };
            }
            true
        }
        VK_RETURN => {
            APP.with_borrow_mut(|a| {
                a.hist_open = false;
                equals(a);
            });
            unsafe { InvalidateRect(hwnd, null(), 0) };
            true
        }
        VK_BACK => {
            APP.with_borrow_mut(|a| {
                a.hist_open = false;
                backspace(a);
            });
            unsafe { InvalidateRect(hwnd, null(), 0) };
            true
        }
        VK_DELETE => {
            APP.with_borrow_mut(|a| {
                a.input.clear();
                reset_edit(a);
            });
            unsafe { InvalidateRect(hwnd, null(), 0) };
            true
        }
        VK_C if ctrl => {
            copy_display(hwnd);
            true
        }
        VK_V if ctrl => {
            paste_display(hwnd);
            unsafe { InvalidateRect(hwnd, null(), 0) };
            true
        }
        _ => false,
    }
}

fn char_input(hwnd: HWND, wp: u32) {
    let Some(ch) = char::from_u32(wp) else { return };
    let act = match ch {
        '0'..='9' => Act::Digit(ch as u8 - b'0'),
        '.' | ',' => Act::Dot,
        '+' => Act::Op('+'),
        '-' => Act::Op('-'),
        '*' => Act::Op('*'),
        '/' => Act::Op('/'),
        '^' => Act::Op('^'),
        '%' => Act::Percent,
        '=' => Act::Eq,
        '(' => Act::Paren('('),
        ')' => Act::Paren(')'),
        '!' => Act::Fact,
        'e' | 'E' => Act::Exp,
        _ => return,
    };
    APP.with_borrow_mut(|a| {
        a.hist_open = false;
        do_act(a, act);
    });
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        if msg == st::msg() {
            st::reload();
            glass_drop();
            let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
            APP.with_borrow_mut(|a| {
                a.dpi = dpi;
                a.mono = !st::style().mono(); // force the fonts to be rebuilt
                refresh_fonts(a);
            });
            InvalidateRect(hwnd, null(), 0);
            return 0;
        }
        match msg {
            // The popup answers these itself: leaving WM_NCCREATE to the default procedure made CreateWindowExW
            // fail with no error and no window (the same trap rust-settings hit).
            WM_NCCREATE => 1,
            WM_CREATE => 0,
            WM_NCCALCSIZE => 0,
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_GETMINMAXINFO => {
                let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
                let mmi = lp as *mut MINMAXINFO;
                if !mmi.is_null() {
                    (*mmi).ptMinTrackSize = POINT { x: px(MIN_W, dpi), y: px(MIN_H, dpi) };
                }
                0
            }
            WM_NCHITTEST => hit_test(hwnd, lp),
            WM_SIZE => {
                glass_drop();
                round_window(hwnd);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_MOVE => {
                glass_drop();
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DPICHANGED => {
                glass_drop();
                let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
                APP.with_borrow_mut(|a| {
                    a.dpi = dpi;
                    refresh_fonts(a);
                });
                round_window(hwnd);
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_SETFOCUS => {
                APP.with_borrow_mut(|a| a.focused = true);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_KILLFOCUS => {
                APP.with_borrow_mut(|a| a.focused = false);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_MOUSEMOVE => {
                let mut te = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                TrackMouseEvent(&mut te);
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
                let hot = APP.with_borrow(|a| hot_at(a, rc.right, rc.bottom, x, y));
                let changed = APP.with_borrow_mut(|a| std::mem::replace(&mut a.hover, hot) != hot);
                if changed {
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_MOUSELEAVE => {
                if APP.with_borrow_mut(|a| std::mem::replace(&mut a.hover, Hot::None)) != Hot::None {
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_LBUTTONDOWN => {
                SetFocus(hwnd);
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
                let hot = APP.with_borrow(|a| hot_at(a, rc.right, rc.bottom, x, y));
                APP.with_borrow_mut(|a| {
                    a.hover = hot;
                    a.pressed = hot;
                });
                SetCapture(hwnd);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                let (x, y) = ((lp & 0xffff) as i16 as i32, ((lp >> 16) & 0xffff) as i16 as i32);
                let (pressed, hot) = APP.with_borrow_mut(|a| {
                    let p = std::mem::replace(&mut a.pressed, Hot::None);
                    a.hover = hot_at(a, rc.right, rc.bottom, x, y);
                    (p, a.hover)
                });
                if pressed != Hot::None && pressed == hot {
                    let act = APP.with_borrow(|a| act_at(a, rc.right, rc.bottom, hot));
                    if let Some(act) = act {
                        let keep = APP.with_borrow_mut(|a| do_act(a, act));
                        if !keep {
                            DestroyWindow(hwnd);
                            return 0;
                        }
                        InvalidateRect(hwnd, null(), 0);
                    }
                } else {
                    InvalidateRect(hwnd, null(), 0);
                }
                0
            }
            WM_MOUSEWHEEL => {
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                let up = (wp >> 16) as i16 > 0;
                APP.with_borrow_mut(|a| {
                    if !a.hist_open {
                        return;
                    }
                    let rows = layout(a, rc.right, rc.bottom).rows.max(1);
                    let max = a.history.len().saturating_sub(rows) as i32;
                    a.hist_scroll = (a.hist_scroll + if up { -2 } else { 2 }).clamp(0, max);
                });
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_KEYDOWN => {
                if !key_shortcut(hwnd, wp as u16) {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                0
            }
            WM_CHAR => {
                char_input(hwnd, wp as u32);
                0
            }
            WM_CLOSE => {
                DestroyWindow(hwnd);
                0
            }
            WM_DESTROY => {
                for f in APP.with_borrow_mut(|a| std::mem::take(&mut a.fonts)) {
                    if f != 0 {
                        DeleteObject(f as _);
                    }
                }
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe fn run() {
    unsafe {
        // a second launch just shows the window that is already there
        let class = wide(CLASS);
        let found = FindWindowW(class.as_ptr(), null());
        if !found.is_null() {
            ShowWindow(found, SW_SHOW);
            winrust_common::fg::force(found);
            return;
        }
        let inst = GetModuleHandleW(null());
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc),
            hInstance: inst,
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let dpi = (GetDpiForSystem() as i32).max(96);
        let mut pt: POINT = std::mem::zeroed();
        GetCursorPos(&mut pt);
        let m = winrust_common::monitors::at(pt.x, pt.y).rc;
        let (mw, mh) = (m.right - m.left, m.bottom - m.top);
        let w = px(W, dpi).min(mw - px(24, dpi)).max(px(MIN_W, dpi));
        let h = px(H, dpi).min(mh - px(48, dpi)).max(px(MIN_H, dpi));
        let x = m.left + (mw - w) / 2;
        let y = m.top + ((mh - h) / 2).max(px(16, dpi));
        let hwnd = CreateWindowExW(WS_EX_APPWINDOW, class.as_ptr(), wide(title()).as_ptr(), WS_POPUP, x, y, w, h, null_mut(), null_mut(), inst, null());
        if hwnd.is_null() {
            return;
        }
        let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
        APP.with_borrow_mut(|a| {
            a.dpi = dpi;
            load(a);
            refresh_fonts(a);
        });
        frame(hwnd);
        round_window(hwnd);
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);
        winrust_common::fg::force(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ---------- the command line ----------

/// Writes UTF-8 to the console this process was started from. A GUI-subsystem exe has no console of its own:
/// attach to the parent's (or use the inherited pipe/file handle when output was redirected).
fn emit(s: &str) {
    unsafe {
        let mut h = GetStdHandle(STD_OUTPUT_HANDLE);
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            if AttachConsole(ATTACH_PARENT_PROCESS) != 0 {
                h = GetStdHandle(STD_OUTPUT_HANDLE);
            }
        }
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            return;
        }
        let bytes = s.as_bytes();
        let mut written = 0u32;
        WriteFile(h, bytes.as_ptr(), bytes.len() as u32, &mut written, null_mut());
    }
}

fn usage() -> String {    format!(
        "{}\n\n{}\n\n{}\n\n{}\n",
        t!(
            "rs-calc — калькулятор WinRust",
            "rs-calc — the WinRust calculator",
            ja: "rs-calc — WinRust の電卓",
            pt: "rs-calc — a calculadora do WinRust",
            zh: "rs-calc — WinRust 计算器"
        ),
        t!(
            "Использование:\n  rs-calc                  открыть окно\n  rs-calc \"2+2*3\"          вычислить и напечатать (код 0, при ошибке 1)\n  rs-calc --rad \"sin(pi/2)\" в радианах (по умолчанию градусы)",
            "Usage:\n  rs-calc                  open the window\n  rs-calc \"2+2*3\"          evaluate and print (exit 0, 1 on error)\n  rs-calc --rad \"sin(pi/2)\" radians (degrees by default)",
            ja: "使い方:\n  rs-calc                  ウィンドウを開く\n  rs-calc \"2+2*3\"          評価して出力 (終了コード 0、エラー時 1)\n  rs-calc --rad \"sin(pi/2)\" ラジアン (既定は度)",
            pt: "Uso:\n  rs-calc                  abrir a janela\n  rs-calc \"2+2*3\"          calcular e imprimir (código 0, 1 em caso de erro)\n  rs-calc --rad \"sin(pi/2)\" radianos (graus por padrão)",
            zh: "用法：\n  rs-calc                  打开窗口\n  rs-calc \"2+2*3\"          求值并输出（成功为 0，出错为 1）\n  rs-calc --rad \"sin(pi/2)\" 弧度（默认角度）"
        ),
        t!(
            "Операторы: + - * / % ^, скобки, !, унарный минус.\nФункции: sin cos tan asin acos atan ln log sqrt abs exp, pi, e.\n% после числа — процент (50% = 0.5), между числами — остаток (7%3 = 1).",
            "Operators: + - * / % ^, parentheses, !, unary minus.\nFunctions: sin cos tan asin acos atan ln log sqrt abs exp, pi, e.\n% after a value is a percent (50% = 0.5); between two values it is the remainder (7%3 = 1).",
            ja: "演算子: + - * / % ^、括弧、!、単項マイナス。\n関数: sin cos tan asin acos atan ln log sqrt abs exp、pi、e。\n値の後の % はパーセント (50% = 0.5)、値の間では剰余 (7%3 = 1)。",
            pt: "Operadores: + - * / % ^, parênteses, !, menos unário.\nFunções: sin cos tan asin acos atan ln log sqrt abs exp, pi, e.\n% após um valor é porcentagem (50% = 0.5); entre dois valores é o resto (7%3 = 1).",
            zh: "运算符：+ - * / % ^、括号、!、一元负号。\n函数：sin cos tan asin acos atan ln log sqrt abs exp、pi、e。\n数值后的 % 是百分比（50% = 0.5），两个数值之间是取余（7%3 = 1）。"
        ),
        t!(
            "Подсказка: cmd не ждёт GUI-программу. Для кода выхода: start /wait rs-calc \"2+2\" или PowerShell Start-Process -Wait.",
            "Hint: cmd does not wait for a GUI program. For the exit code use start /wait rs-calc \"2+2\" or PowerShell Start-Process -Wait.",
            ja: "ヒント: cmd は GUI プログラムを待ちません。終了コードは start /wait rs-calc \"2+2\" か PowerShell の Start-Process -Wait で。",
            pt: "Dica: o cmd não espera um programa GUI. Para o código de saída use start /wait rs-calc \"2+2\" ou Start-Process -Wait no PowerShell.",
            zh: "提示：cmd 不会等待 GUI 程序。要取得退出码请用 start /wait rs-calc \"2+2\" 或 PowerShell 的 Start-Process -Wait。"
        )
    )
}

fn print_strings() {
    let lines = [
        ("title", title().to_string()),
        ("history.title", history_title().to_string()),
        ("history.clear", clear_title().to_string()),
        ("history.empty", empty_title().to_string()),
        ("error", error_title().to_string()),
        ("key.decimal", sep_label().to_string()),
        ("usage", usage()),
    ];
    for (key, value) in lines {
        emit(&format!("{key}\t{}\n", value.replace('\n', " ")));
    }
}

/// A `calc.exe` path handed over by IFEO as the first argument.
fn is_victim(a: &str) -> bool {
    let a = a.trim().trim_matches('"').to_ascii_lowercase();
    a.ends_with("calc.exe") && (a.contains('\\') || a.contains('/'))
}

/// Some(code) for a command line, None when the window has to open instead.
fn cli(args: &[String]) -> Option<i32> {
    // IFEO starts the debugger as `<program> <victim path> [arguments]`: a calc.exe path is not an expression.
    let start = usize::from(args.first().is_some_and(|a| is_victim(a)));
    let mut angle = Angle::Deg;
    let mut expr: Vec<&str> = Vec::new();
    for a in &args[start..] {
        match a.as_str() {
            "-h" | "--help" | "/?" => {
                emit(&usage());
                return Some(0);
            }
            "--print-strings" => {
                print_strings();
                return Some(0);
            }
            "--rad" => angle = Angle::Rad,
            "--deg" => angle = Angle::Deg,
            _ => expr.push(a),
        }
    }
    let src = expr.join(" ");
    if src.trim().is_empty() {
        return None; // only flags (or nothing): open the window
    }
    match calc::eval(&src, angle) {
        Ok(v) => match calc::format(v, '.') {
            Some(s) => {
                emit(&format!("{s}\n"));
                Some(0)
            }
            None => {
                emit(&format!("{}\n", error_title()));
                Some(1)
            }
        },
        Err(_) => {
            emit(&format!("{}\n", error_title()));
            Some(1)
        }
    }
}

fn main() {
    winrust_common::appid::set("WinRust"); // group every WinRust process under one Task Manager entry
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        if let Some(code) = cli(&args) {
            std::process::exit(code);
        }
    }
    unsafe {
        let (mut token, mut input): (usize, GdiplusStartupInput) = (0, std::mem::zeroed());
        input.GdiplusVersion = 1;
        GdiplusStartup(&mut token, &input, null_mut()); // rounded shapes of the Road/Ruby styles
        SetProcessDPIAware();
        run();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_trailing_value() {
        assert_eq!(trailing_number_start("2+3.5"), Some(2));
        assert_eq!(trailing_number_start("sin(30)"), Some(4));
        assert_eq!(trailing_number_start("1e-5+2"), Some(5));
        assert_eq!(trailing_number_start("1e-5"), Some(0));
        assert_eq!(trailing_number_start("2+π"), None);
    }

    #[test]
    fn sign_toggles_the_entry() {
        assert_eq!(toggle_sign_text("-5"), "5");
        assert_eq!(toggle_sign_text("5"), "-5");
        assert_eq!(toggle_sign_text(""), "-0");
        assert_eq!(toggle_sign_text("5-3"), "5--3");
        assert_eq!(toggle_sign_text("5--3"), "5-3");
        assert_eq!(toggle_sign_text("2*3"), "2*-3");
        assert_eq!(toggle_sign_text("sin(30)"), "sin(-30)");
    }

    #[test]
    fn finds_the_last_binary_operator() {
        assert_eq!(last_top_op("-5"), None);
        assert_eq!(last_top_op("2*-3"), Some((1, '*')));
        assert_eq!(last_top_op("2*3+4"), Some((3, '+')));
        assert_eq!(last_top_op("(2+3)*4"), Some((5, '*')));
        assert_eq!(last_top_op("2^3"), None);
    }

    #[test]
    fn recognizes_the_ifeo_victim() {
        assert!(is_victim(r#"C:\Windows\System32\calc.exe"#));
        assert!(is_victim("\"C:/Windows/System32/calc.exe\""));
        assert!(!is_victim("calc.exe"));
        assert!(!is_victim("2+2*3"));
    }
}
