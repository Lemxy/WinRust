//! The notifications under the calendar (#126): Windows keeps them, UserNotificationListener reads and dismisses them.
//! The calls wait, so they run in a thread; the flyout paints the last answer.
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering::Relaxed},
};
use windows::UI::Notifications::{KnownNotificationBindings, Management::UserNotificationListener, Management::UserNotificationListenerAccessStatus, NotificationKinds, UserNotification};

#[derive(Clone)]
pub struct Note {
    pub id: u32,
    pub app: String,
    pub text: String,
}

/// How many the panel shows.
pub const ROWS: usize = 3;

/// None: not read yet or Windows does not let us.
pub static LIST: Mutex<Option<Vec<Note>>> = Mutex::new(None);
static BUSY: AtomicBool = AtomicBool::new(false);

/// One line out of a toast's text elements.
fn one_line(parts: &[String]) -> String {
    parts.iter().flat_map(|p| p.split_whitespace()).collect::<Vec<_>>().join(" ")
}

fn note(n: &UserNotification) -> windows::core::Result<Note> {
    let texts = n.Notification()?.Visual()?.GetBinding(&KnownNotificationBindings::ToastGeneric()?)?.GetTextElements()?;
    let parts: Vec<String> = texts.into_iter().filter_map(|t| t.Text().ok().map(|s| s.to_string())).collect();
    let app = n.AppInfo().and_then(|a| a.DisplayInfo()).and_then(|d| d.DisplayName()).map(|s| s.to_string()).unwrap_or_default();
    Ok(Note { id: n.Id()?, app, text: one_line(&parts) })
}

fn read() -> windows::core::Result<Option<Vec<Note>>> {
    let l = UserNotificationListener::Current()?;
    if l.GetAccessStatus()? != UserNotificationListenerAccessStatus::Allowed {
        return Ok(None);
    }
    let all = l.GetNotificationsAsync(NotificationKinds::Toast)?.join()?;
    let mut v: Vec<Note> = all.into_iter().filter_map(|n| note(&n).ok()).collect();
    v.reverse(); // newest first
    Ok(Some(v))
}

/// Runs `f` on the listener (if given), then reads the list again. One thread at a time.
fn run(f: Option<Box<dyn FnOnce(&UserNotificationListener) + Send>>) {
    if BUSY.swap(true, Relaxed) && f.is_none() {
        return;
    }
    std::thread::spawn(move || {
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED); }
        if let (Some(f), Ok(l)) = (f, UserNotificationListener::Current()) {
            f(&l);
        }
        *LIST.lock().unwrap() = read().ok().flatten();
        BUSY.store(false, Relaxed);
    });
}

pub fn refresh() {
    run(None);
}

pub fn dismiss(id: u32) {
    if let Some(v) = LIST.lock().unwrap().as_mut() {
        v.retain(|n| n.id != id); // the row goes at once; the read after corrects it
    }
    run(Some(Box::new(move |l| _ = l.RemoveNotification(id))));
}

pub fn clear() {
    if let Some(v) = LIST.lock().unwrap().as_mut() {
        v.clear();
    }
    run(Some(Box::new(|l| _ = l.ClearNotifications())));
}

#[cfg(test)]
mod tests {
    #[test]
    fn toast_lines_become_one() {
        assert_eq!(super::one_line(&["Title".into(), "two\r\n lines".into(), String::new()]), "Title two lines");
    }
}
