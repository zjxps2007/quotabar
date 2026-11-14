use quotabar::notification_icon::{MenuCallback, NotificationIcon};
use std::{
    ffi::{CStr, c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
};

#[unsafe(no_mangle)]
pub extern "C" fn qb_tray_create(callback: Option<MenuCallback>) -> *mut NotificationIcon {
    callback
        .and_then(|cb| catch_unwind(|| NotificationIcon::create(cb)).ok()?.ok())
        .map_or(ptr::null_mut(), Box::into_raw)
}

/// # Safety
/// All tray calls use the original live handle on its owning UI thread.
/// Both strings must be valid NUL-terminated UTF-8 until the call returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_tray_update(
    handle: *const NotificationIcon,
    text: *const c_char,
    logo: i32,
    warning: i32,
    tooltip: *const c_char,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    if text.is_null() || tooltip.is_null() {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let text = unsafe { CStr::from_ptr(text) }.to_str().ok()?;
        let tooltip = unsafe { CStr::from_ptr(tooltip) }.to_str().ok()?;
        handle.update(text, logo != 0, warning != 0, tooltip).ok()
    }))
    .ok()
    .flatten()
    .unwrap_or(-1)
}
/// # Safety
/// Pass a live handle on the thread that created it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_tray_window(handle: *const NotificationIcon) -> *mut c_void {
    (unsafe { handle.as_ref() }).map_or(ptr::null_mut(), NotificationIcon::window)
}
/// # Safety
/// Pass a live handle on the thread that created it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_tray_visible(handle: *const NotificationIcon) -> i32 {
    (unsafe { handle.as_ref() }).is_some_and(NotificationIcon::visible) as i32
}
/// # Safety
/// Pass a live handle on the thread that created it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_tray_focus(handle: *const NotificationIcon) {
    if let Some(handle) = unsafe { handle.as_ref() } {
        handle.return_focus();
    }
}
/// # Safety
/// Release once on the owning UI thread after outstanding calls finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_tray_free(handle: *mut NotificationIcon) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}
