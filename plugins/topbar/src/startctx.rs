//! What the three Start menu styles get: the rows with their icons, the geometry, the fonts and the theme colours.
//! Each style lives in its own file (`start7.rs`, `start10.rs`, `start11.rs`) so they can be worked on separately.
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::HFONT,
    UI::WindowsAndMessaging::HICON,
};

use crate::fly::Act;

/// One line of the Start menu: what it says, its icon and what it does.
pub struct Row {
    pub label: String,
    pub icon: HICON,
    pub act: Act,
}

/// Everything a style needs to lay itself out and paint itself.
pub struct Ctx<'a> {
    pub w: i32,
    pub h: i32,
    /// Scaling applied to the logical sizes (`1.0` at 96 dpi).
    pub dpi: f32,
    /// 0: 13 px text, 1: 12 px small, 2: 16 px glyphs, 3: 15 px semibold title, 4: 34 px big.
    pub fonts: [HFONT; 5],
    /// The app list, already filtered by what is typed, with its icons loaded.
    pub rows: &'a [Row],
    /// The first rows again: the pinned apps the grid styles show on top.
    pub pins: &'a [Row],
    /// How many rows the list starts at: the wheel moves it, so the styles can scroll long app lists.
    pub scroll: usize,
    /// What is typed in the search box (empty when nothing is).
    pub query: &'a str,
    /// The element under the pointer, for the hover highlight.
    pub hover: Option<Act>,
    /// The card colour of the current theme (the tiles and the search field sit on it).
    pub card: u32,
}

/// A logical size in pixels at the current scale.
pub fn sc(v: i32) -> i32 {
    (v as f32 * dpi()) as i32
}

fn dpi() -> f32 {
    use std::sync::OnceLock;
    use windows_sys::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, ReleaseDC, LOGPIXELSY};
    static S: OnceLock<f32> = OnceLock::new();
    *S.get_or_init(|| unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let d = GetDeviceCaps(dc, LOGPIXELSY as i32) as f32 / 96.0;
        ReleaseDC(std::ptr::null_mut(), dc);
        d.max(0.75)
    })
}

/// Rectangle helper: left, top, right, bottom.
pub fn r(l: i32, t: i32, rr: i32, b: i32) -> RECT {
    RECT { left: l, top: t, right: rr, bottom: b }
}

/// The theme's text colours, so every style stays consistent with the rest of the bar.
pub fn text_color() -> u32 {
    winrust_common::style::pal().text
}

pub fn dim_color() -> u32 {
    winrust_common::style::pal().dim
}

pub fn accent() -> u32 {
    winrust_common::style::pal().accent
}

pub fn bg() -> u32 {
    winrust_common::style::pal().bg
}

pub fn hover_color() -> u32 {
    winrust_common::style::pal().hover
}
