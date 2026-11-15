#[cfg(windows)]
use quotabar::dashboard::Endpoint;
use quotabar::dashboard::MAX_FRAME;
#[cfg(target_os = "macos")]
use quotabar::dashboard::macos::DashboardChild;
#[cfg(windows)]
use quotabar::dashboard::windows::DashboardChild;
use std::io;
use std::{
    ffi::{CStr, c_char},
    path::Path,
    ptr, slice,
};

/// # Safety
/// Paths are NUL-terminated UTF-8. A non-null result is a uniquely owned child.
#[unsafe(no_mangle)]
#[cfg(windows)]
pub unsafe extern "C" fn qb_dashboard_spawn(
    exe: *const c_char,
    capture: *const c_char,
    os_error: *mut i32,
) -> *mut DashboardChild {
    let Some(status) = (unsafe { os_error.as_mut() }) else {
        return ptr::null_mut();
    };
    *status = 0;
    let spawn = std::panic::catch_unwind(|| -> io::Result<DashboardChild> {
        if exe.is_null() {
            return Err(io::Error::other("Missing dashboard executable"));
        }
        let exe = unsafe { CStr::from_ptr(exe) }
            .to_str()
            .map_err(io::Error::other)?;
        let capture = if capture.is_null() {
            None
        } else {
            Some(Path::new(
                unsafe { CStr::from_ptr(capture) }
                    .to_str()
                    .map_err(io::Error::other)?,
            ))
        };
        DashboardChild::spawn(Path::new(exe), capture)
    });
    match spawn {
        Ok(Ok(child)) => Box::into_raw(Box::new(child)),
        Ok(Err(e)) => {
            *status = e.raw_os_error().unwrap_or(1);
            ptr::null_mut()
        }
        Err(_) => {
            *status = 1;
            ptr::null_mut()
        }
    }
}
/// # Safety
/// Supply a NUL-terminated executable path. Release the returned child exactly once.
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_dashboard_spawn(
    exe: *const c_char,
    demo: i32,
) -> *mut DashboardChild {
    if exe.is_null() {
        return ptr::null_mut();
    }
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let path = Path::new(OsStr::from_bytes(unsafe { CStr::from_ptr(exe) }.to_bytes()));
    std::panic::catch_unwind(|| DashboardChild::spawn(path, demo != 0))
        .ok()
        .and_then(Result::ok)
        .map_or(ptr::null_mut(), |c| Box::into_raw(Box::new(c)))
}
/// # Safety
/// The child must be live for this call, including after its process exits.
#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_dashboard_timed_out(child: *const DashboardChild) -> i32 {
    unsafe { child.as_ref() }.is_some_and(DashboardChild::timed_out) as i32
}
/// # Safety
/// Pass a live child throughout this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_dashboard_pid(child: *const DashboardChild) -> u32 {
    (unsafe { child.as_ref() })
        .filter(|c| c.running())
        .map_or(0, DashboardChild::pid)
}
/// # Safety
/// Pass a live child with a body of exactly length readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_dashboard_send(
    child: *const DashboardChild,
    input: *const u8,
    length: usize,
) -> i32 {
    let Some(child) = (unsafe { child.as_ref() }) else {
        return -1;
    };
    if input.is_null() || length == 0 || length > MAX_FRAME {
        return -1;
    }
    if child
        .send(unsafe { slice::from_raw_parts(input, length) })
        .is_ok()
    {
        0
    } else {
        -1
    }
}
/// # Safety
/// Exactly one receiver may use a child at a time. Both outputs must be writable,
/// and every delivered body must be released with qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_dashboard_read(
    child: *const DashboardChild,
    output: *mut *mut u8,
    length: *mut usize,
) -> i32 {
    let Some(child) = (unsafe { child.as_ref() }) else {
        return -1;
    };
    unsafe { receive(output, length, || child.receive()) }
}
unsafe fn receive(
    output: *mut *mut u8,
    length: *mut usize,
    read: impl FnOnce() -> io::Result<Option<Vec<u8>>>,
) -> i32 {
    let (Some(output), Some(length)) = (unsafe { output.as_mut() }, unsafe { length.as_mut() })
    else {
        return -1;
    };
    *output = ptr::null_mut();
    *length = 0;
    match read() {
        Ok(Some(body)) => {
            let bytes = body.into_boxed_slice();
            *length = bytes.len();
            *output = Box::into_raw(bytes).cast();
            0
        }
        Ok(None) => 1,
        Err(_) => -1,
    }
}
/// # Safety
/// Transfer two distinct, live inherited Windows pipe handles to the result.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_dashboard_endpoint_open(input: usize, output: usize) -> *mut Endpoint {
    unsafe { Endpoint::inherited(input, output) }
        .map_or(ptr::null_mut(), |p| Box::into_raw(Box::new(p)))
}
/// # Safety
/// Pass a live endpoint for this blocking read, two writable outputs, and release the
/// returned frame with qb_secret_free. EOF returns 1 and a protocol error returns -1.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_dashboard_endpoint_read(
    endpoint: *const Endpoint,
    output: *mut *mut u8,
    length: *mut usize,
) -> i32 {
    let Some(endpoint) = (unsafe { endpoint.as_ref() }) else {
        return -1;
    };
    unsafe { receive(output, length, || endpoint.receive()) }
}
/// # Safety
/// Pass a live endpoint and length readable bytes until the write returns.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_dashboard_endpoint_send(
    endpoint: *const Endpoint,
    input: *const u8,
    length: usize,
) -> i32 {
    let Some(endpoint) = (unsafe { endpoint.as_ref() }) else {
        return -1;
    };
    if input.is_null() || !(1..=MAX_FRAME).contains(&length) {
        return -1;
    }
    if endpoint
        .send(unsafe { slice::from_raw_parts(input, length) })
        .is_ok()
    {
        0
    } else {
        -1
    }
}
/// # Safety
/// Release each endpoint once all readers and writers have returned.
#[unsafe(no_mangle)]
#[cfg(windows)]
pub unsafe extern "C" fn qb_dashboard_endpoint_free(endpoint: *mut Endpoint) {
    if !endpoint.is_null() {
        drop(unsafe { Box::from_raw(endpoint) });
    }
}
/// # Safety
/// Pass a live child. Wait and stop may run concurrently with read or send.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_dashboard_control(child: *const DashboardChild, action: i32) {
    if let Some(child) = unsafe { child.as_ref() } {
        match action {
            0 => child.ready(),
            1 => child.close(),
            2 => child.stop(),
            3 => child.wait(),
            _ => {}
        }
    }
}
/// # Safety
/// Release a unique handle once all other native calls have exited.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_dashboard_free(child: *mut DashboardChild) {
    if !child.is_null() {
        drop(unsafe { Box::from_raw(child) });
    }
}
