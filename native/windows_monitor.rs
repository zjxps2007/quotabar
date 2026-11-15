//! The Windows resident process owns the user lease, account store, native tray and
//! disposable Dashboard. Win32 callbacks queue work so nested window messages
//! never borrow the account store while it is publishing a revision.
mod display;
mod errors;
mod probe;
mod store;

use crate::{
    dashboard::windows::host,
    instance::Instance,
    login_item,
    monitor_loop::{Event, MonitorLoop},
    notification_icon::NotificationIcon,
    schedule::Refresh,
    tray_menu::{self, Menu},
    updates::windows::{
        acknowledge,
        resident::{self, Updater},
    },
    windows_settings::Preferences,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    env, fs, io,
    path::{Path, PathBuf},
    ptr::null_mut,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use store::{Reply, Result, Store};
use uuid::Uuid;
use windows_sys::{
    Win32::{
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
        UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    },
    core::w,
};

const VERSION: &str = env!("QUOTABAR_VERSION");
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Frame {
    version: i32,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    request_id: Option<UuidString>,
    #[serde(default)]
    payload: Value,
}
#[derive(Deserialize)]
#[serde(try_from = "String")]
struct UuidString(String);
impl TryFrom<String> for UuidString {
    type Error = uuid::Error;
    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        Ok(Self(Uuid::parse_str(&s)?.to_string()))
    }
}
enum Input {
    System(Event),
    Menu(i32, i32),
    Action(i32),
}
type Queue = Rc<RefCell<VecDeque<Input>>>;
struct Listener {
    instance: Arc<Instance>,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.instance.stop_listening();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct App {
    // Retire workers and the activation listener before destroying their target window.
    listener: Option<Listener>,
    activated: Arc<AtomicBool>,
    updater: Option<Updater>,
    store: Option<Store>,
    icon: Box<NotificationIcon>,
    menu: Box<Menu>,
    dispatcher: Box<MonitorLoop>,
    input: Queue,
    sample: Value,
    exe: PathBuf,
    folder: PathBuf,
    visible: bool,
    suspended: bool,
    quitting: bool,
    route: String,
    captured: Option<String>,
    startup: &'static str,
    last_refresh: Instant,
    resume_at: Option<Instant>,
    update_draining: bool,
    update_exit: bool,
    scan_after_refresh: bool,
    icon_size: i32,
}

fn startup(exe: &Path) -> &'static str {
    match login_item::windows::state(exe, Path::new(RUN_KEY)) {
        Ok(login_item::State::Enabled) => "enabled",
        Ok(login_item::State::Disabled) => "disabled",
        _ => "unavailable",
    }
}
impl App {
    fn new(exe: PathBuf, root: Option<PathBuf>, probe: bool) -> Result<Self> {
        let input = Queue::default();
        let events = input.clone();
        let target = Rc::new(Cell::new(0));
        let hwnd = target.clone();
        let dispatcher = MonitorLoop::new(move |event| {
            events.borrow_mut().push_back(Input::System(event));
            // A synchronous broadcast may arrive inside GetMessage while it is waiting.
            if matches!(
                event,
                Event::Suspend | Event::Resume | Event::PowerStatus | Event::Display
            ) {
                MonitorLoop::wake(hwnd.get());
            }
        })
        .map_err(errors::file)?;
        let hwnd = dispatcher.window() as isize;
        target.set(hwnd);
        let events = input.clone();
        let menu = Menu::with_callback(move |n| {
            events.borrow_mut().push_back(Input::Action(n));
            MonitorLoop::wake(hwnd);
        })
        .map_err(errors::file)?;
        let events = input.clone();
        let icon = NotificationIcon::with_callback(move |x, y| {
            events.borrow_mut().push_back(Input::Menu(x, y));
            MonitorLoop::wake(hwnd);
        })
        .map_err(errors::file)?;
        let folder = exe.parent().ok_or(errors::IO)?.to_owned();
        let store = root
            .clone()
            .map(|root| {
                Store::new(root, move || {
                    MonitorLoop::wake(hwnd);
                })
            })
            .transpose()?;
        let updater = root.map(|root| {
            Updater::new(root, folder.clone(), VERSION.into(), move || {
                MonitorLoop::update_input(hwnd)
            })
        });
        let sample = if store.is_none() {
            display::demo(probe)
        } else {
            Value::Null
        };
        let startup = if store.is_some() {
            startup(&exe)
        } else {
            "disabled"
        };
        Ok(Self {
            listener: None,
            activated: Arc::default(),
            updater,
            store,
            icon,
            menu,
            dispatcher,
            input,
            sample,
            exe,
            folder,
            visible: false,
            suspended: false,
            quitting: false,
            route: "dashboard".into(),
            captured: None,
            startup,
            last_refresh: Instant::now(),
            resume_at: None,
            update_draining: false,
            update_exit: false,
            scan_after_refresh: false,
            icon_size: 0,
        })
    }
    fn listen(&mut self, instance: Arc<Instance>) {
        let received = self.activated.clone();
        let pipe = instance.clone();
        let hwnd = self.dispatcher.window() as isize;
        let worker = thread::spawn(move || {
            while let Ok(true) = pipe.wait_for_activation() {
                received.store(true, Ordering::Release);
                MonitorLoop::wake(hwnd);
            }
        });
        self.listener = Some(Listener {
            instance,
            thread: Some(worker),
        });
    }
    fn state(&self) -> Value {
        self.store.as_ref().map_or_else(
            || self.sample.clone(),
            |s| {
                s.state(
                    self.startup,
                    json!(self.updater.as_ref().map(Updater::state)),
                )
            },
        )
    }
    fn send(&self, kind: &str, payload: &Value, id: &Value) {
        self.dispatcher.dashboard.send(
            &serde_json::to_vec(&json!({"version":1,"type":kind,"requestId":id,"payload":payload}))
                .unwrap(),
        );
    }
    fn publish(&mut self) {
        if self.quitting {
            return;
        }
        let state = self.state();
        let summary = display::summary(&state);
        if let Ok(size) = self.icon.update(
            &summary.text,
            summary.use_logo,
            summary.warning,
            &summary.tooltip,
        ) {
            self.icon_size = size
        }
        if self.menu.visible() {
            self.menu.update(&state)
        }
        if self.dispatcher.dashboard.ready() {
            self.send("state", &state, &Value::Null)
        }
        self.configure();
    }
    fn configure(&mut self) {
        if self.quitting {
            return;
        }
        if let (Some(s), Some(updater)) = (&self.store, &mut self.updater) {
            updater.configure(
                s.settings.automatic_updates,
                !self.visible && self.dispatcher.dashboard.pid() == 0,
                self.suspended || s.logging_in(),
            );
            let (delay, _) = Refresh {
                suspended: self.suspended,
                automatic: s.settings.auto_refresh,
                has_accounts: s.settings.accounts.iter().any(|a| a.enabled),
                dashboard_visible: self.visible,
                interval: s.settings.refresh_seconds,
                ..Default::default()
            }
            .delays();
            let next = self
                .resume_at
                .or_else(|| delay.map(|s| self.last_refresh + Duration::from_secs(s as u64)));
            let _ = self.dispatcher.timer(
                0,
                next.map_or(0, |t| {
                    t.saturating_duration_since(Instant::now())
                        .as_millis()
                        .clamp(1000, i32::MAX as u128) as u32
                }),
            );
            let _ = self.dispatcher.timer(1, updater.delay_ms());
        }
    }
    fn update_events(&mut self) {
        while let Some(event) = self.updater.as_mut().and_then(Updater::next_event) {
            match event {
                resident::Event::State(_) => self.publish(),
                resident::Event::PrepareExit => {
                    self.update_draining = true;
                    if let Some(s) = &mut self.store {
                        s.paused = true;
                        s.suspend();
                    }
                    self.finish_work();
                }
                resident::Event::Resume => {
                    if let Some(s) = &mut self.store {
                        s.paused = false
                    }
                    self.configure();
                }
                resident::Event::Exit => {
                    self.update_exit = true;
                    self.quit();
                }
            }
        }
    }
    fn quit(&mut self) {
        if !self.quitting {
            self.quitting = true;
            let _ = self.dispatcher.timer(0, 0);
            let _ = self.dispatcher.timer(1, 0);
            self.dispatcher.dashboard.close();
            if let Some(s) = &mut self.store {
                s.shutdown();
            }
        }
        self.finish_work()
    }
    fn finish_work(&mut self) {
        if self.quitting {
            if self.store.as_ref().is_none_or(Store::finished) {
                if self.update_exit {
                    self.update_exit = false;
                    self.updater.as_mut().unwrap().exited(true);
                }
                self.dispatcher.quit();
            }
        } else if self.update_draining && self.store.as_ref().is_some_and(Store::drained) {
            self.update_draining = false;
            let result = self
                .store
                .as_ref()
                .unwrap()
                .flush()
                .map_err(|_| "자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다.");
            self.updater.as_mut().unwrap().drained(result);
        } else if self.scan_after_refresh && self.store.as_ref().is_some_and(Store::fetches_done) {
            self.scan_after_refresh = false;
            self.scan();
        }
    }
    fn open(&mut self, route: &str) {
        if self.quitting {
            return;
        }
        self.route = route.into();
        self.visible = true;
        self.configure();
        let exe = self.folder.join("Dashboard/QuotaBar.Dashboard.exe");
        if let Err(e) = self.dispatcher.dashboard.show(&exe, None, route) {
            self.visible = self.dispatcher.dashboard.pid() != 0;
            self.configure();
            tray_menu::show_message(
                if e.kind() == io::ErrorKind::NotFound {
                    "Dashboard 파일을 찾지 못했습니다. 패키지를 다시 설치하세요."
                } else {
                    "실행 파일을 찾거나 시작하지 못했습니다. CLI 설치를 확인하세요."
                },
                false,
            );
        }
    }
    fn scan(&mut self) {
        if self.visible
            && self.route == "dashboard"
            && !self.suspended
            && let Some(s) = &mut self.store
        {
            s.scan();
        }
    }
    fn view_changed(&mut self) {
        let was_scanning = self.store.as_ref().is_some_and(Store::scanning);
        if self.visible && self.route == "dashboard" {
            self.scan();
        } else if let Some(store) = &mut self.store {
            store.stop_scan();
        }
        if was_scanning != self.store.as_ref().is_some_and(Store::scanning) {
            self.publish();
        } else {
            self.configure();
        }
    }
    fn refresh(&mut self, automatic: bool) {
        if self.suspended
            || self.quitting
            || automatic
                && self
                    .store
                    .as_ref()
                    .is_some_and(|s| !s.settings.auto_refresh)
        {
            self.configure();
            return;
        }
        self.last_refresh = Instant::now();
        if let Some(s) = &mut self.store {
            s.refresh(None, None, false);
            self.scan_after_refresh = true;
        }
        self.finish_work();
        self.publish();
    }
    fn dashboard_input(&mut self) {
        let command = RefCell::new(None);
        let exited = Cell::new(false);
        self.dispatcher.dashboard.dispatch(|event, bytes| {
            match event {
                host::Event::Command => match serde_json::from_slice::<Frame>(bytes) {
                    Ok(f) if f.version == 1 => {
                        if f.kind == "ready" {
                            return 1;
                        }
                        command.replace(Some(f));
                    }
                    _ => return -1,
                },
                host::Event::Publish => self.send("state", &self.state(), &Value::Null),
                host::Event::Exited => exited.set(true),
            }
            0
        });
        if let Some(f) = command.into_inner() {
            self.command(f)
        }
        if exited.get() {
            self.visible = false;
            if let Some(s) = &mut self.store {
                s.stop_scan();
            }
            self.configure();
        }
    }
    fn reply(&mut self, r: Reply, result: Result<()>) {
        if let Err(e) = &result
            && let Some(s) = &mut self.store
        {
            s.message = Some(e.clone());
            self.publish();
        }
        if self.dispatcher.dashboard.pid() == r.pid {
            let payload = match result {
                Ok(()) => json!({"success":true}),
                Err(error) => json!({"success":false,"error":error}),
            };
            self.send("result", &payload, &r.id);
        }
    }
    fn command(&mut self, f: Frame) {
        let r = Reply {
            pid: self.dispatcher.dashboard.pid(),
            id: json!(f.request_id.map(|id| id.0)),
        };
        match self.apply(&f.kind, &f.payload, &r) {
            Ok(false) => {}
            result => self.reply(r, result.map(|_| ())),
        }
    }
    fn apply(&mut self, kind: &str, p: &Value, r: &Reply) -> Result<bool> {
        let text = |v: &Value| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| errors::UNKNOWN.to_owned())
        };
        let boolean = |v: &Value| v.as_bool().ok_or_else(|| errors::UNKNOWN.to_owned());
        let id = |v: &Value| {
            Uuid::parse_str(v.as_str().unwrap_or_default())
                .map(|v| v.to_string())
                .map_err(|_| errors::UNKNOWN.to_owned())
        };
        match kind {
            "captured" => {
                self.captured = Some(text(p)?);
                return Ok(false);
            }
            "visibility" => {
                self.visible = boolean(p)?;
                self.view_changed();
                return Ok(false);
            }
            "route" => {
                self.route = text(p)?;
                self.view_changed();
                return Ok(false);
            }
            "quit" => {
                self.quit();
                return Ok(false);
            }
            "refresh" => {
                self.refresh(false);
                return Ok(false);
            }
            "refresh-account" => {
                if !self.suspended
                    && let Some(s) = &mut self.store
                {
                    s.refresh(Some(&id(p)?), None, false);
                    self.publish();
                }
                return Ok(false);
            }
            "scan" => {
                self.scan();
                self.publish();
                return Ok(false);
            }
            "startup-status" => {
                if self.store.is_some() {
                    self.startup = startup(&self.exe);
                    self.publish();
                }
                return Ok(false);
            }
            "startup-settings" => {
                if self.store.is_some()
                    && unsafe {
                        ShellExecuteW(
                            null_mut(),
                            w!("open"),
                            w!("ms-settings:startupapps"),
                            std::ptr::null(),
                            std::ptr::null(),
                            SW_SHOWNORMAL,
                        )
                    } as isize
                        <= 32
                {
                    return Err(
                        "실행 파일을 찾거나 시작하지 못했습니다. CLI 설치를 확인하세요.".into(),
                    );
                }
                return Ok(false);
            }
            "startup" => {
                let enabled = boolean(p)?;
                if self.store.is_some() {
                    let result = login_item::windows::set(&self.exe, Path::new(RUN_KEY), enabled);
                    self.startup = startup(&self.exe);
                    self.publish();
                    result.map_err(|e|if e.kind()==io::ErrorKind::InvalidData{"설치된 QuotaBar 앱에서 자동 실행을 설정하세요. 실행 파일 경로가 너무 길면 더 짧은 경로를 사용하세요."}else{errors::IO})?;
                } else {
                    self.sample["launchAtLogin"] =
                        json!(if enabled { "enabled" } else { "disabled" });
                    self.publish();
                }
                return Ok(false);
            }
            _ => {}
        }
        let Some(s) = &mut self.store else {
            if kind == "preferences" {
                for (from, to) in [
                    ("seconds", "refreshSeconds"),
                    ("theme", "appearance"),
                    ("scan", "scanLocalLogs"),
                    ("taskbar", "showTaskbarIcon"),
                    ("trayDisplay", "trayDisplayMode"),
                    ("autoRefresh", "autoRefresh"),
                    ("automaticUpdates", "automaticUpdates"),
                ] {
                    if let Some(v) = p.get(from) {
                        self.sample["settings"][to] = v.clone();
                    }
                }
            } else {
                self.sample["message"] = json!(
                    "미리보기 모드입니다. 실제 계정을 연결하려면 앱을 종료하고 일반 모드로 실행하세요."
                );
            }
            self.publish();
            return Ok(false);
        };
        let mut immediate = true;
        match kind {
            "check-app-update" => self.updater.as_mut().unwrap().check(true, false),
            "install-app-update" => self.updater.as_mut().unwrap().install_now(),
            "preferences" => s.preferences(
                serde_json::from_value::<Preferences>(p.clone()).map_err(|_| errors::UNKNOWN)?,
            )?,
            "edit" => s.edit(
                &id(&p["id"])?,
                &text(&p["name"])?,
                boolean(&p["enabled"])?,
                boolean(&p["tray"])?,
            )?,
            "remove" => s.remove(&id(p)?)?,
            "cancel-login" => s.cancel_login(),
            "login" => {
                s.login(r.clone())?;
                immediate = false;
            }
            "import" => {
                let provider =
                    serde_json::from_value(json!(text(&p["provider"])?.to_ascii_lowercase()))
                        .map_err(|_| errors::UNKNOWN)?;
                s.import(provider, &text(&p["path"])?, r.clone())?;
                immediate = false;
            }
            "claude" => {
                s.connect_claude(
                    Path::new(&text(&p["path"])?),
                    boolean(&p["consent"])?,
                    &self.folder.join("StatusLine/QuotaBar.StatusLine.exe"),
                    r.clone(),
                )?;
                immediate = false;
            }
            _ => return Err("지원하지 않는 요청입니다.".into()),
        }
        self.publish();
        self.flush_replies();
        Ok(immediate)
    }
    fn flush_replies(&mut self) {
        if let Some(s) = &mut self.store {
            for (r, result) in std::mem::take(&mut s.replies) {
                self.reply(r, result)
            }
        }
    }
    fn events(&mut self) {
        loop {
            let next = self.input.borrow_mut().pop_front();
            let Some(event) = next else { break };
            match event {
                Input::Menu(x, y) => {
                    if !self.quitting {
                        self.menu.update(&self.state());
                        self.menu.show(x, y);
                    }
                }
                Input::Action(0) => self.icon.return_focus(),
                Input::Action(1) => self.open("dashboard"),
                Input::Action(2) => self.quit(),
                Input::Action(_) => {}
                Input::System(Event::Dashboard) => self.dashboard_input(),
                Input::System(Event::UpdateWork) => {
                    if let Some(u) = &mut self.updater {
                        u.dispatch();
                        let _ = self.dispatcher.timer(1, u.delay_ms());
                    }
                }
                Input::System(Event::Update) => {
                    if let Some(u) = &mut self.updater {
                        u.check(false, false);
                        let _ = self.dispatcher.timer(1, u.delay_ms());
                    }
                }
                Input::System(Event::Work) => {
                    if self.activated.swap(false, Ordering::AcqRel) {
                        self.open("dashboard")
                    }
                    if let Some(s) = &mut self.store
                        && s.dispatch()
                    {
                        self.finish_work();
                        self.publish();
                        self.flush_replies();
                    }
                }
                Input::System(Event::Refresh) => {
                    self.resume_at = None;
                    self.refresh(true);
                }
                Input::System(Event::Suspend) => {
                    self.suspended = true;
                    self.resume_at = None;
                    if let Some(s) = &mut self.store {
                        s.suspend()
                    }
                    self.configure();
                }
                Input::System(Event::Resume) => {
                    self.suspended = false;
                    self.resume_at = Some(Instant::now() + Duration::from_secs(2));
                    self.configure();
                }
                Input::System(Event::PowerStatus) => {
                    self.suspended = false;
                    self.configure();
                }
                Input::System(Event::Display) => self.publish(),
            }
            self.update_events();
        }
    }
}

fn start(args: &[std::ffi::OsString], probe: Option<&Path>) -> Result<()> {
    let arg = |name: &str| args.windows(2).find(|v| v[0] == name).map(|v| &v[1]);
    if let Some(p) = arg("--update-manager-probe") {
        return resident::probe::run(Path::new(p), VERSION).map_err(errors::file);
    }
    let root = if probe.is_some() || args.iter().any(|v| v == "--demo") {
        None
    } else {
        Some(match arg("--data-dir") {
            Some(path) => std::path::absolute(path).map_err(errors::file)?,
            None => store::default_root()?,
        })
    };
    let instance = root
        .as_deref()
        .map(Instance::acquire)
        .transpose()
        .map_err(errors::file)?
        .map(Arc::new);
    let background = args.iter().any(|v| v == "--background");
    if let Some(i) = &instance
        && !i.is_owner()
    {
        if !background && i.activate().is_err() {
            tray_menu::show_message(
                "QuotaBar가 이미 실행 중입니다. 알림 영역에서 열어 주세요.",
                false,
            );
        }
        return Ok(());
    }
    let mut app = App::new(
        env::current_exe().map_err(errors::file)?,
        root,
        probe.is_some(),
    )?;
    if let Some(i) = instance {
        app.listen(i);
    }
    app.publish();
    if let Some(p) = probe {
        return probe::run(&mut app, p);
    }
    if !background {
        app.open("dashboard");
    }
    if let (Some(s), Some(tx)) = (
        &app.store,
        arg("--update-ack")
            .and_then(|v| v.to_str())
            .and_then(|s| Uuid::parse_str(s).ok()),
    ) {
        let _ = acknowledge(&s.root, &app.folder, tx, VERSION);
    }
    app.refresh(false);
    while app.dispatcher.pump(true).map_err(errors::file)? {
        app.events();
    }
    Ok(())
}

pub fn run() -> i32 {
    let com = unsafe { CoInitializeEx(null_mut(), COINIT_APARTMENTTHREADED as u32) };
    let args: Vec<_> = env::args_os().skip(1).collect();
    let probe = args
        .windows(2)
        .find(|v| v[0] == "--probe")
        .map(|v| Path::new(&v[1]));
    let result = start(&args, probe);
    if com >= 0 {
        unsafe { CoUninitialize() };
    }
    if let Err(error) = result {
        if let Some(p) = probe {
            let _ = fs::write(p, json!({"passed":false,"error":error}).to_string());
        } else {
            tray_menu::show_message(&error, true);
        }
        return 1;
    }
    0
}
