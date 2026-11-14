//! A private CLI sign-in attempt, owned by the application until the credential
//! is read, the deadline expires or the window/application closes.
use crate::{credentials::Platform, files, oauth, vault::MAX_SIZE};
use objc2_foundation::{NSString, NSURL};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Read},
    os::fd::{AsRawFd, FromRawFd},
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

pub fn find(search_path: &str, home: &Path) -> Option<PathBuf> {
    let candidates=search_path.split(':').filter(|s|!s.is_empty()).map(|s|Path::new(s).join("codex"))
        .chain(["/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
            "/Applications/Codex.app/Contents/Resources/codex","/opt/homebrew/bin/codex","/usr/local/bin/codex"].map(PathBuf::from))
        .chain(std::iter::once(home.join(".local/bin/codex")));
    candidates.filter(|p| p.is_absolute()).find(|path| {
        CString::new(path.as_os_str().as_bytes())
            .is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::X_OK) == 0 })
    })
}

pub fn authorization_url(text: &str) -> Option<String> {
    for token in text.split(|c: char| c.is_whitespace() || matches!(c, '\x1b' | '<' | '>' | '"')) {
        let Some(start) = token.find("https://") else {
            continue;
        };
        let candidate = &token[start..];
        let Some(url) = NSURL::URLWithString(&NSString::from_str(candidate)) else {
            continue;
        };
        if url.scheme().is_some_and(|s| s.to_string() == "https")
            && url
                .host()
                .is_some_and(|s| s.to_string() == "auth.openai.com")
            && url.user().is_none()
            && url.password().is_none()
            && url.port().is_none_or(|p| p.integerValue() == 443)
            && url.path().is_some_and(|p| {
                matches!(
                    p.to_string().as_str(),
                    "/oauth/authorize" | "/api/accounts/authorize"
                )
            })
        {
            return url.absoluteString().map(|s| s.to_string());
        }
    }
    None
}

fn output_pipe() -> io::Result<(File, File)> {
    let mut pipes = [-1; 2];
    if unsafe { libc::pipe(pipes.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (reader, writer) = unsafe { (File::from_raw_fd(pipes[0]), File::from_raw_fd(pipes[1])) };
    for fd in [reader.as_raw_fd(), writer.as_raw_fd()] {
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let flags = unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((reader, writer))
}

struct TemporaryHome(PathBuf);
impl Drop for TemporaryHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub struct Attempt {
    child: Option<Child>,
    home: Option<TemporaryHome>,
    output: File,
    text: Zeroizing<Vec<u8>>,
    sent_url: bool,
    start: Instant,
    timeout: Duration,
}
pub enum Progress {
    Waiting,
    Authorized(String),
    Finished(Zeroizing<Vec<u8>>),
    Failed,
    TimedOut,
}

impl Attempt {
    pub fn start(executable: &Path, temporary: &Path, timeout: Duration) -> io::Result<Self> {
        let path = temporary.join(format!(
            "quotabar-login-{}",
            Uuid::new_v4().to_string().to_ascii_uppercase()
        ));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        let home = TemporaryHome(path);
        let (output, writer) = output_pipe()?;
        let search = std::env::var_os("PATH").unwrap_or_default();
        let mut search = search.into_encoded_bytes();
        search.extend_from_slice(b":/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin");
        let mut cmd = Command::new(executable);
        cmd.args(["-c", "cli_auth_credentials_store=\"file\"", "login"])
            .current_dir(&home.0)
            .env("CODEX_HOME", &home.0)
            .env("PATH", std::ffi::OsStr::from_bytes(&search))
            .stdin(Stdio::null())
            .stdout(writer.try_clone()?)
            .stderr(writer);
        for name in [
            "OPENAI_API_KEY",
            "CODEX_API_KEY",
            "CODEX_ACCESS_TOKEN",
            "OPENAI_BASE_URL",
        ] {
            cmd.env_remove(name);
        }
        let child = cmd.spawn()?;
        Ok(Self {
            child: Some(child),
            home: Some(home),
            output,
            text: Zeroizing::new(Vec::new()),
            sent_url: false,
            start: Instant::now(),
            timeout,
        })
    }
    pub fn poll(&mut self) -> io::Result<Progress> {
        if self.start.elapsed() >= self.timeout {
            return Ok(Progress::TimedOut);
        }
        let mut chunk = [0; 8192];
        // Bound work per turn so verbose output cannot postpone cancellation or credentials.
        for _ in 0..8 {
            match self.output.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) if !self.sent_url => {
                    self.text.extend_from_slice(&chunk[..n]);
                    const LIMIT: usize = 65536;
                    if self.text.len() > LIMIT {
                        let keep = self.text.len() - LIMIT;
                        self.text.copy_within(keep.., 0);
                        self.text[LIMIT..].zeroize();
                        self.text.truncate(LIMIT);
                    }
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        chunk.zeroize();
        if !self.sent_url
            && let Some(url) = authorization_url(&String::from_utf8_lossy(&self.text))
        {
            self.sent_url = true;
            self.text.zeroize();
            return Ok(Progress::Authorized(url));
        }
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(
                self.home
                    .as_ref()
                    .expect("Active login")
                    .0
                    .join("auth.json"),
            );
        if let Ok(file) = file
            && let Ok(metadata) = file.metadata()
            && metadata.is_file()
            && metadata.nlink() == 1
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.len() <= MAX_SIZE as u64
            && let Ok(bytes) = files::read_bounded(file, MAX_SIZE)
        {
            let bytes = Zeroizing::new(bytes);
            if oauth::managed(&bytes, Platform::Mac).is_ok_and(|v| v.refresh_token.is_some()) {
                return Ok(Progress::Finished(bytes));
            }
        }
        Ok(
            if self
                .child
                .as_mut()
                .expect("Active login")
                .try_wait()?
                .is_some()
            {
                Progress::Failed
            } else {
                Progress::Waiting
            },
        )
    }
    pub fn close(mut self, immediately: bool) {
        if immediately {
            return;
        }
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        if unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) } != 0 {
            return;
        }
        // Keep the private home alive until the CLI exits. A slow CLI must not
        // hold the main thread while the dashboard is closing.
        let _ = std::thread::Builder::new()
            .name("quotabar-login-cleanup".into())
            .spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(2);
                while Instant::now() < deadline {
                    if self
                        .child
                        .as_mut()
                        .is_none_or(|child| matches!(child.try_wait(), Ok(Some(_))))
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                drop(self);
            });
    }
}
impl Drop for Attempt {
    fn drop(&mut self) {
        // An interrupted login must finish cleanup even if the hosting app is exiting.
        if let Some(mut child) = self.child.take() {
            if !matches!(child.try_wait(), Ok(Some(_))) {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        drop(self.home.take());
    }
}
