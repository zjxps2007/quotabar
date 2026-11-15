//! C entry points used by the platform application hosts.
#[cfg(windows)]
mod claude;
mod codex_sessions;
mod credentials;
#[cfg(windows)]
mod dashboard;
mod frames;
#[cfg(windows)]
mod instance;
mod login_item;
#[cfg(any(target_os = "macos", feature = "mac-settings-compat"))]
mod mac_claude;
#[cfg(target_os = "macos")]
mod mac_login;
#[cfg(target_os = "macos")]
mod mac_menu;
#[cfg(any(target_os = "macos", feature = "mac-settings-compat"))]
mod mac_refresh;
#[cfg(any(target_os = "macos", feature = "mac-settings-compat"))]
mod mac_settings;
#[cfg(any(target_os = "macos", feature = "mac-settings-compat"))]
mod mac_snapshots;
#[cfg(any(windows, target_os = "macos"))]
mod managed_accounts;
#[cfg(windows)]
mod menu;
#[cfg(windows)]
mod monitor_loop;
#[cfg(windows)]
mod notification_icon;
mod provider_query;
mod releases;
mod schedule;
mod storage;
mod update_monitor;
mod vault;
#[cfg(windows)]
mod windows_installer;
#[cfg(windows)]
mod windows_login;
#[cfg(windows)]
mod windows_operation;
#[cfg(windows)]
mod windows_settings;
#[cfg(windows)]
#[unsafe(no_mangle)]
/// Draws straight-alpha BGRA into the caller's square icon buffer.
/// # Safety
/// Supply NUL-terminated UTF-8 text and a writable buffer of exactly length bytes.
pub unsafe extern "C" fn qb_tray_render(
    text: *const c_char,
    size: i32,
    logo: i32,
    warning: i32,
    light: i32,
    output: *mut u8,
    length: usize,
) -> i32 {
    if text.is_null()
        || output.is_null()
        || !(1..=256).contains(&size)
        || length != size as usize * size as usize * 4
    {
        return -1;
    }
    let rendered = std::panic::catch_unwind(|| {
        let label = unsafe { CStr::from_ptr(text) }
            .to_str()
            .map_err(io::Error::other)?;
        quotabar::tray_icon::render(label, logo != 0, warning != 0, size, light != 0)
    });
    match rendered {
        Ok(Ok(pixels)) => {
            unsafe {
                ptr::copy_nonoverlapping(pixels.as_ptr(), output, length);
            }
            0
        }
        _ => -1,
    }
}
use chrono::{DateTime, FixedOffset, Local, Utc};
use chrono_tz::Tz;
use quotabar::quota;
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
/// The request must be a NUL-terminated UTF-8 JSON object. Release the
/// result with qb_string_free. A null result indicates an invalid response.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_parse_quota(request: *const c_char) -> *mut c_char {
    json_response(request, |bytes| {
        let request = serde_json::from_slice(bytes)?;
        let format = if cfg!(windows) {
            quota::Format::Windows
        } else {
            quota::Format::Mac
        };
        Ok(serde_json::to_vec(&quota::parse(
            &request,
            format,
            Utc::now(),
        )?)?)
    })
}

/// # Safety
/// The NUL-terminated UTF-8 input must remain valid until return. Release the
/// returned summary with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_tray_summary(request: *const c_char) -> *mut c_char {
    json_response(request, |bytes| {
        let state = serde_json::from_slice(bytes)?;
        if cfg!(windows) {
            Ok(serde_json::to_vec(&quotabar::tray::windows(&state))?)
        } else {
            Ok(serde_json::to_vec(&quotabar::tray::mac(&state))?)
        }
    })
}

fn json_response(
    request: *const c_char,
    parse: impl FnOnce(&[u8]) -> io::Result<Vec<u8>> + std::panic::UnwindSafe,
) -> *mut c_char {
    if request.is_null() {
        return ptr::null_mut();
    }
    std::panic::catch_unwind(|| -> io::Result<CString> {
        Ok(CString::new(parse(
            unsafe { CStr::from_ptr(request) }.to_bytes(),
        )?)?)
    })
    .ok()
    .and_then(Result::ok)
    .map_or(ptr::null_mut(), CString::into_raw)
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
