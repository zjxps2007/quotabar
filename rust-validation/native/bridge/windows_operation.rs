use std::{
    ffi::{CStr, CString, c_char},
    path::Path,
    ptr, slice,
};

pub(super) type Reply = Result<Vec<u8>, (i32, String)>;

/// The caller supplies a UTF-8 root and two readable buffers. The caller owns the
/// returned UTF-8 string and releases it with qb_string_free, including on errors.
pub(super) unsafe fn invoke(
    root: *const c_char,
    request: (*const u8, usize),
    input: (*const u8, usize),
    limits: (usize, usize),
    output: *mut *mut c_char,
    operation: impl FnOnce(&Path, &[u8], &[u8]) -> Reply + std::panic::UnwindSafe,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = ptr::null_mut();
    if root.is_null()
        || request.0.is_null()
        || request.1 > limits.0
        || input.1 > limits.1
        || (input.0.is_null() && input.1 != 0)
    {
        return -1;
    }
    let result = std::panic::catch_unwind(|| {
        let root = unsafe { CStr::from_ptr(root) }
            .to_str()
            .map_err(|e| (-1, e.to_string()))?;
        let request = unsafe { slice::from_raw_parts(request.0, request.1) };
        let input = if input.1 == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(input.0, input.1) }
        };
        operation(Path::new(root), request, input)
    });
    let (code, bytes) = match result {
        Ok(Ok(bytes)) => (0, bytes),
        Ok(Err((code, message))) => (code, message.into_bytes()),
        Err(_) => return -1,
    };
    if let Ok(s) = CString::new(bytes) {
        *output = s.into_raw();
        code
    } else {
        -1
    }
}
