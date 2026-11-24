//! Default component selection for the installer window and `install all`.

pub(crate) const RUST_EXPLORER: &str = "rust-explorer";
pub(crate) const TOPBAR: &str = "topbar";

pub(crate) fn selected_by_default(name: &str, installed: bool) -> bool {
    name != RUST_EXPLORER || installed
}

/// The "Express" choice of the window: the recommended components (not rust-explorer, never the extras), and everything
/// already installed stays (express never removes anything, and never moves the shell).
pub(crate) fn express_pick(name: &str, bulk: bool, installed: bool) -> bool {
    installed || (bulk && selected_by_default(name, false))
}

/// What the window's button does with the switches: (to remove, to install). A switch says "I want this installed", so
/// only the difference is touched; `changed` marks an installed one whose options differ (installed again to apply them).
/// `rows` is in install order; removal runs backwards (rust-explorer before topbar).
pub(crate) fn plan(rows: &[usize], installed: impl Fn(usize) -> bool, wanted: impl Fn(usize) -> bool, changed: impl Fn(usize) -> bool) -> (Vec<usize>, Vec<usize>) {
    let remove = rows.iter().rev().copied().filter(|&i| installed(i) && !wanted(i)).collect();
    let install = rows.iter().copied().filter(|&i| wanted(i) && (!installed(i) || changed(i))).collect();
    (remove, install)
}

/// The steps in order, (plugin, install?): removals, then installs. `first` (rust-explorer handing the shell back to Explorer)
/// goes before everything, so topbar is never removed while it is the only bar the session has.
pub(crate) fn steps(remove: &[usize], install: &[usize], first: Option<usize>) -> Vec<(usize, bool)> {
    let first = first.filter(|f| install.contains(f));
    first.map(|f| (f, true)).into_iter()
        .chain(remove.iter().map(|&i| (i, false)))
        .chain(install.iter().copied().filter(|&i| Some(i) != first).map(|i| (i, true)))
        .collect()
}

/// A plugin switch flipped: does the "replace the Explorer shell" wish go with it? It needs both rust-explorer and topbar.
pub(crate) fn drops_shell(name: &str, on: bool) -> bool {
    !on && (name == RUST_EXPLORER || name == TOPBAR)
}

/// The shell switch turned on: the plugins that must be switched on with it.
pub(crate) fn shell_needs() -> [&'static str; 2] {
    [TOPBAR, RUST_EXPLORER]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_touches_only_the_difference() {
        // 0 installed+wanted, 1 installed+unwanted, 2 new+wanted, 3 new+unwanted, 4 installed+unwanted
        let inst = [true, true, false, false, true];
        let want = [true, false, true, false, false];
        let (rm, add) = plan(&[0, 1, 2, 3, 4], |i| inst[i], |i| want[i], |_| false);
        assert_eq!(rm, vec![4, 1]); // backwards
        assert_eq!(add, vec![2]);
    }

    #[test]
    fn changed_options_reinstall_only_what_stays() {
        let inst = [true, true, false];
        let want = [true, false, true];
        let (rm, add) = plan(&[0, 1, 2], |i| inst[i], |i| want[i], |_| true);
        assert_eq!(rm, vec![1]); // a removed one is not reinstalled for its options
        assert_eq!(add, vec![0, 2]);
    }

    #[test]
    fn the_shell_goes_back_before_the_bar_goes() {
        // 2 = topbar being removed, 3 = rust-explorer switching to the file manager only
        assert_eq!(steps(&[2], &[3, 5], Some(3)), vec![(3, true), (2, false), (5, true)]);
        assert_eq!(steps(&[2], &[5], Some(3)), vec![(2, false), (5, true)]); // not reinstalled: nothing to move
        assert_eq!(steps(&[4, 1], &[2], None), vec![(4, false), (1, false), (2, true)]);
    }

    #[test]
    fn the_shell_wish_needs_both() {
        assert!(drops_shell(TOPBAR, false));
        assert!(drops_shell(RUST_EXPLORER, false));
        assert!(!drops_shell(RUST_EXPLORER, true)); // the file manager alone does not need topbar
        assert!(!drops_shell(TOPBAR, true));
        assert!(!drops_shell("rust-search", false));
        assert_eq!(shell_needs(), [TOPBAR, RUST_EXPLORER]);
    }

    #[test]
    fn fresh_install_does_not_replace_explorer_by_default() {
        assert!(!selected_by_default(RUST_EXPLORER, false));
        assert!(selected_by_default(TOPBAR, false));
        assert!(selected_by_default("rust-search", false));
    }

    #[test]
    fn express_installs_the_safe_set_and_keeps_what_is_there() {
        assert!(express_pick(TOPBAR, true, false));
        assert!(!express_pick(RUST_EXPLORER, true, false), "folders change hands only on request");
        assert!(express_pick(RUST_EXPLORER, true, true), "an installed one is not removed");
        assert!(!express_pick("purge", false, false) && !express_pick("debloat", false, false), "extras never by default");
        assert!(express_pick("debloat", false, true));
    }

    #[test]
    fn existing_rust_explorer_is_selected_for_gui_updates() {
        assert!(selected_by_default(RUST_EXPLORER, true));
    }
}
