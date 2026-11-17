use super::{
    CancelHandle, codex_sessions::status, credentials::publish, provider_query::http_failure,
};
use quotabar::{
    codex_sessions::{
        Secret,
        managed::{FetchError, ManagedAccounts},
    },
    credentials::Platform,
    provider_http::{self, Reply, TransportError},
};
use std::{
    ffi::{CStr, c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr,
};
use uuid::Uuid;
use zeroize::Zeroizing;

/// Optional transport for callers supplying their own HTTP handler. It runs on
/// the fetch worker and fills the body receiver with qb_sessions_loaded.
type SendHttp =
    unsafe extern "C" fn(*mut c_void, *const u8, usize, i32, *mut Option<Secret>) -> i32;

/// # Safety
/// The directory must be a NUL-terminated UTF-8 path.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_managed_accounts_new(directory: *const c_char) -> *mut ManagedAccounts {
    if directory.is_null() {
        return ptr::null_mut();
    }
    let Ok(directory) = unsafe { CStr::from_ptr(directory) }.to_str() else {
        return ptr::null_mut();
    };
    let platform = if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Mac
    };
    Box::into_raw(Box::new(ManagedAccounts::new(
        PathBuf::from(directory),
        platform,
    )))
}

/// # Safety
/// Release the owner once all of its calls have returned.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_managed_accounts_free(handle: *mut ManagedAccounts) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

unsafe fn uuid(value: *const c_char) -> Option<Uuid> {
    if value.is_null() {
        return None;
    }
    Uuid::try_parse_ascii(unsafe { CStr::from_ptr(value) }.to_bytes()).ok()
}

/// # Safety
/// The handle and NUL-terminated UUID remain valid for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_managed_accounts_invalidate(
    handle: *const ManagedAccounts,
    id: *const c_char,
) {
    if let (Some(handle), Some(id)) = (unsafe { handle.as_ref() }, unsafe { uuid(id) }) {
        handle.invalidate(id);
    }
}

/// # Safety
/// The owner must remain live until all exchanges and pending vault writes finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_managed_accounts_flush(handle: *const ManagedAccounts) -> i32 {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    catch_unwind(AssertUnwindSafe(|| {
        handle.flush().map_or_else(status, |()| 0)
    }))
    .unwrap_or(-1)
}

/// # Safety
/// All inputs and optional callback must remain valid for this synchronous call.
/// Release returned quota or provider-error data with qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_managed_accounts_fetch(
    handle: *const ManagedAccounts,
    id: *const c_char,
    identity: *const u8,
    identity_length: usize,
    cancel: *const CancelHandle,
    context: *mut c_void,
    send: Option<SendHttp>,
    os_error: *mut i32,
    output: *mut *mut u8,
    count: *mut usize,
) -> i32 {
    let (Some(os_error), Some(output), Some(count)) = (
        unsafe { os_error.as_mut() },
        unsafe { output.as_mut() },
        unsafe { count.as_mut() },
    ) else {
        return -1;
    };
    *os_error = 0;
    *output = ptr::null_mut();
    *count = 0;
    let (Some(handle), Some(id), Some(cancel)) =
        (unsafe { handle.as_ref() }, unsafe { uuid(id) }, unsafe {
            cancel.as_ref()
        })
    else {
        return -1;
    };
    if identity_length > isize::MAX as usize || identity.is_null() && identity_length != 0 {
        return -1;
    }
    let key = if identity_length == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(identity, identity_length) }
    };
    let Ok(identity) = std::str::from_utf8(key) else {
        return -1;
    };
    catch_unwind(AssertUnwindSafe(|| {
        let result = if let Some(send) = send {
            handle.fetch_with(id, identity, &cancel.0, |query, flag, now| {
                provider_http::drive(query, flag, now, |request, oauth, _, _| {
                    let request = Zeroizing::new(
                        serde_json::to_vec(&request).map_err(|_| TransportError::Network(0))?,
                    );
                    let mut body = None;
                    let http = unsafe {
                        send(
                            context,
                            request.as_ptr(),
                            request.len(),
                            i32::from(oauth),
                            &mut body,
                        )
                    };
                    let status = u16::try_from(http).map_err(|_| match http {
                        -41 => TransportError::Cancelled,
                        -42 => TransportError::Timeout,
                        -43 => TransportError::LargeResponse,
                        _ => TransportError::Network(0),
                    })?;
                    Ok(Reply {
                        status,
                        body: body.unwrap_or_default(),
                    })
                })
            })
        } else {
            handle.fetch(id, identity, &cancel.0)
        };
        match result {
            Ok(bytes) => {
                publish(bytes, output, count);
                0
            }
            Err(FetchError::Provider(error)) => http_failure(error, os_error, output, count),
            Err(FetchError::Session(error)) => status(error),
            Err(FetchError::Internal) => -1,
        }
    }))
    .unwrap_or(-1)
}
