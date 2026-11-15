#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(quotabar::mac_monitor::run());
}

#[cfg(windows)]
fn main() {
    std::process::exit(quotabar::windows_monitor::run());
}

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("This monitor executable requires macOS or Windows.");
    std::process::exit(1);
}
