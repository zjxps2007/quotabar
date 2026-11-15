use super::{
    CancelHandle,
    credentials::{publish, publish_secret},
};
use quotabar::{codex_sessions::managed::ManagedAccounts, mac_monitor::fetch};
use std::{
    ffi::{CStr, c_char},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    slice,
};

#[repr(C)]
#[derive(Default)]
pub struct RefreshResult {
    state: *mut u8,
    state_length: usize,
    credential: *mut u8,
    credential_length: usize,
}

/// # Safety
/// The caller owns the session and cancellation handle until the worker returns.
/// Free each returned buffer with qb_secret_free. Only state can be sent to the UI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_fetch_account(
    sessions: *const ManagedAccounts,
    root: *const c_char,
    account: *const u8,
    length: usize,
    cancel: *const CancelHandle,
    result: *mut RefreshResult,
) -> i32 {
    let Some(result) = (unsafe { result.as_mut() }) else {
        return -1;
    };
    *result = RefreshResult::default();
    let (Some(sessions), Some(cancel)) = (unsafe { sessions.as_ref() }, unsafe { cancel.as_ref() })
    else {
        return -1;
    };
    if root.is_null() || account.is_null() || length > isize::MAX as usize {
        return -1;
    }
    let Ok(root) = unsafe { CStr::from_ptr(root) }.to_str() else {
        return -1;
    };
    let account = unsafe { slice::from_raw_parts(account, length) };
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(account) = serde_json::from_slice(account) else {
            return -1;
        };
        let outcome = fetch::fetch(Path::new(root), sessions, &account, &cancel.0);
        let Ok(state) = serde_json::to_vec(&outcome) else {
            return -1;
        };
        publish(state, &mut result.state, &mut result.state_length);
        if let Some(bytes) = outcome.updated_credential {
            publish_secret(
                &bytes,
                &mut result.credential,
                &mut result.credential_length,
            );
        }
        0
    }))
    .unwrap_or(-1)
}
