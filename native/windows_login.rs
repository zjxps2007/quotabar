//! Run the official CLI directly in a private, per-attempt home. Browser
//! processes launched by the CLI have their own lifetime.
use crate::{credentials::Platform, files, oauth, vault::MAX_SIZE};
use std::{
    fs, io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use zeroize::Zeroizing;

pub enum LoginError {
    Launch(io::Error),
    Storage(io::Error),
    Cancelled,
    Timeout,
    Failed,
}

fn npm_executable(path: &Path) -> Option<PathBuf> {
    let architecture = if cfg!(target_pointer_width = "64") {
        "x86_64-pc-windows"
    } else {
        "i686-pc-windows"
    };
    let mut folders = vec![path.to_owned()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if kind.is_dir() && !kind.is_symlink() {
                folders.push(path);
            } else if entry.file_name().eq_ignore_ascii_case("codex.exe")
                && path.is_file()
                && path.to_string_lossy().contains(architecture)
            {
                return Some(path);
            }
        }
    }
    None
}
pub fn find(name: &str, path: &str, user_home: &Path) -> Option<PathBuf> {
    let paths = path
        .split(';')
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .chain(std::iter::once(user_home.join(".local").join("bin")));
    for folder in paths {
        let direct = folder.join(format!("{name}.exe"));
        if direct.is_file() {
            return Some(direct);
        }
        if name == "codex"
            && let Some(exe) =
                npm_executable(&folder.join("node_modules").join("@openai").join("codex"))
        {
            return Some(exe);
        }
    }
    None
}

struct PrivateHome(PathBuf);
impl PrivateHome {
    fn new(parent: &Path) -> io::Result<Self> {
        files::ensure_private_directory(parent)?;
        let path = parent.join(Uuid::new_v4().hyphenated().to_string());
        files::ensure_private_directory(&path)?;
        Ok(Self(path))
    }
}
impl Drop for PrivateHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct LoginChild(Child);
impl Drop for LoginChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

pub fn login(
    executable: &Path,
    base: &Path,
    prefix: &[String],
    timeout: Option<Duration>,
    cancel: &AtomicBool,
) -> Result<Zeroizing<Vec<u8>>, LoginError> {
    let private = PrivateHome::new(base).map_err(LoginError::Storage)?;
    let mut command = Command::new(executable);
    command
        .args(prefix)
        .args(["-c", "cli_auth_credentials_store=\"file\"", "login"])
        .current_dir(&private.0)
        .env("CODEX_HOME", &private.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    for key in [
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "CODEX_ACCESS_TOKEN",
        "OPENAI_BASE_URL",
    ] {
        command.env_remove(key);
    }
    let mut child = LoginChild(command.spawn().map_err(LoginError::Launch)?);
    let auth = private.0.join("auth.json");
    let start = Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(LoginError::Cancelled);
        }
        if timeout.is_some_and(|t| start.elapsed() >= t) {
            return Err(LoginError::Timeout);
        }
        if let Ok(file) = files::open_owned_credential(&auth)
            && let Ok(bytes) = files::read_bounded(file, MAX_SIZE)
        {
            let bytes = Zeroizing::new(bytes);
            if oauth::managed(&bytes, Platform::Windows).is_ok_and(|v| v.refresh_token.is_some()) {
                if cancel.load(Ordering::Acquire) {
                    return Err(LoginError::Cancelled);
                }
                return Ok(bytes);
            }
        }
        if child.0.try_wait().map_err(LoginError::Launch)?.is_some() {
            return Err(LoginError::Failed);
        }
        let delay = Duration::from_millis(150);
        thread::sleep(timeout.map_or(delay, |t| delay.min(t.saturating_sub(start.elapsed()))));
    }
}
