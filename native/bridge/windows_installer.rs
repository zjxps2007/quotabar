//! Managed monitor entry points for the native Windows installer transaction.
use super::CancelHandle;
use quotabar::updates::windows::{self, Installer, PreparationError};
use std::{
    ffi::{CStr, c_char},
    io,
    path::Path,
    ptr, slice,
    sync::Mutex,
};
use uuid::Uuid;

unsafe fn text<'a>(value: *const c_char) -> io::Result<&'a str> {
    if value.is_null() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map_err(io::Error::other)
}
unsafe fn path<'a>(value: *const c_char) -> io::Result<&'a Path> {
    unsafe { text(value) }.map(Path::new)
}

fn status(result: io::Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) if e.kind() == io::ErrorKind::Interrupted => 1,
        Err(e) => e
            .raw_os_error()
            .and_then(|n| n.checked_add(10))
            .filter(|n| *n > 10)
            .unwrap_or(-1),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_prepare(
    data: *const u8,
    len: usize,
    cancel: *const CancelHandle,
) -> i32 {
    let Some(cancel) = (unsafe { cancel.as_ref() }) else {
        return -1;
    };
    if data.is_null() || len > 64 * 1024 {
        return -1;
    }
    match windows::prepare(unsafe { slice::from_raw_parts(data, len) }, &cancel.0) {
        Ok(()) => 0,
        Err(PreparationError::Signature) => 2,
        Err(PreparationError::File(e)) => status(Err(e)),
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_copy(
    source: *const c_char,
    dest: *const c_char,
    cancel: *const CancelHandle,
) -> i32 {
    let Some(cancel) = (unsafe { cancel.as_ref() }) else {
        return -1;
    };
    status((|| unsafe {
        windows::copy_tree(path(source)?, path(dest)?, &cancel.0)
    })())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_overlay(
    source: *const c_char,
    dest: *const c_char,
    cancel: *const CancelHandle,
) -> i32 {
    let Some(cancel) = (unsafe { cancel.as_ref() }) else {
        return -1;
    };
    status((|| unsafe {
        windows::overlay_zip(path(source)?, path(dest)?, &cancel.0)
    })())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_installed(target: *const c_char) -> i32 {
    unsafe { path(target) }
        .and_then(windows::installed)
        .map_or(-1, i32::from)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_clean(root: *const c_char) -> i32 {
    status(unsafe { path(root) }.and_then(windows::clean_completed))
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_discard(
    root: *const c_char,
    target: *const c_char,
    stage: *const c_char,
) -> i32 {
    status((|| unsafe {
        windows::discard(path(root)?, path(target)?, path(stage)?)
    })())
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_ack(
    root: *const c_char,
    target: *const c_char,
    id: *const c_char,
    version: *const c_char,
) -> i32 {
    status((|| unsafe {
        windows::acknowledge(
            path(root)?,
            path(target)?,
            Uuid::parse_str(text(id)?).map_err(io::Error::other)?,
            text(version)?,
        )
    })())
}

type Helper = Mutex<Installer>;

#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_launch(job: *const c_char, error: *mut i32) -> *mut Helper {
    let Some(error) = (unsafe { error.as_mut() }) else {
        return ptr::null_mut();
    };
    match unsafe { path(job) }.and_then(Installer::launch) {
        Ok(installer) => {
            *error = 0;
            Box::into_raw(Box::new(Mutex::new(installer)))
        }
        Err(e) => {
            *error = status(Err(e));
            ptr::null_mut()
        }
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_ready(helper: *const Helper) -> i32 {
    unsafe { helper.as_ref() }
        .and_then(|h| h.lock().ok()?.ready().ok())
        .map_or(-1, i32::from)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_pid(helper: *const Helper) -> u32 {
    unsafe { helper.as_ref() }
        .and_then(|h| h.lock().ok().map(|h| h.pid()))
        .unwrap_or(0)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_detach(helper: *const Helper) {
    if let Some(helper) = unsafe { helper.as_ref() } {
        helper.lock().unwrap_or_else(|e| e.into_inner()).detach();
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn qb_windows_update_free(helper: *mut Helper) {
    if !helper.is_null() {
        drop(unsafe { Box::from_raw(helper) });
    }
}
