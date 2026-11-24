//! The record a program sends to the bar to raise a notification.
//!
//! The bar listens on the named pipe `\\.\pipe\winrust-notify` and reads this struct as it is: no text to quote, no
//! escaping and no parser on either side. Version 1. The field order and the fixed UTF-16 buffers are the protocol;
//! a shorter struct is accepted and the missing tail counts as zero, so an older sender without `id` still works.

use std::mem::size_of;

/// The pipe every WinRust program writes to (one connection per notification).
pub const PIPE: &str = r"\\.\pipe\winrust-notify";

/// Current version of the record.
pub const VERSION: u32 = 1;

/// `flags` bit: `progress` carries a progress bar, including `0.0` (an empty bar). Without the bit a progress of zero
/// means "no bar", which is what a sender that does not know the field sends.
pub const FLAG_PROGRESS: u32 = 1;

pub const APP_LEN: usize = 64;
pub const TITLE_LEN: usize = 128;
pub const BODY_LEN: usize = 512;
pub const ICON_LEN: usize = 260;
pub const ACTION_LEN: usize = 260;
pub const CMD_LEN: usize = 512;
pub const ID_LEN: usize = 64;

/// One notification, as it travels through the pipe.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NotifyWire {
    pub version: u32,
    /// How long the toast stays, milliseconds; 0 = the bar's `notify-timeout`.
    pub timeout_ms: u32,
    /// 0..1 when `flags & FLAG_PROGRESS`, otherwise ignored.
    pub progress: f32,
    pub flags: u32,
    pub app: [u16; APP_LEN],
    pub title: [u16; TITLE_LEN],
    pub body: [u16; BODY_LEN],
    pub icon: [u16; ICON_LEN],
    pub action_label: [u16; ACTION_LEN],
    pub action_cmd: [u16; CMD_LEN],
    pub id: [u16; ID_LEN],
}

/// The exact number of bytes a version 1 record occupies.
pub const WIRE_SIZE: usize = size_of::<NotifyWire>();

impl Default for NotifyWire {
    fn default() -> Self {
        NotifyWire {
            version: VERSION,
            timeout_ms: 0,
            progress: 0.0,
            flags: 0,
            app: [0; APP_LEN],
            title: [0; TITLE_LEN],
            body: [0; BODY_LEN],
            icon: [0; ICON_LEN],
            action_label: [0; ACTION_LEN],
            action_cmd: [0; CMD_LEN],
            id: [0; ID_LEN],
        }
    }
}

/// Copies a Rust string into a fixed UTF-16 buffer, NUL terminated. A text that does not fit is cut and a dangling
/// high surrogate (the first half of an emoji) is dropped, so the receiver always decodes valid UTF-16.
pub fn put(buf: &mut [u16], s: &str) {
    if buf.is_empty() {
        return;
    }
    let max = buf.len() - 1;
    let mut n = 0;
    for u in s.encode_utf16() {
        if n >= max {
            break;
        }
        buf[n] = u;
        n += 1;
    }
    if n > 0 && (0xD800..0xDC00).contains(&buf[n - 1]) {
        n -= 1;
    }
    buf[n] = 0;
    for v in &mut buf[n + 1..] {
        *v = 0;
    }
}

/// Reads a fixed UTF-16 buffer up to its first NUL.
pub fn get(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

/// A record from the bytes the pipe returned: a full record copies as is, a short one is zero padded.
pub fn from_bytes(bytes: &[u8]) -> NotifyWire {
    // all fields are integers, floats or arrays of integers, so all-zero is a valid record
    let mut w: NotifyWire = unsafe { std::mem::zeroed() };
    let n = bytes.len().min(WIRE_SIZE);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), &mut w as *mut NotifyWire as *mut u8, n);
    }
    w
}

impl NotifyWire {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self as *const NotifyWire as *const u8, WIRE_SIZE) }
    }

    pub fn app(&self) -> String {
        get(&self.app)
    }
    pub fn title(&self) -> String {
        get(&self.title)
    }
    pub fn body(&self) -> String {
        get(&self.body)
    }
    pub fn icon(&self) -> String {
        get(&self.icon)
    }
    pub fn action_label(&self) -> String {
        get(&self.action_label)
    }
    pub fn action_cmd(&self) -> String {
        get(&self.action_cmd)
    }
    pub fn id(&self) -> String {
        get(&self.id)
    }

    pub fn set_app(&mut self, s: &str) {
        put(&mut self.app, s);
    }
    pub fn set_title(&mut self, s: &str) {
        put(&mut self.title, s);
    }
    pub fn set_body(&mut self, s: &str) {
        put(&mut self.body, s);
    }
    pub fn set_icon(&mut self, s: &str) {
        put(&mut self.icon, s);
    }
    pub fn set_action_label(&mut self, s: &str) {
        put(&mut self.action_label, s);
    }
    pub fn set_action_cmd(&mut self, s: &str) {
        put(&mut self.action_cmd, s);
    }
    pub fn set_id(&mut self, s: &str) {
        put(&mut self.id, s);
    }

    /// The progress to draw, if any. `FLAG_PROGRESS` makes an explicit 0 count; without it a zero field is "none".
    pub fn progress(&self) -> Option<f32> {
        if self.flags & FLAG_PROGRESS != 0 || self.progress > 0.0 {
            Some(self.progress.clamp(0.0, 1.0))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips_and_is_nul_terminated() {
        let mut w = NotifyWire::new();
        w.set_app("WinRust");
        assert_eq!(w.app(), "WinRust");
        assert_eq!(w.app[7], 0);
        let mut buf = [1u16; 8];
        put(&mut buf, "abcdefghij");
        assert_eq!(get(&buf), "abcdefg");
    }

    #[test]
    fn short_records_zero_pad() {
        let mut w = NotifyWire::new();
        w.set_title("hola");
        let bytes = w.as_bytes();
        let back = from_bytes(&bytes[..32]);
        assert_eq!(back.version, VERSION);
        assert_eq!(back.title(), "");
        assert_eq!(from_bytes(bytes).title(), "hola");
    }

    #[test]
    fn progress_needs_the_flag_for_zero() {
        let mut w = NotifyWire::new();
        assert_eq!(w.progress(), None);
        w.progress = 0.42;
        assert_eq!(w.progress(), Some(0.42));
        w.progress = 0.0;
        assert_eq!(w.progress(), None);
        w.flags = FLAG_PROGRESS;
        assert_eq!(w.progress(), Some(0.0));
    }
}
