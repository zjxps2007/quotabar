use quotabar::claude::mac_snapshots::Snapshot;
use std::{
    ffi::{CStr, CString, c_char},
    io,
    path::Path,
    ptr, slice,
};

unsafe fn text<'a>(value: *const c_char) -> io::Result<&'a str> {
    if value.is_null() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    unsafe { CStr::from_ptr(value) }
        .to_str()
        .map_err(io::Error::other)
}

unsafe fn bytes<'a>(data: *const u8, len: usize) -> io::Result<&'a [u8]> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() || len > isize::MAX as usize {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    Ok(unsafe { slice::from_raw_parts(data, len) })
}

fn json(value: Option<Snapshot>) -> io::Result<*mut c_char> {
    Ok(CString::new(serde_json::to_vec(&value)?)?.into_raw())
}

/// # Safety
/// The identifiers are NUL-terminated UTF-8. Input and output remain valid until return.
/// Free a successful JSON result, including JSON null, with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_snapshot_parse(
    id: *const c_char,
    generation: *const c_char,
    now: f64,
    data: *const u8,
    len: usize,
    output: *mut *mut c_char,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = ptr::null_mut();
    if len > 2 * 1024 * 1024 {
        return -2;
    }
    match std::panic::catch_unwind(|| unsafe {
        json(Snapshot::parse(
            bytes(data, len)?,
            text(id)?,
            text(generation)?,
            now,
        )?)
    }) {
        Ok(Ok(result)) => {
            *output = result;
            0
        }
        Ok(Err(error)) if error.get_ref().is_some_and(|v| v.is::<serde_json::Error>()) => -3,
        _ => -1,
    }
}

/// # Safety
/// Supply NUL-terminated UTF-8 strings and free the returned JSON with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_snapshot_latest(
    folder: *const c_char,
    id: *const c_char,
    generation: *const c_char,
    now: f64,
) -> *mut c_char {
    std::panic::catch_unwind(|| unsafe {
        json(Snapshot::latest(
            Path::new(text(folder)?),
            text(id)?,
            text(generation)?,
            now,
        ))
    })
    .ok()
    .and_then(Result::ok)
    .unwrap_or(ptr::null_mut())
}

/// # Safety
/// The UTF-8 directory and Foundation-encoded snapshot must remain readable until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_snapshot_save(
    folder: *const c_char,
    data: *const u8,
    len: usize,
) -> i32 {
    match std::panic::catch_unwind(|| -> io::Result<()> {
        let sample: Snapshot = serde_json::from_slice(unsafe { bytes(data, len)? })?;
        sample.save(Path::new(unsafe { text(folder)? }))
    }) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => e.raw_os_error().unwrap_or(-1),
        Err(_) => -1,
    }
}
