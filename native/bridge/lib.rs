//! C entry points for hosts using the shared Rust usage scanner.
use chrono::{DateTime, FixedOffset, Local, Utc};
use chrono_tz::Tz;
use quotabar::usage::{Report, Scanner};
use serde::Deserialize;
use std::{
    ffi::{CStr, CString, c_char},
    io,
    path::PathBuf,
    ptr,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

pub struct ScanHandle(Mutex<Scanner>);
pub struct CancelHandle(AtomicBool);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanRequest {
    codex_roots: Vec<PathBuf>,
    claude_roots: Vec<PathBuf>,
    history_days: i32,
    now_micros: i64,
    time_zone: Option<String>,
    offset_seconds: Option<i32>,
}

fn report(handle: &ScanHandle, cancel: &CancelHandle, request: &CStr) -> io::Result<Report> {
    let request: ScanRequest = serde_json::from_slice(request.to_bytes())?;
    let now: DateTime<Utc> = DateTime::from_timestamp_micros(request.now_micros)
        .ok_or_else(|| io::Error::other("Invalid scan timestamp"))?;
    let days = request.history_days.clamp(1, 366) as u16;
    let mut scanner = handle
        .0
        .lock()
        .map_err(|_| io::Error::other("Scan unavailable"))?;
    let (codex, claude) = (&request.codex_roots, &request.claude_roots);
    if let Some(name) = request.time_zone {
        let zone: Tz = name
            .parse()
            .map_err(|_| io::Error::other("Unknown time zone"))?;
        scanner.scan(codex, claude, now, &zone, days, &cancel.0)
    } else if let Some(seconds) = request.offset_seconds {
        let zone =
            FixedOffset::east_opt(seconds).ok_or_else(|| io::Error::other("Invalid UTC offset"))?;
        scanner.scan(codex, claude, now, &zone, days, &cancel.0)
    } else {
        scanner.scan(codex, claude, now, &Local, days, &cancel.0)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_scanner_new() -> *mut ScanHandle {
    Box::into_raw(Box::new(ScanHandle(Mutex::new(Scanner::default()))))
}
/// # Safety
/// Pass one live handle returned by qb_scanner_new, after all calls on it finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_scanner_free(handle: *mut ScanHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}
/// # Safety
/// The handle must stay alive for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_scanner_release_cache(handle: *const ScanHandle) {
    if let Some(handle) = unsafe { handle.as_ref() }
        && let Ok(mut scanner) = handle.0.lock()
    {
        scanner.release_cache();
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn qb_cancel_new() -> *mut CancelHandle {
    Box::into_raw(Box::new(CancelHandle(AtomicBool::new(false))))
}
/// # Safety
/// Pass one live handle returned by qb_cancel_new, after the scan and its callbacks finish.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_cancel_free(handle: *mut CancelHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}
/// # Safety
/// The handle must stay alive for this call. It may be in use by a scan.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_cancel_set(handle: *const CancelHandle) {
    if let Some(cancel) = unsafe { handle.as_ref() } {
        cancel.0.store(true, Ordering::Relaxed);
    }
}
/// # Safety
/// The handles and NUL-terminated UTF-8 request must stay alive until return.
/// On success, output receives an owned JSON string to release with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_scanner_scan(
    scanner: *const ScanHandle,
    cancel: *const CancelHandle,
    request: *const c_char,
    output: *mut *mut c_char,
) -> i32 {
    let (Some(scanner), Some(cancel), Some(output)) = (
        unsafe { scanner.as_ref() },
        unsafe { cancel.as_ref() },
        unsafe { output.as_mut() },
    ) else {
        return -1;
    };
    *output = ptr::null_mut();
    if request.is_null() {
        return -1;
    }
    let run = std::panic::catch_unwind(|| -> io::Result<CString> {
        let report = report(scanner, cancel, unsafe { CStr::from_ptr(request) })?;
        Ok(CString::new(serde_json::to_vec(&report)?)?)
    });
    match run {
        Ok(Ok(result)) => {
            *output = result.into_raw();
            0
        }
        Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => 1,
        _ => -1,
    }
}
/// # Safety
/// Pass a string returned by qb_scanner_scan exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}
