use std::{
    io,
    path::{Path, PathBuf},
};

/// Path.GetFullPath expands existing DOS aliases, including the parent of a
/// path that has not been created yet. Persisted profile and pipe IDs use this spelling.
pub fn full_path(path: &Path) -> io::Result<PathBuf> {
    use std::{
        ffi::OsString,
        os::windows::ffi::OsStringExt,
        path::{Component, Prefix},
    };
    use windows_sys::Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND},
        Storage::FileSystem::GetLongPathNameW,
    };
    let absolute = std::path::absolute(path)?;
    if !absolute.as_os_str().as_encoded_bytes().contains(&b'~') {
        return Ok(absolute);
    }
    for parent in absolute.ancestors() {
        let input = crate::files::windows::wide_file_path(parent)?;
        let mut buf = vec![0u16; input.len()];
        loop {
            let len =
                unsafe { GetLongPathNameW(input.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
            if len == 0 {
                match io::Error::last_os_error().raw_os_error().map(|n| n as u32) {
                    Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => break,
                    _ => return Ok(absolute),
                }
            }
            if len as usize >= buf.len() {
                buf.resize(len as usize, 0);
                continue;
            }
            let mut result = PathBuf::from(OsString::from_wide(&buf[..len as usize]));
            // Strip only a prefix added by wide_file_path, preserving the caller's namespace.
            if !parent.as_os_str().as_encoded_bytes().starts_with(br"\\?\")
                && let Some(Component::Prefix(prefix)) = result.components().next()
            {
                let ordinary = match prefix.kind() {
                    Prefix::VerbatimDisk(drive) => {
                        Some(PathBuf::from(format!("{}:\\", char::from(drive))))
                    }
                    Prefix::VerbatimUNC(server, share) => {
                        Some(Path::new(r"\\").join(server).join(share))
                    }
                    _ => None,
                };
                if let Some(root) = ordinary {
                    result = root.join(result.components().skip(2).collect::<PathBuf>());
                }
            }
            let suffix = absolute
                .strip_prefix(parent)
                .expect("ancestor of absolute path");
            if !suffix.as_os_str().is_empty() {
                result.push(suffix);
            }
            return Ok(result);
        }
    }
    Ok(absolute)
}

pub(crate) fn known_folder(id: &windows_sys::core::GUID) -> Option<std::path::PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr::null_mut};
    use windows_sys::Win32::{
        Globalization::lstrlenW, System::Com::CoTaskMemFree, UI::Shell::SHGetKnownFolderPath,
    };
    unsafe {
        let mut path = null_mut();
        if SHGetKnownFolderPath(id, 0, null_mut(), &mut path) < 0 || path.is_null() {
            CoTaskMemFree(path.cast());
            return None;
        }
        let value = OsString::from_wide(std::slice::from_raw_parts(path, lstrlenW(path) as usize));
        CoTaskMemFree(path.cast());
        Some(value.into())
    }
}

fn upper(c: char) -> char {
    use windows_sys::Win32::Globalization::u_toupper;
    match c {
        '\u{131}' => c,
        c => char::from_u32(unsafe { u_toupper(c as i32) } as u32).unwrap_or(c),
    }
}

/// Matches the invariant simple casing used by .NET when deriving persisted Windows IDs.
pub(crate) fn invariant_uppercase(text: &str) -> String {
    text.chars().map(upper).collect()
}

/// OrdinalIgnoreCase preserves long s, even though its invariant uppercase is ASCII S.
pub fn ordinal_uppercase(text: &str) -> String {
    text.chars()
        .map(|c| if c == 'ſ' { c } else { upper(c) })
        .collect()
}

fn locale_string(kind: u32) -> Option<String> {
    use windows_sys::Win32::Globalization::GetLocaleInfoEx;
    let mut buf = [0; 32];
    let len =
        unsafe { GetLocaleInfoEx(std::ptr::null(), kind, buf.as_mut_ptr(), buf.len() as i32) };
    (len > 1).then(|| String::from_utf16_lossy(&buf[..len as usize - 1]))
}

pub fn month_day_time(date: chrono::DateTime<chrono::Local>) -> String {
    let sep = locale_string(windows_sys::Win32::Globalization::LOCALE_SDATE)
        .unwrap_or_else(|| "/".into());
    format!("{}{sep}{}", date.format("%m"), date.format("%d %H:%M"))
}

pub fn grouped(value: u64) -> String {
    use windows_sys::Win32::Globalization::{GetNumberFormatEx, LOCALE_SDECIMAL};
    let source: Vec<_> = value.to_string().encode_utf16().chain(Some(0)).collect();
    let mut buf = [0; 96];
    let len = unsafe {
        GetNumberFormatEx(
            std::ptr::null(),
            0,
            source.as_ptr(),
            std::ptr::null(),
            buf.as_mut_ptr(),
            buf.len() as i32,
        )
    };
    if len <= 1 {
        return value.to_string();
    }
    let mut result = String::from_utf16_lossy(&buf[..len as usize - 1]);
    if let Some(at) = locale_string(LOCALE_SDECIMAL).and_then(|decimal| result.rfind(&decimal)) {
        result.truncate(at);
    }
    result
}
