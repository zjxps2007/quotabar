//! Account refreshes owned by the macOS monitor's main actor.
use indexmap::IndexSet;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Job {
    pub account: Uuid,
    pub generation: u64,
}

#[derive(Default)]
pub struct Accounts {
    waiting: IndexSet<Uuid>,
    active: HashMap<Uuid, u64>,
    install_workers: HashSet<u64>,
    generation: u64,
    sleeping: bool,
    installing: bool,
}

impl Accounts {
    pub fn enqueue(&mut self, account: Uuid) {
        if !self.sleeping && !self.installing && !self.active.contains_key(&account) {
            self.waiting.insert(account);
        }
    }

    pub fn start_next(&mut self) -> Option<Job> {
        if self.active.len() >= 3 {
            return None;
        }
        let account = self.waiting.shift_remove_index(0)?;
        self.generation = self
            .generation
            .checked_add(1)
            .expect("refresh generation exhausted");
        self.active.insert(account, self.generation);
        Some(Job {
            account,
            generation: self.generation,
        })
    }

    /// A cancelled request may finish after the same account was enabled or
    /// connected again. Only its current worker may publish results or credentials.
    pub fn finish(&mut self, job: Job) -> bool {
        self.install_workers.remove(&job.generation);
        if self.active.get(&job.account) == Some(&job.generation) {
            self.active.remove(&job.account);
            true
        } else {
            false
        }
    }

    pub fn cancel(&mut self, account: Uuid) {
        self.waiting.shift_remove(&account);
        self.active.remove(&account);
    }

    pub fn set_sleeping(&mut self, sleeping: bool) {
        self.sleeping = sleeping;
        if sleeping {
            self.waiting.clear();
            self.active.clear();
        }
    }

    pub fn set_installing(&mut self, installing: bool) {
        if installing && !self.installing {
            // A subsequent sleep, removal or reconnect can retire a generation from the UI
            // before its HTTP worker has unwound and finished writing rotated credentials.
            self.install_workers.extend(self.active.values().copied());
            self.waiting.clear();
        } else if !installing {
            self.install_workers.clear();
        }
        self.installing = installing;
    }

    pub fn ready_to_install(&self) -> bool {
        self.installing && self.install_workers.is_empty()
    }

    pub fn loading(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.waiting.iter().chain(self.active.keys()).copied()
    }

    pub fn status(&self) -> (usize, usize, bool, bool) {
        (
            self.waiting.len(),
            self.active.len(),
            self.sleeping,
            self.installing,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_and_stale_completions_cannot_displace_an_account() {
        let [a, b, c, d, e] = [1, 2, 3, 4, 5].map(Uuid::from_u128);
        let mut queue = Accounts::default();
        for account in [a, b, a, c, d, e] {
            queue.enqueue(account);
        }
        let first = queue.start_next().unwrap();
        assert_eq!(first.account, a);
        assert_eq!(queue.start_next().unwrap().account, b);
        let third = queue.start_next().unwrap();
        assert_eq!(third.account, c);
        queue.enqueue(a);
        assert!(queue.start_next().is_none());
        queue.cancel(d);
        queue.cancel(a);
        queue.enqueue(a);
        assert_eq!(queue.start_next().unwrap().account, e);
        assert!(queue.finish(third));
        let replacement = queue.start_next().unwrap();
        assert_eq!(replacement.account, a);
        assert!(!queue.finish(first));
        assert!(queue.loading().any(|id| id == a));
        assert!(queue.finish(replacement));
        assert!(!queue.finish(replacement));
    }

    #[test]
    fn sleep_invalidates_workers_while_update_handoff_drains_them() {
        let accounts = [1, 2, 3, 4, 5].map(Uuid::from_u128);
        let mut queue = Accounts::default();
        for id in accounts {
            queue.enqueue(id);
        }
        let jobs = [0; 3].map(|_| queue.start_next().unwrap());
        queue.set_installing(true);
        queue.enqueue(accounts[4]);
        assert_eq!(queue.status(), (0, 3, false, true));
        assert!(queue.finish(jobs[0]));
        assert!(queue.start_next().is_none());
        queue.set_sleeping(true);
        assert!(!queue.ready_to_install());
        assert!(!queue.finish(jobs[1]));
        assert!(!queue.ready_to_install());
        assert!(!queue.finish(jobs[2]));
        assert!(queue.ready_to_install());
        queue.set_installing(false);
        queue.enqueue(accounts[1]);
        assert_eq!(queue.status(), (0, 0, true, false));
        queue.set_sleeping(false);
        queue.enqueue(accounts[1]);
        let resumed = queue.start_next().unwrap();
        assert!(!queue.finish(jobs[1]));
        assert!(queue.finish(resumed));
        assert_eq!(queue.loading().count(), 0);
    }
}
