use super::{Event, Owner, Phase, Stage, State, Updater};
use crate::updates::{
    Release,
    macos::{self, Installer, Plan, Staged, download},
    monitor::Offer,
};
use dispatch2::DispatchQueue;
use objc2::{MainThreadMarker, rc::autoreleasepool};
use objc2_foundation::{NSString, NSURL};
use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use uuid::Uuid;

pub(super) enum Operation {
    Check {
        previous: Option<Stage>,
        force: bool,
    },
    Install {
        force: bool,
    },
}

pub(super) enum Error {
    Transfer(download::Error),
    Handoff,
    Stopped,
}

impl From<download::Error> for Error {
    fn from(error: download::Error) -> Self {
        match error {
            download::Error::Cancelled => Self::Stopped,
            other => Self::Transfer(other),
        }
    }
}

pub(super) fn transfer_message(error: download::Error) -> &'static str {
    use download::Error::*;
    match error {
        Development => "배포 앱에서 업데이트를 확인하세요.",
        RateLimited => "업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요.",
        InvalidFeed => "업데이트 정보를 확인할 수 없습니다. 잠시 후 다시 확인하세요.",
        Download => "서명된 업데이트 파일을 내려받지 못했습니다. 현재 버전을 유지합니다.",
        Signature => "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다.",
        _ => "자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다.",
    }
}

pub(super) struct Worker {
    pub owner: Owner,
    pub cancel: Arc<AtomicBool>,
    pub target: PathBuf,
    pub current: String,
    pub updates: PathBuf,
}

impl Worker {
    pub fn run(self, operation: Operation) {
        autoreleasepool(|_| {
            let result = match operation {
                Operation::Check { previous, force } => {
                    if let Some(stage) = previous {
                        let _ = macos::discard(&stage.directory, &self.target);
                    }
                    self.check(force)
                }
                Operation::Install { force } => self.apply(force),
            };
            let result = match result {
                Err(Error::Transfer(_)) if self.cancel.load(Ordering::Acquire) => {
                    Err(Error::Stopped)
                }
                other => other,
            };
            let _ = self.main(|updater| updater.finished(result));
        });
    }

    // Only a worker calls this. The main thread never joins a worker or waits for
    // a download. A closing monitor cannot be resurrected by a late completion.
    fn main<T: Send>(&self, action: impl FnOnce(&Updater) -> T + Send) -> Result<T, Error> {
        let mut result = None;
        DispatchQueue::main().exec_sync(|| {
            let main = MainThreadMarker::new().expect("main dispatch queue");
            if let Some(updater) = self.owner.get(main).upgrade().filter(|u| u.live.get()) {
                result = Some(action(&updater));
            }
        });
        result.ok_or(Error::Stopped)
    }

    fn running(&self) -> Result<(), Error> {
        if self.cancel.load(Ordering::Acquire) {
            Err(Error::Stopped)
        } else {
            Ok(())
        }
    }

    fn check(&self, force: bool) -> Result<(), Error> {
        let release = download::latest(macos::platform(), &self.current, &self.cancel)?;
        self.running()?;
        let Some(release) = release else {
            return self.main(|u| {
                u.set(State::checked(
                    Phase::Idle,
                    None,
                    Some("공개된 정식 릴리즈가 없습니다."),
                ))
            });
        };
        let download = self.main(|u| {
            let version = Some(release.version.clone());
            let offer = u.policy.borrow().offer(&self.current, &release.version);
            match offer {
                Offer::Current => u.set(State::checked(Phase::Current, Some(self.current.clone()), None)),
                Offer::Blocked => u.set(State::checked(Phase::Failed, version, Some("이 버전의 자동 설치에 실패했습니다. ‘업데이트 확인’으로 다시 시도할 수 있습니다."))),
                Offer::Available => {
                    let mut state = State::checked(Phase::Available, version, None);
                    let download = u.policy.borrow().should_download(force);
                    if download && release.asset.is_none() {
                        state.message = Some("이 Mac에 맞는 업데이트 파일이 아직 없습니다.".to_owned());
                    }
                    u.set(state);
                    return download && release.asset.is_some();
                }
            }
            false
        })?;
        if download {
            self.stage(&release)?;
            self.apply(force)?;
        }
        Ok(())
    }

    fn stage(&self, release: &Release) -> Result<(), Error> {
        let key = macos::bundle_key(&self.target).map_err(download::Error::File)?;
        self.main(|u| {
            {
                let mut state = u.state.borrow_mut();
                state.phase = Phase::Downloading;
                state.progress = Some(0);
            }
            u.publish();
        })?;
        let directory = self
            .updates
            .join(Uuid::new_v4().hyphenated().to_string().to_uppercase());
        let update = download::stage(
            release,
            &self.current,
            &key,
            &directory,
            &self.cancel,
            |percent| {
                let _ = self.main(|u| {
                    if u.state.borrow().phase == Phase::Downloading {
                        u.state.borrow_mut().progress = Some(percent);
                        u.publish();
                    }
                });
            },
        )?;
        if self
            .main(|u| {
                u.stage.replace(Some(Stage {
                    update,
                    directory: directory.clone(),
                    prepared: false,
                }));
                u.policy.borrow_mut().staged(&release.version);
                if !self.cancel.load(Ordering::Acquire) {
                    u.set(State::checked(
                        Phase::Ready,
                        Some(release.version.clone()),
                        None,
                    ));
                }
            })
            .is_err()
        {
            let _ = macos::discard(&directory, &self.target);
            return Err(Error::Stopped);
        }
        self.running()
    }

    fn apply(&self, force: bool) -> Result<(), Error> {
        let stage = self.main(|u| {
            let stage = u.stage.borrow().clone()?;
            if !u.policy.borrow_mut().begin_handoff(force) {
                return None;
            }
            u.state.borrow_mut().phase = Phase::Installing;
            u.publish();
            Some(stage)
        })?;
        let Some(stage) = stage else { return Ok(()) };
        if !stage.prepared {
            self.prepare(&stage).map_err(|_| Error::Handoff)?;
            self.main(|u| {
                if let Some(s) = u.stage.borrow_mut().as_mut() {
                    s.prepared = true;
                }
            })?;
        }
        let drain = self.main(|u| {
            if !eligible(u, force) {
                return None;
            }
            let (done, waiting) = mpsc::channel();
            u.drain.replace(Some(done));
            u.paused.set(true);
            (u.notify)(Event::PrepareExit);
            Some(waiting)
        })?;
        let Some(drain) = drain else { return Ok(()) };
        if !drain.recv().unwrap_or(false) {
            return Err(Error::Handoff);
        }
        if !self.main(|u| eligible(u, force))? {
            return Ok(());
        }
        let child =
            Installer::launch(&stage.directory.join("install.json")).map_err(|_| Error::Handoff)?;
        let mut child = WaitingInstaller(Some(child));
        for _ in 0..200 {
            self.running()?;
            let process = child.0.as_mut().expect("live update helper");
            if process.ready().map_err(|_| Error::Handoff)? {
                if !process.ready().map_err(|_| Error::Handoff)? {
                    return Err(Error::Handoff);
                }
                return self.main(|u| {
                    if eligible(u, force) {
                        child.0.take().expect("acknowledged helper").finish(false);
                        (u.notify)(Event::Exit);
                    }
                });
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(Error::Handoff)
    }

    fn prepare(&self, stage: &Stage) -> io::Result<()> {
        let plan = Plan {
            staged: Staged {
                update: stage.update.clone(),
                directory: directory_url(&stage.directory)?,
            },
            target: directory_url(&self.target)?,
            current_version: self.current.clone(),
        };
        macos::prepare(&serde_json::to_vec(&plan)?)
    }
}

fn directory_url(path: &Path) -> io::Result<String> {
    let path = path
        .to_str()
        .ok_or_else(|| io::Error::other("Invalid bundle path"))?;
    NSURL::fileURLWithPath_isDirectory(&NSString::from_str(path), true)
        .absoluteString()
        .map(|url| url.to_string())
        .ok_or_else(|| io::Error::other("Invalid bundle URL"))
}

fn eligible(u: &Updater, force: bool) -> bool {
    if u.policy.borrow().may_install(force) {
        return true;
    }
    u.resume();
    u.state.borrow_mut().phase = Phase::Ready;
    u.publish();
    false
}

struct WaitingInstaller(Option<Installer>);
impl Drop for WaitingInstaller {
    fn drop(&mut self) {
        if let Some(process) = self.0.take() {
            process.finish(true);
        }
    }
}
