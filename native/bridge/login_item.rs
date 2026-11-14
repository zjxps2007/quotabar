#[cfg(windows)]
use std::{
    ffi::{CStr, c_char},
    path::Path,
};

/// # Safety
/// Supply two NUL-terminated UTF-8 strings. Op 0 reads, 1 enables, 2 disables.
/// State codes are shared with the macOS entry points. Negative values indicate an error.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_login_item(
    exe: *const c_char,
    key: *const c_char,
    op: i32,
) -> i32 {
    use quotabar::login_item::windows;
    if exe.is_null() || key.is_null() {
        return -1;
    }
    std::panic::catch_unwind(|| {
        let (Ok(exe), Ok(key)) = (
            unsafe { CStr::from_ptr(exe) }.to_str(),
            unsafe { CStr::from_ptr(key) }.to_str(),
        ) else {
            return -1;
        };
        let (exe, key) = (Path::new(exe), Path::new(key));
        match op {
            0 => windows::state(exe, key).map_or(-1, |s| s as i32),
            1 | 2 => match windows::set(exe, key, op == 1) {
                Ok(()) => 0,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => -2,
                Err(_) => -1,
            },
            _ => -1,
        }
    })
    .unwrap_or(-1)
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_login_item(op: i32) -> i32 {
    use quotabar::login_item::macos;
    std::panic::catch_unwind(|| match op {
        0 => macos::state() as i32,
        1 | 2 => macos::set(op == 1).map_or(-1, |()| 0),
        3 => {
            macos::open_settings();
            0
        }
        _ => -1,
    })
    .unwrap_or(-1)
}
