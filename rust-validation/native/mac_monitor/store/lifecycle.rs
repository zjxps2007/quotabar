use super::{Store, now};
use crate::{schedule::Refresh, updates::macos::resident::Event};
use block2::RcBlock;
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSWorkspace, NSWorkspaceDidWakeNotification, NSWorkspaceWillSleepNotification,
};
use objc2_foundation::{NSNotification, NSOperationQueue, NSTimer};
use serde_json::{Value, json};
use std::{cell::RefCell, ffi::c_void, fs::OpenOptions, io::Write, ptr::NonNull};

unsafe extern "C" {
    fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
}

fn invalidate(slot: &RefCell<Option<Retained<NSTimer>>>) {
    if let Some(timer) = slot.take() {
        timer.invalidate();
    }
}

impl Store {
    pub(in crate::mac_monitor) fn record(&self, event: &str, details: Value) {
        let Some(path) = &self.diagnostic else { return };
        let (queued, running, ..) = self.state.borrow().queue.status();
        let mut row = json!({"event":event,"date":now()+978307200., "windowVisible":self.visible.get(),
            "scanning":false,"runningRequests":running,"queuedRequests":queued,"demo":self.demo,
            "nextRefresh":self.deadline.get().map(|t| t+978307200.),
            "timerActive":self.timer.borrow().as_ref().is_some_and(|t| t.isValid())});
        if let (Some(object), Value::Object(fields)) = (row.as_object_mut(), details) {
            object.extend(fields);
        }
        if let Ok(mut line) = serde_json::to_vec(&row)
            && let Ok(mut file) = OpenOptions::new().append(true).create(true).open(path)
        {
            line.push(b'\n');
            let _ = file.write_all(&line);
        }
    }

    pub(super) fn configure_updates(&self) {
        if let Some(updater) = self.updater.get() {
            let state = self.state.borrow();
            let automatic = state.settings.automatic_updates.unwrap_or(true) && self.healthy;
            let suspended = state.queue.status().2;
            drop(state);
            updater.configure(
                automatic,
                !self.visible.get() && !self.dashboard.get(),
                suspended,
            );
        }
    }

    pub(super) fn update_event(&self, event: Event<'_>) {
        match event {
            Event::State(value) => {
                if let Ok(value) = serde_json::to_value(value) {
                    let mut state = self.state.borrow_mut();
                    if state.update != value {
                        state.update = value;
                        drop(state);
                        self.changed();
                    }
                }
            }
            Event::PrepareExit => {
                self.state.borrow_mut().queue.set_installing(true);
                invalidate(&self.timer);
                self.changed();
                self.finish_update_if_ready();
            }
            Event::Resume => {
                self.flushing.set(false);
                self.state.borrow_mut().queue.set_installing(false);
                self.refresh_all();
                self.schedule();
            }
            Event::Exit => NSApplication::sharedApplication(self.main).terminate(None),
        }
    }

    pub(in crate::mac_monitor) fn dashboard_running(&self, value: bool) {
        self.dashboard.set(value);
        self.configure_updates();
    }

    pub(in crate::mac_monitor) fn set_visible(&self, value: bool) {
        let old = self.visible.replace(value);
        if !old && !value {
            return;
        }
        invalidate(&self.cleanup);
        if !value {
            self.reclaim();
        }
        self.schedule();
        if old != value {
            self.changed();
            self.record(
                if value {
                    "window-visible"
                } else {
                    "window-hidden"
                },
                Value::Null,
            );
        }
        self.configure_updates();
    }

    fn timer(&self, delay: f64, action: fn(&Store)) -> Retained<NSTimer> {
        let owner = self.owner.clone();
        let block = RcBlock::new(move |_: NonNull<NSTimer>| {
            let main = MainThreadMarker::new().expect("main run loop");
            if let Some(store) = owner.get(main).upgrade().filter(|s| s.live.get()) {
                action(&store);
            }
        });
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(delay, false, &block) }
    }

    pub(in crate::mac_monitor) fn reclaim(&self) {
        if self.visible.get() {
            return;
        }
        invalidate(&self.cleanup);
        self.cleanup.replace(Some(self.timer(2., |s| {
            s.cleanup.take();
            if !s.visible.get() {
                let bytes = unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
                s.record("idle-reclaimed", json!({"releasedBytes":bytes}));
            }
        })));
    }

    fn next(&self) -> Option<f64> {
        let s = self.state.borrow();
        let (_, _, sleep, install) = s.queue.status();
        Refresh {
            suspended: sleep || install || self.demo || !self.healthy,
            has_accounts: s.settings.accounts.iter().any(|a| a.enabled),
            automatic: s.settings.auto_refresh.unwrap_or(true),
            interval: s.settings.refresh_seconds as i32,
            ..Refresh::default()
        }
        .delays()
        .0
        .map(|delay| self.last_refresh.get() + f64::from(delay))
    }

    pub(super) fn schedule(&self) {
        let deadline = self.next();
        if self.deadline.get() == deadline
            && self.timer.borrow().as_ref().is_some_and(|t| t.isValid())
        {
            return;
        }
        invalidate(&self.timer);
        self.deadline.set(deadline);
        let Some(at) = deadline else { return };
        let delay = (at - now()).max(1.);
        let timer = self.timer(delay, |s| {
            s.timer.take();
            s.deadline.set(None);
            if s.next().is_some_and(|at| at <= now()) {
                s.refresh_all();
            }
            s.schedule();
            s.record("scheduled-refresh", Value::Null);
        });
        timer.setTolerance((delay / 10.).min(30.));
        self.timer.replace(Some(timer));
    }

    pub(super) fn observe_power(&self) {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let queue = NSOperationQueue::mainQueue();
        let names = unsafe {
            [
                (NSWorkspaceWillSleepNotification, true),
                (NSWorkspaceDidWakeNotification, false),
            ]
        };
        for (name, sleep) in names {
            let owner = self.owner.clone();
            let block = RcBlock::new(move |_: NonNull<NSNotification>| {
                let main = MainThreadMarker::new().expect("main operation queue");
                if let Some(store) = owner.get(main).upgrade().filter(|s| s.live.get()) {
                    if sleep {
                        store.cancel_workers();
                    } else {
                        store.state.borrow_mut().queue.set_sleeping(false);
                        if store.state.borrow().settings.auto_refresh.unwrap_or(true) {
                            store.refresh_all();
                        }
                    }
                    store.schedule();
                    store.configure_updates();
                }
            });
            let token = unsafe {
                center.addObserverForName_object_queue_usingBlock(
                    Some(name),
                    None,
                    Some(&queue),
                    &block,
                )
            };
            self.observers.borrow_mut().push(token);
        }
    }

    pub(in crate::mac_monitor) fn stop(&self) {
        self.record("ui-terminated", Value::Null);
        self.live.set(false);
        invalidate(&self.timer);
        invalidate(&self.cleanup);
        self.cancel_workers();
        if let Some(updater) = self.updater.get() {
            updater.stop();
        }
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        for token in self.observers.take() {
            unsafe {
                center.removeObserver((*token).as_ref());
            }
        }
    }
}
