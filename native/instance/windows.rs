use crate::files::{
    invalid,
    windows::{TokenSid, attributes},
};
use sha2::{Digest, Sha256};
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::Authorization::SE_KERNEL_OBJECT,
    Storage::FileSystem::*,
    System::{IO::*, Pipes::*, Threading::*},
};

fn event() -> io::Result<OwnedHandle> {
    let raw = unsafe { CreateEventW(null(), 1, 0, null()) };
    if raw.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}

// .NET ToUpperInvariant uses simple ICU mappings and preserves the Turkish dotless i.
// Existing monitors hash the uppercased path and current SID to find each other's pipe.
fn pipe_name(root: &Path, sid: &str) -> io::Result<Vec<u16>> {
    let path = root
        .to_str()
        .ok_or_else(|| invalid("Invalid monitor directory"))?;
    let upper = crate::windows_format::invariant_uppercase(path);
    let hash = Sha256::new()
        .chain_update(upper.as_bytes())
        .chain_update(sid.as_bytes())
        .finalize();
    let mut name = String::from(r"\\.\pipe\QuotaBar-");
    use std::fmt::Write;
    for byte in &hash[..12] {
        write!(name, "{byte:02X}").unwrap();
    }
    Ok(name.encode_utf16().chain([0]).collect())
}

pub(super) struct Activation {
    name: Vec<u16>,
    user: TokenSid,
    owner: TokenSid,
    stop: OwnedHandle,
}

impl Activation {
    pub(super) fn new(root: &Path) -> io::Result<Self> {
        let user = TokenSid::user()?;
        Ok(Self {
            name: pipe_name(root, &user.sid_text()?)?,
            user,
            owner: TokenSid::owner()?,
            stop: event()?,
        })
    }

    pub(super) fn stop(&self) {
        unsafe {
            SetEvent(self.stop.as_raw_handle());
        }
    }

    fn stopped(&self, wait_ms: u32) -> bool {
        unsafe { WaitForSingleObject(self.stop.as_raw_handle(), wait_ms) == WAIT_OBJECT_0 }
    }

    // Cancel and drain pending I/O before dropping its stack buffer or OVERLAPPED structure.
    fn transfer(
        &self,
        pipe: &OwnedHandle,
        timeout: u32,
        start: impl FnOnce(*mut OVERLAPPED, *mut u32) -> i32,
    ) -> io::Result<u32> {
        let done = event()?;
        let mut op = OVERLAPPED {
            hEvent: done.as_raw_handle(),
            ..Default::default()
        };
        let mut count = 0;
        if start(&mut op, &mut count) != 0 {
            return Ok(count);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(error);
        }
        let handles = [self.stop.as_raw_handle(), done.as_raw_handle()];
        unsafe {
            let wait = WaitForMultipleObjects(2, handles.as_ptr(), 0, timeout);
            if wait == WAIT_OBJECT_0 + 1 {
                if GetOverlappedResult(pipe.as_raw_handle(), &op, &mut count, 0) == 0 {
                    return Err(io::Error::last_os_error());
                }
                return Ok(count);
            }
            let error = match wait {
                WAIT_OBJECT_0 => io::ErrorKind::Interrupted.into(),
                WAIT_TIMEOUT => io::ErrorKind::TimedOut.into(),
                _ => io::Error::last_os_error(),
            };
            CancelIoEx(pipe.as_raw_handle(), &op);
            GetOverlappedResult(pipe.as_raw_handle(), &op, &mut count, 1);
            Err(error)
        }
    }

    pub(super) fn send(&self) -> io::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(3);
        let pipe = loop {
            let raw = unsafe {
                CreateFileW(
                    self.name.as_ptr(),
                    GENERIC_WRITE | READ_CONTROL,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    null_mut(),
                )
            };
            if raw != INVALID_HANDLE_VALUE {
                break unsafe { OwnedHandle::from_raw_handle(raw) };
            }
            let error = io::Error::last_os_error();
            if !matches!(
                error.raw_os_error().map(|e| e as u32),
                Some(ERROR_FILE_NOT_FOUND | ERROR_PIPE_BUSY)
            ) {
                return Err(error);
            }
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            if self.stopped(25) {
                return Err(io::ErrorKind::Interrupted.into());
            }
        };
        self.owner.check_owner(&pipe, SE_KERNEL_OBJECT)?;
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u32;
        let byte = [1u8];
        let n = self.transfer(&pipe, remaining, |op, count| unsafe {
            WriteFile(pipe.as_raw_handle(), byte.as_ptr(), 1, count, op)
        })?;
        if n == 1 {
            Ok(())
        } else {
            Err(io::ErrorKind::WriteZero.into())
        }
    }

    pub(super) fn receive(&self) -> io::Result<bool> {
        // .NET checks the token's default owner, which can differ from the user's SID when elevated.
        let security = self.user.descriptor_for_owner(false, &self.owner)?;
        while !self.stopped(0) {
            let raw = unsafe {
                CreateNamedPipeW(
                    self.name.as_ptr(),
                    PIPE_ACCESS_INBOUND | FILE_FLAG_OVERLAPPED | FILE_FLAG_FIRST_PIPE_INSTANCE,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    0,
                    4096,
                    0,
                    &attributes(&security),
                )
            };
            if raw == INVALID_HANDLE_VALUE {
                let error = io::Error::last_os_error();
                if matches!(
                    error.raw_os_error().map(|e| e as u32),
                    Some(ERROR_PIPE_BUSY | ERROR_ACCESS_DENIED)
                ) {
                    if self.stopped(100) {
                        return Ok(false);
                    }
                    continue;
                }
                return Err(error);
            }
            let pipe = unsafe { OwnedHandle::from_raw_handle(raw) };
            let read = self.receive_byte(&pipe);
            unsafe {
                DisconnectNamedPipe(raw);
            }
            match read {
                Ok(Some(1)) => return Ok(true),
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted && self.stopped(0) => {
                    return Ok(false);
                }
                Err(e)
                    if e.kind() == io::ErrorKind::TimedOut
                        || matches!(
                            e.raw_os_error().map(|v| v as u32),
                            Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED)
                        ) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(false)
    }

    fn receive_byte(&self, pipe: &OwnedHandle) -> io::Result<Option<u8>> {
        let raw = pipe.as_raw_handle();
        self.transfer(pipe, INFINITE, |op, _| unsafe {
            let connected = ConnectNamedPipe(raw, op);
            if connected == 0 && GetLastError() == ERROR_PIPE_CONNECTED {
                1
            } else {
                connected
            }
        })?;
        let mut byte = 0;
        let n = self.transfer(pipe, 1000, |op, count| unsafe {
            ReadFile(raw, &mut byte, 1, count, op)
        })?;
        Ok((n == 1).then_some(byte))
    }
}
