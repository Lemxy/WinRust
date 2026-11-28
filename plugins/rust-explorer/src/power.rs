//! The shut-down box behind Alt+F4 on the desktop (#189). Explorer shows one there, so our desktop shows its own:
//! Windows' task dialog (keyboard, screen reader, high contrast and DPI come with it) with one command link per action.
#![allow(unsafe_op_in_unsafe_fn)]
use crate::w;
use std::{ffi::c_void, ptr, sync::atomic::{AtomicBool, AtomicIsize, Ordering::SeqCst}};
use winrust_common::{power::Action, t};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::LibraryLoader::{GetProcAddress, LoadLibraryW},
    UI::WindowsAndMessaging::{HWND_TOPMOST, MB_ICONWARNING, MessageBoxW, SWP_NOMOVE, SWP_NOSIZE, SetForegroundWindow, SetWindowPos},
};

static OPEN: AtomicBool = AtomicBool::new(false); // one box at a time
static BOX: AtomicIsize = AtomicIsize::new(0); // its window while it is on screen

/// Top to bottom. The first one has the focus, like in Explorer's box: Alt+F4, Enter shuts down.
const ACTIONS: [Action; 4] = [Action::ShutDown, Action::Restart, Action::Sleep, Action::SignOut];
const FIRST_ID: i32 = 101; // clear of the dialog's own ids (IDOK, IDCANCEL...)

fn action_of(id: i32) -> Option<Action> {
    usize::try_from(id - FIRST_ID).ok().and_then(|i| ACTIONS.get(i)).copied()
}

/// `&` marks the key that picks the line.
fn label(a: Action) -> &'static str {
    match a {
        Action::ShutDown => t!("&Завершение работы", "&Shut down", ja: "シャットダウン(&U)", pt: "&Desligar", zh: "关机(&U)"),
        Action::Restart => t!("&Перезагрузка", "&Restart", ja: "再起動(&R)", pt: "&Reiniciar", zh: "重启(&R)"),
        Action::Sleep => t!("&Сон", "S&leep", ja: "スリープ(&S)", pt: "&Suspender", zh: "睡眠(&S)"),
        Action::SignOut => t!("&Выйти из системы", "Sign &out", ja: "サインアウト(&O)", pt: "S&air", zh: "注销(&O)"),
    }
}

// TASKDIALOG_BUTTON and TASKDIALOGCONFIG (commctrl.h, byte-packed).
#[repr(C, packed)]
struct Button {
    id: i32,
    text: *const u16,
}

type Callback = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM, isize) -> i32;

#[repr(C, packed)]
struct Config {
    size: u32,
    parent: HWND,
    instance: *mut c_void,
    flags: i32,
    common_buttons: i32,
    title: *const u16,
    main_icon: *const u16,
    heading: *const u16,
    content: *const u16,
    button_count: u32,
    buttons: *const Button,
    default_button: i32,
    radio_count: u32,
    radios: *const Button,
    default_radio: i32,
    verification: *const u16,
    expanded: *const u16,
    expanded_control: *const u16,
    collapsed_control: *const u16,
    footer_icon: *const u16,
    footer: *const u16,
    callback: Option<Callback>,
    callback_data: isize,
    width: u32,
}

const _: () = assert!(size_of::<Config>() == if cfg!(target_pointer_width = "64") { 160 } else { 96 });

const ALLOW_CANCEL: i32 = 0x0008; // TDF_ALLOW_DIALOG_CANCELLATION: Esc, Alt+F4 and the close button
const COMMAND_LINKS: i32 = 0x0010; // TDF_USE_COMMAND_LINKS
const CANCEL_BUTTON: i32 = 0x0008; // TDCBF_CANCEL_BUTTON
const CREATED: u32 = 0; // TDN_CREATED
const DESTROYED: u32 = 5; // TDN_DESTROYED

type TaskDialogIndirect = unsafe extern "system" fn(*const Config, *mut i32, *mut i32, *mut i32) -> i32;

/// The box has no owner (the desktop must stay at the bottom), so it is brought above the other windows itself.
unsafe extern "system" fn on_event(h: HWND, event: u32, _: WPARAM, _: LPARAM, _: isize) -> i32 {
    match event {
        CREATED => {
            BOX.store(h as isize, SeqCst);
            SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
            SetForegroundWindow(h);
        }
        DESTROYED => BOX.store(0, SeqCst),
        _ => {}
    }
    0 // S_OK
}

/// Shows the box and waits. Ok(None): cancelled.
unsafe fn show() -> Result<Option<Action>, String> {
    // looked up at run time: only comctl32 v6 (our manifest) exports it, and the test exe has no manifest, so a normal
    // import would stop it from starting at all
    let lib = LoadLibraryW(w("comctl32.dll").as_ptr());
    let Some(f) = GetProcAddress(lib, c"TaskDialogIndirect".as_ptr().cast()) else { return Err("TaskDialogIndirect is missing".into()) };
    let dialog: TaskDialogIndirect = std::mem::transmute(f);
    let texts: Vec<Vec<u16>> = ACTIONS.iter().map(|&a| w(label(a))).collect();
    let buttons: Vec<Button> = texts.iter().zip(FIRST_ID..).map(|(s, id)| Button { id, text: s.as_ptr() }).collect();
    let title = w(t!("Завершение работы Windows", "Shut Down Windows", ja: "Windows のシャットダウン", pt: "Desligar o Windows", zh: "关闭 Windows"));
    let heading = w(t!("Что должен сделать компьютер?", "What do you want the computer to do?", ja: "コンピューターの操作を選んでください", pt: "O que você deseja que o computador faça?", zh: "希望计算机做什么？"));
    let mut c: Config = std::mem::zeroed();
    c.size = size_of::<Config>() as u32;
    c.flags = ALLOW_CANCEL | COMMAND_LINKS;
    c.common_buttons = CANCEL_BUTTON;
    c.title = title.as_ptr();
    c.heading = heading.as_ptr();
    c.button_count = buttons.len() as u32;
    c.buttons = buttons.as_ptr();
    c.default_button = FIRST_ID;
    c.callback = Some(on_event);
    let mut picked = 0;
    let hr = dialog(&c, &mut picked, ptr::null_mut(), ptr::null_mut());
    if hr < 0 {
        return Err(format!("TaskDialogIndirect: 0x{hr:08X}"));
    }
    Ok(action_of(picked))
}

/// Opens the box, or brings the open one to the front. Returns at once: the box has its own thread, so the desktop keeps
/// working and Sleep (which returns only after wake-up) blocks nothing.
pub fn ask() {
    if OPEN.swap(true, SeqCst) {
        let h = BOX.load(SeqCst) as HWND;
        if !h.is_null() {
            unsafe { SetForegroundWindow(h) };
        }
        return;
    }
    std::thread::spawn(|| {
        let picked = unsafe { show() };
        OPEN.store(false, SeqCst); // before the action: after a sleep the box must open again at once
        let done = picked.and_then(|a| a.map_or(Ok(()), |a| a.run().map_err(|e| e.to_string())));
        if let Err(e) = done {
            let text = w(&format!("{}\n{e}", t!("Не получилось.", "That did not work.", ja: "操作できませんでした。", pt: "Não foi possível concluir.", zh: "操作失败。")));
            unsafe { MessageBoxW(ptr::null_mut(), text.as_ptr(), w("WinRust").as_ptr(), MB_ICONWARNING) };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_ids_map_back_to_actions() {
        for (a, id) in ACTIONS.into_iter().zip(FIRST_ID..) {
            assert_eq!(action_of(id), Some(a));
        }
        assert_eq!(action_of(2), None); // IDCANCEL: Esc, Alt+F4, Cancel
        assert_eq!(action_of(0), None);
        assert_eq!(action_of(FIRST_ID + ACTIONS.len() as i32), None);
    }

    #[test]
    fn every_line_has_its_own_key() {
        let keys: std::collections::HashSet<_> = ACTIONS.into_iter().filter_map(|a| label(a).split_once('&')?.1.chars().next()).flat_map(char::to_lowercase).collect();
        assert_eq!(keys.len(), ACTIONS.len());
    }
}
