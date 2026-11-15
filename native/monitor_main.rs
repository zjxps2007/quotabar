#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(quotabar::mac_monitor::run());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("This monitor executable requires macOS.");
    std::process::exit(1);
}
