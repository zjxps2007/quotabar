use super::{MAX_FRAME, read_frame, write_frame};
use crate::files::{invalid, wide};
use std::{
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr::{null, null_mut},
    sync::{Arc, Mutex, mpsc},
    thread,
};
use windows_sys::Win32::{
    Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{JobObjects::*, Pipes::CreatePipe, Threading::*},
};

const CLOSE: &[u8] = br#"{"version":1,"type":"close","requestId":null,"payload":true}"#;
fn check(ok: i32) -> io::Result<()> {
    if ok != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
fn own(raw: HANDLE) -> io::Result<OwnedHandle> {
    if raw.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let (mut r, mut w) = (null_mut(), null_mut());
    unsafe {
        check(CreatePipe(&mut r, &mut w, null(), 0))?;
        Ok((own(r)?, own(w)?))
    }
}
fn manual_event() -> io::Result<OwnedHandle> {
    own(unsafe { CreateEventW(null(), 1, 0, null()) })
}

struct Attributes(Vec<usize>);
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut bytes = 0;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
        unsafe {
            check(InitializeProcThreadAttributeList(
                storage.as_mut_ptr().cast(),
                2,
                0,
                &mut bytes,
            ))?;
        }
        Ok(Self(storage))
    }
    fn set(&mut self, key: u32, handles: &[HANDLE]) -> io::Result<()> {
        unsafe {
            check(UpdateProcThreadAttribute(
                self.0.as_mut_ptr().cast(),
                0,
                key as usize,
                handles.as_ptr().cast(),
                size_of_val(handles),
                null_mut(),
                null(),
            ))
        }
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe {
            DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast());
        }
    }
}

struct Process {
    handle: OwnedHandle,
    job: OwnedHandle,
    ready: OwnedHandle,
    closing: OwnedHandle,
    pid: u32,
}
impl Process {
    fn spawn(exe: &Path, capture: Option<&Path>) -> io::Result<(Self, File, File)> {
        let (child_input, write_to_child) = pipe()?;
        let (read_from_child, child_output) = pipe()?;
        let children = [child_input.as_raw_handle(), child_output.as_raw_handle()];
        for &handle in &children {
            unsafe {
                check(SetHandleInformation(
                    handle,
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAG_INHERIT,
                ))?;
            }
        }
        let job = own(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            check(SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of_val(&limits) as u32,
            ))?;
        }
        // Assign the job in CreateProcess itself. Even a crash in the next
        // instruction cannot leave an unowned Dashboard running.
        let jobs = [job.as_raw_handle()];
        let mut attributes = Attributes::new()?;
        attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &children)?;
        attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
        let exe = wide(exe)?;
        let mut command = vec![34u16];
        command.extend_from_slice(&exe[..exe.len() - 1]);
        command.extend(
            format!(
                "\" --in {} --out {}",
                children[0] as usize, children[1] as usize
            )
            .encode_utf16(),
        );
        if let Some(path) = capture {
            let arg = wide(path)?;
            if arg.contains(&34) {
                return Err(invalid("Invalid capture path"));
            }
            command.extend(" --capture \"".encode_utf16());
            let units = &arg[..arg.len() - 1];
            command.extend(units);
            command.extend(units.iter().rev().take_while(|&&c| c == 92));
            command.push(34);
        }
        command.push(0);
        let ready = manual_event()?;
        let closing = manual_event()?;
        let mut startup = STARTUPINFOEXW {
            lpAttributeList: attributes.0.as_mut_ptr().cast(),
            ..Default::default()
        };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        let mut info = PROCESS_INFORMATION::default();
        unsafe {
            check(CreateProcessW(
                exe.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                null(),
                null(),
                &startup.StartupInfo,
                &mut info,
            ))?;
        }
        let handle = own(info.hProcess)?;
        drop(own(info.hThread)?);
        Ok((
            Self {
                handle,
                job,
                ready,
                closing,
                pid: info.dwProcessId,
            },
            File::from(read_from_child),
            File::from(write_to_child),
        ))
    }
    fn running(&self) -> bool {
        unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) == WAIT_TIMEOUT }
    }
    fn signal_close(&self) {
        unsafe {
            SetEvent(self.closing.as_raw_handle());
        }
    }
    fn kill(&self) {
        unsafe {
            TerminateJobObject(self.job.as_raw_handle(), 1);
        }
    }
    fn supervise(&self, out: mpsc::Sender<Vec<u8>>) {
        let handles = [
            self.handle.as_raw_handle(),
            self.closing.as_raw_handle(),
            self.ready.as_raw_handle(),
        ];
        let initial = unsafe { WaitForMultipleObjects(3, handles.as_ptr(), 0, 10_000) };
        let transition = if initial == WAIT_OBJECT_0 + 2 {
            unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) }
        } else {
            initial
        };
        match transition {
            WAIT_OBJECT_0 => return,
            WAIT_TIMEOUT => {
                let _ = out.send(CLOSE.to_vec());
                self.signal_close();
            }
            _ => {}
        }
        if unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 2000) } != WAIT_OBJECT_0 {
            self.kill();
        }
    }
}

pub struct DashboardChild {
    process: Arc<Process>,
    sender: mpsc::Sender<Vec<u8>>,
    reader: Mutex<File>,
}
impl DashboardChild {
    pub fn spawn(exe: &Path, capture: Option<&Path>) -> io::Result<Self> {
        let (process, reader, mut writer) = Process::spawn(exe, capture)?;
        let process = Arc::new(process);
        let (sender, messages) = mpsc::channel::<Vec<u8>>();
        let child = Self {
            process,
            sender,
            reader: Mutex::new(reader),
        };
        let process = Arc::clone(&child.process);
        thread::Builder::new()
            .name("dashboard-writer".into())
            .spawn(move || {
                for frame in messages {
                    if write_frame(&mut writer, &frame).is_err() {
                        process.signal_close();
                        break;
                    }
                }
            })?;
        let process = Arc::clone(&child.process);
        let output = child.sender.clone();
        thread::Builder::new()
            .name("dashboard-lifetime".into())
            .spawn(move || process.supervise(output))?;
        Ok(child)
    }
    pub fn pid(&self) -> u32 {
        self.process.pid
    }
    pub fn running(&self) -> bool {
        self.process.running()
    }
    pub fn send(&self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() || data.len() > MAX_FRAME {
            return Err(invalid("Invalid dashboard frame size"));
        }
        self.sender
            .send(data.to_vec())
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
    pub fn receive(&self) -> io::Result<Option<Vec<u8>>> {
        read_frame(
            &mut *self
                .reader
                .lock()
                .map_err(|_| invalid("Dashboard pipe unavailable"))?,
        )
    }
    pub fn ready(&self) {
        unsafe {
            SetEvent(self.process.ready.as_raw_handle());
        }
    }
    pub fn close(&self) {
        let _ = self.send(CLOSE);
        self.process.signal_close();
    }
    pub fn stop(&self) {
        self.process.kill();
    }
    pub fn wait(&self) {
        unsafe {
            WaitForSingleObject(self.process.handle.as_raw_handle(), INFINITE);
        }
    }
}
impl Drop for DashboardChild {
    fn drop(&mut self) {
        if self.running() {
            self.stop();
        }
    }
}
