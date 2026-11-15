use quotabar::mac_menu::{Callback, Status};
use std::{ffi::c_void, ptr, rc::Rc};
#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_appearance(mode: i32) {
    quotabar::mac_menu::apply_appearance(match mode {
        1 => "light",
        2 => "dark",
        _ => "system",
    });
}
#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_dock_visible(visible: i32) -> i32 {
    quotabar::mac_menu::dock_visible(visible != 0) as i32
}
// Handles and callbacks belong to the main AppKit thread.
#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_status_new(context: *mut c_void, callback: Callback) -> *mut Status {
    Status::new(context, callback).map_or(ptr::null_mut(), |s| Rc::into_raw(s).cast_mut())
}
#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_monitor_status_new(
    context: *mut c_void,
    callback: Callback,
) -> *mut Status {
    Status::monitor(context, callback).map_or(ptr::null_mut(), |s| Rc::into_raw(s).cast_mut())
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_status_show_popover(handle: *const Status) {
    if let Some(status) = unsafe { handle.as_ref() } {
        status.show_popover();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_status_state(handle: *const Status, data: *const u8, len: usize) {
    let Some(menu) = (unsafe { handle.as_ref() }) else {
        return;
    };
    if data.is_null() || len > quotabar::dashboard::MAX_FRAME {
        return;
    }
    if let Ok(state) = serde_json::from_slice(unsafe { std::slice::from_raw_parts(data, len) }) {
        menu.update(state);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_status_publish(
    handle: *const Status,
    data: *const u8,
    len: usize,
) -> i32 {
    let Some(menu) = (unsafe { handle.as_ref() }) else {
        return 0;
    };
    if data.is_null() || len > quotabar::dashboard::MAX_FRAME {
        return 0;
    }
    match quotabar::tray::mac_popover(unsafe { std::slice::from_raw_parts(data, len) }) {
        Ok(view) => {
            menu.update(view);
            1
        }
        Err(_) => 0,
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_status_control(handle: *const Status, action: i32) -> i32 {
    let Some(s) = (unsafe { handle.as_ref() }) else {
        return 0;
    };
    match action {
        0 => s.toggle(),
        1 => s.close(),
        2 => s.clear(),
        _ => {}
    }
    s.shown() as i32
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_status_free(handle: *mut Status) {
    if !handle.is_null() {
        drop(unsafe { Rc::from_raw(handle) });
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_monitor_run() -> i32 {
    quotabar::mac_monitor::run()
}
