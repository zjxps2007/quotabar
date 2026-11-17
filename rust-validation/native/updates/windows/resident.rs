//! Update state belongs to the Win32 monitor thread. Workers report through its
//! message queue; only the owner can pause account activity or authorize exit.
pub mod probe;
mod worker;

use super::{Installer, Staged, clean_completed, discard, installer::read_result};
use crate::updates::{
    FeedError, Release,
    download::{Error, Transfer},
    monitor::{Monitor, Offer, OnConfigure, Phase},
};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use worker::{Job, Paths, Reply, Transport, Workers};

const FAILED: &str = "자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다.";

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    phase: Phase,
    version: Option<String>,
    progress: Option<i32>,
    message: Option<String>,
    checked_at: Option<String>,
}

impl State {
    fn checked(phase: Phase, version: Option<String>, message: Option<&str>) -> Self {
        Self {
            phase,
            version,
            message: message.map(str::to_owned),
            checked_at: Some(Utc::now().to_rfc3339_opts(SecondsFormat::Micros, false)),
            ..Self::default()
        }
    }
}

pub enum Event {
    State(State),
    PrepareExit,
    Resume,
    Exit,
}

struct Operation {
    cancel: Arc<AtomicBool>,
    force: bool,
}

pub struct Updater {
    workers: Workers,
    policy: Monitor,
    state: State,
    events: VecDeque<Event>,
    operation: Option<Operation>,
    stage: Option<Staged>,
    prepared: bool,
    waiting_for_drain: bool,
    paused: bool,
    helper: Option<Installer>,
}

impl Updater {
    pub fn new(
        root: PathBuf,
        target: PathBuf,
        version: String,
        wake: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self::with_transport(
            root,
            target,
            version,
            wake,
            Arc::new(|request: Transfer<'_>| {
                #[cfg(feature = "windows-update-fixture")]
                if let Some(root) = std::env::var_os("QUOTABAR_TEST_RELEASE_DIRECTORY") {
                    return probe::file_response(&PathBuf::from(root), request);
                }
                request.send()
            }),
        )
    }

    fn with_transport(
        root: PathBuf,
        target: PathBuf,
        version: String,
        wake: impl Fn() -> bool + Send + Sync + 'static,
        transport: Transport,
    ) -> Self {
        let mut policy = Monitor::default();
        let mut state = State::default();
        if let Some(previous) =
            read_result(&root.join("Updates/last-result.json")).filter(|result| !result.success)
        {
            policy.block(&previous.version);
            state = State {
                phase: Phase::Failed,
                version: Some(previous.version),
                message: Some(previous.message),
                ..State::default()
            };
        }
        let _ = clean_completed(&root);
        Self {
            workers: Workers::new(
                Paths {
                    root,
                    target,
                    version,
                },
                transport,
                wake,
            ),
            policy,
            state,
            events: VecDeque::new(),
            operation: None,
            stage: None,
            prepared: false,
            waiting_for_drain: false,
            paused: false,
            helper: None,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }
    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    fn set(&mut self, state: State) {
        if state != self.state {
            self.events.push_back(Event::State(state.clone()));
            self.state = state;
        }
    }

    pub fn delay_ms(&self) -> u32 {
        self.policy
            .delay(now())
            .map_or(0, |seconds| (seconds * 1000.).min(i32::MAX as f64) as u32)
    }

    pub fn configure(&mut self, enabled: bool, can_install: bool, suspended: bool) {
        match self.policy.configure(enabled, can_install, suspended) {
            OnConfigure::Cancel => {
                if let Some(active) = &self.operation {
                    active.cancel.store(true, Ordering::Release);
                }
            }
            OnConfigure::Install => self.install(false),
            OnConfigure::Schedule => {}
        }
    }

    pub fn check(&mut self, manual: bool, force: bool) {
        if !self.policy.begin_check(manual) {
            return;
        }
        self.operation = Some(Operation {
            cancel: Arc::default(),
            force,
        });
        self.policy.attempted(now());
        self.prepared = false;
        let previous = self.stage.take();
        self.set(State {
            phase: Phase::Checking,
            ..State::default()
        });
        self.start(Job::Discover(previous));
    }

    pub fn install_now(&mut self) {
        if self.stage.is_some() {
            self.install(true);
        } else {
            self.check(true, true);
        }
    }

    fn install(&mut self, force: bool) {
        if self.policy.begin_install(force) {
            self.operation = Some(Operation {
                cancel: Arc::default(),
                force,
            });
            self.apply(force);
        }
    }

    fn start(&mut self, job: Job) {
        let cancel = self
            .operation
            .as_ref()
            .expect("active update")
            .cancel
            .clone();
        if self.workers.start(job, cancel).is_err() {
            self.failed(FAILED);
        }
    }

    fn resume(&mut self) {
        self.waiting_for_drain = false;
        if std::mem::take(&mut self.paused) {
            self.events.push_back(Event::Resume);
        }
    }

    fn finish(&mut self, cancelled: bool) {
        self.operation = None;
        self.policy.finish(cancelled);
    }

    fn failed(&mut self, message: &str) {
        if self.state.phase == Phase::Installing
            && let Some(staged) = &self.stage
        {
            self.policy.block(&staged.update.release.version);
        }
        self.helper.take();
        self.resume();
        self.set(State::checked(
            Phase::Failed,
            self.state.version.clone(),
            Some(message),
        ));
        self.finish(false);
    }

    fn cancelled(&mut self) {
        if let Some(stage) = self.stage.take() {
            let _ = discard(
                &self.workers.paths.root,
                &self.workers.paths.target,
                &stage.directory,
            );
        }
        self.prepared = false;
        self.helper.take();
        self.resume();
        self.set(State::default());
        self.finish(true);
    }

    /// Process one posted worker reply per window message, in the same order as
    /// power, window visibility and settings changes on the owner thread.
    pub fn dispatch(&mut self) {
        let Some(reply) = self.workers.receive() else {
            return;
        };
        let Some(active) = &self.operation else {
            return;
        };
        let force = active.force;
        if matches!(reply, Reply::Progress(_)) && active.cancel.load(Ordering::Acquire) {
            return;
        }
        if active.cancel.load(Ordering::Acquire) {
            if let Reply::Downloaded(staged) = reply {
                self.stage = Some(staged);
            }
            self.cancelled();
            return;
        }
        match reply {
            Reply::Release(release) => self.offer(release, force),
            Reply::Progress(percent) if self.state.phase == Phase::Downloading => {
                self.set(State {
                    progress: Some(percent),
                    ..self.state.clone()
                });
            }
            Reply::Progress(_) => {}
            Reply::Downloaded(staged) => {
                let version = staged.update.release.version.clone();
                self.policy.staged(&version);
                self.stage = Some(staged);
                self.set(State::checked(Phase::Ready, Some(version), None));
                self.apply(force);
            }
            Reply::Prepared => {
                self.prepared = true;
                self.request_drain();
            }
            Reply::Ready(mut helper) => {
                if !self.eligible() {
                    return;
                }
                if !matches!(helper.ready(), Ok(true)) {
                    self.failed(FAILED);
                    return;
                }
                self.helper = Some(helper);
                self.events.push_back(Event::Exit);
            }
            Reply::Failed(error) => self.failed(error_message(&error)),
        }
    }

    fn offer(&mut self, release: Option<Release>, force: bool) {
        let Some(release) = release else {
            self.set(State::checked(
                Phase::Idle,
                None,
                Some("공개된 정식 릴리즈가 없습니다."),
            ));
            self.finish(false);
            return;
        };
        let current = &self.workers.paths.version;
        match self.policy.offer(current, &release.version) {
            Offer::Current => self.set(State::checked(Phase::Current, Some(current.clone()), None)),
            Offer::Blocked => self.set(State { phase: Phase::Failed, version: Some(release.version),
                message: Some("이 버전의 자동 설치에 실패했습니다. ‘업데이트 확인’으로 다시 시도할 수 있습니다.".into()), ..State::default() }),
            Offer::Available => {
                self.set(State::checked(Phase::Available, Some(release.version.clone()), None));
                if self.policy.should_download(force) {
                    if release.asset.is_some() {
                        self.set(State { phase: Phase::Downloading, progress: Some(0), ..self.state.clone() });
                        self.start(Job::Download(release));
                        return;
                    }
                    self.set(State { message: Some("이 PC에 맞는 업데이트 파일이 아직 없습니다.".into()), ..self.state.clone() });
                }
            }
        }
        self.finish(false);
    }

    fn apply(&mut self, force: bool) {
        if !self.policy.begin_handoff(force) {
            self.finish(false);
            return;
        }
        self.set(State {
            phase: Phase::Installing,
            progress: None,
            ..self.state.clone()
        });
        if self.prepared {
            self.request_drain();
        } else {
            self.start(Job::Prepare(
                self.stage.as_ref().expect("staged update").clone(),
            ));
        }
    }

    fn eligible(&mut self) -> bool {
        if self
            .operation
            .as_ref()
            .is_some_and(|op| self.policy.may_install(op.force))
        {
            return true;
        }
        self.set(State {
            phase: Phase::Ready,
            ..self.state.clone()
        });
        self.resume();
        self.finish(false);
        false
    }

    fn request_drain(&mut self) {
        if self.eligible() {
            self.paused = true;
            self.waiting_for_drain = true;
            self.events.push_back(Event::PrepareExit);
        }
    }

    pub fn drained(&mut self, result: Result<(), &str>) {
        if !std::mem::take(&mut self.waiting_for_drain) {
            return;
        }
        if let Err(message) = result {
            self.failed(message);
            return;
        }
        if self.eligible() {
            let job = self
                .stage
                .as_ref()
                .expect("prepared update")
                .directory
                .join("install.json");
            self.start(Job::Launch(job));
        }
    }

    pub fn exited(&mut self, success: bool) {
        let Some(mut helper) = self.helper.take() else {
            return;
        };
        if success {
            helper.detach();
            self.finish(false);
        } else {
            self.failed(FAILED);
        }
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        if let Some(operation) = &self.operation {
            operation.cancel.store(true, Ordering::Release);
        }
        self.workers.stop();
    }
}

fn now() -> f64 {
    Utc::now().timestamp_millis() as f64 / 1000.
}

fn error_message(error: &Error) -> &'static str {
    match error {
        Error::Development => "배포 앱에서 업데이트를 확인하세요.",
        Error::RateLimited => "업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요.",
        Error::Feed(FeedError::InvalidRelease) => {
            "업데이트 정보를 확인할 수 없습니다. 잠시 후 다시 확인하세요."
        }
        Error::Download => "서명된 업데이트 파일을 내려받지 못했습니다. 현재 버전을 유지합니다.",
        Error::Signature => "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다.",
        _ => FAILED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::{Asset, Verified};
    use std::{sync::mpsc, time::Duration};
    use uuid::Uuid;

    #[test]
    fn prepared_install_waits_for_account_drain_and_rechecks_window_visibility() {
        let root = std::env::temp_dir().join(format!("QuotaBar.Update.Drain-{}", Uuid::new_v4()));
        let mut updater = Updater::new(root.clone(), root.join("app"), "0.1.0".into(), || true);
        let asset = Asset {
            name: "fixture.zip".into(),
            url: String::new(),
            bytes: 1,
            sha256: String::new(),
        };
        let release = Release {
            tag: "v0.1.1".into(),
            version: "0.1.1".into(),
            page: String::new(),
            asset: Some(asset.clone()),
        };
        updater.stage = Some(Staged {
            update: Verified {
                release,
                build: 2,
                asset,
            },
            directory: root.join("stage"),
        });
        updater.prepared = true;
        updater.policy.staged("0.1.1");
        updater.state = State::checked(Phase::Ready, Some("0.1.1".into()), None);

        updater.configure(true, true, false);
        assert!(matches!(
            updater.next_event(),
            Some(Event::State(State {
                phase: Phase::Installing,
                ..
            }))
        ));
        assert!(matches!(updater.next_event(), Some(Event::PrepareExit)));
        updater.configure(true, false, false);
        updater.drained(Ok(()));
        assert!(matches!(
            updater.next_event(),
            Some(Event::State(State {
                phase: Phase::Ready,
                ..
            }))
        ));
        assert!(matches!(updater.next_event(), Some(Event::Resume)));
        updater.drained(Ok(()));
        assert!(updater.next_event().is_none());
        assert!(updater.operation.is_none() && updater.prepared);

        updater.configure(true, true, false);
        updater.drained(Err("credential write failed"));
        assert_eq!(updater.state.phase, Phase::Failed);
        assert_eq!(
            updater.state.message.as_deref(),
            Some("credential write failed")
        );
        updater.configure(true, true, false);
        assert!(updater.operation.is_none());
        assert!(updater.delay_ms() > 0);
    }

    #[test]
    fn suspension_retires_the_inflight_check_before_a_manual_retry() {
        let root = std::env::temp_dir().join(format!("QuotaBar.Update.Cancel-{}", Uuid::new_v4()));
        let (wake, wait) = mpsc::channel();
        let (started, first) = mpsc::channel();
        let mut updater = Updater::with_transport(
            root.clone(),
            root.join("app"),
            "0.1.0".into(),
            move || wake.send(()).is_ok(),
            Arc::new(move |request| {
                started.send(()).unwrap();
                while !request.cancel.load(Ordering::Acquire) {
                    std::thread::park_timeout(Duration::from_millis(2));
                }
                Err(Error::Cancelled)
            }),
        );
        updater.configure(true, true, false);
        updater.check(true, false);
        first.recv_timeout(Duration::from_secs(5)).unwrap();
        updater.configure(true, false, true);
        updater.check(true, false);
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        updater.dispatch();
        assert_eq!(updater.state.phase, Phase::Idle);
        assert!(updater.operation.is_none());
        assert_eq!(updater.delay_ms(), 0);
        updater.configure(true, true, false);
        assert_eq!(updater.delay_ms(), 15_000);
    }
}
