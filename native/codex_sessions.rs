//! Coordinates app-owned Codex token rotations. Never hold the state lock over the network.
use crate::{credentials, oauth, vault};
use chrono::{DateTime, TimeDelta, Utc};
use credentials::Platform;
use indexmap::IndexMap;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque, hash_map::Entry},
    path::Path,
    sync::Arc,
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use zeroize::Zeroizing;

#[cfg(test)]
mod tests;

pub type Secret = Zeroizing<Vec<u8>>;

pub trait Store {
    fn load(&mut self, id: Uuid) -> Result<Option<Secret>, Error>;
    fn save(&mut self, id: Uuid, bytes: &[u8]) -> Result<(), Error>;
}

pub struct Directory<'a>(pub &'a Path);

impl Store for Directory<'_> {
    fn load(&mut self, id: Uuid) -> Result<Option<Secret>, Error> {
        vault::load(self.0, id).map_err(|_| Error::Storage)
    }
    fn save(&mut self, id: Uuid, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > vault::MAX_SIZE {
            return Err(Error::WriteTooLarge);
        }
        vault::save(self.0, id, bytes).map_err(|_| Error::Storage)
    }
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Credentials(credentials::Error),
    Missing,
    Identity,
    Revoked,
    Superseded,
    Storage,
    WriteTooLarge,
}

pub enum Decision {
    Ready(Secret),
    Rotate(u64, Secret),
    Join(u64),
}

struct Flight {
    ticket: u64,
    original: Arc<Secret>,
}

struct PendingWrite {
    original: Arc<Secret>,
    replacement: Secret,
}

pub struct Sessions {
    platform: Platform,
    state: RefCell<State>,
}

struct State {
    next_ticket: u64,
    jobs: HashMap<Uuid, Flight>,
    pending: IndexMap<Uuid, Arc<PendingWrite>>,
    revoked: HashSet<Uuid>,
    windows_queue: HashMap<Uuid, VecDeque<usize>>,
}

impl Sessions {
    pub fn new(platform: Platform) -> Self {
        Self {
            platform,
            state: RefCell::new(State {
                next_ticket: 1,
                jobs: HashMap::new(),
                pending: IndexMap::new(),
                revoked: HashSet::new(),
                windows_queue: HashMap::new(),
            }),
        }
    }

    // Windows queues each caller, including across reconnection. It re-reads the
    // vault on admission. macOS instead joins the currently running task in begin().
    pub fn enter_windows(&self, id: Uuid, waiter: usize) -> bool {
        match self.state.borrow_mut().windows_queue.entry(id) {
            Entry::Vacant(entry) => {
                entry.insert(VecDeque::new());
                true
            }
            Entry::Occupied(mut entry) => {
                entry.get_mut().push_back(waiter);
                false
            }
        }
    }

    pub fn leave_windows(&self, id: Uuid) -> Option<usize> {
        let mut state = self.state.borrow_mut();
        if let Some(next) = state.windows_queue.get_mut(&id)?.pop_front() {
            return Some(next);
        }
        state.windows_queue.remove(&id);
        None
    }

    pub fn invalidate(&self, id: Uuid) -> Option<u64> {
        let mut state = self.state.borrow_mut();
        state.pending.shift_remove(&id);
        state.revoked.remove(&id);
        state.jobs.remove(&id).map(|job| job.ticket)
    }

    // Leave a failed write in place. A new login in storage takes precedence over it.
    fn recover(
        &self,
        store: &mut dyn Store,
        id: Uuid,
        current: Option<Secret>,
    ) -> Result<Option<Secret>, Error> {
        let Some(write) = self.state.borrow().pending.get(&id).cloned() else {
            return Ok(current);
        };
        let current = if current.as_ref() == Some(write.original.as_ref()) {
            store.save(id, &write.replacement)?;
            Some(write.replacement.clone())
        } else {
            current
        };
        let mut state = self.state.borrow_mut();
        if state
            .pending
            .get(&id)
            .is_some_and(|p| Arc::ptr_eq(p, &write))
        {
            state.pending.shift_remove(&id);
        }
        Ok(current)
    }

    pub fn flush(&self, store: &mut dyn Store) -> Result<(), Error> {
        let ids: Vec<_> = self.state.borrow().pending.keys().copied().collect();
        for id in ids {
            let current = store.load(id)?;
            self.recover(store, id, current)?;
        }
        Ok(())
    }

    pub fn begin(
        &self,
        store: &mut dyn Store,
        id: Uuid,
        identity: &str,
        rejected: Option<&[u8]>,
        now: DateTime<Utc>,
    ) -> Result<Decision, Error> {
        if self.state.borrow().revoked.contains(&id) {
            return Err(Error::Revoked);
        }
        if let Some(job) = self.state.borrow().jobs.get(&id) {
            return Ok(Decision::Join(job.ticket));
        }
        let current = store.load(id)?.ok_or(Error::Missing)?;
        let current = self
            .recover(store, id, Some(current))?
            .ok_or(Error::Missing)?;
        let credentials = oauth::managed(&current, self.platform).map_err(Error::Credentials)?;
        if credentials.identity != identity
            && !(self.platform == Platform::Mac && credentials.identity.nfc().eq(identity.nfc()))
        {
            return Err(Error::Identity);
        }
        let due = match credentials.expires_at {
            Some(expires) => expires.signed_duration_since(now) <= TimeDelta::minutes(5),
            None => credentials
                .last_refresh
                .is_none_or(|last| now.signed_duration_since(last) >= TimeDelta::hours(1)),
        };
        if !due && rejected != Some(current.as_slice()) {
            return Ok(Decision::Ready(current));
        }
        let mut state = self.state.borrow_mut();
        let ticket = state.next_ticket;
        state.next_ticket = ticket
            .checked_add(1)
            .expect("Codex session ticket exhausted");
        state.jobs.insert(
            id,
            Flight {
                ticket,
                original: Arc::new(current.clone()),
            },
        );
        Ok(Decision::Rotate(ticket, current))
    }

    pub fn complete(
        &self,
        store: &mut dyn Store,
        id: Uuid,
        ticket: u64,
        replacement: &[u8],
    ) -> Result<(), Error> {
        let original = self
            .state
            .borrow()
            .jobs
            .get(&id)
            .filter(|j| j.ticket == ticket)
            .map(|j| j.original.clone())
            .ok_or(Error::Superseded)?;
        if store.load(id)?.as_ref() != Some(original.as_ref()) || !self.is_active(id, ticket) {
            return Err(Error::Superseded);
        }
        self.state.borrow_mut().pending.insert(
            id,
            Arc::new(PendingWrite {
                original,
                replacement: Zeroizing::new(replacement.to_vec()),
            }),
        );
        store.save(id, replacement)?;
        if self.is_active(id, ticket) {
            self.state.borrow_mut().pending.shift_remove(&id);
        }
        Ok(())
    }

    fn is_active(&self, id: Uuid, ticket: u64) -> bool {
        self.state
            .borrow()
            .jobs
            .get(&id)
            .is_some_and(|job| job.ticket == ticket)
    }

    pub fn revoke(&self, id: Uuid, ticket: u64) {
        if self.is_active(id, ticket) {
            self.state.borrow_mut().revoked.insert(id);
        }
    }

    pub fn end(&self, id: Uuid, ticket: u64) {
        if self.is_active(id, ticket) {
            self.state.borrow_mut().jobs.remove(&id);
        }
    }
}
