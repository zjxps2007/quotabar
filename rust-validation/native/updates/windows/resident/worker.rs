use super::super::{
    Installer, PreparationError, Staged, clean_completed, discard, installed, installer,
};
use crate::updates::{
    PUBLIC_KEY, Platform, Release,
    download::{self, Error, Transfer},
};
use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use uuid::Uuid;

pub(super) struct Paths {
    pub root: PathBuf,
    pub target: PathBuf,
    pub version: String,
}

pub(super) type Transport = Arc<dyn Fn(Transfer<'_>) -> Result<bool, Error> + Send + Sync>;

pub(super) enum Job {
    Discover(Option<Staged>),
    Download(Release),
    Prepare(Staged),
    Launch(PathBuf),
}

pub(super) enum Reply {
    Release(Option<Release>),
    Progress(i32),
    Downloaded(Staged),
    Prepared,
    Ready(Installer),
    Failed(Error),
}

struct Mailbox {
    wake: Option<Box<dyn Fn() -> bool + Send + Sync>>,
    replies: VecDeque<Reply>,
}

#[derive(Clone)]
pub(super) struct Workers {
    pub paths: Arc<Paths>,
    transport: Transport,
    mailbox: Arc<Mutex<Mailbox>>,
}

impl Workers {
    pub fn new(
        paths: Paths,
        transport: Transport,
        wake: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            paths: Arc::new(paths),
            transport,
            mailbox: Arc::new(Mutex::new(Mailbox {
                wake: Some(Box::new(wake)),
                replies: VecDeque::new(),
            })),
        }
    }

    fn post(&self, reply: Reply) {
        let mut queue = self.mailbox.lock().unwrap();
        if queue.wake.as_ref().is_some_and(|wake| wake()) {
            queue.replies.push_back(reply);
        } else {
            drop(queue);
            self.abandon(reply);
        }
    }

    fn abandon(&self, reply: Reply) {
        if let Reply::Downloaded(staged) = reply {
            let _ = discard(&self.paths.root, &self.paths.target, &staged.directory);
        }
        // An unclaimed Installer terminates its child on drop.
    }

    pub fn receive(&self) -> Option<Reply> {
        self.mailbox.lock().unwrap().replies.pop_front()
    }

    pub fn stop(&self) {
        let pending = {
            let mut queue = self.mailbox.lock().unwrap();
            queue.wake = None;
            std::mem::take(&mut queue.replies)
        };
        for reply in pending {
            self.abandon(reply);
        }
    }

    pub fn start(&self, job: Job, cancel: Arc<AtomicBool>) -> io::Result<()> {
        let worker = self.clone();
        thread::Builder::new()
            .name("QuotaBar Windows update".into())
            .spawn(move || {
                let reply = worker.execute(job, &cancel).unwrap_or_else(Reply::Failed);
                worker.post(reply);
            })?;
        Ok(())
    }

    fn execute(&self, job: Job, cancel: &AtomicBool) -> Result<Reply, Error> {
        let paths = &self.paths;
        match job {
            Job::Discover(previous) => {
                let _ = clean_completed(&paths.root);
                if let Some(old) = previous {
                    let _ = discard(&paths.root, &paths.target, &old.directory);
                }
                download::check_cancel(cancel)?;
                let platform = if installed(&paths.target)? {
                    Platform::WindowsInstaller
                } else {
                    Platform::WindowsPortable
                };
                download::latest_with(platform, &paths.version, cancel, |request| {
                    (self.transport)(request)
                })
                .map(Reply::Release)
            }
            Job::Download(release) => {
                let directory = paths.root.join("Updates").join(Uuid::new_v4().to_string());
                let update = download::stage_with(
                    &release,
                    &paths.version,
                    PUBLIC_KEY,
                    &directory,
                    cancel,
                    |percent| self.post(Reply::Progress(percent)),
                    |request| (self.transport)(request),
                )?;
                Ok(Reply::Downloaded(Staged { update, directory }))
            }
            Job::Prepare(staged) => {
                installer::prepare_job(
                    staged,
                    &paths.target,
                    &paths.root,
                    paths.version.clone(),
                    cancel,
                )
                .map_err(|error| match error {
                    PreparationError::Signature => Error::Signature,
                    PreparationError::File(e) if e.kind() == io::ErrorKind::Interrupted => {
                        Error::Cancelled
                    }
                    PreparationError::File(e) => Error::File(e),
                })?;
                Ok(Reply::Prepared)
            }
            Job::Launch(job) => {
                download::check_cancel(cancel)?;
                let mut helper = Installer::launch(&job)?;
                for _ in 0..200 {
                    if cancel.load(Ordering::Acquire) {
                        return Err(Error::Cancelled);
                    }
                    if helper.ready()? {
                        return Ok(Reply::Ready(helper));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Update helper did not become ready",
                )
                .into())
            }
        }
    }
}
