use crate::dashboard::{read_frame, write_frame};
use std::{
    io,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Condvar, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

const CLOSE: &[u8] = br#"{"close":{}}"#;
pub(super) fn suppress_sigpipe(writer: &impl std::os::fd::AsRawFd) -> io::Result<()> {
    // The helper can also run independently of the monitor. Protect this pipe in either process.
    if unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETNOSIGPIPE, 1) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
struct Status {
    ready: bool,
    closing: Option<Instant>,
    exited: bool,
    timed_out: bool,
}
struct State {
    status: Mutex<Status>,
    changed: Condvar,
}
impl State {
    fn close(&self) {
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        if status.closing.is_none() {
            status.closing = Some(Instant::now() + Duration::from_secs(2));
        }
        self.changed.notify_all();
    }
    fn supervise(&self, pid: u32, sender: mpsc::Sender<Zeroizing<Vec<u8>>>) {
        let startup = Instant::now() + Duration::from_secs(10);
        let mut status = self.status.lock().unwrap_or_else(|e| e.into_inner());
        let mut terminated = false;
        loop {
            if status.exited {
                break;
            }
            let deadline = if let Some(d) = status.closing {
                d
            } else if !status.ready {
                startup
            } else {
                status = self.changed.wait(status).unwrap_or_else(|e| e.into_inner());
                continue;
            };
            let now = Instant::now();
            if deadline > now {
                status = self
                    .changed
                    .wait_timeout(status, deadline - now)
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
                continue;
            }
            if status.closing.is_none() {
                status.timed_out = true;
                let _ = sender.send(Zeroizing::new(CLOSE.to_vec()));
                status.closing = Some(now + Duration::from_secs(2));
            } else {
                // waitid(WNOWAIT) reserves this PID until the waiter takes this lock.
                let signal = if terminated {
                    libc::SIGKILL
                } else {
                    libc::SIGTERM
                };
                unsafe {
                    libc::kill(pid as libc::pid_t, signal);
                }
                if terminated {
                    break;
                }
                terminated = true;
                status.closing = Some(now + Duration::from_secs(2));
            }
        }
    }
}
pub struct DashboardChild {
    child: Mutex<Child>,
    reader: Mutex<ChildStdout>,
    pid: u32,
    state: Arc<State>,
    sender: mpsc::Sender<Zeroizing<Vec<u8>>>,
}
impl DashboardChild {
    pub fn spawn(executable: &Path, demo: bool) -> io::Result<Self> {
        let mut command = Command::new(executable);
        command
            .arg("--dashboard-helper")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if demo {
            command.arg("--demo");
        }
        let mut process = command.spawn()?;
        let input = process.stdin.take().expect("piped stdin");
        let output = process.stdout.take().expect("piped stdout");
        let pid = process.id();
        let (sender, messages) = mpsc::channel();
        let child = Self {
            child: Mutex::new(process),
            reader: Mutex::new(output),
            pid,
            state: Arc::new(State {
                status: Mutex::new(Status {
                    ready: false,
                    closing: None,
                    exited: false,
                    timed_out: false,
                }),
                changed: Condvar::new(),
            }),
            sender,
        };
        suppress_sigpipe(&input)?;
        let state = child.state.clone();
        thread::Builder::new()
            .name("dashboard-writer".into())
            .spawn(move || write_messages(input, messages, &state))?;
        let state = child.state.clone();
        let sender = child.sender.clone();
        thread::Builder::new()
            .name("dashboard-lifetime".into())
            .spawn(move || state.supervise(pid, sender))?;
        Ok(child)
    }
    pub fn send(&self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() || data.len() > super::MAX_FRAME {
            return Err(crate::files::invalid("Invalid dashboard frame size"));
        }
        self.sender
            .send(Zeroizing::new(data.to_vec()))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
    pub fn ready(&self) {
        self.state
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ready = true;
        self.state.changed.notify_all();
    }
    pub fn close(&self) {
        let _ = self.send(CLOSE);
        self.state.close();
    }
    pub fn pid(&self) -> u32 {
        self.pid
    }
    pub fn receive(&self) -> io::Result<Option<Vec<u8>>> {
        read_frame(&mut *self.reader.lock().unwrap_or_else(|e| e.into_inner()))
    }
    pub fn timed_out(&self) -> bool {
        self.state
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .timed_out
    }
    pub fn running(&self) -> bool {
        !self
            .state
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .exited
    }
    pub fn wait(&self) {
        let mut child = self.child.lock().unwrap_or_else(|e| e.into_inner());
        if !self.running() {
            return;
        }
        // Observe exit without reaping so a timeout or shutdown can never
        // signal a different process that inherited the same numeric PID.
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        loop {
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.pid,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            };
            if rc == 0 || io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
        let mut status = self.state.status.lock().unwrap_or_else(|e| e.into_inner());
        status.exited = true;
        self.state.changed.notify_all();
        drop(status);
        let _ = child.wait();
    }
    pub fn stop(&self) {
        let status = self.state.status.lock().unwrap_or_else(|e| e.into_inner());
        if !status.exited {
            unsafe {
                libc::kill(self.pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
}
fn write_messages(
    mut pipe: ChildStdin,
    messages: mpsc::Receiver<Zeroizing<Vec<u8>>>,
    state: &State,
) {
    for message in messages {
        if write_frame(&mut pipe, &message).is_err() {
            state.close();
            break;
        }
    }
}
impl Drop for DashboardChild {
    fn drop(&mut self) {
        self.stop();
        self.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn helper_args_private_pipes_and_normal_exit() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&dir).unwrap();
        let exe = dir.join("QuotaBar Dashboard");
        let script = br#"#!/bin/sh
[ "$1" = --dashboard-helper ] && [ "$2" = --demo ] || exit 1
printf '\000\000\000\005ready'
exec /bin/dd bs=1 count=9 2>/dev/null
"#;
        fs::write(&exe, script).unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        let child = DashboardChild::spawn(&exe, true).unwrap();
        assert_eq!(child.receive().unwrap().unwrap(), b"ready");
        child.ready();
        child.send(b"hello").unwrap();
        assert_eq!(child.receive().unwrap().unwrap(), b"hello");
        assert!(child.receive().unwrap().is_none());
        child.wait();
        assert!(!child.running());
        assert!(!child.timed_out());
        fs::remove_file(exe).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
