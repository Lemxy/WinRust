//! Windows Service Control Manager operations for rust-explorer.
use super::service_state::ServiceState;
use crate::{run, wide};
use std::{ptr::{null, null_mut}, thread, time::{Duration, Instant}};
use windows_sys::Win32::System::Services::{
    ChangeServiceConfigW, CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_HANDLE,
    SC_MANAGER_CONNECT, SERVICE_CHANGE_CONFIG, SERVICE_CONTINUE_PENDING, SERVICE_NO_CHANGE,
    SERVICE_PAUSE_PENDING, SERVICE_PAUSED, SERVICE_QUERY_STATUS, SERVICE_START_PENDING, SERVICE_STATUS,
    SERVICE_STOP_PENDING,
};

struct Handle(SC_HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}

impl Handle {
    fn open(name: &str, access: u32) -> Result<Self, String> {
        let scm = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
        if scm.is_null() {
            return Err(service_error(name));
        }
        let scm = Self(scm);
        let service = unsafe { OpenServiceW(scm.0, wide(name).as_ptr(), access) };
        if service.is_null() {
            return Err(service_error(name));
        }
        Ok(Self(service))
    }

    fn status(&self, name: &str) -> Result<u32, String> {
        let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        if unsafe { QueryServiceStatus(self.0, &mut status) } == 0 {
            return Err(service_error(name));
        }
        Ok(status.dwCurrentState)
    }

    fn wait_for(&self, name: &str, accept: impl Fn(u32) -> bool) -> Result<u32, String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let state = self.status(name)?;
            if accept(state) {
                return Ok(state);
            }
            if Instant::now() >= deadline {
                return Err(tf!("{name}: служба не достигла нужного состояния (код {state})",
                    "{name}: service did not reach the expected state (status {state})"));
            }
            thread::sleep(Duration::from_millis(250));
        }
    }
}

fn service_error(name: &str) -> String {
    format!("{name}: {}", std::io::Error::last_os_error())
}

fn is_settled(status: u32) -> bool {
    !matches!(status, SERVICE_START_PENDING | SERVICE_STOP_PENDING | SERVICE_CONTINUE_PENDING | SERVICE_PAUSE_PENDING)
}

fn settled_status(name: &str) -> Result<u32, String> {
    Handle::open(name, SERVICE_QUERY_STATUS)?.wait_for(name, is_settled)
}

pub(super) fn is_stopped(name: &str) -> Result<bool, String> {
    Ok(settled_status(name)? == ServiceState::Stopped as u32)
}

pub(super) fn current_state(name: &str) -> Result<ServiceState, String> {
    let status = settled_status(name)?;
    ServiceState::from_status(status).ok_or_else(|| tf!("{name}: нельзя сохранить состояние службы (код {status})",
        "{name}: cannot save the service state (status {status})"))
}

pub(super) fn change_start(name: &str, start: u32) -> Result<(), String> {
    let service = Handle::open(name, SERVICE_CHANGE_CONFIG)?;
    // Keep the SCM's configuration in sync so a restored start mode is usable
    // immediately. SERVICE_NO_CHANGE and null parameters leave other fields alone.
    let changed = unsafe {
        ChangeServiceConfigW(service.0, SERVICE_NO_CHANGE, start, SERVICE_NO_CHANGE, null(),
            null(), null_mut(), null(), null(), null(), null())
    };
    if changed == 0 {
        return Err(service_error(name));
    }
    Ok(())
}

pub(super) fn ensure_state(name: &str, expected: ServiceState) -> Result<(), String> {
    let handle = Handle::open(name, SERVICE_QUERY_STATUS)?;
    let current = handle.wait_for(name, is_settled)?;
    if current == expected as u32 {
        return Ok(());
    }
    let command = match expected {
        ServiceState::Stopped => "stop",
        ServiceState::Running if current == SERVICE_PAUSED => "continue",
        ServiceState::Running => "start",
    };
    if let Err(error) = run("sc.exe", &[command, name]) {
        if handle.status(name)? != expected as u32 {
            return Err(error);
        }
    }
    handle.wait_for(name, |status| status == expected as u32)?;
    Ok(())
}
