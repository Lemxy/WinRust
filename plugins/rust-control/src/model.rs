//! Shared data model: the rows the pages produce and the actions the window runs.
//! The model has no Win32 calls, so a page module only describes what it found.

use windows_sys::core::GUID;

/// One section of the navigation sidebar.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageId {
    Home,
    System,
    Programs,
    Services,
    Devices,
    Network,
    Users,
    Power,
    DateTime,
    Accessibility,
    Security,
    Tools,
}

/// Every page, in navigation order.
pub const NAV: [PageId; 12] = [
    PageId::Home,
    PageId::System,
    PageId::Programs,
    PageId::Services,
    PageId::Devices,
    PageId::Network,
    PageId::Users,
    PageId::Power,
    PageId::DateTime,
    PageId::Accessibility,
    PageId::Security,
    PageId::Tools,
];

impl PageId {
    pub fn index(self) -> usize {
        NAV.iter().position(|&p| p == self).unwrap_or(0)
    }

    pub fn from_index(i: usize) -> PageId {
        NAV.get(i).copied().unwrap_or(PageId::Home)
    }
}

/// Metadata of a page, built once at start (the language is fixed for the life of the process).
#[derive(Clone)]
pub struct PageMeta {
    pub id: PageId,
    pub title: String,
    pub description: String,
    pub icon: &'static str,
    pub tile: u32,
    pub keywords: String,
}

/// What the window opens outside itself.
#[derive(Clone, Copy)]
pub enum Target {
    /// A registered URI such as `ms-settings:network` or `windowsdefender:`.
    Uri(&'static str),
    /// An executable or a control panel applet such as `devmgmt.msc` or `timedate.cpl`.
    File(&'static str),
    /// `control.exe /name <applet>` for the classic control panel pages.
    Control(&'static str),
}

/// A real action. Destructive ones ask first in `main`.
#[derive(Clone)]
pub enum Action {
    None,
    Go(PageId),
    Open(Target),
    Uninstall { name: String, command: String },
    ServiceStart { name: String, label: String },
    ServiceStop { name: String, label: String },
    /// The "Startup" button: the window opens a menu with the three writable start types.
    ServiceStartupMenu { name: String, label: String, current: u32 },
    ServiceStartup { name: String, label: String, start: u32 },
    SetPlan { guid: GUID, label: String },
    SetHomeView(bool), // true = the flat icon list
    Refresh,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BtnKind {
    Normal,
    Accent,
    Danger,
}

#[derive(Clone)]
pub struct Button {
    pub label: String,
    pub action: Action,
    pub kind: BtnKind,
}

impl Button {
    pub fn new(label: impl Into<String>, action: Action, kind: BtnKind) -> Button {
        Button { label: label.into(), action, kind }
    }
}

/// A little status colour; the theme maps it to the palette.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Good,
    Warn,
    Bad,
    Dim,
}

/// One column of a table row. `weight` is relative to the other columns.
#[derive(Clone)]
pub struct Col {
    pub text: String,
    pub weight: u16,
    pub tone: Tone,
}

pub fn col(text: impl Into<String>, weight: u16, tone: Tone) -> Col {
    Col { text: text.into(), weight, tone }
}

/// A line of a page. Sections start cards; entries and tables live inside them.
#[derive(Clone)]
pub enum Row {
    Section(String),
    Info { label: String, value: String, tone: Tone },
    Entry {
        title: String,
        subtitle: String,
        detail: String,
        value: String,
        tone: Tone,
        action: Option<Action>,
        buttons: Vec<Button>,
    },
    Table { cols: Vec<Col>, tone: Tone, action: Option<Action>, buttons: Vec<Button>, reserve: i32 },
    Head { cols: Vec<Col>, reserve: i32 },
    Note { text: String, tone: Tone },
}

impl Row {
    pub fn section(title: impl Into<String>) -> Row {
        Row::Section(title.into())
    }

    pub fn info(label: impl Into<String>, value: impl Into<String>, tone: Tone) -> Row {
        Row::Info { label: label.into(), value: value.into(), tone }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn entry(
        title: impl Into<String>,
        subtitle: impl Into<String>,
        detail: impl Into<String>,
        value: impl Into<String>,
        tone: Tone,
        action: Option<Action>,
        buttons: Vec<Button>,
    ) -> Row {
        Row::Entry { title: title.into(), subtitle: subtitle.into(), detail: detail.into(), value: value.into(), tone, action, buttons }
    }

    pub fn link(title: impl Into<String>, subtitle: impl Into<String>, action: Action) -> Row {
        Row::entry(title, subtitle, "", "", Tone::Neutral, Some(action), vec![])
    }

    pub fn table(cols: Vec<Col>, tone: Tone, action: Option<Action>, buttons: Vec<Button>, reserve: i32) -> Row {
        Row::Table { cols, tone, action, buttons, reserve }
    }

    pub fn head(cols: Vec<Col>, reserve: i32) -> Row {
        Row::Head { cols, reserve }
    }

    pub fn note(text: impl Into<String>, tone: Tone) -> Row {
        Row::Note { text: text.into(), tone }
    }

    /// Height in logical pixels (34 is the standard row, an entry grows with its two small lines).
    pub fn height(&self) -> i32 {
        match self {
            Row::Section(_) => 34,
            Row::Note { .. } => 40,
            Row::Entry { subtitle, detail, .. } => {
                if !detail.is_empty() {
                    64
                } else if !subtitle.is_empty() {
                    50
                } else {
                    38
                }
            }
            Row::Info { .. } | Row::Table { .. } | Row::Head { .. } => 34,
        }
    }

    /// Everything the search box looks at.
    pub fn haystack(&self) -> String {
        let mut s = String::new();
        match self {
            Row::Section(t) => s.push_str(t),
            Row::Info { label, value, .. } => {
                s.push_str(label);
                s.push(' ');
                s.push_str(value);
            }
            Row::Entry { title, subtitle, detail, value, buttons, .. } => {
                s.push_str(title);
                s.push(' ');
                s.push_str(subtitle);
                s.push(' ');
                s.push_str(detail);
                s.push(' ');
                s.push_str(value);
                for b in buttons {
                    s.push(' ');
                    s.push_str(&b.label);
                }
            }
            Row::Table { cols, buttons, .. } => {
                for c in cols {
                    s.push_str(&c.text);
                    s.push(' ');
                }
                for b in buttons {
                    s.push_str(&b.label);
                    s.push(' ');
                }
            }
            Row::Head { cols, .. } => {
                for c in cols {
                    s.push_str(&c.text);
                    s.push(' ');
                }
            }
            Row::Note { text, .. } => s.push_str(text),
        }
        s.to_lowercase()
    }
}

/// A home-page category: a coloured icon, a title and 1–3 links.
#[derive(Clone)]
pub struct Category {
    pub title: String,
    pub icon: &'static str,
    pub tile: u32,
    pub links: Vec<HomeLink>,
}

#[derive(Clone)]
pub struct HomeLink {
    pub label: String,
    pub action: Action,
}

/// A page shown by the flat "Icons" view.
#[derive(Clone)]
pub struct Applet {
    pub title: String,
    pub icon: &'static str,
    pub tile: u32,
    pub action: Action,
    pub keywords: String,
}

/// What an armed/destructive action waits for.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HomeView {
    Category,
    Icons,
}
