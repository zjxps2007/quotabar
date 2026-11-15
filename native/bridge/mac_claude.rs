use quotabar::claude::mac_connection;
use std::{
    ffi::{CStr, CString, c_char},
    path::Path,
    ptr, slice,
};

/// # Safety
/// The profile root is NUL-terminated UTF-8. The JSON request is readable for
/// length bytes and output is writable. Free a successful reply with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_claude(
    root: *const c_char,
    request: *const u8,
    length: usize,
    output: *mut *mut c_char,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = ptr::null_mut();
    if root.is_null() || request.is_null() || length > isize::MAX as usize {
        return -1;
    }
    match std::panic::catch_unwind(|| {
        let root = unsafe { CStr::from_ptr(root) }.to_str().map_err(|_| -1)?;
        let request = unsafe { slice::from_raw_parts(request, length) };
        let reply = mac_connection::run(Path::new(root), request).map_err(|e| e.status())?;
        CString::new(reply).map_err(|_| -1)
    }) {
        Ok(Ok(reply)) => {
            *output = reply.into_raw();
            0
        }
        Ok(Err(code)) => code,
        Err(_) => -1,
    }
}
