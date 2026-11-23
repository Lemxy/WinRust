//! Default component selection for the installer window and `install all`.

pub(crate) const RUST_EXPLORER: &str = "rust-explorer";

pub(crate) fn selected_by_default(name: &str, installed: bool) -> bool {
    name != RUST_EXPLORER || installed
}

/// What the window's button does with the switches: (to remove, to install). A switch says "I want this installed", so
/// only the difference is touched. `rows` is in install order; removal runs backwards (rust-explorer before topbar).
pub(crate) fn plan(rows: &[usize], installed: impl Fn(usize) -> bool, wanted: impl Fn(usize) -> bool) -> (Vec<usize>, Vec<usize>) {
    let remove = rows.iter().rev().copied().filter(|&i| installed(i) && !wanted(i)).collect();
    let install = rows.iter().copied().filter(|&i| !installed(i) && wanted(i)).collect();
    (remove, install)
}

/// A switch that drags another along: rust-explorer needs topbar (on pulls topbar on, topbar off pulls rust-explorer off).
pub(crate) fn linked(name: &str, on: bool) -> Option<(&'static str, bool)> {
    match (name, on) {
        (RUST_EXPLORER, true) => Some(("topbar", true)),
        ("topbar", false) => Some((RUST_EXPLORER, false)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_touches_only_the_difference() {
        // 0 installed+wanted, 1 installed+unwanted, 2 new+wanted, 3 new+unwanted, 4 installed+unwanted
        let inst = [true, true, false, false, true];
        let want = [true, false, true, false, false];
        let (rm, add) = plan(&[0, 1, 2, 3, 4], |i| inst[i], |i| want[i]);
        assert_eq!(rm, vec![4, 1]); // backwards
        assert_eq!(add, vec![2]);
    }

    #[test]
    fn explorer_and_topbar_go_together() {
        assert_eq!(linked(RUST_EXPLORER, true), Some(("topbar", true)));
        assert_eq!(linked("topbar", false), Some((RUST_EXPLORER, false)));
        assert_eq!(linked("topbar", true), None);
    }

    #[test]
    fn fresh_install_does_not_replace_explorer_by_default() {
        assert!(!selected_by_default(RUST_EXPLORER, false));
        assert!(selected_by_default("topbar", false));
        assert!(selected_by_default("rust-search", false));
    }

    #[test]
    fn existing_rust_explorer_is_selected_for_gui_updates() {
        assert!(selected_by_default(RUST_EXPLORER, true));
    }
}
