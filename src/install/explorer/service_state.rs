//! Restore the saved running state in the order Windows allows.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceState {
    Stopped = 1,
    Running = 4,
}

impl ServiceState {
    pub(crate) fn from_status(value: u32) -> Option<Self> {
        match value {
            1 => Some(Self::Stopped),
            4 => Some(Self::Running),
            _ => None,
        }
    }
}

pub(crate) fn restore_start_and_state(
    start: u32,
    state: ServiceState,
    needs_temporary_enable: impl FnOnce() -> Result<bool, String>,
    set_start: impl Fn(u32) -> Result<(), String>,
    set_state: impl FnOnce(ServiceState) -> Result<(), String>,
) -> Result<(), String> {
    // Changing a running service to disabled does not stop it, but a disabled
    // service cannot be started. This can occur when someone disabled a service
    // without stopping it before installing rust-explorer.
    const DISABLED: u32 = 4;
    if start == DISABLED && state == ServiceState::Running {
        if needs_temporary_enable()? {
            const MANUAL: u32 = 3;
            let started = set_start(MANUAL).and_then(|_| set_state(state));
            let disabled = set_start(DISABLED);
            return match (started, disabled) {
                (Err(start), Err(mode)) => Err(format!("{start}; {mode}")),
                (result, mode) => result.and(mode),
            };
        }
        set_state(state)?;
        return set_start(start);
    }

    set_start(start)?;
    set_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    fn restore_steps(start: u32, state: ServiceState) -> Vec<String> {
        let steps = RefCell::new(Vec::new());
        restore_start_and_state(start, state, || Ok(false), |m| {
            steps.borrow_mut().push(format!("start mode {m}"));
            Ok(())
        }, |s| {
            steps.borrow_mut().push(format!("{s:?}"));
            Ok(())
        }).unwrap();
        steps.into_inner()
    }

    #[test]
    fn restores_startup_mode_before_most_runtime_changes() {
        assert_eq!(restore_steps(3, ServiceState::Running), ["start mode 3", "Running"]);
        assert_eq!(restore_steps(2, ServiceState::Stopped), ["start mode 2", "Stopped"]);
    }

    #[test]
    fn starts_a_previously_running_service_before_disabling_it() {
        assert_eq!(restore_steps(4, ServiceState::Running), ["Running", "start mode 4"]);
    }

    #[test]
    fn takes_no_follow_up_action_when_a_step_fails() {
        let failed = restore_start_and_state(3, ServiceState::Running, || Ok(false),
            |_| Err("cannot restore startup mode".into()), |_| panic!("service state changed"));
        assert_eq!(failed, Err("cannot restore startup mode".into()));
        let failed = restore_start_and_state(4, ServiceState::Running, || Ok(false),
            |_| panic!("disabled service after start failure"), |_| Err("cannot start service".into()));
        assert_eq!(failed, Err("cannot start service".into()));
    }

    #[test]
    fn enables_a_disabled_stopped_service_until_it_is_running() {
        let start = Cell::new(4);
        let state = Cell::new(ServiceState::Stopped);
        restore_start_and_state(4, ServiceState::Running, || Ok(true),
            |mode| { start.set(mode); Ok(()) },
            |service_state| {
                assert_eq!(start.get(), 3);
                state.set(service_state);
                Ok(())
            }).unwrap();
        assert_eq!(start.get(), 4);
        assert_eq!(state.get(), ServiceState::Running);
    }

    #[test]
    fn restores_the_disabled_mode_even_if_start_fails() {
        let start = Cell::new(4);
        let result = restore_start_and_state(4, ServiceState::Running, || Ok(true),
            |mode| { start.set(mode); Ok(()) },
            |_| {
                assert_eq!(start.get(), 3);
                Err("cannot start service".into())
            });
        assert_eq!(result, Err("cannot start service".into()));
        assert_eq!(start.get(), 4);
    }

    #[test]
    fn only_running_and_stopped_are_valid_snapshots() {
        assert_eq!(ServiceState::from_status(1), Some(ServiceState::Stopped));
        assert_eq!(ServiceState::from_status(4), Some(ServiceState::Running));
        assert_eq!(ServiceState::from_status(2), None);
        assert_eq!(ServiceState::from_status(7), None);
    }
}
