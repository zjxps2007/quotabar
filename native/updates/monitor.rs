//! Update decisions shared by the two persistent monitors. The host runs the
//! download and installer, and reports their outcomes on its UI thread.
use super::Version;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Operation {
    #[default]
    Idle,
    Working,
    Installing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum OnConfigure {
    Schedule,
    Cancel,
    Install,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Offer {
    Current,
    Blocked,
    Available,
}

#[derive(Default)]
pub struct Monitor {
    automatic: bool,
    can_install: bool,
    suspended: bool,
    operation: Operation,
    last_attempt: Option<f64>,
    staged: Option<String>,
    blocked: Option<String>,
}

impl Monitor {
    pub fn configure(
        &mut self,
        automatic: bool,
        can_install: bool,
        suspended: bool,
    ) -> OnConfigure {
        let stop = (self.automatic && !automatic) || (!self.suspended && suspended);
        self.automatic = automatic;
        self.can_install = can_install;
        self.suspended = suspended;
        if stop && self.operation == Operation::Working {
            OnConfigure::Cancel
        } else if self.operation == Operation::Idle && self.may_install(false) {
            OnConfigure::Install
        } else {
            OnConfigure::Schedule
        }
    }

    pub fn begin_check(&mut self, manual: bool) -> bool {
        if self.suspended || self.operation != Operation::Idle {
            return false;
        }
        if manual {
            self.blocked = None;
        }
        self.operation = Operation::Working;
        true
    }

    pub fn attempted(&mut self, now: f64) {
        self.staged = None;
        self.last_attempt = Some(now);
    }

    pub fn offer(&self, current: &str, version: &str) -> Offer {
        if !matches!((current.parse::<Version>(), version.parse::<Version>()), (Ok(have), Ok(new)) if new > have)
        {
            Offer::Current
        } else if self.blocked.as_deref() == Some(version) {
            Offer::Blocked
        } else {
            Offer::Available
        }
    }

    pub fn should_download(&self, requested: bool) -> bool {
        self.automatic || requested
    }

    pub fn staged(&mut self, version: &str) {
        self.staged = Some(version.to_owned());
    }

    pub fn block(&mut self, version: &str) {
        self.blocked = Some(version.to_owned());
    }

    pub fn may_install(&self, force: bool) -> bool {
        !self.suspended
            && self.staged.is_some()
            && (force || (self.automatic && self.can_install && self.staged != self.blocked))
    }

    pub fn begin_install(&mut self, force: bool) -> bool {
        if self.operation != Operation::Idle || !self.may_install(force) {
            return false;
        }
        self.operation = Operation::Working;
        true
    }

    /// Both hosts recheck eligibility after preparing the package, draining
    /// credentials, and acknowledging the helper, before exiting the monitor.
    pub fn begin_handoff(&mut self, force: bool) -> bool {
        if self.operation != Operation::Working || !self.may_install(force) {
            return false;
        }
        self.operation = Operation::Installing;
        true
    }

    pub fn finish(&mut self, cancelled: bool) {
        self.operation = Operation::Idle;
        if cancelled {
            self.last_attempt = None;
            self.staged = None;
        }
    }

    pub fn delay(&self, now: f64) -> Option<f64> {
        if !self.automatic
            || self.suspended
            || self.operation != Operation::Idle
            || (self.staged.is_some() && self.staged != self.blocked)
        {
            return None;
        }
        Some(
            self.last_attempt
                .map_or(15., |last| (last + 86400. - now).max(15.)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_staged_update_waits_for_idle_and_rechecks_before_handoff() {
        let mut p = Monitor::default();
        assert_eq!(p.configure(true, false, false), OnConfigure::Schedule);
        assert_eq!(p.delay(100.), Some(15.));
        assert!(p.begin_check(false));
        p.attempted(100.);
        assert!(!p.begin_check(true));
        p.staged("1.2.3");
        assert!(!p.may_install(false));
        p.finish(false);
        assert_eq!(p.delay(101.), None);
        assert_eq!(p.configure(true, true, false), OnConfigure::Install);
        assert!(p.begin_install(false));
        assert!(p.begin_handoff(false));
        assert_eq!(p.configure(false, false, true), OnConfigure::Schedule);
        assert!(!p.may_install(true));
        p.finish(false);
        p.configure(false, false, false);
        assert!(p.begin_install(true));
        assert!(p.begin_handoff(true));
        assert!(p.may_install(true));
    }

    #[test]
    fn cancellation_and_failed_versions_have_distinct_retry_deadlines() {
        let mut p = Monitor::default();
        p.configure(true, true, false);
        p.begin_check(false);
        p.attempted(10.);
        assert_eq!(p.configure(true, false, true), OnConfigure::Cancel);
        p.finish(true);
        assert!(!p.begin_check(true));
        p.configure(true, true, false);
        assert_eq!(p.delay(200.), Some(15.));
        assert!(p.begin_check(false));
        p.attempted(200.);
        p.staged("1.0.1");
        p.block("1.0.1");
        p.finish(false);
        assert_eq!(p.delay(201.), Some(86399.));
        assert_eq!(p.delay(90000.), Some(15.));
        assert!(!p.begin_install(false));
        assert!(p.begin_check(false));
        assert_eq!(p.offer("1.0.0", "1.0.1"), Offer::Blocked);
        p.finish(false);
        assert!(p.begin_check(true));
        assert_eq!(p.offer("1.0.0", "1.0.1"), Offer::Available);
        assert_eq!(p.offer("1.0.1", "1.0.1"), Offer::Current);
        assert_eq!(p.offer("dev", "1.0.1"), Offer::Current);
    }
}
