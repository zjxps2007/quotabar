use super::{Verification, release_platform};
use crate::CancelHandle;
use quotabar::updates::FeedError;
use quotabar::updates::download::{
    self, Error, Resource, Transfer,
    windows::{Response, TIMEOUT, fetch_with},
};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    ptr, slice,
};

type Send = extern "C" fn(*const c_char, *const c_char, i32, *mut c_void) -> i32;
type Progress = extern "C" fn(i32);

struct Reader<'a, 'b> {
    response: &'a mut Response<'b>,
    error: Option<Error>,
}

fn fetch(transfer: Transfer<'_>, send: Option<Send>) -> Result<bool, Error> {
    let Some(send) = send else {
        #[cfg(any(windows, target_os = "macos"))]
        return transfer.send();
        #[cfg(not(any(windows, target_os = "macos")))]
        return Err(Error::Network(0));
    };
    let agent = CString::new(transfer.agent).map_err(|_| Error::Development)?;
    let feed = i32::from(transfer.resource == Resource::Feed);
    fetch_with(transfer, |url, response| {
        let url = CString::new(url.as_str()).map_err(|_| Error::Network(0))?;
        let mut reader = Reader {
            response,
            error: None,
        };
        let status = send(url.as_ptr(), agent.as_ptr(), feed, (&raw mut reader).cast());
        if let Some(error) = reader.error {
            return Err(error);
        }
        if status != 0 {
            return Err(Error::Network(0));
        }
        Ok(())
    })
}

/// # Safety
/// The receiver is borrowed only during the synchronous custom transport call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_release_wait_ms(receiver: *mut c_void) -> i32 {
    unsafe { receiver.cast::<Reader>().as_ref() }.map_or(0, |r| {
        r.response.remaining().as_millis().min(i32::MAX as u128) as i32
    })
}

/// # Safety
/// The receiver is live and the optional location is a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_release_headers(
    receiver: *mut c_void,
    status: u16,
    length: i64,
    location: *const c_char,
) -> i32 {
    let Some(reader) = (unsafe { receiver.cast::<Reader>().as_mut() }) else {
        return -1;
    };
    let location = if location.is_null() {
        None
    } else {
        unsafe { CStr::from_ptr(location) }.to_str().ok()
    };
    match reader
        .response
        .headers(status, length.try_into().ok(), location)
    {
        Ok(proceed) => i32::from(proceed),
        Err(error) => {
            reader.error = Some(error);
            -1
        }
    }
}

/// # Safety
/// The live receiver and the chunk of at most 64 KiB are borrowed for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_release_chunk(
    receiver: *mut c_void,
    bytes: *const u8,
    len: usize,
) -> i32 {
    if bytes.is_null() || len > 65536 {
        return -1;
    }
    let Some(reader) = (unsafe { receiver.cast::<Reader>().as_mut() }) else {
        return -1;
    };
    match reader
        .response
        .write(unsafe { slice::from_raw_parts(bytes, len) })
    {
        Ok(()) => 0,
        Err(error) => {
            reader.error = Some(error);
            -1
        }
    }
}

unsafe fn result<T: serde::Serialize>(
    out: *mut *mut c_char,
    os: *mut i32,
    operation: impl FnOnce() -> Result<T, Error>,
) -> i32 {
    let (Some(out), Some(os)) = (unsafe { out.as_mut() }, unsafe { os.as_mut() }) else {
        return -1;
    };
    *out = ptr::null_mut();
    *os = 0;
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(value)) => match serde_json::to_vec(&value)
            .ok()
            .and_then(|v| CString::new(v).ok())
        {
            Some(json) => {
                *out = json.into_raw();
                0
            }
            None => -1,
        },
        Ok(Err(error)) => match error {
            Error::Development => 1,
            Error::RateLimited => 2,
            Error::Feed(FeedError::InvalidRelease) => 3,
            Error::Feed(FeedError::MalformedJson) => 10,
            Error::Feed(FeedError::MissingProperty) => 11,
            Error::Feed(FeedError::WrongType) => 12,
            Error::Download => 4,
            Error::Signature => 5,
            Error::Cancelled => 6,
            Error::Network(TIMEOUT) => 7,
            Error::Network(hr) => {
                *os = hr;
                8
            }
            Error::File(error) => {
                *os = error.raw_os_error().unwrap_or(0);
                9
            }
        },
        Err(_) => -1,
    }
}

/// # Safety
/// The UTF-8 version, cancellation handle and optional transport stay live for this call.
/// Release successful JSON with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_release_latest(
    platform: i32,
    version: *const c_char,
    cancel: *const CancelHandle,
    send: Option<Send>,
    out: *mut *mut c_char,
    os: *mut i32,
) -> i32 {
    unsafe {
        result(out, os, || {
            let platform =
                release_platform(platform).ok_or(Error::Feed(FeedError::InvalidRelease))?;
            if version.is_null() {
                return Err(Error::Development);
            }
            let version = CStr::from_ptr(version)
                .to_str()
                .map_err(|_| Error::Development)?;
            let cancel = cancel.as_ref().ok_or(Error::Cancelled)?;
            download::latest_with(platform, version, &cancel.0, |t| fetch(t, send))
        })
    }
}

/// # Safety
/// Feed bytes are borrowed for this call. Release the result with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_release_parse(
    platform: i32,
    data: *const u8,
    len: usize,
    out: *mut *mut c_char,
) -> i32 {
    unsafe {
        result(out, &mut 0, || {
            if len > quotabar::updates::MAX_FEED {
                return Err(Error::Feed(FeedError::InvalidRelease));
            }
            let bytes = if len == 0 {
                &[]
            } else if data.is_null() {
                return Err(Error::Feed(FeedError::MalformedJson));
            } else {
                slice::from_raw_parts(data, len)
            };
            let platform =
                release_platform(platform).ok_or(Error::Feed(FeedError::InvalidRelease))?;
            quotabar::updates::parse_feed(bytes, platform).map_err(Error::Feed)
        })
    }
}

/// # Safety
/// Metadata, path, cancellation and callbacks are borrowed until staging returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_release_stage(
    metadata: *const u8,
    length: usize,
    directory: *const c_char,
    cancel: *const CancelHandle,
    send: Option<Send>,
    progress: Progress,
    out: *mut *mut c_char,
    os: *mut i32,
) -> i32 {
    unsafe {
        result(out, os, || {
            if metadata.is_null() || length > quotabar::updates::MAX_FEED || directory.is_null() {
                return Err(Error::Signature);
            }
            let plan: Verification =
                serde_json::from_slice(slice::from_raw_parts(metadata, length))
                    .map_err(|_| Error::Signature)?;
            let path = CStr::from_ptr(directory)
                .to_str()
                .map_err(|_| Error::Download)?;
            let cancel = cancel.as_ref().ok_or(Error::Cancelled)?;
            download::stage_with(
                &plan.release,
                &plan.current_version,
                &plan.public_key,
                Path::new(path),
                &cancel.0,
                |v| progress(v),
                |t| fetch(t, send),
            )
        })
    }
}
