//! Capture a dashboard fixture independently of its renderer.
#[cfg(windows)]
#[path = "../../native/ui/windows_capture.rs"]
mod windows_capture;

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("Expected a window handle and output PNG".into());
    }
    let handle = args[0].to_str().ok_or("Invalid handle")?.parse::<usize>()?;
    windows_capture::client_pixels(handle)?.save(&args[1])?;
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
