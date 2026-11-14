use super::CancelHandle;
use quotabar::windows_login::{self, LoginError};
use serde::Deserialize;
use std::{
    ffi::{CStr, CString, c_char},
    path::{Path, PathBuf},
    ptr,
    time::Duration,
};

/// # Safety
/// The caller owns three valid NUL-terminated UTF-8 strings for this call.
/// Release a non-null result using qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_find_executable(
    name: *const c_char,
    path: *const c_char,
    home: *const c_char,
) -> *mut c_char {
    if name.is_null() || path.is_null() || home.is_null() {
        return ptr::null_mut();
    }
    std::panic::catch_unwind(|| {
        let string = |p| unsafe { CStr::from_ptr(p) }.to_str().ok();
        let found = windows_login::find(string(name)?, string(path)?, Path::new(string(home)?))?;
        CString::new(found.to_str()?).ok()
    })
    .ok()
    .flatten()
    .map_or(ptr::null_mut(), CString::into_raw)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Attempt {
    executable: PathBuf,
    directory: PathBuf,
    prefix: Vec<String>,
    timeout_ms: Option<u64>,
}
/// # Safety
/// The UTF-8 request and cancellation handle must outlive this call.
/// Release a successful output with qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_login(
    request: *const c_char,
    cancel: *const CancelHandle,
    output: *mut *mut u8,
    length: *mut usize,
    os_error: *mut i32,
) -> i32 {
    let (Some(cancel), Some(output), Some(length), Some(os_error)) = (
        unsafe { cancel.as_ref() },
        unsafe { output.as_mut() },
        unsafe { length.as_mut() },
        unsafe { os_error.as_mut() },
    ) else {
        return -1;
    };
    *output = ptr::null_mut();
    *length = 0;
    *os_error = 0;
    if request.is_null() {
        return -1;
    }
    let result = std::panic::catch_unwind(|| {
        let attempt: Attempt =
            serde_json::from_slice(unsafe { CStr::from_ptr(request) }.to_bytes()).ok()?;
        Some(windows_login::login(
            &attempt.executable,
            &attempt.directory,
            &attempt.prefix,
            attempt.timeout_ms.map(Duration::from_millis),
            &cancel.0,
        ))
    });
    match result {
        Ok(Some(Ok(bytes))) => {
            let data: Box<[u8]> = bytes.as_slice().into();
            *length = data.len();
            *output = Box::into_raw(data).cast();
            0
        }
        Ok(Some(Err(LoginError::Cancelled | LoginError::Timeout))) => 1,
        Ok(Some(Err(LoginError::Failed))) => 2,
        Ok(Some(Err(LoginError::Launch(e)))) => {
            *os_error = e.raw_os_error().unwrap_or(1);
            3
        }
        Ok(Some(Err(LoginError::Storage(e)))) => {
            *os_error = e.raw_os_error().unwrap_or(1);
            4
        }
        _ => -1,
    }
}
