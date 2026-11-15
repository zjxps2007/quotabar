use super::windows_installer::{path, text};
use quotabar::{
    monitor_loop::MonitorLoop,
    updates::windows::resident::{Event, Updater, probe},
};
use std::{
    ffi::{CString, c_char},
    ptr,
};

/// # Safety
/// The monitor loop and the NUL-terminated paths must be live. The owner is
/// used on its creating thread and freed before destroying the monitor window.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_updater_new(
    dispatcher: &MonitorLoop,
    root: *const c_char,
    target: *const c_char,
    version: *const c_char,
) -> *mut Updater {
    let (Ok(root), Ok(target), Ok(version)) =
        (unsafe { path(root) }, unsafe { path(target) }, unsafe {
            text(version)
        })
    else {
        return ptr::null_mut();
    };
    let window = dispatcher.window() as isize;
    Box::into_raw(Box::new(Updater::new(
        root.to_owned(),
        target.to_owned(),
        version.to_owned(),
        move || MonitorLoop::update_input(window),
    )))
}

/// # Safety
/// Consume the owner exactly once on the monitor thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_updater_free(handle: *mut Updater) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_windows_updater_configure(
    owner: &mut Updater,
    automatic: i32,
    idle: i32,
    suspended: i32,
) {
    owner.configure(automatic != 0, idle != 0, suspended != 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_windows_updater_check(owner: &mut Updater, manual: i32, install: i32) {
    if install != 0 {
        owner.install_now();
    } else {
        owner.check(manual != 0, false);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_windows_updater_dispatch(owner: &mut Updater) {
    owner.dispatch();
}
#[unsafe(no_mangle)]
pub extern "C" fn qb_windows_updater_delay(owner: &Updater) -> u32 {
    owner.delay_ms()
}

/// # Safety
/// An optional failure is a NUL-terminated UTF-8 message borrowed for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_updater_drained(owner: &mut Updater, failure: *const c_char) {
    owner.drained(if failure.is_null() {
        Ok(())
    } else {
        Err(unsafe { text(failure) }
            .unwrap_or("자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다."))
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_windows_updater_exited(owner: &mut Updater, success: i32) {
    owner.exited(success != 0);
}

/// Pull an event before invoking managed callbacks, which may reenter the
/// updater. The initial snapshot is returned without consuming an event.
#[unsafe(no_mangle)]
pub extern "C" fn qb_windows_updater_event(
    owner: &mut Updater,
    initial: i32,
    json: &mut *mut c_char,
) -> i32 {
    *json = ptr::null_mut();
    let state = if initial != 0 {
        owner.state().clone()
    } else {
        match owner.next_event() {
            None => return 0,
            Some(Event::State(state)) => state,
            Some(Event::PrepareExit) => return 2,
            Some(Event::Resume) => return 3,
            Some(Event::Exit) => return 4,
        }
    };
    *json = CString::new(serde_json::to_vec(&state).expect("update state"))
        .expect("JSON escapes NUL")
        .into_raw();
    1
}

/// # Safety
/// The NUL-terminated output path and version are valid for this synchronous probe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_updater_probe(
    report: *const c_char,
    version: *const c_char,
) -> i32 {
    let (Ok(report), Ok(version)) = (unsafe { path(report) }, unsafe { text(version) }) else {
        return -1;
    };
    probe::run(report, version).map_or(-1, |()| 0)
}
