use quotabar::vault;
use std::{
    ffi::{CStr, c_char},
    io,
    path::Path,
    ptr, slice,
};
use uuid::Uuid;
use zeroize::Zeroize;

fn call(
    folder: *const c_char,
    id: *const c_char,
    action: impl FnOnce(&Path, Uuid) -> io::Result<i32>,
) -> i32 {
    if folder.is_null() || id.is_null() {
        return -1;
    }
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> io::Result<i32> {
        let folder = unsafe { CStr::from_ptr(folder) }
            .to_str()
            .map_err(io::Error::other)?;
        let id = unsafe { CStr::from_ptr(id) }
            .to_str()
            .map_err(io::Error::other)?;
        action(
            Path::new(folder),
            Uuid::parse_str(id).map_err(io::Error::other)?,
        )
    }))
    .ok()
    .and_then(Result::ok)
    .unwrap_or(-1)
}

/// # Safety
/// Supply NUL-terminated UTF-8 strings, an output pointer and a length. Release
/// the returned buffer with qb_secret_free after copying it into host-owned storage.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_vault_load(
    folder: *const c_char,
    id: *const c_char,
    output: *mut *mut u8,
    length: *mut usize,
) -> i32 {
    let (Some(output), Some(length)) = (unsafe { output.as_mut() }, unsafe { length.as_mut() })
    else {
        return -1;
    };
    *output = ptr::null_mut();
    *length = 0;
    call(folder, id, |folder, id| {
        let Some(data) = vault::load(folder, id)? else {
            return Ok(1);
        };
        let bytes: Box<[u8]> = data.as_slice().into();
        *length = bytes.len();
        *output = Box::into_raw(bytes).cast::<u8>();
        Ok(0)
    })
}
/// # Safety
/// Supply NUL-terminated UTF-8 strings and length readable bytes, or null for zero bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_vault_save(
    folder: *const c_char,
    id: *const c_char,
    data: *const u8,
    length: usize,
) -> i32 {
    if length > vault::MAX_SIZE || (data.is_null() && length != 0) {
        return -1;
    }
    call(folder, id, |folder, id| {
        let bytes = if length == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(data, length) }
        };
        vault::save(folder, id, bytes)?;
        Ok(0)
    })
}
/// # Safety
/// Both strings must be valid NUL-terminated UTF-8 for the duration of the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_vault_delete(folder: *const c_char, id: *const c_char) -> i32 {
    call(folder, id, |folder, id| {
        vault::delete(folder, id)?;
        Ok(0)
    })
}
/// # Safety
/// Release each buffer from qb_vault_load exactly once with its original length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_secret_free(data: *mut u8, length: usize) {
    if !data.is_null() {
        let mut bytes = unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(data, length)) };
        bytes.zeroize();
    }
}
