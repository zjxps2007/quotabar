use super::{Store, now};
use crate::{
    mac_monitor::fetch::{self, Outcome, failure::STORAGE},
    mac_settings::AccountId,
    schedule::accounts::Job,
    vault,
};
use dispatch2::DispatchQueue;
use objc2::{MainThreadMarker, rc::autoreleasepool};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

impl Store {
    pub(in crate::mac_monitor) fn refresh_all(&self) {
        let mut state = self.state.borrow_mut();
        if self.demo || !self.healthy || state.queue.status().3 {
            return;
        }
        self.last_refresh.set(now());
        let ids: Vec<_> = state
            .settings
            .accounts
            .iter()
            .filter(|a| a.enabled)
            .map(|a| a.id.0)
            .collect();
        for id in ids {
            state.queue.enqueue(id);
        }
        drop(state);
        self.drain();
        self.schedule();
    }

    pub(super) fn refresh(&self, id: AccountId) {
        if self.demo || !self.healthy {
            return;
        }
        let mut s = self.state.borrow_mut();
        if s.settings.accounts.iter().any(|a| a.id == id && a.enabled) {
            s.queue.enqueue(id.0);
        }
        drop(s);
        self.drain();
    }

    pub(super) fn cancel(&self, id: AccountId) {
        let mut s = self.state.borrow_mut();
        s.queue.cancel(id.0);
        if let Some(flag) = s.workers.remove(&id.0) {
            flag.store(true, Ordering::Release);
        }
        drop(s);
        self.drain();
    }

    pub(super) fn cancel_workers(&self) {
        let mut s = self.state.borrow_mut();
        s.queue.set_sleeping(true);
        for (_, flag) in std::mem::take(&mut s.workers) {
            flag.store(true, Ordering::Release);
        }
        drop(s);
        self.changed();
    }

    fn drain(&self) {
        let mut s = self.state.borrow_mut();
        while let Some(job) = s.queue.start_next() {
            let Some(row) = s
                .settings
                .accounts
                .iter()
                .find(|a| a.id.0 == job.account && a.enabled)
                .cloned()
            else {
                s.queue.finish(job);
                continue;
            };
            let flag = Arc::new(AtomicBool::new(false));
            s.workers.insert(job.account, flag.clone());
            let sessions = self.sessions.clone();
            let root = self.root.clone();
            let owner = self.owner.clone();
            std::thread::spawn(move || {
                let result = autoreleasepool(|_| fetch::fetch(&root, &sessions, &row, &flag));
                DispatchQueue::main().exec_async(move || {
                    let main = MainThreadMarker::new().expect("main dispatch queue");
                    if let Some(store) = owner.get(main).upgrade().filter(|s| s.live.get()) {
                        store.finish(job, result);
                    }
                });
            });
        }
        drop(s);
        self.changed();
        self.finish_update_if_ready();
    }

    fn finish(&self, job: Job, result: Outcome) {
        let mut state = self.state.borrow_mut();
        if !state.queue.finish(job) {
            return;
        }
        state.workers.remove(&job.account);
        if state
            .settings
            .accounts
            .iter()
            .any(|a| a.id.0 == job.account && a.enabled)
        {
            if let Some(secret) = result.updated_credential
                && vault::save(self.sessions.vault_directory(), job.account, &secret).is_err()
            {
                state.message = Some(STORAGE.into());
            }
            if let Some(snapshot) = result.snapshot {
                state.snapshots.insert(job.account, snapshot);
            }
            if let Some(error) = result.error {
                state.errors.insert(job.account, error);
            } else {
                state.errors.remove(&job.account);
            }
            if let Some(note) = result.note {
                state.notes.insert(job.account, note);
            } else {
                state.notes.remove(&job.account);
            }
        }
        drop(state);
        self.drain();
    }

    pub(super) fn finish_update_if_ready(&self) {
        let state = self.state.borrow();
        if !state.queue.status().3 || !state.workers.is_empty() || self.flushing.replace(true) {
            return;
        }
        drop(state);
        let sessions = self.sessions.clone();
        let owner = self.owner.clone();
        std::thread::spawn(move || {
            let saved = sessions.flush().is_ok();
            DispatchQueue::main().exec_async(move || {
                let main = MainThreadMarker::new().expect("main dispatch queue");
                if let Some(store) = owner.get(main).upgrade().filter(|s| s.live.get())
                    && let Some(updater) = store.updater.get()
                {
                    updater.drained(saved);
                }
            });
        });
    }
}
