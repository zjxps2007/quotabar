//! The caller serializes all operations on the monitor's main actor.
use quotabar::schedule::accounts::{Accounts, Job};
use std::ffi::c_void;
use uuid::Uuid;

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct AccountId {
    bytes: [u8; 16],
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct AccountJob {
    account: AccountId,
    generation: u64,
}

#[derive(Default)]
#[repr(C)]
pub struct Status {
    queued: usize,
    running: usize,
    sleeping: i32,
    installing: i32,
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_mac_refresh_new() -> *mut Accounts {
    Box::into_raw(Box::new(Accounts::default()))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_refresh_free(queue: *mut Accounts) {
    if !queue.is_null() {
        drop(unsafe { Box::from_raw(queue) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_refresh_enqueue(queue: *mut Accounts, id: AccountId) {
    if let Some(queue) = unsafe { queue.as_mut() } {
        queue.enqueue(Uuid::from_bytes(id.bytes));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_refresh_next(queue: *mut Accounts) -> AccountJob {
    unsafe { queue.as_mut() }
        .and_then(Accounts::start_next)
        .map(|j| AccountJob {
            account: AccountId {
                bytes: j.account.into_bytes(),
            },
            generation: j.generation,
        })
        .unwrap_or_default()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_refresh_finish(queue: *mut Accounts, job: AccountJob) -> i32 {
    i32::from(unsafe { queue.as_mut() }.is_some_and(|q| {
        q.finish(Job {
            account: Uuid::from_bytes(job.account.bytes),
            generation: job.generation,
        })
    }))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_refresh_cancel(queue: *mut Accounts, id: AccountId) {
    if let Some(queue) = unsafe { queue.as_mut() } {
        queue.cancel(Uuid::from_bytes(id.bytes));
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_refresh_sleeping(queue: *mut Accounts, value: i32) {
    if let Some(queue) = unsafe { queue.as_mut() } {
        queue.set_sleeping(value != 0);
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_refresh_installing(queue: *mut Accounts, value: i32) {
    if let Some(queue) = unsafe { queue.as_mut() } {
        queue.set_installing(value != 0);
    }
}

/// The optional visitor only collects IDs. It must not mutate the queue during this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_mac_refresh_status(
    queue: *const Accounts,
    context: *mut c_void,
    visitor: Option<extern "C" fn(*mut c_void, AccountId)>,
) -> Status {
    let Some(queue) = (unsafe { queue.as_ref() }) else {
        return Status::default();
    };
    if let Some(visit) = visitor {
        for id in queue.loading() {
            visit(
                context,
                AccountId {
                    bytes: id.into_bytes(),
                },
            );
        }
    }
    let (queued, running, sleeping, installing) = queue.status();
    Status {
        queued,
        running,
        sleeping: i32::from(sleeping),
        installing: i32::from(installing),
    }
}
