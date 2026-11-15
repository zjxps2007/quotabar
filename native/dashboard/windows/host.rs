//! The monitor's UI thread owns the current dashboard session. Pipe readers only
//! post notifications to its window and never enter application code.
use super::DashboardChild;
use crate::monitor_loop::MonitorLoop;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    io,
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex},
    thread,
};
use zeroize::Zeroizing;

struct Session {
    child: Arc<DashboardChild>,
    ready: Cell<bool>,
    invalid: Cell<bool>,
}
struct Notice {
    child: Arc<DashboardChild>,
    frame: Option<Zeroizing<Vec<u8>>>,
}
struct Mailbox {
    window: isize,
    notices: VecDeque<Notice>,
}
impl Mailbox {
    fn post(&mut self, child: &Arc<DashboardChild>, frame: Option<Zeroizing<Vec<u8>>>) -> bool {
        if !MonitorLoop::dashboard_input(self.window) {
            return false;
        }
        self.notices.push_back(Notice {
            child: child.clone(),
            frame,
        });
        true
    }
}

#[repr(i32)]
pub enum Event {
    Command,
    Publish,
    Exited,
}
pub struct Host {
    mailbox: Arc<Mutex<Mailbox>>,
    current: RefCell<Option<Rc<Session>>>,
    route: RefCell<String>,
}

impl Host {
    pub(crate) fn new(window: isize) -> Self {
        Self {
            mailbox: Arc::new(Mutex::new(Mailbox {
                window,
                notices: VecDeque::new(),
            })),
            current: RefCell::default(),
            route: RefCell::new("dashboard".into()),
        }
    }
    pub fn pid(&self) -> u32 {
        self.current
            .borrow()
            .as_ref()
            .filter(|s| s.child.running())
            .map_or(0, |s| s.child.pid())
    }
    pub fn ready(&self) -> bool {
        self.current
            .borrow()
            .as_ref()
            .is_some_and(|s| s.ready.get())
    }
    pub fn show(&self, exe: &Path, capture: Option<&Path>, route: &str) -> io::Result<()> {
        self.route.replace(route.into());
        if let Some(session) = self.current.borrow().as_ref().filter(|s| s.child.running()) {
            if session.ready.get() {
                self.send_route(session);
            }
            return Ok(());
        }
        if !exe.is_file() {
            return Err(io::ErrorKind::NotFound.into());
        }
        let child = Arc::new(DashboardChild::spawn(exe, capture)?);
        let mailbox = self.mailbox.clone();
        let reader = child.clone();
        thread::Builder::new()
            .name("dashboard-reader".into())
            .spawn(move || {
                while let Ok(Some(frame)) = reader.receive() {
                    if !mailbox
                        .lock()
                        .unwrap()
                        .post(&reader, Some(Zeroizing::new(frame)))
                    {
                        break;
                    }
                }
                reader.close();
                reader.wait();
                mailbox.lock().unwrap().post(&reader, None);
            })?;
        self.current.replace(Some(Rc::new(Session {
            child,
            ready: Cell::new(false),
            invalid: Cell::new(false),
        })));
        Ok(())
    }
    fn send_route(&self, session: &Session) {
        let route = self.route.borrow();
        let message =
            serde_json::json!({"version":1, "type":"show", "requestId":null, "payload": &*route});
        let _ = session
            .child
            .send(&serde_json::to_vec(&message).expect("dashboard route"));
    }
    // Deliver one notice per window message, preserving order with timer, tray
    // and continuation messages. Store callbacks may reenter the host.
    pub fn dispatch(&self, notify: impl Fn(Event, &[u8]) -> i32) {
        let notice = self.mailbox.lock().unwrap().notices.pop_front();
        let Some(notice) = notice else {
            return;
        };
        let session = self.current.borrow().clone();
        let Some(session) = session.filter(|s| Arc::ptr_eq(&s.child, &notice.child)) else {
            return;
        };
        let Some(frame) = notice.frame else {
            self.current.take();
            notify(Event::Exited, &[]);
            return;
        };
        if session.invalid.get() {
            return;
        }
        match notify(Event::Command, &frame) {
            1 => {
                session.ready.set(true);
                session.child.ready();
                if notify(Event::Publish, &[]) == 0
                    && self
                        .current
                        .borrow()
                        .as_ref()
                        .is_some_and(|s| Rc::ptr_eq(s, &session))
                {
                    self.send_route(&session);
                }
            }
            0 => {}
            _ => {
                session.invalid.set(true);
                session.child.close();
            }
        }
    }
    pub fn send(&self, frame: &[u8]) {
        if let Some(session) = self.current.borrow().as_ref() {
            let _ = session.child.send(frame);
        }
    }
    pub fn close(&self) {
        if let Some(session) = self.current.borrow().as_ref() {
            session.child.close();
        }
    }
    pub fn stop(&self) {
        if let Some(session) = self.current.take() {
            session.child.stop();
        }
    }
    pub(crate) fn shutdown(&self) {
        // Retire the HWND under the posting lock before Windows may recycle it.
        let mut mailbox = self.mailbox.lock().unwrap();
        mailbox.window = 0;
        mailbox.notices.clear();
        drop(mailbox);
        self.stop();
    }
}
