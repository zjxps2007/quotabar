use quotabar::monitor_loop::MonitorLoop;
use std::ptr;

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
