use quotabar::{
    dashboard::{MAX_FRAME, windows::host::Event},
    monitor_loop::MonitorLoop,
};
use std::{
    ffi::{CStr, c_char},
    path::Path,
    ptr, slice,
};

/// # Safety
/// The callback remains valid on the creating thread until qb_loop_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_new(
    callback: Option<extern "C" fn(i32)>,
    window: *mut isize,
) -> *mut MonitorLoop {
    let (Some(callback), Some(window)) = (callback, unsafe { window.as_mut() }) else {
        return ptr::null_mut();
    };
    *window = 0;
    match MonitorLoop::new(move |event| callback(event as i32)) {
        Ok(state) => {
            *window = state.window() as isize;
            Box::into_raw(state)
        }
        Err(_) => ptr::null_mut(),
    }
}

/// # Safety
/// Run the live loop on its creating thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_run(state: *const MonitorLoop) -> i32 {
    (unsafe { state.as_ref() })
        .and_then(|s| s.run().ok())
        .map_or(-1, |()| 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_loop_wake(window: isize) -> i32 {
    i32::from(MonitorLoop::wake(window))
}

/// # Safety
/// The caller owns the live loop on this thread. A zero delay cancels that timer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_timer(
    state: *const MonitorLoop,
    index: usize,
    delay_ms: u32,
) -> i32 {
    (unsafe { state.as_ref() })
        .and_then(|s| s.timer(index, delay_ms).ok())
        .map_or(-1, |()| 0)
}

/// # Safety
/// Request exit on the loop's owning thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_quit(state: *const MonitorLoop) {
    if let Some(state) = unsafe { state.as_ref() } {
        state.quit();
    }
}

/// # Safety
/// Release exactly once on the creating thread after the message loop returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_free(state: *mut MonitorLoop) {
    if !state.is_null() {
        drop(unsafe { Box::from_raw(state) });
    }
}

/// The monitor owns the host and calls these entry points only on its UI thread.
/// A null capture path selects the normal interactive dashboard.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_dashboard_show(
    state: *const MonitorLoop,
    exe: *const c_char,
    capture: *const c_char,
    route: *const c_char,
) -> i32 {
    let Some(state) = (unsafe { state.as_ref() }) else {
        return 87;
    };
    let string = |p: *const c_char| {
        if p.is_null() {
            None
        } else {
            unsafe { CStr::from_ptr(p) }.to_str().ok()
        }
    };
    let (Some(exe), Some(route)) = (string(exe), string(route)) else {
        return 87;
    };
    let capture = string(capture).map(Path::new);
    match state.dashboard.show(Path::new(exe), capture, route) {
        Ok(()) => 0,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => -1,
        Err(e) => e.raw_os_error().unwrap_or(1),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_dashboard_status(
    state: *const MonitorLoop,
    ready: *mut i32,
) -> u32 {
    let (Some(state), Some(ready)) = (unsafe { state.as_ref() }, unsafe { ready.as_mut() }) else {
        return 0;
    };
    *ready = i32::from(state.dashboard.ready());
    state.dashboard.pid()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_dashboard_dispatch(
    state: *const MonitorLoop,
    callback: extern "C" fn(Event, *const u8, usize) -> i32,
) {
    if let Some(state) = unsafe { state.as_ref() } {
        state
            .dashboard
            .dispatch(|event, data| callback(event, data.as_ptr(), data.len()));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_dashboard_send(
    state: *const MonitorLoop,
    data: *const u8,
    length: usize,
) {
    if let Some(state) = unsafe { state.as_ref() }
        && !data.is_null()
        && (1..=MAX_FRAME).contains(&length)
    {
        state
            .dashboard
            .send(unsafe { slice::from_raw_parts(data, length) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_loop_dashboard_close(state: *const MonitorLoop, force: i32) {
    if let Some(state) = unsafe { state.as_ref() } {
        if force != 0 {
            state.dashboard.stop();
        } else {
            state.dashboard.close();
        }
    }
}
