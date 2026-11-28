//! Snapshot and restore the DWORDs changed by rust-explorer.

pub(crate) type Dword<'a> = (&'a str, &'a str);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OriginalDword {
    Present(u32),
    Absent,
}

pub(crate) struct OptionalDwordBackup<'a> {
    pub(crate) target: Dword<'a>,
    pub(crate) saved: Dword<'a>,
    pub(crate) absent: Dword<'a>,
}

impl<'a> OptionalDwordBackup<'a> {
    pub(crate) fn load(&self, read: impl Fn(Dword<'a>) -> Result<Option<u32>, String>) -> Result<Option<OriginalDword>, String> {
        match (read(self.saved)?, read(self.absent)?) {
            (Some(old), None) => Ok(Some(OriginalDword::Present(old))),
            (None, Some(1)) => Ok(Some(OriginalDword::Absent)),
            (None, None) => Ok(None),
            _ => Err(format!("{}: invalid snapshot for {}", self.saved.0, self.target.1)),
        }
    }

    pub(crate) fn capture(
        &self,
        read: impl Fn(Dword<'a>) -> Result<Option<u32>, String>,
        write: impl Fn(Dword<'a>, u32) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.load(&read)?.is_some() {
            return Ok(());
        }
        let original = read(self.target)?;
        // Keep a baseline that was saved while we read the live setting.
        if self.load(&read)?.is_some() {
            return Ok(());
        }
        match original {
            Some(old) => write(self.saved, old),
            None => write(self.absent, 1),
        }
    }

    pub(crate) fn restore(
        &self,
        read: impl Fn(Dword<'a>) -> Result<Option<u32>, String>,
        write: impl Fn(Dword<'a>, u32) -> Result<(), String>,
        remove: impl Fn(Dword<'a>) -> Result<(), String>,
    ) -> Result<bool, String> {
        match self.load(read)? {
            Some(OriginalDword::Present(old)) => write(self.target, old)?,
            Some(OriginalDword::Absent) => remove(self.target)?,
            None => return Ok(false),
        }
        Ok(true)
    }
}

pub(crate) fn backup_and_set<'a>(
    setting: Dword<'a>,
    backup: Dword<'a>,
    new: u32,
    read: impl Fn(Dword<'a>) -> Result<Option<u32>, String>,
    write: impl Fn(Dword<'a>, u32) -> Result<(), String>,
) -> Result<bool, String> {
    let Some(old) = read(setting)? else { return Ok(false) };
    if old == new {
        return Ok(false);
    }
    if read(backup)?.is_none() {
        write(backup, old)?;
    }
    write(setting, new)?;
    Ok(true)
}

pub(crate) fn restore<'a>(
    setting: Dword<'a>,
    backup: Dword<'a>,
    read: impl Fn(Dword<'a>) -> Result<Option<u32>, String>,
    write: impl Fn(Dword<'a>, u32) -> Result<(), String>,
) -> Result<bool, String> {
    let Some(old) = read(backup)? else { return Ok(false) };
    if read(setting)? != Some(old) {
        write(setting, old)?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};

    const START: Dword<'static> = ("Services\\Example", "Start");
    const BACKUP: Dword<'static> = ("Installer", "Example");

    #[derive(Default)]
    struct Registry {
        values: RefCell<HashMap<Dword<'static>, u32>>,
        write_error: Option<Dword<'static>>,
    }

    impl Registry {
        fn get(&self, key: Dword<'static>) -> Option<u32> {
            self.values.borrow().get(&key).copied()
        }

        fn write(&self, key: Dword<'static>, value: u32) -> Result<(), String> {
            if self.write_error == Some(key) {
                return Err("access denied".into());
            }
            self.values.borrow_mut().insert(key, value);
            Ok(())
        }

        fn apply(&self, new: u32) -> Result<bool, String> {
            backup_and_set(START, BACKUP, new, |key| Ok(self.get(key)), |key, value| self.write(key, value))
        }

        fn undo(&self) -> Result<bool, String> {
            restore(START, BACKUP, |key| Ok(self.get(key)), |key, value| self.write(key, value))
        }
    }

    const SHELL: OptionalDwordBackup<'static> = OptionalDwordBackup {
        target: ("Winlogon", "AutoRestartShell"),
        saved: ("Installer", "AutoRestartShell"),
        absent: ("Installer", "AutoRestartShellAbsent"),
    };

    fn capture_shell(reg: &Registry) -> Result<(), String> {
        SHELL.capture(|key| Ok(reg.get(key)), |key, value| reg.write(key, value))
    }

    fn restore_shell(reg: &Registry) -> Result<bool, String> {
        SHELL.restore(|key| Ok(reg.get(key)), |key, value| reg.write(key, value), |key| {
            if reg.write_error == Some(key) {
                return Err("access denied".into());
            }
            reg.values.borrow_mut().remove(&key);
            Ok(())
        })
    }

    #[test]
    fn shell_value_returns_to_its_first_snapshot() {
        let reg = Registry::default();
        reg.write(SHELL.target, u32::MAX).unwrap();
        capture_shell(&reg).unwrap();
        reg.write(SHELL.target, 0).unwrap();
        capture_shell(&reg).unwrap();
        assert!(restore_shell(&reg).unwrap());
        assert_eq!(reg.get(SHELL.target), Some(u32::MAX));
    }

    #[test]
    fn shell_value_originally_absent_is_deleted_even_on_retry() {
        let reg = Registry::default();
        capture_shell(&reg).unwrap();
        reg.write(SHELL.target, 0).unwrap();
        capture_shell(&reg).unwrap();
        assert!(restore_shell(&reg).unwrap());
        assert!(restore_shell(&reg).unwrap());
        assert_eq!(reg.get(SHELL.target), None);
        assert_eq!(reg.get(SHELL.absent), Some(1));
    }

    #[test]
    fn missing_snapshot_leaves_shell_value_alone_and_legacy_backup_is_read() {
        let reg = Registry::default();
        reg.write(SHELL.target, 0).unwrap();
        assert!(!restore_shell(&reg).unwrap());
        assert_eq!(reg.get(SHELL.target), Some(0));
        reg.write(SHELL.saved, 1).unwrap();
        assert!(restore_shell(&reg).unwrap());
        assert_eq!(reg.get(SHELL.target), Some(1));
    }

    #[test]
    fn unknown_or_inconsistent_registry_state_is_not_treated_as_absent() {
        let reg = Registry::default();
        assert!(SHELL.capture(|key| {
            if key == SHELL.target { Err("access denied".into()) } else { Ok(reg.get(key)) }
        }, |key, value| reg.write(key, value)).is_err());
        assert!(reg.values.borrow().is_empty());
        reg.write(SHELL.target, 0).unwrap();
        reg.write(SHELL.saved, 1).unwrap();
        reg.write(SHELL.absent, 1).unwrap();
        assert!(restore_shell(&reg).is_err());
        assert_eq!(reg.get(SHELL.target), Some(0));
    }

    #[test]
    fn a_failed_delete_leaves_the_absence_marker_for_retry() {
        let reg = Registry { write_error: Some(SHELL.target), ..Registry::default() };
        reg.write(SHELL.absent, 1).unwrap();
        reg.values.borrow_mut().insert(SHELL.target, 0);
        assert!(restore_shell(&reg).is_err());
        assert_eq!(reg.get(SHELL.absent), Some(1));
        assert_eq!(reg.get(SHELL.target), Some(0));
    }

    #[test]
    fn keeps_the_original_across_retries_and_restores_it() {
        let reg = Registry::default();
        reg.write(START, 3).unwrap();
        assert!(reg.apply(4).unwrap());
        assert!(reg.apply(2).unwrap());
        assert_eq!(reg.get(BACKUP), Some(3));
        assert!(reg.undo().unwrap());
        assert_eq!(reg.get(START), Some(3));
    }

    #[test]
    fn leaves_missing_targets_and_legacy_installations_alone() {
        let reg = Registry::default();
        assert!(!reg.apply(4).unwrap());
        assert!(reg.values.borrow().is_empty());
        reg.write(START, 4).unwrap();
        assert!(!reg.undo().unwrap());
        assert_eq!(reg.get(START), Some(4));
    }

    #[test]
    fn a_failed_snapshot_prevents_the_system_write() {
        let reg = Registry { write_error: Some(BACKUP), ..Registry::default() };
        reg.write(START, 3).unwrap();
        assert!(reg.apply(4).is_err());
        assert_eq!(reg.get(START), Some(3));
    }

    #[test]
    fn a_failed_restore_leaves_the_backup_for_retry() {
        let reg = Registry { write_error: Some(START), ..Registry::default() };
        reg.write(BACKUP, 3).unwrap();
        reg.values.borrow_mut().insert(START, 4);
        assert!(reg.undo().is_err());
        assert_eq!(reg.get(BACKUP), Some(3));
    }

    #[test]
    fn a_read_error_is_not_treated_as_a_missing_value() {
        let reg = Registry::default();
        let result = backup_and_set(START, BACKUP, 4, |_| Err("access denied".into()), |key, value| reg.write(key, value));
        assert!(result.is_err());
        assert!(reg.values.borrow().is_empty());
    }
}
