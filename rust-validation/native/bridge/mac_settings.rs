use quotabar::mac_settings::imported::{self, Import, Selection};
use quotabar::mac_settings::{self, Error};
use quotabar::{
    codex_sessions::Secret,
    mac_settings::migration::{self, LegacyItems},
};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::Path,
    ptr, slice,
};
use uuid::Uuid;

type LegacyCallback = unsafe extern "C" fn(*mut c_void, *const c_char, *mut Option<Secret>);
struct LegacyAdapter(*mut c_void, LegacyCallback);

impl LegacyItems for LegacyAdapter {
    fn read(&mut self, id: Uuid) -> Option<Secret> {
        let mut name = [0; 37];
        id.hyphenated().encode_upper(&mut name);
        let mut data = None;
        unsafe {
            (self.1)(self.0, name.as_ptr().cast(), &mut data);
        }
        data
    }
    fn delete(&mut self, id: Uuid) {
        let mut name = [0; 37];
        id.hyphenated().encode_upper(&mut name);
        unsafe {
            (self.1)(self.0, name.as_ptr().cast(), ptr::null_mut());
        }
    }
}

/// # Safety
/// Paths are NUL-terminated UTF-8. The caller owns the callback context until
/// return and releases successful settings JSON with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_migrate(
    path: *const c_char,
    directory: *const c_char,
    data: *const u8,
    len: usize,
    context: *mut c_void,
    callback: Option<LegacyCallback>,
    output: *mut *mut c_char,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = ptr::null_mut();
    if path.is_null() || directory.is_null() {
        return -1;
    }
    let result = std::panic::catch_unwind(|| unsafe {
        let path = Path::new(CStr::from_ptr(path).to_str().map_err(|_| -1)?);
        let directory = Path::new(CStr::from_ptr(directory).to_str().map_err(|_| -1)?);
        let settings = bytes(data, len)?;
        let result = match callback {
            Some(callback) => migration::run(
                settings,
                path,
                directory,
                &mut LegacyAdapter(context, callback),
            ),
            #[cfg(target_os = "macos")]
            None => migration::run(
                settings,
                path,
                directory,
                &mut migration::keychain::Keychain,
            ),
            #[cfg(not(target_os = "macos"))]
            None => return Err(-1),
        }
        .map_err(code)?;
        CString::new(result).map_err(|_| -1)
    });
    match result {
        Ok(Ok(reply)) => {
            *output = reply.into_raw();
            0
        }
        Ok(Err(code)) => code,
        Err(_) => -1,
    }
}

#[cfg(target_os = "macos")]
unsafe fn legacy_id(id: *const c_char) -> Option<Uuid> {
    if id.is_null() {
        return None;
    }
    Uuid::try_parse_ascii(unsafe { CStr::from_ptr(id) }.to_bytes()).ok()
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_legacy_read(
    id: *const c_char,
    value: *mut *mut u8,
    length: *mut usize,
) -> i32 {
    let (Some(value), Some(length)) = (unsafe { value.as_mut() }, unsafe { length.as_mut() })
    else {
        return -1;
    };
    *value = ptr::null_mut();
    *length = 0;
    let Some(id) = (unsafe { legacy_id(id) }) else {
        return -1;
    };
    let Some(secret) = migration::keychain::Keychain.read(id) else {
        return 1;
    };
    super::credentials::publish_secret(&secret, value, length);
    0
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_legacy_delete(id: *const c_char) {
    if let Some(id) = unsafe { legacy_id(id) } {
        migration::keychain::Keychain.delete(id);
    }
}

/// Paths and account JSON are valid until return. Successful data is owned
/// by the caller and is released through qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_imported(
    directory: *const c_char,
    account: *const u8,
    account_len: usize,
    output: *mut *mut u8,
    length: *mut usize,
) -> i32 {
    let (Some(output), Some(length)) = (unsafe { output.as_mut() }, unsafe { length.as_mut() })
    else {
        return -1;
    };
    *output = ptr::null_mut();
    *length = 0;
    if directory.is_null() {
        return -24;
    }
    let result = std::panic::catch_unwind(|| unsafe {
        let directory = CStr::from_ptr(directory).to_str().map_err(|_| -24)?;
        let account: Import =
            serde_json::from_slice(bytes(account, account_len)?).map_err(|_| -20)?;
        account
            .select(Path::new(directory))
            .map_err(|error| match error {
                imported::Error::Vault => -24,
                imported::Error::Missing => -50,
                imported::Error::Mismatch => -51,
                imported::Error::Credential(error) => super::credentials::error_code(error),
            })
    });
    match result {
        Ok(Ok((data, selection))) => {
            super::credentials::publish_secret(&data, output, length);
            match selection {
                Selection::Stored => 0,
                Selection::Updated => 1,
                Selection::OtherAccount => 2,
            }
        }
        Ok(Err(code)) => code,
        Err(_) => -1,
    }
}

unsafe fn bytes<'a>(data: *const u8, len: usize) -> Result<&'a [u8], i32> {
    if len == 0 {
        return Ok(&[]);
    }
    if data.is_null() || len > isize::MAX as usize {
        return Err(-1);
    }
    Ok(unsafe { slice::from_raw_parts(data, len) })
}

fn code(error: Error) -> i32 {
    match error {
        Error::Json => -20,
        Error::Claude(e) => e.status(),
        Error::Unavailable => -21,
        Error::WrongAccount => -22,
        Error::MissingRefresh => -23,
        Error::Vault => -24,
        Error::LargeImport => -25,
        Error::Import(e) => match e.kind() {
            std::io::ErrorKind::NotFound => -26,
            std::io::ErrorKind::PermissionDenied => -27,
            std::io::ErrorKind::InvalidInput => -28,
            _ => -29,
        },
        Error::Io(e) => e.raw_os_error().unwrap_or(-1),
        Error::Credential(e) => super::credentials::error_code(e),
    }
}

/// # Safety
/// Paths are NUL-terminated UTF-8. The optional vault path may be null.
/// Both buffers must remain readable for their lengths, and output writable.
/// Free a successful reply with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_settings(
    path: *const c_char,
    directory: *const c_char,
    request: *const u8,
    request_len: usize,
    credential: *const u8,
    credential_len: usize,
    output: *mut *mut c_char,
) -> i32 {
    let Some(output) = (unsafe { output.as_mut() }) else {
        return -1;
    };
    *output = ptr::null_mut();
    if path.is_null() {
        return -1;
    }
    if credential_len > quotabar::vault::MAX_SIZE {
        return -2;
    }
    let result = std::panic::catch_unwind(|| unsafe {
        let path = CStr::from_ptr(path).to_str().map_err(|_| -1)?;
        let directory = if directory.is_null() {
            None
        } else {
            Some(Path::new(
                CStr::from_ptr(directory).to_str().map_err(|_| -1)?,
            ))
        };
        let reply = mac_settings::run(
            Path::new(path),
            directory,
            bytes(request, request_len)?,
            bytes(credential, credential_len)?,
        )
        .map_err(code)?;
        CString::new(reply).map_err(|_| -1)
    });
    match result {
        Ok(Ok(reply)) => {
            *output = reply.into_raw();
            0
        }
        Ok(Err(code)) => code,
        Err(_) => -1,
    }
}
