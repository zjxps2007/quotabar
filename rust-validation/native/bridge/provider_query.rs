use super::credentials::{error_code, publish};
use chrono::DateTime;
use quotabar::{
    credentials::Platform,
    oauth::RotationError,
    provider_query::{Error, Operation, Query},
};
use std::{ptr, slice};

fn operation(op: i32) -> Option<Operation> {
    match op {
        0 => Some(Operation::Codex),
        1 => Some(Operation::Gemini),
        2 => Some(Operation::Refresh),
        _ => None,
    }
}

fn failure(error: Error, output: &mut *mut u8, count: &mut usize) -> i32 {
    match error {
        Error::Credentials(error) | Error::Rotation(RotationError::Credentials(error)) => {
            error_code(error)
        }
        Error::QueryCredentials(status) => status,
        Error::Rotation(RotationError::MalformedResponse | RotationError::InvalidResponse) => -8,
        Error::Rotation(RotationError::ReturnedAccountChanged) => -9,
        Error::Rotation(RotationError::StoredAccountChanged) => -10,
        Error::Json => -31,
        Error::Provider(code, message) => {
            publish(
                serde_json::json!({ "code": code, "message": message })
                    .to_string()
                    .into_bytes(),
                output,
                count,
            );
            -30
        }
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub(super) fn http_failure(
    error: quotabar::provider_http::Failure,
    os_error: &mut i32,
    output: &mut *mut u8,
    count: &mut usize,
) -> i32 {
    use quotabar::provider_http::{Failure, TransportError};
    match error {
        Failure::Query(error) => failure(error, output, count),
        Failure::Transport(TransportError::Network(code)) => {
            *os_error = code;
            -40
        }
        Failure::Transport(TransportError::Cancelled) => -41,
        Failure::Transport(TransportError::Timeout) => -42,
        Failure::Transport(TransportError::LargeResponse) => -43,
    }
}

fn request(query: &Query, output: &mut *mut u8, count: &mut usize) -> i32 {
    let Ok(bytes) = serde_json::to_vec(&query.request()) else {
        return -1;
    };
    publish(bytes, output, count);
    if query.optional() { 1 } else { 2 }
}

/// # Safety
/// Inputs must be readable for the supplied length. All output locations must be
/// writable. One task owns the returned query and frees it after its final use.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_provider_query_start(
    op: i32,
    mac: i32,
    auth: *const u8,
    auth_len: usize,
    timeout: f64,
    handle: *mut *mut Query,
    output: *mut *mut u8,
    count: *mut usize,
) -> i32 {
    let (Some(handle), Some(output), Some(count)) = (
        unsafe { handle.as_mut() },
        unsafe { output.as_mut() },
        unsafe { count.as_mut() },
    ) else {
        return -1;
    };
    *handle = ptr::null_mut();
    *output = ptr::null_mut();
    *count = 0;
    if auth_len > isize::MAX as usize {
        return -2;
    }
    if auth.is_null() && auth_len != 0 {
        return -1;
    }
    let Some(op) = operation(op) else { return -1 };
    let platform = if mac == 1 {
        Platform::Mac
    } else {
        Platform::Windows
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let auth = if auth_len == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(auth, auth_len) }
        };
        match Query::new(op, auth, platform, timeout) {
            Ok(query) => {
                let status = request(&query, output, count);
                if status > 0 {
                    *handle = Box::into_raw(Box::new(query));
                }
                status
            }
            Err(error) => failure(error, output, count),
        }
    }))
    .unwrap_or(-1)
}

/// # Safety
/// The caller exclusively owns this live query. A negative status skips an
/// optional discovery request. Release each returned byte buffer using qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_provider_query_step(
    query: *mut Query,
    status: i32,
    body: *const u8,
    length: usize,
    now_micros: i64,
    output: *mut *mut u8,
    count: *mut usize,
) -> i32 {
    let (Some(query), Some(output), Some(count)) = (
        unsafe { query.as_mut() },
        unsafe { output.as_mut() },
        unsafe { count.as_mut() },
    ) else {
        return -1;
    };
    *output = ptr::null_mut();
    *count = 0;
    if body.is_null() && length != 0 {
        return -1;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if status == -1 {
            return if query.skip_discovery() {
                request(query, output, count)
            } else {
                -1
            };
        }
        let (Ok(status), Some(now)) = (
            u16::try_from(status),
            DateTime::from_timestamp_micros(now_micros),
        ) else {
            return -1;
        };
        let body = if length == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(body, length) }
        };
        match query.accept(status, body, now) {
            Ok(None) => request(query, output, count),
            Ok(Some(bytes)) => {
                publish(bytes, output, count);
                0
            }
            Err(error) => failure(error, output, count),
        }
    }))
    .unwrap_or(-1)
}

/// # Safety
/// Supply a pointer from qb_provider_query_start once, after its last step.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_provider_query_free(query: *mut Query) {
    if !query.is_null() {
        drop(unsafe { Box::from_raw(query) });
    }
}

/// # Safety
/// The input and atomic cancellation handle stay live until the worker returns.
/// Release a successful result or provider-error payload using qb_secret_free.
#[cfg(any(windows, target_os = "macos"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_provider_query_run(
    op: i32,
    auth: *const u8,
    auth_len: usize,
    timeout: f64,
    now_micros: i64,
    cancel: *const super::CancelHandle,
    os_error: *mut i32,
    output: *mut *mut u8,
    count: *mut usize,
) -> i32 {
    use quotabar::provider_http;
    let (Some(cancel), Some(os_error), Some(output), Some(count)) = (
        unsafe { cancel.as_ref() },
        unsafe { os_error.as_mut() },
        unsafe { output.as_mut() },
        unsafe { count.as_mut() },
    ) else {
        return -1;
    };
    *output = ptr::null_mut();
    *count = 0;
    *os_error = 0;
    if auth_len > isize::MAX as usize {
        return -2;
    }
    if auth.is_null() && auth_len != 0 {
        return -1;
    }
    let Some(op) = operation(op) else { return -1 };
    let platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Mac
    };
    let now = if now_micros == i64::MIN {
        None
    } else {
        let Some(now) = DateTime::from_timestamp_micros(now_micros) else {
            return -1;
        };
        Some(now)
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let auth = if auth_len == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(auth, auth_len) }
        };
        let query = match Query::new(op, auth, platform, timeout) {
            Ok(value) => value,
            Err(error) => return failure(error, output, count),
        };
        match provider_http::run(query, &cancel.0, now) {
            Ok(bytes) => {
                publish(bytes, output, count);
                0
            }
            Err(error) => http_failure(error, os_error, output, count),
        }
    }))
    .unwrap_or(-1)
}
