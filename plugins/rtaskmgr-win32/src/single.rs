//! Один экземпляр диспетчера задач: второй запуск поднимает уже открытое окно, а не открывает второе.
//! Ключ — именованный мьютекс в сеансе (`Local\`); окно ищем по свойству окна, а если экземпляр старой
//! сборки — по заголовку. Ни то, ни другое не требует доступа к чужому процессу: перезапуск с правами
//! администратора идёт через границу целостности (UAC), где `OpenProcess` уже не работает.
use std::{
    mem, ptr,
    sync::atomic::{AtomicIsize, Ordering},
};
use winapi::{
    shared::{
        minwindef::{BOOL, FALSE, LPARAM, TRUE},
        windef::HWND,
        winerror::ERROR_ALREADY_EXISTS,
    },
    um::{
        errhandlingapi::GetLastError,
        handleapi::CloseHandle,
        processthreadsapi::GetCurrentProcessId,
        synchapi::CreateMutexW,
        winnt::HANDLE,
        winuser::*,
    },
};

/// Мьютекс в пространстве имён сеанса: у каждого пользователя свой диспетчер задач, и это правильно.
const MUTEX: &str = "Local\\RTaskMgr-SingleInstance";
/// Сообщение «покажи окно»: его посылает второй запуск первому.
const ACTIVATE: &str = "RTaskMgr-Activate-Window";
/// Свойство главного окна — метка, по которой второй запуск находит окно первого, не трогая его процесс.
const MARK: &str = "RTaskMgr-Main-Window";

/// Дескриптор мьютекса живёт до конца процесса — систему сама закроет его при выходе.
static HELD: AtomicIsize = AtomicIsize::new(0);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn claim_named(name: &str) -> bool {
    unsafe {
        let h = CreateMutexW(ptr::null_mut(), FALSE, wide(name).as_ptr());
        let err = GetLastError();
        if h.is_null() {
            // ERROR_ACCESS_DENIED: мьютекс создан процессом с более высокими правами (запуск через UAC) —
            // значит, экземпляр уже есть, и мы не имеем права даже открыть его объект.
            return false;
        }
        if err == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return false;
        }
        HELD.store(h as isize, Ordering::SeqCst);
        true
    }
}

/// true — мы единственный экземпляр; false — диспетчер задач уже запущен (в этом же сеансе).
pub fn claim() -> bool {
    claim_named(MUTEX)
}

/// Помечает главное окно свойством: так его находит второй запуск, даже если первый работает с правами
/// администратора (чужой процесс тогда недоступен, а свойство окна — доступно).
pub fn mark_window(hwnd: HWND) {
    unsafe { SetPropW(hwnd, wide(MARK).as_ptr(), 1 as HANDLE) };
}

struct Find {
    me: u32,
    found: HWND,
}

unsafe extern "system" fn find_cb(hwnd: HWND, l: LPARAM) -> BOOL {
    unsafe {
        let f = &mut *(l as *mut Find);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != f.me && !GetPropW(hwnd, wide(MARK).as_ptr()).is_null() {
            f.found = hwnd;
            return FALSE; // нашли — обход заканчиваем
        }
        TRUE
    }
}

/// Поднимает окно уже запущенного экземпляра: разворачивает из трея/с панели задач и отдаёт фокус.
/// false — окно не нашлось (например, первый экземпляр ещё стартует) — тогда просто выходим.
pub fn activate_existing(title: &str) -> bool {
    let me = unsafe { GetCurrentProcessId() };
    let mut f = Find { me, found: ptr::null_mut() };
    unsafe { EnumWindows(Some(find_cb), &mut f as *mut Find as LPARAM) };
    // помеченного окна нет (экземпляр старой сборки) — берём по заголовку
    let hwnd = if f.found.is_null() { unsafe { FindWindowW(ptr::null(), wide(title).as_ptr()) } } else { f.found };
    if hwnd.is_null() {
        return false;
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        } else {
            ShowWindow(hwnd, SW_SHOW);
        }
        // просим приложение показать себя «по-своему» (свежие цифры, сброс таймера); из процесса с низкой
        // целостностью UIPI это сообщение может не дойти — тогда работает ShowWindow выше
        let m = RegisterWindowMessageW(wide(ACTIVATE).as_ptr());
        if m != 0 {
            PostMessageW(hwnd, m, 0, 0);
        }
        SetForegroundWindow(hwnd);
        // Windows вправе не отдать фокус фоновому процессу — тогда хотя бы мигаем в панели задач
        let mut fi: FLASHWINFO = mem::zeroed();
        fi.cbSize = mem::size_of::<FLASHWINFO>() as u32;
        fi.hwnd = hwnd;
        fi.dwFlags = FLASHW_ALL | FLASHW_TIMERNOFG;
        fi.uCount = 2;
        FlashWindowEx(&mut fi);
    }
    true
}

/// Идентификатор сообщения «покажи окно» для уже работающего экземпляра. 0 — не зарегистрировалось.
pub fn activate_message() -> u32 {
    unsafe { RegisterWindowMessageW(wide(ACTIVATE).as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutex_is_taken_only_once() {
        // своё имя на процесс: не мешаем настоящему диспетчеру, если он запущен
        let name = format!("Local\\RTaskMgr-Test-{}", std::process::id());
        assert!(claim_named(&name), "первый запрос должен взять мьютекс");
        assert!(!claim_named(&name), "второй запрос должен увидеть занятый мьютекс");
    }
}
