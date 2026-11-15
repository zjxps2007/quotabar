use super::CancelHandle;
use quotabar::updates::{self, Asset, Platform, Release, Version};
use serde::Deserialize;
use std::{
    ffi::{CStr, CString, c_char},
    io,
    path::Path,
    ptr, slice,
    sync::atomic::Ordering,
};

fn release_platform(index: i32) -> Option<Platform> {
    [
        Platform::MacArm64,
        Platform::MacX64,
        Platform::WindowsInstaller,
        Platform::WindowsPortable,
    ]
    .get(index as usize)
    .copied()
}

#[cfg(target_os = "macos")]
mod mac_installer {
    use super::*;
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    use updates::macos::{self, Installer, download};

    fn transfer_result<T: serde::Serialize>(
        value: Result<T, download::Error>,
        out: *mut *mut c_char,
    ) -> i32 {
        use download::Error;
        match value {
            Ok(result) => match serde_json::to_vec(&result)
                .ok()
                .and_then(|v| CString::new(v).ok())
            {
                Some(json) => {
                    unsafe { *out = json.into_raw() };
                    0
                }
                None => -1017,
            },
            Err(Error::Development) => 1,
            Err(Error::RateLimited) => 2,
            Err(Error::InvalidFeed) => 3,
            Err(Error::Download) => 4,
            Err(Error::Signature) => 5,
            Err(Error::Cancelled) => 6,
            Err(Error::Network(code)) => code,
            Err(Error::File(error)) => error.raw_os_error().map_or(7, |code| 1000 + code),
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn qb_mac_release_latest(
        platform: i32,
        current: *const c_char,
        cancel: *const CancelHandle,
        out: *mut *mut c_char,
    ) -> i32 {
        if current.is_null() || out.is_null() || cancel.is_null() {
            return -1017;
        }
        unsafe { *out = ptr::null_mut() };
        let Ok(current) = (unsafe { CStr::from_ptr(current) }).to_str() else {
            return 1;
        };
        let Some(platform) = release_platform(platform) else {
            return 3;
        };
        let cancel = unsafe { &*cancel };
        transfer_result(download::latest(platform, current, &cancel.0), out)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn qb_mac_release_stage(
        metadata: *const u8,
        length: usize,
        directory: *const c_char,
        cancel: *const CancelHandle,
        context: *mut std::ffi::c_void,
        progress: extern "C" fn(*mut std::ffi::c_void, i32),
        out: *mut *mut c_char,
    ) -> i32 {
        if metadata.is_null() || length > updates::MAX_FEED || out.is_null() || cancel.is_null() {
            return -1017;
        }
        unsafe { *out = ptr::null_mut() };
        let Some(directory) = (unsafe { file_path(directory) }) else {
            return -1000;
        };
        let Ok(plan) = serde_json::from_slice::<Verification>(unsafe {
            slice::from_raw_parts(metadata, length)
        }) else {
            return 5;
        };
        let cancel = unsafe { &*cancel };
        transfer_result(
            download::stage(
                &plan.release,
                &plan.current_version,
                &plan.public_key,
                directory,
                &cancel.0,
                |value| progress(context, value),
            ),
            out,
        )
    }

    // Swift supplies fileSystemRepresentation for the duration of the call.
    unsafe fn file_path<'a>(path: *const c_char) -> Option<&'a Path> {
        (!path.is_null()).then(|| {
            Path::new(OsStr::from_bytes(
                unsafe { CStr::from_ptr(path) }.to_bytes(),
            ))
        })
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn qb_mac_update_prepare(plan: *const u8, length: usize) -> i32 {
        if plan.is_null() || length > 64 * 1024 {
            return -1;
        }
        macos::prepare(unsafe { slice::from_raw_parts(plan, length) }).map_or(-1, |()| 0)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn qb_mac_update_discard(stage: *const c_char, target: *const c_char) {
        if let (Some(stage), Some(target)) = unsafe { (file_path(stage), file_path(target)) } {
            let _ = macos::discard(stage, target);
        }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn qb_mac_update_launch(job: *const c_char) -> *mut Installer {
        unsafe { file_path(job) }
            .and_then(|path| Installer::launch(path).ok())
            .map_or(ptr::null_mut(), |child| Box::into_raw(Box::new(child)))
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn qb_mac_update_ready(child: *mut Installer) -> i32 {
        unsafe { child.as_mut() }
            .and_then(|c| c.ready().ok())
            .map_or(-1, i32::from)
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn qb_mac_update_finish(child: *mut Installer, terminate: i32) {
        if !child.is_null() {
            unsafe { Box::from_raw(child) }.finish(terminate != 0);
        }
    }
}

/// # Safety
/// Both arguments are NUL-terminated UTF-8 strings.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_release_windows_components(
    root: *const c_char,
    version: *const c_char,
) -> i32 {
    if root.is_null() || version.is_null() {
        return -1;
    }
    let (Ok(root), Ok(version)) = (
        unsafe { CStr::from_ptr(root) }.to_str(),
        unsafe { CStr::from_ptr(version) }.to_str(),
    ) else {
        return -1;
    };
    updates::windows::validate_version(Path::new(root), version).map_or(-1, |()| 0)
}

/// # Safety
/// Input is exactly length UTF-8 bytes. Output has space for three signed integers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_release_version(data: *const u8, length: usize, out: *mut i32) -> i32 {
    if data.is_null() || out.is_null() || length > 33 {
        return -1;
    }
    let bytes = unsafe { slice::from_raw_parts(data, length) };
    let Some(Version(a, b, c)) = std::str::from_utf8(bytes).ok().and_then(|s| s.parse().ok())
    else {
        return -1;
    };
    unsafe {
        ptr::copy_nonoverlapping([a, b, c].as_ptr(), out, 3);
    }
    0
}
/// # Safety
/// Supply length feed bytes. Release the returned JSON value using qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_release_feed(
    platform: i32,
    data: *const u8,
    length: usize,
) -> *mut c_char {
    if data.is_null() || length > updates::MAX_FEED {
        return ptr::null_mut();
    }
    let Some(platform) = release_platform(platform) else {
        return ptr::null_mut();
    };
    let bytes = unsafe { slice::from_raw_parts(data, length) };
    updates::parse_feed(bytes, platform)
        .ok()
        .and_then(|v| serde_json::to_vec(&v).ok())
        .and_then(|json| CString::new(json).ok())
        .map_or(ptr::null_mut(), CString::into_raw)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Verification {
    release: Release,
    current_version: String,
    public_key: String,
}
/// # Safety
/// The three input buffers must be readable through the call. JSON contains only
/// public release metadata and a PEM key. Success returns a positive signed build.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_release_verify(
    metadata: *const u8,
    metadata_len: usize,
    manifest: *const u8,
    manifest_len: usize,
    signature: *const u8,
    signature_len: usize,
) -> i32 {
    if metadata.is_null()
        || manifest.is_null()
        || signature.is_null()
        || metadata_len > updates::MAX_FEED
        || manifest_len > updates::MAX_MANIFEST
        || !(8..=128).contains(&signature_len)
    {
        return -1;
    }
    let verify = || -> io::Result<i32> {
        let request: Verification =
            serde_json::from_slice(unsafe { slice::from_raw_parts(metadata, metadata_len) })?;
        let platform = Platform::for_asset(&request.release)?;
        Ok(updates::verify_manifest(
            &request.release,
            platform,
            &request.current_version,
            unsafe { slice::from_raw_parts(manifest, manifest_len) },
            unsafe { slice::from_raw_parts(signature, signature_len) },
            &request.public_key,
        )?
        .build)
    };
    verify().unwrap_or(-1)
}
/// # Safety
/// Path is a valid NUL-terminated platform path. The metadata and optional
/// cancellation handle must remain live for the whole call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_release_file(
    path: *const c_char,
    data: *const u8,
    length: usize,
    cancel: *const CancelHandle,
) -> i32 {
    if path.is_null() || data.is_null() || length > updates::MAX_FEED {
        return -1;
    }
    let verify = || -> io::Result<()> {
        let asset: Asset = serde_json::from_slice(unsafe { slice::from_raw_parts(data, length) })?;
        let path = unsafe { CStr::from_ptr(path) };
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStrExt;
            Path::new(std::ffi::OsStr::from_bytes(path.to_bytes()))
        };
        #[cfg(not(unix))]
        let path = Path::new(path.to_str().map_err(io::Error::other)?);
        updates::verify_file_while(path, &asset, || {
            unsafe { cancel.as_ref() }.is_some_and(|c| c.0.load(Ordering::Relaxed))
        })
    };
    match verify() {
        Ok(()) => 0,
        Err(e) if e.kind() == io::ErrorKind::Interrupted => 1,
        Err(e) => e
            .raw_os_error()
            .and_then(|v| v.checked_add(1))
            .filter(|n| *n > 1)
            .unwrap_or(-1),
    }
}
