//! A resident monitor's app-owned Codex accounts. Only quota data leaves this service.
use super::{Decision, Directory, Error, Secret, Sessions};
use crate::{
    credentials::Platform,
    provider_http::{self, Failure},
    provider_query::{Error as QueryError, Operation, Query},
};
use chrono::{DateTime, Utc};
use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};
use uuid::Uuid;

#[cfg(test)]
#[path = "managed_tests.rs"]
mod tests;

#[derive(Clone, Debug)]
pub enum FetchError {
    Session(Error),
    Provider(Failure),
    Internal,
}

impl From<Error> for FetchError {
    fn from(error: Error) -> Self {
        Self::Session(error)
    }
}

#[derive(Default)]
struct Rotation {
    cancel: AtomicBool,
    result: OnceLock<Result<Secret, FetchError>>,
}

struct State {
    sessions: Sessions<mpsc::Sender<()>>,
    rotations: HashMap<u64, Arc<Rotation>>,
}

pub struct ManagedAccounts {
    directory: PathBuf,
    platform: Platform,
    state: Mutex<State>,
    changed: Condvar,
}

// Each Windows caller re-reads the vault in admission order, including after a failed exchange.
struct Admission<'a> {
    accounts: &'a ManagedAccounts,
    id: Uuid,
}

impl Drop for Admission<'_> {
    fn drop(&mut self) {
        if let Some(next) = self
            .accounts
            .state
            .lock()
            .unwrap()
            .sessions
            .leave_windows(self.id)
        {
            let _ = next.send(());
        }
    }
}

impl ManagedAccounts {
    #[cfg(target_os = "macos")]
    pub(crate) fn vault_directory(&self) -> &std::path::Path {
        &self.directory
    }

    pub fn new(directory: PathBuf, platform: Platform) -> Self {
        Self {
            directory,
            platform,
            state: Mutex::new(State {
                sessions: Sessions::with_waiters(platform),
                rotations: HashMap::new(),
            }),
            changed: Condvar::new(),
        }
    }

    pub fn invalidate(&self, id: Uuid) {
        let state = self.state.lock().unwrap();
        if let Some(ticket) = state.sessions.invalidate(id)
            && self.platform == Platform::Mac
            && let Some(rotation) = state.rotations.get(&ticket)
        {
            rotation.cancel.store(true, Ordering::Release);
        }
    }

    /// The monitor stops admission before draining for an update.
    pub fn flush(&self) -> Result<(), Error> {
        let state = self
            .changed
            .wait_while(self.state.lock().unwrap(), |s| !s.rotations.is_empty())
            .unwrap();
        state.sessions.flush(&mut Directory(&self.directory))
    }

    fn admit(&self, id: Uuid) -> Option<Admission<'_>> {
        if self.platform != Platform::Windows {
            return None;
        }
        let (tx, rx) = mpsc::channel();
        let first = self.state.lock().unwrap().sessions.enter_windows(id, tx);
        if !first {
            rx.recv()
                .expect("Codex session admission outlives each waiter");
        }
        Some(Admission { accounts: self, id })
    }

    fn credential(
        &self,
        id: Uuid,
        identity: &str,
        rejected: Option<&[u8]>,
        exchange: impl FnOnce(&[u8], &AtomicBool) -> Result<Vec<u8>, Failure>,
    ) -> Result<Secret, FetchError> {
        let _admission = self.admit(id);
        let mut state = self.state.lock().unwrap();
        let mut store = Directory(&self.directory);
        let (ticket, original) =
            match state
                .sessions
                .begin(&mut store, id, identity, rejected, Utc::now())?
            {
                Decision::Ready(data) => return Ok(data),
                Decision::Join(ticket) => {
                    let flight = state
                        .rotations
                        .get(&ticket)
                        .ok_or(FetchError::Internal)?
                        .clone();
                    let _state = self
                        .changed
                        .wait_while(state, |_| flight.result.get().is_none())
                        .unwrap();
                    return flight.result.get().unwrap().clone();
                }
                Decision::Rotate(ticket, bytes) => (ticket, bytes),
            };
        let flight = Arc::new(Rotation::default());
        state.rotations.insert(ticket, flight.clone());
        drop(state);

        // A quota refresh can be cancelled during sleep. An issued replacement still
        // has to reach the vault. Only explicit Mac account invalidation cancels the exchange.
        let exchanged = catch_unwind(AssertUnwindSafe(|| exchange(&original, &flight.cancel)));
        let mut state = self.state.lock().unwrap();
        let result = match exchanged {
            Ok(Ok(bytes)) => {
                let replacement = Secret::new(bytes);
                state
                    .sessions
                    .complete(&mut store, id, ticket, &replacement)
                    .map(|()| replacement)
                    .map_err(FetchError::Session)
            }
            Ok(Err(error)) => {
                if matches!(
                    error,
                    Failure::Query(QueryError::Provider("reauth_required", _))
                ) {
                    state.sessions.revoke(id, ticket);
                }
                Err(FetchError::Provider(error))
            }
            Err(_) => Err(FetchError::Internal),
        };
        state.sessions.end(id, ticket);
        state.rotations.remove(&ticket);
        let _ = flight.result.set(result.clone());
        self.changed.notify_all();
        result
    }

    pub fn fetch(
        &self,
        id: Uuid,
        identity: &str,
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>, FetchError> {
        self.fetch_with(id, identity, cancel, provider_http::run)
    }

    pub fn fetch_with(
        &self,
        id: Uuid,
        identity: &str,
        cancel: &AtomicBool,
        mut run: impl FnMut(Query, &AtomicBool, Option<DateTime<Utc>>) -> Result<Vec<u8>, Failure>,
    ) -> Result<Vec<u8>, FetchError> {
        let mut request = |op, bytes: &[u8], flag: &AtomicBool| {
            let now = (op == Operation::Refresh && self.platform == Platform::Mac).then(Utc::now);
            let query = Query::new(op, bytes, self.platform, 20.0).map_err(Failure::Query)?;
            run(query, flag, now)
        };
        let current = self.credential(id, identity, None, |bytes, flag| {
            request(Operation::Refresh, bytes, flag)
        })?;
        match request(Operation::Codex, &current, cancel) {
            Err(Failure::Query(QueryError::Provider("unauthorized", _))) => {
                let fresh = self.credential(id, identity, Some(&current), |bytes, flag| {
                    request(Operation::Refresh, bytes, flag)
                })?;
                request(Operation::Codex, &fresh, cancel).map_err(FetchError::Provider)
            }
            other => other.map_err(FetchError::Provider),
        }
    }
}
