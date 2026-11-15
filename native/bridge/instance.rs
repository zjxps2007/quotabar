use quotabar::instance::Instance;
use std::{
    ffi::{CStr, c_char},
    io,
    path::Path,
    ptr,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_GEN_FAILURE, ERROR_INVALID_DATA, ERROR_INVALID_PARAMETER,
    ERROR_OPERATION_ABORTED, ERROR_SEM_TIMEOUT, ERROR_WRITE_FAULT,
};

fn win32_error(error: io::Error) -> i32 {
    error.raw_os_error().unwrap_or_else(|| match error.kind() {
        io::ErrorKind::PermissionDenied => ERROR_ACCESS_DENIED,
        io::ErrorKind::InvalidData => ERROR_INVALID_DATA,
        io::ErrorKind::InvalidInput => ERROR_INVALID_PARAMETER,
        io::ErrorKind::TimedOut => ERROR_SEM_TIMEOUT,
        io::ErrorKind::Interrupted => ERROR_OPERATION_ABORTED,
        io::ErrorKind::WriteZero => ERROR_WRITE_FAULT,
        _ => ERROR_GEN_FAILURE,
    } as i32)
}

/// # Safety
/// Pass a NUL-terminated UTF-8 directory and two writable integers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_instance_acquire(
    root: *const c_char,
    owner: *mut i32,
    error: *mut i32,
) -> *mut Instance {
    let (Some(owner), Some(error)) = (unsafe { owner.as_mut() }, unsafe { error.as_mut() }) else {
        return ptr::null_mut();
    };
    *owner = 0;
    *error = 0;
    let acquired = std::panic::catch_unwind(|| -> io::Result<Instance> {
        if root.is_null() {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let path = unsafe { CStr::from_ptr(root) }
            .to_str()
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        Instance::acquire(Path::new(path))
    });
    match acquired {
        Ok(Ok(instance)) => {
            *owner = i32::from(instance.is_owner());
            Box::into_raw(Box::new(instance))
        }
        Ok(Err(e)) => {
            *error = win32_error(e);
            ptr::null_mut()
        }
        Err(_) => {
            *error = ERROR_GEN_FAILURE as i32;
            ptr::null_mut()
        }
    }
}

/// # Safety
/// The instance must stay alive for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_instance_activate(instance: *const Instance) -> i32 {
    let Some(instance) = (unsafe { instance.as_ref() }) else {
        return ERROR_INVALID_PARAMETER as i32;
    };
    instance.activate().err().map_or(0, win32_error)
}

/// # Safety
/// Exactly one receiver may wait on a live owner. Returns 1 for activation, 0 on stop, -1 on failure.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_instance_wait(instance: *const Instance) -> i32 {
    (unsafe { instance.as_ref() })
        .and_then(|i| i.wait_for_activation().ok())
        .map(i32::from)
        .unwrap_or(-1)
}

/// # Safety
/// The instance must stay alive until the receiver returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_instance_stop(instance: *const Instance) {
    if let Some(instance) = unsafe { instance.as_ref() } {
        instance.stop_listening();
    }
}

/// # Safety
/// Release a uniquely owned instance after all concurrent operations have returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_instance_free(instance: *mut Instance) {
    if !instance.is_null() {
        drop(unsafe { Box::from_raw(instance) });
    }
}
