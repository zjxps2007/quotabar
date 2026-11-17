use quotabar::schedule::{INTERVALS, LOCAL_INTERVAL, Refresh};

#[repr(C)]
pub struct Delays {
    pub accounts: i32,
    pub local: i32,
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_refresh_delays(
    suspended: i32,
    automatic: i32,
    accounts: i32,
    dashboard: i32,
    logs: i32,
    scanning: i32,
    interval: i32,
) -> Delays {
    let policy = Refresh {
        suspended: suspended != 0,
        automatic: automatic != 0,
        has_accounts: accounts != 0,
        dashboard_visible: dashboard != 0,
        scan_enabled: logs != 0,
        scanning: scanning != 0,
        interval,
    };
    let (accounts, local) = policy.delays();
    Delays {
        accounts: accounts.unwrap_or(-1),
        local: local.unwrap_or(-1),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_refresh_interval(index: usize) -> i32 {
    INTERVALS.get(index).copied().unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn qb_local_interval() -> i32 {
    LOCAL_INTERVAL
}
