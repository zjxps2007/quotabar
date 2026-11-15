//! The application owns a non-null policy handle and serializes its calls on
//! the monitor UI thread. Version arguments are UTF-8, NUL-terminated strings.
use quotabar::updates::monitor::{Monitor, Offer, OnConfigure};
use std::ffi::{CStr, c_char};

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_policy_new() -> *mut Monitor {
    Box::into_raw(Box::default())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_update_policy_free(policy: *mut Monitor) {
    if !policy.is_null() {
        drop(unsafe { Box::from_raw(policy) });
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_configure(
    p: &mut Monitor,
    automatic: i32,
    can_install: i32,
    suspended: i32,
) -> OnConfigure {
    p.configure(automatic != 0, can_install != 0, suspended != 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_begin_check(p: &mut Monitor, manual: i32) -> i32 {
    i32::from(p.begin_check(manual != 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_attempted(p: &mut Monitor, now: f64) {
    p.attempted(now);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_update_offer(
    p: &Monitor,
    current: *const c_char,
    version: *const c_char,
) -> Offer {
    if current.is_null() || version.is_null() {
        return Offer::Current;
    }
    let current = unsafe { CStr::from_ptr(current) }.to_str().unwrap_or("");
    let version = unsafe { CStr::from_ptr(version) }.to_str().unwrap_or("");
    p.offer(current, version)
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_should_download(p: &Monitor, requested: i32) -> i32 {
    i32::from(p.should_download(requested != 0))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_update_staged(p: &mut Monitor, version: *const c_char) {
    if !version.is_null()
        && let Ok(version) = unsafe { CStr::from_ptr(version) }.to_str()
    {
        p.staged(version);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_update_block(p: &mut Monitor, version: *const c_char) {
    if !version.is_null()
        && let Ok(version) = unsafe { CStr::from_ptr(version) }.to_str()
    {
        p.block(version);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_begin_install(p: &mut Monitor, force: i32) -> i32 {
    i32::from(p.begin_install(force != 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_begin_handoff(p: &mut Monitor, force: i32) -> i32 {
    i32::from(p.begin_handoff(force != 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_may_install(p: &Monitor, force: i32) -> i32 {
    i32::from(p.may_install(force != 0))
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_finish(p: &mut Monitor, cancelled: i32) {
    p.finish(cancelled != 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_update_delay(p: &Monitor, now: f64) -> f64 {
    p.delay(now).unwrap_or(0.)
}
