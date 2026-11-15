//! The monitor and detached updater take the same per-user lease as older releases.
use std::{
    fs::File,
    io,
    path::Path,
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
mod windows;

#[cfg(windows)]
const LOCK_FILE: &str = "monitor.lock";
#[cfg(target_os = "macos")]
const LOCK_FILE: &str = "instance.lock";

pub struct Instance {
    lease: Option<File>,
    #[cfg(windows)]
    activation: windows::Activation,
}

impl Instance {
    pub fn acquire(root: &Path) -> io::Result<Self> {
        let lease = match acquire_lease(root, Duration::ZERO) {
            Ok(file) => Some(file),
            Err(e) if busy(&e) => None,
            Err(e) => return Err(e),
        };
        Ok(Self {
            lease,
            #[cfg(windows)]
            activation: windows::Activation::new(root)?,
        })
    }

    pub fn is_owner(&self) -> bool {
        self.lease.is_some()
    }

    #[cfg(windows)]
    pub fn activate(&self) -> io::Result<()> {
        self.activation.send()
    }

    #[cfg(windows)]
    pub fn wait_for_activation(&self) -> io::Result<bool> {
        if !self.is_owner() {
            return Err(io::Error::other(
                "Only the running monitor can receive activation",
            ));
        }
        self.activation.receive()
    }

    #[cfg(windows)]
    pub fn stop_listening(&self) {
        self.activation.stop();
    }

    #[cfg(target_os = "macos")]
    pub fn activate(&self) -> io::Result<()> {
        use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
        use objc2_foundation::NSString;
        let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(
            &NSString::from_str("com.aiusage.lite"),
        );
        let current = NSRunningApplication::currentApplication();
        if let Some(app) = apps.iter().find(|app| app != &current) {
            app.activateWithOptions(NSApplicationActivationOptions::empty());
        }
        Ok(())
    }
}

#[cfg(windows)]
fn busy(error: &io::Error) -> bool {
    error.raw_os_error() == Some(windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION as i32)
}

#[cfg(target_os = "macos")]
fn busy(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
}

pub(crate) fn acquire_lease(root: &Path, timeout: Duration) -> io::Result<File> {
    #[cfg(windows)]
    {
        use crate::files::{ensure_private_directory, reject_links};
        use std::os::windows::fs::OpenOptionsExt;
        ensure_private_directory(root)?;
        let path = root.join(LOCK_FILE);
        reject_links(&path)?;
        retry(timeout, || {
            File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(&path)
        })
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::{
            fs::{DirBuilderExt, OpenOptionsExt},
            io::AsRawFd,
        };
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)?;
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.join(LOCK_FILE))?;
        retry(timeout, || {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        })?;
        Ok(file)
    }
}

fn retry<T>(timeout: Duration, mut attempt: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        match attempt() {
            Err(e) if busy(&e) && Instant::now() < deadline => {
                thread::sleep(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(200)),
                );
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn monitor_and_updater_exclude_each_other_until_release() {
        let root = std::env::temp_dir().join(format!("QuotaBar-lease-{}", Uuid::new_v4()));
        let monitor = Instance::acquire(&root).unwrap();
        assert!(monitor.is_owner());
        assert!(!Instance::acquire(&root).unwrap().is_owner());
        assert!(busy(&acquire_lease(&root, Duration::ZERO).unwrap_err()));
        thread::scope(|scope| {
            let update = scope.spawn(|| acquire_lease(&root, Duration::from_secs(3)));
            drop(monitor);
            let updating = update.join().unwrap().unwrap();
            assert!(!Instance::acquire(&root).unwrap().is_owner());
            #[cfg(target_os = "macos")]
            {
                use std::os::fd::AsRawFd;
                let flags = unsafe { libc::fcntl(updating.as_raw_fd(), libc::F_GETFD) };
                assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
            }
            drop(updating);
        });
        assert!(Instance::acquire(&root).unwrap().is_owner());
        std::fs::remove_file(root.join(LOCK_FILE)).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
