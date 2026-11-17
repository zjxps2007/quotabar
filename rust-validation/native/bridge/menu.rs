use quotabar::tray_menu::{Callback, Menu};
use std::{
    ffi::{CStr, c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
};
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_monitor_message(message: *const c_char, error: i32) {
    if !message.is_null() {
        quotabar::tray_menu::show_message(
            &unsafe { CStr::from_ptr(message) }.to_string_lossy(),
            error != 0,
        );
    }
}
// The owning Win32 UI thread serializes all calls, including button callbacks.
#[unsafe(no_mangle)]
pub extern "C" fn qb_menu_create(callback: Callback) -> *mut Menu {
    catch_unwind(|| Menu::new(callback))
        .ok()
        .and_then(Result::ok)
        .map_or(ptr::null_mut(), Box::into_raw)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_menu_window(handle: *const Menu) -> *mut c_void {
    unsafe { handle.as_ref() }.map_or(ptr::null_mut(), Menu::hwnd)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_menu_state(handle: *const Menu, bytes: *const u8, len: usize) -> i32 {
    let Some(menu) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    if bytes.is_null() || len > quotabar::dashboard::MAX_FRAME {
        return -1;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(state) = serde_json::from_slice(unsafe { std::slice::from_raw_parts(bytes, len) })
        else {
            return -1;
        };
        menu.update(&state);
        0
    }))
    .unwrap_or(-1)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_menu_show(handle: *const Menu, x: i32, y: i32) {
    if let Some(menu) = unsafe { handle.as_ref() } {
        menu.show(x, y);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_menu_close(handle: *const Menu) {
    if let Some(menu) = unsafe { handle.as_ref() } {
        menu.close(false);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_tray_samples(directory: *const c_char) -> i32 {
    if directory.is_null() {
        return -1;
    }
    let Ok(path) = (unsafe { CStr::from_ptr(directory) }).to_str() else {
        return -1;
    };
    quotabar::tray_capture::samples(std::path::Path::new(path)).map_or(-1, |()| 0)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_menu_capture(handle: *const Menu, path: *const c_char) -> i32 {
    let Some(menu) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    if path.is_null() {
        return -1;
    }
    let Ok(path) = (unsafe { CStr::from_ptr(path) }).to_str() else {
        return -1;
    };
    quotabar::tray_capture::menu(menu, std::path::Path::new(path)).map_or(-1, |()| 0)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_menu_free(handle: *mut Menu) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}
