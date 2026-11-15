//! The resident monitor's end of a disposable dashboard session. AppKit and the
//! store are main-thread-only; a reader and the child's writer own the pipes.
use super::DashboardChild;
use dispatch2::{DispatchQueue, MainThreadBound};
use objc2::MainThreadMarker;
use objc2_foundation::NSBundle;
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    path::PathBuf,
    rc::{Rc, Weak},
    sync::Arc,
    thread,
};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Copy)]
#[repr(i32)]
pub enum Event {
    Started,
    Exited,
    MissingHelper,
    LaunchFailed,
    PublishState,
    Command,
}

// A command callback returns 1 for a decoded ready command, 0 for a store
// action, and -1 for malformed input. Event data is borrowed for the call.
pub type Callback = extern "C" fn(*mut c_void, Event, u32, *const u8, usize) -> i32;
type Owner = Arc<MainThreadBound<Weak<Host>>>;

struct Session {
    id: Uuid,
    child: Arc<DashboardChild>,
    requested_route: RefCell<String>,
    ready: Cell<bool>,
    pending: Cell<bool>,
    closing: Cell<bool>,
}

impl Session {
    fn send(&self, frame: &[u8]) {
        if !self.closing.get() && self.child.send(frame).is_err() {
            self.close();
        }
    }

    fn show(&self) {
        let route = self.requested_route.borrow();
        let event = serde_json::json!({"show":{"route": &*route}});
        self.send(&serde_json::to_vec(&event).expect("string route"));
    }

    fn close(&self) {
        if !self.closing.replace(true) {
            self.child.close();
        }
    }
}

pub struct Host {
    owner: Owner,
    context: *mut c_void,
    callback: Callback,
    demo: bool,
    session: RefCell<Option<Rc<Session>>>,
}

impl Host {
    pub fn new(context: *mut c_void, callback: Callback, demo: bool) -> Option<Rc<Self>> {
        let main = MainThreadMarker::new()?;
        Some(Rc::new_cyclic(|weak| Self {
            owner: Arc::new(MainThreadBound::new(weak.clone(), main)),
            context,
            callback,
            demo,
            session: RefCell::default(),
        }))
    }

    fn notify(&self, event: Event, value: u32, data: &[u8]) -> i32 {
        (self.callback)(self.context, event, value, data.as_ptr(), data.len())
    }

    fn current(&self) -> Option<Rc<Session>> {
        self.session.borrow().clone()
    }

    pub fn running(&self) -> bool {
        self.session
            .borrow()
            .as_ref()
            .is_some_and(|s| s.child.running())
    }

    pub fn show(&self, route: &str) {
        if let Some(session) = self
            .current()
            .filter(|s| s.child.running() && !s.closing.get())
        {
            session.requested_route.replace(route.to_owned());
            if session.ready.get() {
                session.show();
            }
            return;
        }
        // Clear the previous store visibility even when the reopen gets here
        // before the old reader's exit notification. Its queued frames are stale.
        if let Some(old) = self.current() {
            old.close();
            self.exited(&old);
        }
        let bundle = NSBundle::mainBundle()
            .bundleURL()
            .URLByAppendingPathComponent(&objc2_foundation::NSString::from_str(
                "Contents/Helpers/QuotaBar Dashboard.app",
            ));
        let executable = bundle
            .and_then(|url| NSBundle::bundleWithURL(&url))
            .and_then(|bundle| bundle.executablePath());
        let Some(executable) = executable else {
            self.notify(Event::MissingHelper, 0, &[]);
            return;
        };
        let Ok(child) = DashboardChild::spawn(&PathBuf::from(executable.to_string()), self.demo)
        else {
            self.notify(Event::LaunchFailed, 0, &[]);
            return;
        };
        let child = Arc::new(child);
        let id = Uuid::new_v4();
        let reader = child.clone();
        let owner = self.owner.clone();
        if thread::Builder::new()
            .name("dashboard-reader".into())
            .spawn(move || {
                while let Ok(Some(bytes)) = reader.receive() {
                    let bytes = Zeroizing::new(bytes);
                    post(&owner, id, move |host, session| {
                        host.receive(session, &bytes)
                    });
                }
                reader.close();
                reader.wait();
                post(&owner, id, |host, session| host.exited(session));
            })
            .is_err()
        {
            self.notify(Event::LaunchFailed, 0, &[]);
            return;
        }
        let pid = child.pid();
        self.session.replace(Some(Rc::new(Session {
            id,
            child,
            requested_route: RefCell::new(route.to_owned()),
            ready: Cell::new(false),
            pending: Cell::new(false),
            closing: Cell::new(false),
        })));
        self.notify(Event::Started, pid, &[]);
    }

    fn receive(&self, session: &Session, bytes: &[u8]) {
        if session.closing.get() {
            return;
        }
        match self.notify(Event::Command, 0, bytes) {
            1 => {
                session.child.ready();
                session.ready.set(true);
                self.notify(Event::PublishState, 0, &[]);
                session.show();
            }
            0 => {}
            _ => session.close(),
        }
    }

    fn exited(&self, session: &Session) {
        self.session.take();
        self.notify(Event::Exited, u32::from(session.child.timed_out()), &[]);
    }

    pub fn changed(&self) {
        let Some(session) = self.current() else {
            return;
        };
        if session.ready.get() && !session.closing.get() && !session.pending.replace(true) {
            // The store announces changes before mutation. Deliver its finished
            // snapshot on the next main turn and coalesce a burst of changes.
            post(&self.owner, session.id, |host, session| {
                session.pending.set(false);
                if !session.closing.get() {
                    host.notify(Event::PublishState, 0, &[]);
                }
            });
        }
    }

    pub fn send(&self, frame: &[u8]) {
        if let Some(session) = self.current() {
            session.send(frame);
        }
    }

    pub fn close(&self) {
        if let Some(session) = self.current() {
            session.close();
        }
    }

    pub fn stop(&self) {
        if let Some(session) = self.session.take() {
            session.close();
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}

fn post(owner: &Owner, id: Uuid, action: impl FnOnce(&Host, &Session) + Send + 'static) {
    let owner = owner.clone();
    DispatchQueue::main().exec_async(move || {
        let main = MainThreadMarker::new().expect("main dispatch queue");
        if let Some(host) = owner.get(main).upgrade()
            && let Some(session) = host.current().filter(|s| s.id == id)
        {
            action(&host, &session);
        }
    });
}
