//! Resident account state lives on the AppKit main thread. Background provider requests
//! return a generation-tagged result, never a mutable reference to the published state.
mod commands;
mod frames;
mod lifecycle;
mod workers;

use super::{fetch::Snapshot, presentation::Presentation};
use crate::{
    codex_sessions::managed::ManagedAccounts,
    credentials::Platform,
    login_item,
    mac_settings::{Appearance, Settings, migration},
    schedule::{self, accounts::Accounts},
    updates::macos::resident::Updater,
};
use dispatch2::MainThreadBound;
use objc2::{MainThreadMarker, rc::Retained, runtime::ProtocolObject};
use objc2_foundation::{NSDate, NSHomeDirectory, NSObjectProtocol, NSTimer};
use serde_json::{Value, json};
use std::{
    cell::{Cell, OnceCell, RefCell},
    collections::BTreeMap,
    ffi::OsString,
    path::PathBuf,
    rc::{Rc, Weak},
    sync::{Arc, atomic::AtomicBool},
};
use uuid::Uuid;

fn now() -> f64 {
    NSDate::timeIntervalSinceReferenceDate_class()
}

pub(super) fn login_launch() -> bool {
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};
    let Some(event) = NSAppleEventManager::sharedAppleEventManager().currentAppleEvent() else {
        return false;
    };
    // CoreServices' kCoreEventClass, kAEOpenApplication, keyAEPropData and keyAELaunchedAsLogInItem.
    let class: u32 = unsafe { objc2::msg_send![&*event, eventClass] };
    let id: u32 = unsafe { objc2::msg_send![&*event, eventID] };
    if class != u32::from_be_bytes(*b"aevt") || id != u32::from_be_bytes(*b"oapp") {
        return false;
    }
    let param: Option<Retained<NSAppleEventDescriptor>> = unsafe {
        objc2::msg_send![&*event, paramDescriptorForKeyword: u32::from_be_bytes(*b"prdt")]
    };
    param.is_some_and(|p| p.enumCodeValue() == u32::from_be_bytes(*b"lgit"))
}

struct State {
    settings: Settings,
    snapshots: BTreeMap<Uuid, Snapshot>,
    errors: BTreeMap<Uuid, String>,
    notes: BTreeMap<Uuid, &'static str>,
    queue: Accounts,
    workers: BTreeMap<Uuid, Arc<AtomicBool>>,
    message: Option<String>,
    report: Option<Value>,
    login: login_item::State,
    update: Value,
}

pub(super) struct Store {
    main: MainThreadMarker,
    owner: Arc<MainThreadBound<Weak<Self>>>,
    presentation: Weak<Presentation>,
    root: PathBuf,
    sessions: Arc<ManagedAccounts>,
    state: RefCell<State>,
    demo: bool,
    healthy: bool,
    live: Cell<bool>,
    visible: Cell<bool>,
    dashboard: Cell<bool>,
    last_refresh: Cell<f64>,
    deadline: Cell<Option<f64>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
    cleanup: RefCell<Option<Retained<NSTimer>>>,
    observers: RefCell<Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
    updater: OnceCell<Rc<Updater>>,
    flushing: Cell<bool>,
    diagnostic: Option<PathBuf>,
}

impl Store {
    pub(super) fn new(
        main: MainThreadMarker,
        presentation: Weak<Presentation>,
        args: &[OsString],
    ) -> Rc<Self> {
        let home = PathBuf::from(NSHomeDirectory().to_string());
        let root = home.join("Library/Application Support/AIUsage Lite");
        let path = root.join("settings.json");
        let vault = root.join("Credentials");
        let demo = args.iter().any(|s| s == "--demo");
        let option = |flag: &str| args.windows(2).find(|w| w[0] == flag).map(|w| &w[1]);
        let mut settings = Settings::default();
        let mut message = None;
        if demo {
            settings.appearance = option("--demo-appearance").and_then(|s| match s.to_str()? {
                "light" => Some(Appearance::Light),
                "dark" => Some(Appearance::Dark),
                "system" => Some(Appearance::System),
                _ => None,
            });
        } else {
            match Settings::load(&path) {
                Err(_) => message = Some("저장된 설정을 읽을 수 없어 쓰기를 중단했습니다. settings.json을 확인한 뒤 앱을 다시 실행하세요.".into()),
                Ok(loaded) => {
                    let mut migrated = loaded.clone();
                    if migration::migrate(&mut migrated, &path, &vault, &mut migration::keychain::Keychain).is_ok() {
                        settings = migrated;
                    } else {
                        settings = loaded;
                        message = Some("로그인 저장소를 준비하지 못했습니다. 앱 데이터 폴더의 권한과 저장 공간을 확인한 뒤 다시 실행하세요.".into());
                    }
                }
            }
            if !schedule::INTERVALS
                .iter()
                .any(|&s| i64::from(s) == settings.refresh_seconds)
            {
                settings.refresh_seconds = 300;
            }
        }
        let healthy = message.is_none();
        let mut state = State {
            settings,
            message,
            snapshots: BTreeMap::new(),
            errors: BTreeMap::new(),
            notes: BTreeMap::new(),
            queue: Accounts::default(),
            workers: BTreeMap::new(),
            report: None,
            login: login_item::State::Disabled,
            update: json!({"phase":"idle"}),
        };
        if demo {
            frames::preview(&mut state, &home);
        }
        let sessions = Arc::new(ManagedAccounts::new(vault, Platform::Mac));
        Rc::new_cyclic(|weak| Self {
            main,
            owner: Arc::new(MainThreadBound::new(weak.clone(), main)),
            presentation,
            root,
            sessions,
            state: RefCell::new(state),
            demo,
            healthy,
            live: Cell::new(true),
            visible: Cell::new(false),
            dashboard: Cell::new(false),
            last_refresh: Cell::new(-63114076800.),
            deadline: Cell::default(),
            timer: RefCell::default(),
            cleanup: RefCell::default(),
            observers: RefCell::default(),
            updater: OnceCell::new(),
            flushing: Cell::new(false),
            diagnostic: option("--diagnostics").map(PathBuf::from),
        })
    }

    pub(super) fn start(&self) {
        self.apply_appearance();
        if self.demo {
            return;
        }
        self.refresh_login();
        self.observe_power();
        self.refresh_all();
        let weak = self.owner.get(self.main).clone();
        if let Some(updater) = Updater::new(move |event| {
            if let Some(store) = weak.upgrade().filter(|s| s.live.get()) {
                store.update_event(event);
            }
        }) {
            let _ = self.updater.set(updater);
        }
        self.configure_updates();
    }

    fn changed(&self) {
        if let Some(p) = self.presentation.upgrade() {
            p.changed();
        }
    }
    pub(super) fn message(&self, value: Option<String>) {
        self.state.borrow_mut().message = value;
        self.changed();
    }
    fn refresh_login(&self) {
        if !self.demo {
            self.state.borrow_mut().login = login_item::macos::state();
            self.changed();
        }
    }
    fn apply_appearance(&self) {
        let mode = self.state.borrow().settings.appearance.unwrap_or_default();
        crate::mac_menu::apply_appearance(match mode {
            Appearance::Dark => "dark",
            Appearance::Light => "light",
            Appearance::System => "system",
        });
    }
    pub(super) fn apply_dock(&self) {
        let visible = self.state.borrow().settings.show_dock_icon.unwrap_or(true);
        if !crate::mac_menu::dock_visible(visible) {
            self.message(Some(
                "Dock 표시를 변경하지 못했습니다. 앱을 다시 실행해 주세요.".into(),
            ));
        }
    }
    pub(super) fn snapshot(&self, dashboard: bool) -> Vec<u8> {
        frames::snapshot(&self.state.borrow(), dashboard)
    }
}
