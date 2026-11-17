use quotabar::mac_login::{self, Attempt, Progress};
use std::{
    ffi::{CStr, CString, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    ptr,
    time::Duration,
};
use zeroize::Zeroizing;

fn input<'a>(value: *const c_char) -> Option<&'a str> {
    if value.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(value) }.to_str().ok()
}
/// # Safety
/// Supply valid C strings and release a returned string with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_find_codex(
    path: *const c_char,
    home: *const c_char,
) -> *mut c_char {
    catch_unwind(|| {
        let result = mac_login::find(input(path)?, Path::new(input(home)?))?;
        CString::new(result.to_str()?).ok()
    })
    .ok()
    .flatten()
    .map_or(ptr::null_mut(), CString::into_raw)
}

/// # Safety
/// Supply a valid UTF-8 string; release a returned string with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_login_url(text: *const c_char) -> *mut c_char {
    catch_unwind(|| CString::new(mac_login::authorization_url(input(text)?)?).ok())
        .ok()
        .flatten()
        .map_or(ptr::null_mut(), CString::into_raw)
}
/// # Safety
/// Supply two NUL-terminated UTF-8 paths and release the attempt exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_login_start(
    executable: *const c_char,
    temporary: *const c_char,
    seconds: f64,
) -> *mut Attempt {
    catch_unwind(|| {
        let duration = if seconds == f64::INFINITY {
            Duration::MAX
        } else {
            Duration::try_from_secs_f64(seconds.max(0.)).unwrap_or(Duration::MAX)
        };
        Attempt::start(
            Path::new(input(executable)?),
            Path::new(input(temporary)?),
            duration,
        )
        .ok()
    })
    .ok()
    .flatten()
    .map_or(ptr::null_mut(), |attempt| Box::into_raw(Box::new(attempt)))
}
/// # Safety
/// Pass an exclusively owned live handle and two valid output locations.
/// Release outputs (URL or credential) with qb_secret_free and the exact length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_login_poll(
    handle: *mut Attempt,
    output: *mut *mut u8,
    length: *mut usize,
) -> i32 {
    let (Some(handle), Some(output), Some(length)) = (
        unsafe { handle.as_mut() },
        unsafe { output.as_mut() },
        unsafe { length.as_mut() },
    ) else {
        return -1;
    };
    *output = ptr::null_mut();
    *length = 0;
    match catch_unwind(AssertUnwindSafe(|| handle.poll())) {
        Ok(Ok(event)) => {
            let (kind, data) = match event {
                Progress::Waiting => return 0,
                Progress::TimedOut => return -2,
                Progress::Failed => return -1,
                Progress::Authorized(url) => (1, Zeroizing::new(url.into_bytes())),
                Progress::Finished(bytes) => (2, bytes),
            };
            let bytes: Box<[u8]> = data.as_slice().into();
            *length = bytes.len();
            *output = Box::into_raw(bytes).cast();
            kind
        }
        _ => -1,
    }
}
/// # Safety
/// Release the handle once after polling completes, on its owning thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_login_close(handle: *mut Attempt, immediately: i32) {
    if !handle.is_null() {
        unsafe { Box::from_raw(handle) }.close(immediately != 0);
    }
}
