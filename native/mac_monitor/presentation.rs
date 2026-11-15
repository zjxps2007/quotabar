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

#[repr(i32)]
pub enum StoreEvent {
    Launch,
    ApplyDock,
    Terminate,
    Refresh,
    Reclaim,
    Diagnostic,
    DashboardStarted,
    DashboardExited,
    MissingHelper,
    LaunchFailed,
    Snapshot,
    Command,
}
pub type Callback =
    extern "C" fn(*mut c_void, *const Presentation, StoreEvent, u32, *const u8, usize) -> i32;

const MENU: u32 = 1;
const DASHBOARD: u32 = 2;

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
    context: *mut c_void,
    callback: Callback,
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
    pub(super) fn new(
        main: MainThreadMarker,
        context: *mut c_void,
        callback: Callback,
        arguments: &[OsString],
    ) -> Rc<Self> {
        let has = |flag: &str| arguments.iter().any(|arg| arg == flag);
        Rc::new_cyclic(|weak| Self {
            main,
            context,
            callback,
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

    fn notify(&self, event: StoreEvent, value: u32, data: &[u8]) -> i32 {
        (self.callback)(self.context, self, event, value, data.as_ptr(), data.len())
    }

    pub(super) fn launch(&self) {
        let login_launch = self.notify(StoreEvent::Launch, 0, &[]) != 0;
        let context = std::ptr::from_ref(self).cast_mut().cast();
        if let Some(host) = Host::new(context, host_event, self.demo) {
            let _ = self.host.set(host);
        }
        if let Some(status) = Status::monitor(context, menu_event) {
            let _ = self.status.set(status);
        }
        self.notify(StoreEvent::ApplyDock, 0, &[]);
        self.active.set(true);
        self.notify(StoreEvent::Snapshot, MENU, &[]);
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
            self.notify(StoreEvent::Terminate, 0, &[]);
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
                    presentation.notify(
                        StoreEvent::Snapshot,
                        if ready { MENU | DASHBOARD } else { MENU },
                        &[],
                    );
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
        self.notify(
            StoreEvent::Diagnostic,
            u32::from(running) | (u32::from(shown) << 1),
            name.as_bytes(),
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
            Err(_) => return self.notify(StoreEvent::Command, 0, data),
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
            presentation.notify(StoreEvent::Reclaim, 0, &[]);
            presentation.record("popover-closed");
        }
        mac_menu::REFRESH => {
            presentation.notify(StoreEvent::Refresh, 0, &[]);
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
    let event = match event {
        HostEvent::Started => StoreEvent::DashboardStarted,
        HostEvent::Exited => StoreEvent::DashboardExited,
        HostEvent::MissingHelper => StoreEvent::MissingHelper,
        HostEvent::LaunchFailed => StoreEvent::LaunchFailed,
        HostEvent::PublishState => {
            return presentation.notify(StoreEvent::Snapshot, DASHBOARD, &[]);
        }
        HostEvent::Command => {
            return presentation.command(unsafe { std::slice::from_raw_parts(data, len) });
        }
    };
    presentation.notify(event, value, &[])
}
