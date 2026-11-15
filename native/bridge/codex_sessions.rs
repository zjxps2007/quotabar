use super::credentials::{error_code, publish_secret};
use parking_lot::ReentrantMutex;
use quotabar::{
    codex_sessions::{Decision, Directory, Error, Secret, Sessions, Store},
    credentials::Platform,
};
use std::{
    ffi::{CStr, c_char, c_void},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    ptr, slice, str,
};
use uuid::Uuid;

pub struct SessionHandle {
    sessions: ReentrantMutex<Sessions>,
    directory: Option<PathBuf>,
}

#[repr(C)]
pub struct StoreCallback {
    context: *mut c_void,
    access: unsafe extern "C" fn(
        *mut c_void,
        *const c_char,
        *const u8,
        usize,
        *mut Option<Secret>,
    ) -> i32,
}

impl Store for &StoreCallback {
    fn load(&mut self, id: Uuid) -> Result<Option<Secret>, Error> {
        let mut name = [0; 37];
        id.hyphenated().encode_lower(&mut name);
        let mut result = None;
        match unsafe {
            (self.access)(
                self.context,
                name.as_ptr().cast(),
                ptr::null(),
                0,
                &mut result,
            )
        } {
            0 if result.is_some() => Ok(result),
            1 => Ok(None),
            _ => Err(Error::Storage),
        }
    }

    fn save(&mut self, id: Uuid, bytes: &[u8]) -> Result<(), Error> {
        let mut name = [0; 37];
        id.hyphenated().encode_lower(&mut name);
        let status = unsafe {
            (self.access)(
                self.context,
                name.as_ptr().cast(),
                bytes.as_ptr(),
                bytes.len(),
                ptr::null_mut(),
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(Error::Storage)
        }
    }
}

pub(super) fn status(error: Error) -> i32 {
    match error {
        Error::Credentials(error) => error_code(error),
        Error::Missing => -50,
        Error::Identity => -51,
        Error::Revoked => -52,
        Error::Superseded => -53,
        Error::Storage => -54,
        Error::WriteTooLarge => -55,
    }
}

unsafe fn bytes<'a>(p: *const u8, n: usize) -> Option<&'a [u8]> {
    if n == 0 {
        Some(&[])
    } else if p.is_null() || n > isize::MAX as usize {
        None
    } else {
        Some(unsafe { slice::from_raw_parts(p, n) })
    }
}

unsafe fn account(id: *const c_char) -> Option<Uuid> {
    if id.is_null() {
        return None;
    }
    Uuid::try_parse_ascii(unsafe { CStr::from_ptr(id) }.to_bytes()).ok()
}

fn with_store(
    handle: &SessionHandle,
    callbacks: Option<&StoreCallback>,
    action: impl FnOnce(&Sessions, &mut dyn Store) -> Result<i32, Error>,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let sessions = handle.sessions.lock();
        let result = match (&handle.directory, callbacks) {
            (Some(path), None) => action(&sessions, &mut Directory(path)),
            (None, Some(mut store)) => action(&sessions, &mut store),
            _ => return -1,
        };
        result.unwrap_or_else(status)
    }))
    .unwrap_or(-1)
}

/// # Safety
/// directory is a NUL-terminated UTF-8 vault path, or null for a caller-supplied store.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_new(mac: i32, directory: *const c_char) -> *mut SessionHandle {
    let platform = if mac == 0 {
        Platform::Windows
    } else {
        Platform::Mac
    };
    let directory = if directory.is_null() {
        None
    } else {
        let Ok(path) = unsafe { CStr::from_ptr(directory) }.to_str() else {
            return ptr::null_mut();
        };
        Some(PathBuf::from(path))
    };
    Box::into_raw(Box::new(SessionHandle {
        sessions: ReentrantMutex::new(Sessions::new(platform)),
        directory,
    }))
}

/// # Safety
/// Release exactly once, after the last use of the handle and all its callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_free(handle: *mut SessionHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// # Safety
/// Called only during a load callback with the supplied receiver and length readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_loaded(
    receiver: *mut Option<Secret>,
    data: *const u8,
    length: usize,
) -> i32 {
    let (Some(receiver), Some(data)) =
        (unsafe { receiver.as_mut() }, unsafe { bytes(data, length) })
    else {
        return -1;
    };
    *receiver = Some(Secret::new(data.to_vec()));
    0
}

/// # Safety
/// All inputs and callback storage must stay valid for the call. Release ready/rotate
/// buffers with qb_secret_free. A join returns the active ticket without a buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_begin(
    handle: *const SessionHandle,
    id: *const c_char,
    identity: *const u8,
    identity_len: usize,
    rejected: *const u8,
    rejected_len: usize,
    store: *const StoreCallback,
    ticket: *mut u64,
    output: *mut *mut u8,
    length: *mut usize,
) -> i32 {
    let (Some(ticket), Some(output), Some(length)) = (
        unsafe { ticket.as_mut() },
        unsafe { output.as_mut() },
        unsafe { length.as_mut() },
    ) else {
        return -1;
    };
    *ticket = 0;
    *output = ptr::null_mut();
    *length = 0;
    let (Some(handle), Some(id), Some(identity), Some(rejected_bytes)) = (
        unsafe { handle.as_ref() },
        unsafe { account(id) },
        unsafe { bytes(identity, identity_len) }.and_then(|b| str::from_utf8(b).ok()),
        unsafe { bytes(rejected, rejected_len) },
    ) else {
        return -1;
    };
    with_store(
        handle,
        unsafe { store.as_ref() },
        |sessions, store| match sessions.begin(
            store,
            id,
            identity,
            (!rejected.is_null()).then_some(rejected_bytes),
            chrono::Utc::now(),
        )? {
            Decision::Ready(data) => {
                publish_secret(&data, output, length);
                Ok(0)
            }
            Decision::Rotate(generation, data) => {
                *ticket = generation;
                publish_secret(&data, output, length);
                Ok(1)
            }
            Decision::Join(generation) => {
                *ticket = generation;
                Ok(2)
            }
        },
    )
}

/// # Safety
/// The handle, UUID string, replacement bytes and optional store stay valid until return.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_complete(
    handle: *const SessionHandle,
    id: *const c_char,
    ticket: u64,
    replacement: *const u8,
    length: usize,
    store: *const StoreCallback,
) -> i32 {
    let (Some(handle), Some(id), Some(replacement)) =
        (unsafe { handle.as_ref() }, unsafe { account(id) }, unsafe {
            bytes(replacement, length)
        })
    else {
        return -1;
    };
    with_store(handle, unsafe { store.as_ref() }, |sessions, store| {
        sessions.complete(store, id, ticket, replacement)?;
        Ok(0)
    })
}

/// # Safety
/// The handle and optional callback storage stay valid until return. Drain rotations first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_flush(
    handle: *const SessionHandle,
    store: *const StoreCallback,
) -> i32 {
    let Some(handle) = (unsafe { handle.as_ref() }) else {
        return -1;
    };
    with_store(handle, unsafe { store.as_ref() }, |sessions, store| {
        sessions.flush(store)?;
        Ok(0)
    })
}

/// # Safety
/// Supply a live handle and NUL-terminated UUID. Only the owning rotation may revoke or end a ticket.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_event(
    handle: *const SessionHandle,
    id: *const c_char,
    ticket: u64,
    event: i32,
) -> u64 {
    let (Some(handle), Some(id)) = (unsafe { handle.as_ref() }, unsafe { account(id) }) else {
        return 0;
    };
    let sessions = handle.sessions.lock();
    match event {
        0 => return sessions.invalidate(id).unwrap_or(0),
        1 => sessions.revoke(id, ticket),
        2 => sessions.end(id, ticket),
        _ => {}
    }
    0
}

/// # Safety
/// The nonzero waiter cookie belongs to the caller. On a queued result (1) it
/// stays live until returned by qb_sessions_leave_windows, even across invalidation.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_enter_windows(
    handle: *const SessionHandle,
    id: *const c_char,
    waiter: usize,
) -> i32 {
    let (Some(handle), Some(id)) = (unsafe { handle.as_ref() }, unsafe { account(id) }) else {
        return -1;
    };
    if waiter == 0 {
        return -1;
    }
    i32::from(!handle.sessions.lock().enter_windows(id, waiter))
}

/// # Safety
/// Call once after each admitted Windows operation. A nonzero cookie wakes exactly one waiting caller.
#[cfg(windows)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_sessions_leave_windows(
    handle: *const SessionHandle,
    id: *const c_char,
) -> usize {
    let (Some(handle), Some(id)) = (unsafe { handle.as_ref() }, unsafe { account(id) }) else {
        return 0;
    };
    handle.sessions.lock().leave_windows(id).unwrap_or(0)
}
