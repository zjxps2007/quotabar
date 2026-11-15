//! The installed monitor's update controller. Eligibility, state publication
//! and the account-service handshake live on the AppKit thread. The worker owns
//! blocking network, filesystem and child-process operations.
mod worker;

use super::{InstallResult, Verified, data_root, discard, read_file};
use crate::updates::monitor::{Monitor, OnConfigure};
use block2::RcBlock;
use dispatch2::MainThreadBound;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_foundation::{NSBundle, NSString, NSTimer};
use serde::Serialize;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};
use worker::{Error, Operation, Worker};

#[derive(Clone, Copy, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum Phase {
    #[default]
    Idle,
    Checking,
    Current,
    Available,
    Downloading,
    Ready,
    Installing,
    Failed,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    phase: Phase,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    progress: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    checked_at: Option<f64>,
}

impl State {
    fn checked(phase: Phase, version: Option<String>, message: Option<&str>) -> Self {
        Self {
            phase,
            version,
            message: message.map(str::to_owned),
            checked_at: Some(now() - 978_307_200.),
            ..Self::default()
        }
    }
}

fn now() -> f64 {
    objc2_foundation::NSDate::timeIntervalSinceReferenceDate_class() + 978_307_200.
}

pub enum Event<'a> {
    State(&'a State),
    PrepareExit,
    Resume,
    Exit,
}

#[derive(Clone)]
struct Stage {
    update: Verified,
    directory: PathBuf,
    prepared: bool,
}

type Owner = Arc<MainThreadBound<Weak<Updater>>>;

pub struct Updater {
    owner: Owner,
    notify: Box<dyn Fn(Event<'_>)>,
    target: PathBuf,
    current: String,
    updates: PathBuf,
    policy: RefCell<Monitor>,
    state: RefCell<State>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    stage: RefCell<Option<Stage>>,
    cancel: RefCell<Option<Arc<AtomicBool>>>,
    drain: RefCell<Option<mpsc::Sender<bool>>>,
    paused: Cell<bool>,
    live: Cell<bool>,
}

impl Updater {
    pub fn new(notify: impl Fn(Event<'_>) + 'static) -> Option<Rc<Self>> {
        let main = MainThreadMarker::new()?;
        let bundle = NSBundle::mainBundle();
        let target = PathBuf::from(bundle.bundlePath().to_string());
        let current = bundle
            .objectForInfoDictionaryKey(&NSString::from_str("CFBundleShortVersionString"))
            .and_then(|v| v.downcast::<NSString>().ok())
            .map_or_else(|| "dev".to_owned(), |v| v.to_string());
        let updates = data_root().join("Updates");
        let mut policy = Monitor::default();
        let mut state = State::default();
        if let Some(previous) = read_file(&updates.join("last-result.json"), 4095)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<InstallResult>(&bytes).ok())
            .filter(|result| !result.success)
        {
            policy.block(&previous.version);
            state = State {
                phase: Phase::Failed,
                version: Some(previous.version),
                message: Some(previous.message),
                ..State::default()
            };
        }
        Some(Rc::new_cyclic(|weak| Self {
            owner: Arc::new(MainThreadBound::new(weak.clone(), main)),
            notify: Box::new(notify),
            target,
            current,
            updates,
            policy: RefCell::new(policy),
            state: RefCell::new(state),
            timer: RefCell::default(),
            stage: RefCell::default(),
            cancel: RefCell::default(),
            drain: RefCell::default(),
            paused: Cell::new(false),
            live: Cell::new(true),
        }))
    }

    fn publish(&self) {
        let snapshot = self.state.borrow().clone();
        (self.notify)(Event::State(&snapshot));
    }

    fn set(&self, state: State) {
        self.state.replace(state);
        self.publish();
    }

    pub fn configure(&self, automatic: bool, can_install: bool, suspended: bool) {
        let action = self
            .policy
            .borrow_mut()
            .configure(automatic, can_install, suspended);
        self.publish();
        match action {
            OnConfigure::Install => self.install(false),
            OnConfigure::Cancel => {
                if let Some(cancel) = self.cancel.borrow().as_ref() {
                    cancel.store(true, Ordering::Release);
                }
                self.schedule();
            }
            OnConfigure::Schedule => self.schedule(),
        }
    }

    pub fn check(&self) {
        self.begin_check(true, false);
    }

    pub fn install_now(&self) {
        if self.stage.borrow().is_some() {
            self.install(true);
        } else {
            self.begin_check(true, true);
        }
    }

    fn begin_check(&self, manual: bool, force: bool) {
        if !self.policy.borrow_mut().begin_check(manual) {
            return;
        }
        self.invalidate_timer();
        self.policy.borrow_mut().attempted(now());
        let previous = self.stage.take();
        self.start(Operation::Check { previous, force });
    }

    fn install(&self, force: bool) {
        if self.policy.borrow_mut().begin_install(force) {
            self.start(Operation::Install { force });
        }
    }

    fn start(&self, operation: Operation) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel.replace(Some(cancel.clone()));
        let worker = Worker {
            owner: self.owner.clone(),
            cancel,
            target: self.target.clone(),
            current: self.current.clone(),
            updates: self.updates.clone(),
        };
        let check = matches!(operation, Operation::Check { .. });
        if check {
            self.set(State {
                phase: Phase::Checking,
                ..State::default()
            });
        }
        if thread::Builder::new()
            .name("QuotaBar update".into())
            .spawn(move || worker.run(operation))
            .is_err()
        {
            self.finished(Err(if check {
                Error::Transfer(super::download::Error::Download)
            } else {
                Error::Handoff
            }));
        }
    }

    fn finished(&self, result: Result<(), Error>) {
        let mut cancelled = false;
        match result {
            Err(Error::Stopped) => {
                cancelled = true;
                if let Some(staged) = self.stage.take() {
                    let _ = discard(&staged.directory, &self.target);
                }
                self.set(State::default());
            }
            Err(Error::Transfer(error)) => {
                let version = self.state.borrow().version.clone();
                self.set(State::checked(
                    Phase::Failed,
                    version,
                    Some(worker::transfer_message(error)),
                ));
            }
            Err(Error::Handoff) => {
                self.resume();
                let version = self.state.borrow().version.clone();
                if let Some(version) = &version {
                    self.policy.borrow_mut().block(version);
                }
                self.set(State::checked(
                    Phase::Failed,
                    version,
                    Some("업데이트 설치를 시작하지 못했습니다. 현재 앱을 유지합니다."),
                ));
            }
            Ok(()) => {}
        }
        self.cancel.take();
        self.policy.borrow_mut().finish(cancelled);
        self.schedule();
    }

    fn resume(&self) {
        if self.paused.replace(false) {
            (self.notify)(Event::Resume);
        }
    }

    pub fn drained(&self, saved: bool) {
        if let Some(done) = self.drain.take() {
            let _ = done.send(saved);
        }
    }

    fn invalidate_timer(&self) {
        if let Some(timer) = self.timer.take() {
            timer.invalidate();
        }
    }

    fn schedule(&self) {
        self.invalidate_timer();
        let Some(delay) = self.policy.borrow().delay(now()) else {
            return;
        };
        let owner = self.owner.clone();
        let block = RcBlock::new(move |_| {
            let main = MainThreadMarker::new().expect("main run loop timer");
            if let Some(updater) = owner.get(main).upgrade() {
                updater.begin_check(false, false);
            }
        });
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(delay, false, &block) };
        timer.setTolerance((delay / 10.).min(60.));
        self.timer.replace(Some(timer));
    }

    pub fn stop(&self) {
        self.live.set(false);
        self.invalidate_timer();
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.drain.take();
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        self.stop();
    }
}
