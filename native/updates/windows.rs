use super::{InstallResult, Platform, Verified, Version, verified_stage};
use crate::files::{
    ensure_private_directory, invalid, read_file, reject_links, wide, write_private,
};
use crate::instance::acquire_lease;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    fs, io,
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::Command,
    ptr::null_mut,
    thread,
    time::Duration,
};
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::*,
    Globalization::{CSTR_EQUAL, CompareStringOrdinal},
    Storage::FileSystem::*,
    System::{Registry::*, Threading::*},
};

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\QuotaBar.Windows_is1";
const RETRY: Duration = Duration::from_millis(200);

mod installer;
mod package;
pub use installer::{Installer, PreparationError, acknowledge, clean_completed, discard, prepare};
pub use package::{copy_tree, overlay_zip};

#[derive(Deserialize, Serialize)]
struct Staged {
    update: Verified,
    directory: PathBuf,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    staged: Staged,
    target: PathBuf,
    data_root: PathBuf,
    current_version: String,
    installer: bool,
    parent_pid: u32,
    parent_started: i64,
}

fn same(a: &Path, b: &Path) -> bool {
    let (Ok(a), Ok(b)) = (wide(a), wide(b)) else {
        return false;
    };
    unsafe { CompareStringOrdinal(a.as_ptr(), -1, b.as_ptr(), -1, 1) == CSTR_EQUAL }
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("Relative update path"));
    }
    let path = std::path::absolute(path)?;
    reject_links(&path)?;
    Ok(path)
}
fn parent(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| invalid("Invalid update location"))
}
fn wide_string(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}
fn reg_status(status: u32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}
pub fn installed(target: &Path) -> io::Result<bool> {
    let target: PathBuf = std::path::absolute(target)?.components().collect();
    let key = wide_string(UNINSTALL);
    let value = wide_string("InstallLocation");
    let mut bytes = 0;
    unsafe {
        let status = RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut bytes,
        );
        if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
            return Ok(false);
        }
        reg_status(status)?;
        if bytes > 128 * 1024 {
            return Err(invalid("Invalid installation registry entry"));
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        reg_status(RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        ))?;
        let end = buffer.iter().position(|u| *u == 0).unwrap_or(buffer.len());
        let name = PathBuf::from(OsString::from_wide(&buffer[..end]));
        Ok(same(
            &std::path::absolute(name)?.components().collect::<PathBuf>(),
            &target,
        ))
    }
}
fn restore_registered_version(version: &str) -> io::Result<()> {
    let mut key = null_mut();
    unsafe {
        let status = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            wide_string(UNINSTALL).as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        );
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        reg_status(status)?;
        let data = wide_string(version);
        let status = RegSetValueExW(
            key,
            wide_string("DisplayVersion").as_ptr(),
            0,
            REG_SZ,
            data.as_ptr().cast(),
            (2 * data.len()) as u32,
        );
        RegCloseKey(key);
        reg_status(status)
    }
}

fn file_version(path: &Path) -> io::Result<Version> {
    let name = wide(path)?;
    unsafe {
        let size = GetFileVersionInfoSizeW(name.as_ptr(), null_mut());
        if size == 0 || size > 4 * 1024 * 1024 {
            return Err(invalid("Missing file version"));
        }
        let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
        if GetFileVersionInfoW(name.as_ptr(), 0, size, buffer.as_mut_ptr().cast()) == 0 {
            return Err(io::Error::last_os_error());
        }
        let (mut info, mut length) = (null_mut(), 0);
        if VerQueryValueW(
            buffer.as_ptr().cast(),
            [92, 0].as_ptr(),
            &mut info,
            &mut length,
        ) == 0
            || length < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
            || info.is_null()
        {
            return Err(invalid("Invalid file version"));
        }
        let v = std::ptr::read_unaligned(info.cast::<VS_FIXEDFILEINFO>());
        if v.dwSignature != 0xfeef04bd {
            return Err(invalid("Invalid file version"));
        }
        Ok(Version(
            (v.dwFileVersionMS >> 16) as i32,
            (v.dwFileVersionMS & 0xffff) as i32,
            (v.dwFileVersionLS >> 16) as i32,
        ))
    }
}
pub fn validate_version(folder: &Path, version: &str) -> io::Result<()> {
    let version: Version = version.parse()?;
    for base in [
        "QuotaBar.Monitor",
        "Dashboard/QuotaBar.Dashboard",
        "StatusLine/QuotaBar.StatusLine",
    ] {
        let exe = folder.join(format!("{base}.exe"));
        reject_links(&exe)?;
        if !exe.is_file() {
            return Err(invalid("Missing update program"));
        }
        // A Rust helper carries its version in its EXE. An older .NET helper
        // also has a runtimeconfig and must have a matching managed assembly.
        if base != "QuotaBar.Monitor" && file_version(&exe)? != version {
            return Err(invalid("Update package version mismatch"));
        }
        if base == "QuotaBar.Monitor" || folder.join(format!("{base}.runtimeconfig.json")).is_file()
        {
            let assembly = folder.join(format!("{base}.dll"));
            reject_links(&assembly)?;
            if file_version(&assembly)? != version {
                return Err(invalid("Update package version mismatch"));
            }
        }
    }
    Ok(())
}

struct Replacement {
    target: PathBuf,
    incoming: PathBuf,
    backup: PathBuf,
}
impl Replacement {
    fn new(target: &Path, transaction: Uuid) -> io::Result<Self> {
        let folder = parent(target)?;
        let name = target
            .file_name()
            .ok_or_else(|| invalid("Invalid update target"))?;
        let sibling = |kind| {
            let mut path = OsString::from(".");
            path.push(name);
            path.push(format!(".{kind}-{transaction}"));
            folder.join(path)
        };
        Ok(Self {
            target: target.to_owned(),
            incoming: sibling("update"),
            backup: sibling("backup"),
        })
    }
    fn swap(&self, rollback: bool) -> io::Result<()> {
        let (old, new) = if rollback {
            (&self.incoming, &self.backup)
        } else {
            (&self.backup, &self.incoming)
        };
        for path in [&self.target, old, new] {
            reject_links(path)?;
        }
        if !new.is_dir() || old.try_exists()? {
            return Err(invalid("Invalid update transaction state"));
        }
        let moved = self.target.is_dir();
        if moved {
            fs::rename(&self.target, old)?;
        } else if !rollback {
            return Err(invalid("Missing existing app"));
        }
        if let Err(error) = fs::rename(new, &self.target) {
            if moved && !self.target.exists() {
                fs::rename(old, &self.target)?;
            }
            return Err(error);
        }
        if rollback && moved {
            fs::remove_dir_all(old)?;
        }
        Ok(())
    }
}

fn process_started(handle: HANDLE) -> io::Result<i64> {
    let [mut created, mut exited, mut kernel, mut user] = [FILETIME::default(); 4];
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let file_ticks = (i64::from(created.dwHighDateTime) << 32) | i64::from(created.dwLowDateTime);
    // Windows FILETIME starts in 1601. The on-disk .NET plan uses ticks since year 1.
    file_ticks
        .checked_add(504_911_232_000_000_000)
        .ok_or_else(|| invalid("Invalid process creation time"))
}

fn wait_parent(pid: u32, started: i64) -> io::Result<()> {
    unsafe {
        let raw = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        );
        if raw.is_null() {
            let e = io::Error::last_os_error();
            return if e.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                Ok(())
            } else {
                Err(e)
            };
        }
        let process = OwnedHandle::from_raw_handle(raw);
        if process_started(process.as_raw_handle())? != started {
            return Ok(());
        }
        if WaitForSingleObject(process.as_raw_handle(), 120_000) != WAIT_OBJECT_0 {
            return Err(invalid("Timed out waiting for the old app"));
        }
        Ok(())
    }
}

impl Plan {
    fn platform(&self) -> Platform {
        if self.installer {
            Platform::WindowsInstaller
        } else {
            Platform::WindowsPortable
        }
    }
    fn archive(&self) -> PathBuf {
        self.staged.directory.join(&self.staged.update.asset.name)
    }
    fn result(&self, path: &Path, success: bool, message: &str) -> io::Result<()> {
        let result = InstallResult {
            version: self.staged.update.release.version.clone(),
            success,
            message: message.to_owned(),
        };
        write_private(path, &serde_json::to_vec(&result)?)
    }
    fn verify(&self) -> io::Result<()> {
        verified_stage(
            &self.staged.update,
            self.platform(),
            &self.current_version,
            &self.staged.directory,
            super::PUBLIC_KEY,
        )
    }
    fn start(&self, acknowledgement: Option<Uuid>) -> io::Result<()> {
        let mut command = Command::new(self.target.join("QuotaBar.Monitor.exe"));
        command
            .current_dir(&self.target)
            .args(["--background", "--data-dir"])
            .arg(&self.data_root)
            .creation_flags(CREATE_NO_WINDOW);
        if let Some(id) = acknowledgement {
            command.arg("--update-ack").arg(id.to_string());
        }
        command.spawn()?;
        Ok(())
    }
    fn install(&self) -> io::Result<()> {
        let mut dir = OsString::from("/DIR=");
        dir.push(&self.target);
        let mut log = OsString::from("/LOG=");
        log.push(self.staged.directory.join("setup.log"));
        if !Command::new(self.archive())
            .current_dir(&self.staged.directory)
            .creation_flags(CREATE_NO_WINDOW)
            .args([
                "/SP-",
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/RESTARTEXITCODE=3010",
                "/NORESTARTAPPLICATIONS",
                "/NOCLOSEAPPLICATIONS",
            ])
            .arg(dir)
            .arg(log)
            .status()?
            .success()
        {
            return Err(invalid("Setup failed"));
        }
        Ok(())
    }
}

pub fn apply(job: &Path) -> io::Result<()> {
    let job = absolute(job)?;
    if job.file_name().and_then(|s| s.to_str()) != Some("install.json") {
        return Err(invalid("Invalid installer job"));
    }
    let stage = parent(&job)?;
    let transaction = stage
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| invalid("Invalid transaction ID"))?;
    ensure_private_directory(stage)?;
    let mut plan: Plan = serde_json::from_slice(&read_file(&job, 64 * 1024)?)?;
    plan.target = absolute(&plan.target)?.components().collect();
    plan.data_root = absolute(&plan.data_root)?.components().collect();
    if !same(stage, &absolute(&plan.staged.directory)?)
        || !same(parent(stage)?, &plan.data_root.join("Updates"))
        || plan
            .data_root
            .ancestors()
            .any(|ancestor| same(ancestor, &plan.target))
        || installed(&plan.target)? != plan.installer
    {
        return Err(invalid("Invalid installer location"));
    }
    plan.staged.directory = stage.to_owned();
    validate_version(&plan.target, &plan.current_version)?;
    let replacement = Replacement::new(&plan.target, transaction)?;
    let version = &plan.staged.update.release.version;
    let result = plan.data_root.join("Updates/last-result.json");
    let mut changed = false;
    let mut lock = None;
    let attempt = (|| {
        plan.verify()?;
        if !plan.installer {
            validate_version(&replacement.incoming, version)?;
        }
        plan.result(&stage.join("installer-ready.json"), true, "ready")?;
        wait_parent(plan.parent_pid, plan.parent_started)?;
        lock = Some(acquire_lease(&plan.data_root, Duration::from_secs(30))?);
        validate_version(&plan.target, &plan.current_version)?;
        if plan.installer {
            copy_tree(
                &plan.target,
                &replacement.backup,
                &std::sync::atomic::AtomicBool::new(false),
            )?;
            changed = true;
            plan.install()?;
        } else {
            replacement.swap(false)?;
            changed = true;
        }
        validate_version(&plan.target, version)?;
        drop(lock.take());
        plan.start(Some(transaction))?;
        let ack = stage.join("started.json");
        for _ in 0..200 {
            if let Some(bytes) = read_file(&ack, 4096).ok().filter(|bytes| {
                serde_json::from_slice::<InstallResult>(bytes)
                    .is_ok_and(|r| r.success && r.version == *version)
            }) {
                write_private(&result, &bytes)?;
                if reject_links(&replacement.backup).is_ok() {
                    let _ = fs::remove_dir_all(&replacement.backup);
                }
                write_private(&stage.join("completed.json"), &bytes)?;
                return Ok(());
            }
            thread::sleep(RETRY);
        }
        Err(invalid("Updated application did not acknowledge startup"))
    })();
    if attempt.is_err() {
        let restored = (|| -> io::Result<()> {
            if !changed {
                return Ok(());
            }
            if lock.is_none() {
                lock = Some(acquire_lease(&plan.data_root, Duration::from_secs(1))?);
            }
            replacement.swap(true)?;
            if plan.installer {
                restore_registered_version(&plan.current_version)?;
            }
            Ok(())
        })()
        .is_ok();
        let message = if restored {
            "자동 업데이트 설치에 실패해 기존 버전을 유지했습니다. 업데이트를 다시 확인해 주세요."
        } else {
            "업데이트 시작을 확인하지 못했습니다. 복구용 이전 앱을 보관했습니다."
        };
        let _ = plan.result(&result, false, message);
        drop(lock);
        if restored {
            let _ = plan.start(None);
        }
    }
    attempt
}
