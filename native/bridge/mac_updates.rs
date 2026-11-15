use quotabar::updates::macos::resident::{Event, Updater};
use std::{ffi::c_void, ptr, rc::Rc};

#[repr(i32)]
pub enum UpdateEvent {
    State,
    PrepareExit,
    Resume,
    Exit,
}
type Callback = extern "C" fn(*mut c_void, UpdateEvent, *const u8, usize);

/// The store owns the handle on the main AppKit thread, and the callback
/// borrows state bytes only for the duration of its invocation.
#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_updater_new(context: *mut c_void, notify: Callback) -> *const Updater {
    Updater::new(move |event| {
        let kind = match event {
            Event::State(state) => {
                let bytes = serde_json::to_vec(state).expect("finite update state");
                return notify(context, UpdateEvent::State, bytes.as_ptr(), bytes.len());
            }
            Event::PrepareExit => UpdateEvent::PrepareExit,
            Event::Resume => UpdateEvent::Resume,
            Event::Exit => UpdateEvent::Exit,
        };
        notify(context, kind, ptr::null(), 0);
    })
    .map_or(ptr::null(), Rc::into_raw)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_updater_configure(
    handle: &Updater,
    automatic: i32,
    can_install: i32,
    suspended: i32,
) {
    handle.configure(automatic != 0, can_install != 0, suspended != 0);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_updater_check(handle: &Updater, install: i32) {
    if install != 0 {
        handle.install_now();
    } else {
        handle.check();
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_updater_drained(handle: &Updater, saved: i32) {
    handle.drained(saved != 0);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_updater_free(handle: *const Updater) {
    if !handle.is_null() {
        let updater = unsafe { Rc::from_raw(handle) };
        updater.stop();
    }
}
