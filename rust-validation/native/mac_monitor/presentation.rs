//! Main-thread presentation and lifetime of the resident menu and its disposable dashboard.
use crate::{
    dashboard::{
        MAX_FRAME,
        macos::host::{Event as HostEvent, Host},
    },
    mac_menu::{self, Status},
};
use dispatch2::{DispatchQueue, DispatchTime, MainThreadBound};
use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use serde::Deserialize;
use std::{
    cell::{Cell, OnceCell},
    ffi::{OsString, c_void},
    rc::{Rc, Weak},
    sync::Arc,
    time::Duration,
};

const MENU: u32 = 1;
pub(super) const DASHBOARD: u32 = 2;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum WindowCommand {
    Ready {},
    Route { _0: String },
    ShowPopover {},
    Quit {},
}

pub struct Presentation {
    main: MainThreadMarker,
    owner: Arc<MainThreadBound<Weak<Self>>>,
    store: OnceCell<Rc<super::store::Store>>,
    host: OnceCell<Rc<Host>>,
    status: OnceCell<Rc<Status>>,
    active: Cell<bool>,
    pending: Cell<bool>,
    route: Cell<&'static str>,
    demo: bool,
    menu_only: bool,
    probe: bool,
}

impl Presentation {
    pub(super) fn new(main: MainThreadMarker, arguments: &[OsString]) -> Rc<Self> {
        let has = |flag: &str| arguments.iter().any(|arg| arg == flag);
        Rc::new_cyclic(|weak| Self {
            main,
            store: OnceCell::new(),
            owner: Arc::new(MainThreadBound::new(weak.clone(), main)),
            host: OnceCell::new(),
            status: OnceCell::new(),
            active: Cell::new(false),
            pending: Cell::new(false),
            route: Cell::new("dashboard"),
            demo: has("--demo"),
            menu_only: has("--menu-bar-only"),
            probe: has("--memory-probe") && has("--diagnostics"),
        })
    }

    fn store(&self) -> &super::store::Store {
        self.store
            .get()
            .expect("resident store initialized at launch")
    }

    fn snapshot(&self, targets: u32) {
        self.send(targets, &self.store().snapshot(targets & DASHBOARD != 0));
    }

    pub(super) fn launch(self: &Rc<Self>, arguments: &[OsString]) {
        let store = super::store::Store::new(self.main, Rc::downgrade(self), arguments);
        let _ = self.store.set(store);
        self.store().start();
        let login_launch = super::store::login_launch();
        let context = Rc::as_ptr(self).cast_mut().cast();
        if let Some(host) = Host::new(context, host_event, self.demo) {
            let _ = self.host.set(host);
        }
        if let Some(status) = Status::monitor(context, menu_event) {
            let _ = self.status.set(status);
        }
        self.store().apply_dock();
        self.active.set(true);
        self.snapshot(MENU);
        if !login_launch && !self.menu_only {
            self.reopen();
        }
        self.record("ui-launched");
        if self.probe {
            self.probe_step(0);
        }
    }

    pub(super) fn reopen(&self) {
        if self.active.get() {
            self.show(self.route.get());
        }
    }

    fn show(&self, route: &str) {
        if let Some(host) = self.host.get() {
            host.show(route);
        }
    }

    pub(super) fn terminate(&self) {
        if self.active.replace(false) {
            if let Some(host) = self.host.get() {
                host.stop();
            }
            self.store().stop();
        }
    }

    pub fn changed(&self) {
        if !self.active.get() || self.pending.replace(true) {
            return;
        }
        let owner = self.owner.clone();
        DispatchQueue::main().exec_async(move || {
            let main = MainThreadMarker::new().expect("main dispatch queue");
            if let Some(presentation) = owner.get(main).upgrade() {
                presentation.pending.set(false);
                if presentation.active.get() {
                    let ready = presentation.host.get().is_some_and(|host| host.ready());
                    presentation.snapshot(if ready { MENU | DASHBOARD } else { MENU });
                }
            }
        });
    }

    /// A ready handshake or request reply can deliver its state synchronously,
    /// independently of a pending menu refresh on the next turn of the run loop.
    pub fn send(&self, targets: u32, data: &[u8]) {
        if !self.active.get() {
            return;
        }
        if targets & MENU != 0
            && let Some(status) = self.status.get()
            && let Ok(view) = crate::tray::mac_dashboard_popover(data, self.demo)
        {
            status.update(view);
        }
        if targets & DASHBOARD != 0
            && let Some(host) = self.host.get()
        {
            if (1..=MAX_FRAME).contains(&data.len()) {
                host.send(data);
            } else {
                host.close();
            }
        }
    }

    fn record(&self, name: &str) {
        let running = self.host.get().is_some_and(|host| host.running());
        let shown = self.status.get().is_some_and(|status| status.shown());
        self.store().record(
            name,
            serde_json::json!({
                "dashboardRetained":running, "popoverRetained":shown, "popoverVisible":shown
            }),
        );
    }

    fn command(&self, data: &[u8]) -> i32 {
        match serde_json::from_slice::<WindowCommand>(data) {
            Ok(WindowCommand::Ready {}) => return 1,
            Ok(WindowCommand::Route { _0: route }) => {
                let route = match route.as_str() {
                    "dashboard" => "dashboard",
                    "accounts" => "accounts",
                    "settings" => "settings",
                    _ => return 0,
                };
                self.route.set(route);
                self.changed();
            }
            Ok(WindowCommand::ShowPopover {}) => {
                if let Some(status) = self.status.get() {
                    status.show_popover();
                }
            }
            Ok(WindowCommand::Quit {}) => {
                NSApplication::sharedApplication(self.main).terminate(None)
            }
            Err(_) => return self.store().command(data),
        }
        0
    }

    fn probe_step(&self, index: usize) {
        use ProbeStep::*;
        let steps: &[(ProbeStep, u64)] = if self.menu_only {
            &[(OpenPopover, 2), (ClosePopover, 1)]
        } else {
            &[
                (Show("dashboard"), 5),
                (Show("accounts"), 2),
                (Show("settings"), 2),
                (CloseDashboard, 2),
                (OpenPopover, 2),
                (ClosePopover, 1),
            ]
        };
        if index == 2 * steps.len() {
            self.record("memory-probe-idle");
            return;
        }
        let (step, delay) = &steps[index % steps.len()];
        match step {
            Show(route) => self.show(route),
            CloseDashboard => {
                if let Some(host) = self.host.get() {
                    host.close();
                }
            }
            OpenPopover => {
                if let Some(status) = self.status.get() {
                    status.show_popover();
                }
            }
            ClosePopover => {
                if let Some(status) = self.status.get() {
                    status.close();
                }
            }
        }
        let owner = self.owner.clone();
        let when = DispatchTime::try_from(Duration::from_secs(*delay)).expect("short probe delay");
        let _ = DispatchQueue::main().after(when, move || {
            let main = MainThreadMarker::new().expect("main dispatch queue");
            if let Some(presentation) = owner.get(main).upgrade().filter(|p| p.active.get()) {
                presentation.probe_step(index + 1);
            }
        });
    }
}

enum ProbeStep {
    Show(&'static str),
    CloseDashboard,
    OpenPopover,
    ClosePopover,
}

extern "C" fn menu_event(context: *mut c_void, event: i32) {
    // The application delegate owns this controller and both callback sources.
    // Host deliveries hold weak owners and AppKit invokes the target on the main thread.
    let presentation = unsafe { &*context.cast::<Presentation>() };
    if !presentation.active.get() {
        return;
    }
    match event {
        mac_menu::OPENED => presentation.record("popover-opened"),
        mac_menu::CLOSED => {
            presentation.store().reclaim();
            presentation.record("popover-closed");
        }
        mac_menu::REFRESH => {
            presentation.store().refresh_all();
        }
        mac_menu::DASHBOARD => presentation.show("dashboard"),
        mac_menu::SETTINGS => presentation.show("settings"),
        _ => {}
    }
}

extern "C" fn host_event(
    context: *mut c_void,
    event: HostEvent,
    value: u32,
    data: *const u8,
    len: usize,
) -> i32 {
    let presentation = unsafe { &*context.cast::<Presentation>() };
    if !presentation.active.get() {
        return -1;
    }
    let store = presentation.store();
    match event {
        HostEvent::Started => {
            store.dashboard_running(true);
            store.record(
                "dashboard-process-started",
                serde_json::json!({"pid":value}),
            );
        }
        HostEvent::Exited => {
            if value != 0 {
                store.message(Some(
                    "Dashboard가 응답하지 않습니다. 다시 열어 주세요.".into(),
                ));
            }
            store.set_visible(false);
            store.dashboard_running(false);
            store.record("dashboard-process-exited", serde_json::Value::Null);
        }
        HostEvent::MissingHelper => store.message(Some(
            "Dashboard 실행 파일을 찾지 못했습니다. 앱을 다시 설치해 주세요.".into(),
        )),
        HostEvent::LaunchFailed => {
            store.dashboard_running(false);
            store.message(Some(
                "Dashboard를 열지 못했습니다. 앱을 다시 실행해 주세요.".into(),
            ));
        }
        HostEvent::PublishState => presentation.snapshot(DASHBOARD),
        HostEvent::Command => {
            return presentation.command(unsafe { std::slice::from_raw_parts(data, len) });
        }
    }
    0
}
