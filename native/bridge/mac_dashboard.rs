use quotabar::dashboard::{
    MAX_FRAME,
    macos::host::{Callback, Host},
};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
    rc::Rc,
    slice,
};

// All handles and store callbacks stay on the monitor's main thread.
#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_dashboard_host_new(
    context: *mut c_void,
    callback: Callback,
    demo: i32,
) -> *const Host {
    Host::new(context, callback, demo != 0).map_or(ptr::null(), Rc::into_raw)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_dashboard_host_show(host: *const Host, route: *const c_char) {
    if let Some(host) = unsafe { host.as_ref() }
        && !route.is_null()
        && let Ok(route) = unsafe { CStr::from_ptr(route) }.to_str()
    {
        host.show(route);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_dashboard_host_send(
    host: *const Host,
    data: *const u8,
    len: usize,
) {
    if let Some(host) = unsafe { host.as_ref() } {
        if data.is_null() || !(1..=MAX_FRAME).contains(&len) {
            host.close();
        } else {
            host.send(unsafe { slice::from_raw_parts(data, len) });
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_dashboard_host_changed(host: *const Host) {
    if let Some(host) = unsafe { host.as_ref() } {
        host.changed()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_dashboard_host_close(host: *const Host) {
    if let Some(host) = unsafe { host.as_ref() } {
        host.close()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_dashboard_host_running(host: *const Host) -> i32 {
    unsafe { host.as_ref() }.is_some_and(Host::running) as i32
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_dashboard_host_free(host: *const Host) {
    if !host.is_null() {
        let host = unsafe { Rc::from_raw(host) };
        host.stop();
    }
}
