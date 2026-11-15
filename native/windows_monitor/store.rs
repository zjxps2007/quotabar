//! The monitor thread owns account selection and persistence. Workers only return
//! results. A cancelled request may finish rotating a credential after a reconnect.
use super::errors;
use crate::{
    claude::connection,
    codex_sessions::managed::ManagedAccounts,
    credentials::{Platform, Provider},
    files, provider_http,
    provider_query::{Operation, Query},
    usage, vault,
    windows_format::known_folder,
    windows_login,
    windows_settings::{Account, Preferences, Settings, accounts, snapshots},
};
use chrono::{Local, Utc};
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, VecDeque},
    env,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};
use uuid::Uuid;
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, FOLDERID_Profile};
use zeroize::Zeroizing;

pub(super) type Result<T> = std::result::Result<T, String>;
type Cancel = Arc<AtomicBool>;

pub(super) fn default_root() -> Result<PathBuf> {
    Ok(known_folder(&FOLDERID_LocalAppData)
        .ok_or(errors::IO)?
        .join("QuotaBar"))
}

struct Paths {
    home: PathBuf,
    codex: PathBuf,
    claude: PathBuf,
}
impl Paths {
    fn new() -> Result<Self> {
        let home = known_folder(&FOLDERID_Profile).ok_or(errors::IO)?;
        let from_env = |key: &str, leaf: &str| {
            env::var_os(key)
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(leaf))
        };
        Ok(Self {
            codex: from_env("CODEX_HOME", ".codex"),
            claude: from_env("CLAUDE_CONFIG_DIR", ".claude"),
            home,
        })
    }
    fn defaults(&self) -> Value {
        json!({"codex":self.codex.join("auth.json"),"claude":self.claude,
               "gemini":self.home.join(".gemini").join("oauth_creds.json")})
    }
}

#[derive(Clone)]
pub(super) struct Reply {
    pub pid: u32,
    pub id: Value,
}
struct Job {
    account: Account,
    cancel: Cancel,
    reply: Option<Reply>,
    login: bool,
}
enum Fetch {
    Claude(PathBuf, PathBuf),
    Managed(Arc<ManagedAccounts>, Uuid, String),
    Imported(Operation, Zeroizing<Vec<u8>>),
}
impl Fetch {
    fn run(self, cancel: &AtomicBool) -> Result<Value> {
        let bytes = match self {
            Self::Claude(root, path) => {
                let usage = connection::fetch(&root, &path).map_err(errors::claude)?;
                serde_json::to_vec(&usage).map_err(|_| errors::INVALID)?
            }
            Self::Managed(owner, id, identity) => owner
                .fetch(id, &identity, cancel)
                .map_err(errors::managed)?,
            Self::Imported(op, auth) => {
                let query = Query::new(op, &auth, Platform::Windows, 20.)
                    .map_err(provider_http::Failure::Query);
                query
                    .and_then(|q| provider_http::run(q, cancel, None))
                    .map_err(errors::provider)?
            }
        };
        snapshots::display(&bytes).map_err(errors::settings)
    }
}

enum Event {
    Fetched(Job, Result<Value>),
    LoggedIn(Reply, Result<Zeroizing<Vec<u8>>>),
    Scanned(usage::Scanner, Result<Value>),
}
#[derive(Clone)]
struct Outbox {
    tx: Sender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}
impl Outbox {
    fn send(&self, e: Event) {
        if self.tx.send(e).is_ok() {
            (self.wake)();
        }
    }
}

pub(super) struct Store {
    pub root: PathBuf,
    paths: Paths,
    pub settings: Settings,
    snapshots: Map<String, Value>,
    errors: HashMap<String, String>,
    pub message: Option<String>,
    activity: Option<Value>,
    sessions: Arc<ManagedAccounts>,
    requests: HashMap<String, Cancel>,
    queue: VecDeque<Job>,
    active: usize,
    scanner: Option<usage::Scanner>,
    scan_cancel: Option<Cancel>,
    login_cancel: Option<Cancel>,
    pub paused: bool,
    quitting: bool,
    inbox: Receiver<Event>,
    outbox: Outbox,
    pub replies: Vec<(Reply, Result<()>)>,
}

impl Store {
    pub fn new(root: PathBuf, wake: impl Fn() + Send + Sync + 'static) -> Result<Self> {
        files::ensure_private_directory(&root).map_err(errors::file)?;
        let settings = Settings::load(&root).map_err(errors::settings)?;
        let ids = settings.accounts.iter().map(|a| a.id.clone()).collect();
        let (snapshots, message) = match snapshots::load(&root, &ids) {
            Ok(bytes) => (
                serde_json::from_slice(&bytes).map_err(|_| errors::INVALID)?,
                None,
            ),
            Err(crate::windows_settings::Error::Io(e))
                if e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                return Err(errors::file(e));
            }
            Err(crate::windows_settings::Error::Usage(_, msg)) => return Err(msg.into()),
            Err(_) => (
                Map::new(),
                Some("이전 사용량 캐시를 읽지 못했습니다. 새로고침하면 다시 저장합니다.".into()),
            ),
        };
        let (tx, inbox) = mpsc::channel();
        let sessions = Arc::new(ManagedAccounts::new(
            root.join("Credentials"),
            Platform::Windows,
        ));
        Ok(Self {
            root,
            paths: Paths::new()?,
            settings,
            snapshots,
            message,
            sessions,
            errors: HashMap::new(),
            activity: None,
            requests: HashMap::new(),
            queue: VecDeque::new(),
            active: 0,
            scanner: Some(usage::Scanner::default()),
            scan_cancel: None,
            login_cancel: None,
            paused: false,
            quitting: false,
            inbox,
            outbox: Outbox {
                tx,
                wake: Arc::new(wake),
            },
            replies: Vec::new(),
        })
    }

    pub fn state(&self, startup: &str, update: Value) -> Value {
        json!({"settings":self.settings,"snapshots":self.snapshots,"errors":self.errors,
             "loading":self.settings.accounts.iter().filter(|a|self.requests.contains_key(&a.id)).map(|a| &a.id).collect::<Vec<_>>(),
             "message":self.message,"demo":false,"activity":self.activity,"scanning":self.scan_cancel.is_some(),
             "loggingIn":self.logging_in(),"dataDirectory":self.root,"defaultPaths":self.paths.defaults(),"launchAtLogin":startup,"appUpdate":update})
    }
    fn save_snapshots(&self) -> Result<()> {
        snapshots::save(
            &self.root,
            &serde_json::to_vec(&self.snapshots).map_err(|_| errors::INVALID)?,
        )
        .map_err(errors::settings)
    }
    fn commit(&mut self, settings: Settings) -> Result<()> {
        settings.save(&self.root).map_err(errors::settings)?;
        self.settings = settings;
        Ok(())
    }
    pub fn preferences(&mut self, options: Preferences) -> Result<()> {
        let mut next = self.settings.clone();
        next.preferences(options).map_err(errors::settings)?;
        self.commit(next)?;
        if !self.settings.scan_local_logs {
            self.stop_scan();
            self.activity = None;
        }
        Ok(())
    }
    fn cancel(&mut self, id: &str) {
        if let Some(token) = self.requests.remove(id) {
            token.store(true, Ordering::Release);
        }
    }
    pub fn edit(&mut self, id: &str, name: &str, enabled: bool, tray: bool) -> Result<()> {
        let mut next = self.settings.clone();
        next.edit(id, name, enabled, tray)
            .map_err(errors::settings)?;
        self.commit(next)?;
        if !enabled {
            self.cancel(id);
        }
        Ok(())
    }
    fn connect(&mut self, source: accounts::Source<'_>) -> Result<String> {
        let root = self.root.clone();
        let saved = accounts::connect(&root, self.settings.clone(), source, |id| {
            self.sessions.invalidate(id);
            self.cancel(&id.to_string());
            Ok(())
        })
        .map_err(errors::settings)?;
        self.settings = Settings::read(&saved).map_err(errors::settings)?;
        let id = self.settings.accounts.last().unwrap().id.clone();
        self.errors.remove(&id);
        self.message = Some("계정을 연결했습니다.".into());
        Ok(id)
    }
    pub fn import(&mut self, provider: Provider, path: &str, reply: Reply) -> Result<()> {
        let id = self.connect(accounts::Source::File { provider, path })?;
        self.refresh(Some(&id), Some(reply), false);
        Ok(())
    }
    pub fn connect_claude(
        &mut self,
        path: &Path,
        consent: bool,
        helper: &Path,
        reply: Reply,
    ) -> Result<()> {
        if !consent {
            return Err("기존 status line을 보존해 연결하는 동의를 선택하세요.".into());
        }
        if !helper.is_file() {
            return Err("StatusLine 실행 파일을 찾지 못했습니다.".into());
        }
        let directory = connection::canonical(path).map_err(errors::claude)?;
        let identity = format!("claude:{}", connection::profile_id(&directory));
        let name = Path::new(&directory)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut next = self.settings.clone();
        let existing = next
            .connect(Provider::Claude, name, identity, directory.clone(), false)
            .map_err(errors::settings)?;
        let id = next.accounts.last().unwrap().id.clone();
        let profile = connection::install(&self.root, Path::new(&directory), helper, None)
            .map_err(errors::claude)?;
        if let Err(e) = self.commit(next) {
            if !existing {
                connection::uninstall(&self.root, &profile.id).map_err(errors::claude)?;
            }
            return Err(e);
        }
        self.message =
            Some("Claude Code를 다시 열고 응답을 한 번 받으면 사용량이 전달됩니다.".into());
        self.refresh(Some(&id), Some(reply), false);
        Ok(())
    }
    pub fn remove(&mut self, id: &str) -> Result<()> {
        let account = self
            .settings
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or(errors::UNKNOWN)?;
        if account.provider == Provider::Claude {
            let directory = connection::canonical(Path::new(
                account.source_path.as_deref().unwrap_or_default(),
            ))
            .map_err(errors::claude)?;
            let restored = connection::uninstall(&self.root, &connection::profile_id(&directory))
                .map_err(errors::claude)?;
            self.message = Some(
                if restored {
                    "Claude 연결을 해제하고 기존 status line을 복원했습니다."
                } else {
                    "연결을 해제했습니다. 사용자가 변경한 Claude status line은 보존했습니다."
                }
                .into(),
            );
        }
        let mut next = self.settings.clone();
        next.remove(id);
        self.commit(next)?;
        self.cancel(id);
        let guid = Uuid::parse_str(id).map_err(|_| errors::UNKNOWN)?;
        self.sessions.invalidate(guid);
        vault::delete(&self.root.join("Credentials"), guid).map_err(errors::file)?;
        self.snapshots.remove(id);
        self.errors.remove(id);
        self.save_snapshots()
    }

    pub fn refresh(&mut self, id: Option<&str>, reply: Option<Reply>, login: bool) {
        let mut reply = reply;
        if !self.quitting && !self.paused {
            for a in &self.settings.accounts {
                if a.enabled && id.is_none_or(|v| v == a.id) && !self.requests.contains_key(&a.id) {
                    let token = Cancel::default();
                    self.requests.insert(a.id.clone(), token.clone());
                    self.queue.push_back(Job {
                        account: a.clone(),
                        cancel: token,
                        reply: reply.take(),
                        login,
                    });
                }
            }
        }
        if let Some(r) = reply {
            if login {
                self.login_cancel = None;
            }
            self.replies.push((r, Ok(())));
        }
        // Publish the loading state before a local validation failure can complete the request.
        if !self.queue.is_empty() {
            (self.outbox.wake)();
        }
    }
    fn fetch_input(&self, a: &Account) -> Result<Fetch> {
        let path = a.source_path.as_deref().unwrap_or_default();
        let id = Uuid::parse_str(&a.id).map_err(|_| errors::UNKNOWN)?;
        match a.provider {
            Provider::Claude => Ok(Fetch::Claude(self.root.clone(), path.into())),
            Provider::Codex if a.app_managed_login => Ok(Fetch::Managed(
                self.sessions.clone(),
                id,
                a.identity.clone(),
            )),
            provider => {
                // Imported credentials may update the vault. Do this on the same thread as remove/reconnect.
                let auth = accounts::imported(&self.root, id, provider, path, &a.identity)
                    .map_err(errors::settings)?;
                Ok(Fetch::Imported(
                    if provider == Provider::Gemini {
                        Operation::Gemini
                    } else {
                        Operation::Codex
                    },
                    auth,
                ))
            }
        }
    }
    fn start_jobs(&mut self) {
        while self.active < 3 {
            let Some(job) = self.queue.pop_front() else {
                break;
            };
            if job.cancel.load(Ordering::Acquire) {
                self.finish(job, Err(errors::CANCELLED.into()));
                continue;
            }
            match self.fetch_input(&job.account) {
                Err(e) => self.finish(job, Err(e)),
                Ok(input) => {
                    self.active += 1;
                    let out = self.outbox.clone();
                    thread::spawn(move || {
                        let result = input.run(&job.cancel);
                        out.send(Event::Fetched(job, result));
                    });
                }
            }
        }
    }
    fn finish(&mut self, job: Job, result: Result<Value>) {
        let id = job.account.id;
        if self
            .requests
            .get(&id)
            .is_some_and(|c| Arc::ptr_eq(c, &job.cancel))
        {
            self.requests.remove(&id);
            if !job.cancel.load(Ordering::Acquire) && !self.quitting {
                let result = result.and_then(|value| {
                    self.snapshots.insert(id.clone(), value);
                    self.errors.remove(&id);
                    self.save_snapshots()
                });
                if let Err(e) = result {
                    self.errors.insert(id, e);
                }
            }
        }
        if job.login {
            self.login_cancel = None;
        }
        if let Some(r) = job.reply {
            self.replies.push((r, Ok(())));
        }
    }
    pub fn logging_in(&self) -> bool {
        self.login_cancel.is_some()
    }
    pub fn login(&mut self, reply: Reply) -> Result<()> {
        if self.paused {
            return Err("업데이트 설치 준비가 끝난 뒤 다시 시도하세요.".into());
        }
        if self.logging_in() {
            return Err("진행 중인 로그인을 완료하거나 취소하세요.".into());
        }
        let dir = self.root.join("Login");
        let home = self.paths.home.clone();
        let token = Cancel::default();
        self.login_cancel = Some(token.clone());
        self.message =
            Some("브라우저에서 Codex 로그인을 완료하세요. 최대 5분 동안 기다립니다.".into());
        let out = self.outbox.clone();
        thread::spawn(move || {
            let result = windows_login::find("codex", &env::var("PATH").unwrap_or_default(), &home)
                .ok_or_else(|| "Windows용 공식 Codex CLI를 설치하고 앱을 다시 실행하세요.".into())
                .and_then(|exe| {
                    windows_login::login(&exe, &dir, &[], Some(Duration::from_secs(300)), &token)
                        .map_err(errors::login)
                });
            out.send(Event::LoggedIn(reply, result));
        });
        Ok(())
    }
    pub fn cancel_login(&self) {
        if let Some(c) = &self.login_cancel {
            c.store(true, Ordering::Release);
        }
    }

    pub fn stop_scan(&mut self) {
        if let Some(c) = &self.scan_cancel {
            c.store(true, Ordering::Release);
        }
        if let Some(scanner) = &mut self.scanner {
            scanner.release_cache();
        }
    }
    pub fn scanning(&self) -> bool {
        self.scan_cancel.is_some()
    }
    pub fn scan(&mut self) {
        if self.paused || self.quitting || !self.settings.scan_local_logs {
            return;
        }
        let Some(mut scanner) = self.scanner.take() else {
            return;
        };
        let mut claude = vec![];
        for path in self
            .settings
            .accounts
            .iter()
            .filter(|a| a.provider == Provider::Claude)
            .filter_map(|a| a.source_path.as_ref())
            .map(PathBuf::from)
            .chain([self.paths.claude.clone()])
        {
            let p = path.join("projects");
            if !claude
                .iter()
                .any(|v: &PathBuf| v.as_os_str().eq_ignore_ascii_case(p.as_os_str()))
            {
                claude.push(p);
            }
        }
        let codex = [
            self.paths.codex.join("sessions"),
            self.paths.codex.join("archived_sessions"),
        ];
        let token = Cancel::default();
        self.scan_cancel = Some(token.clone());
        let out = self.outbox.clone();
        thread::spawn(move || {
            let result = scanner
                .scan(&codex, &claude, Utc::now(), &Local, 182, &token)
                .map_err(errors::file)
                .map(|report| {
                    let mut data = json!(report);
                    for (day, record) in data["days"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .zip(&report.days)
                    {
                        day["counts"]["total"] = json!(record.counts.total());
                    }
                    data
                });
            out.send(Event::Scanned(scanner, result));
        });
    }
    pub fn dispatch(&mut self) -> bool {
        let mut changed = false;
        while let Ok(event) = self.inbox.try_recv() {
            changed = true;
            match event {
                Event::Fetched(job, result) => {
                    self.active -= 1;
                    self.finish(job, result);
                    self.start_jobs();
                }
                Event::LoggedIn(r, result) => {
                    let cancelled = self.quitting
                        || self
                            .login_cancel
                            .as_ref()
                            .is_none_or(|v| v.load(Ordering::Acquire));
                    let result = result.and_then(|bytes| {
                        if cancelled {
                            Err(errors::CANCELLED.into())
                        } else {
                            self.connect(accounts::Source::CodexLogin(&bytes))
                        }
                    });
                    match result {
                        Ok(id) => self.refresh(Some(&id), Some(r), true),
                        Err(e) => {
                            self.login_cancel = None;
                            self.replies.push((r, Err(e)));
                        }
                    }
                }
                Event::Scanned(mut scanner, result) => {
                    if self
                        .scan_cancel
                        .take()
                        .is_some_and(|v| v.load(Ordering::Acquire))
                    {
                        scanner.release_cache();
                    } else {
                        match result {
                            Ok(report) => self.activity = Some(report),
                            Err(e) => self.message = Some(e),
                        }
                    }
                    self.scanner = Some(scanner);
                }
            }
        }
        if self.active < 3 && !self.queue.is_empty() {
            self.start_jobs();
            changed = true;
        }
        changed
    }
    pub fn suspend(&mut self) {
        for token in self.requests.values() {
            token.store(true, Ordering::Release);
        }
        self.stop_scan();
    }
    pub fn shutdown(&mut self) {
        self.quitting = true;
        self.cancel_login();
        self.suspend();
    }
    pub fn fetches_done(&self) -> bool {
        self.active == 0 && self.queue.is_empty()
    }
    pub fn drained(&self) -> bool {
        self.fetches_done() && !self.logging_in()
    }
    pub fn finished(&self) -> bool {
        self.drained() && self.scan_cancel.is_none()
    }
    pub fn flush(&self) -> Result<()> {
        self.sessions.flush().map_err(errors::session)
    }
}
