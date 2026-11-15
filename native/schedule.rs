pub const INTERVALS: [i32; 5] = [60, 300, 900, 1800, 3600];
pub const LOCAL_INTERVAL: i32 = 300;

#[derive(Default)]
pub struct Refresh {
    pub suspended: bool,
    pub automatic: bool,
    pub has_accounts: bool,
    pub dashboard_visible: bool,
    pub scan_enabled: bool,
    pub scanning: bool,
    pub interval: i32,
}

impl Refresh {
    /// Delays relative to the last account refresh and log scan. Each source
    /// has its own deadline. The host's main loop sleeps until the earliest.
    pub fn delays(&self) -> (Option<i32>, Option<i32>) {
        if self.suspended || !self.automatic {
            return (None, None);
        }
        (
            (self.has_accounts && self.interval > 0).then_some(self.interval),
            (self.dashboard_visible && self.scan_enabled && !self.scanning)
                .then_some(LOCAL_INTERVAL),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_and_visible_log_scan_keep_independent_deadlines() {
        let mut r = Refresh {
            automatic: true,
            has_accounts: true,
            scan_enabled: true,
            interval: 900,
            ..Refresh::default()
        };
        assert_eq!(r.delays(), (Some(900), None));
        r.dashboard_visible = true;
        assert_eq!(r.delays(), (Some(900), Some(300)));
        r.has_accounts = false;
        assert_eq!(r.delays(), (None, Some(300)));
        r.scanning = true;
        assert_eq!(r.delays(), (None, None));
        r.has_accounts = true;
        r.suspended = true;
        assert_eq!(r.delays(), (None, None));
        r.suspended = false;
        r.automatic = false;
        assert_eq!(r.delays(), (None, None));
    }
}
