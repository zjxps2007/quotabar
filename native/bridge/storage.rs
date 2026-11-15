use quotabar::files;
#[cfg(windows)]
use std::ptr;
use std::{
    ffi::{CStr, c_char},
    io,
    path::Path,
    slice,
};

fn status(path: *const c_char, operation: impl FnOnce(&Path) -> io::Result<()>) -> i32 {
    if path.is_null() {
        return -1;
    }
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        return -1;
    };
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(Path::new(path))));
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(error)) => error.raw_os_error().unwrap_or(-1),
        Err(_) => -1,
    }
}

/// # Safety
/// Supply a NUL-terminated UTF-8 path and a buffer of length bytes (null is valid for zero).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_private_write(
    path: *const c_char,
    data: *const u8,
    length: usize,
) -> i32 {
    if length > isize::MAX as usize || (data.is_null() && length != 0) {
        return -1;
    }
    let bytes = if length == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(data, length) }
    };
    status(path, |p| files::write_private(p, bytes))
}

/// # Safety
/// Supply a UTF-8 path and output locations. Release a successful read with qb_secret_free.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_file_read(
    path: *const c_char,
    limit: usize,
    output: *mut *mut u8,
    length: *mut usize,
) -> i32 {
    let (Some(output), Some(length)) = (unsafe { output.as_mut() }, unsafe { length.as_mut() })
    else {
        return -1;
    };
    *output = ptr::null_mut();
    *length = 0;
    if limit > i32::MAX as usize {
        return -1;
    }
    status(path, |p| {
        let bytes = files::read_file(p, limit)?.into_boxed_slice();
        *length = bytes.len();
        *output = Box::into_raw(bytes).cast();
        Ok(())
    })
}

/// # Safety
/// The path must be a readable NUL-terminated UTF-8 string.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_private_directory(path: *const c_char) -> i32 {
    status(path, files::ensure_private_directory)
}

/// # Safety
/// The path must be a readable NUL-terminated UTF-8 string.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_reject_links(path: *const c_char) -> i32 {
    status(path, files::reject_links)
}
