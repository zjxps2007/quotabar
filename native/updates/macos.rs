use super::{InstallResult, PUBLIC_KEY, Platform, Verified, verified_stage};
use crate::files::{invalid, read_file, write_private};
use crate::instance::acquire_lease;
use objc2_foundation::{NSBundle, NSString, NSURL};
use serde::Deserialize;
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    fs, io,
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};
use uuid::Uuid;

#[derive(Deserialize)]
struct Staged {
    update: Verified,
    directory: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    staged: Staged,
    target: String,
    current_version: String,
}

fn local_url(value: &str) -> io::Result<PathBuf> {
    let url = NSURL::URLWithString(&NSString::from_str(value))
        .filter(|u| {
            u.isFileURL()
                && u.host()
                    .is_none_or(|h| h.is_empty() || h.to_string().eq_ignore_ascii_case("localhost"))
        })
        .ok_or_else(|| invalid("Expected a local file URL"))?;
    let path = unsafe { CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) };
    let path = PathBuf::from(OsStr::from_bytes(path.to_bytes()));
    if !path.is_absolute() {
        return Err(invalid("Expected an absolute file URL"));
    }
    Ok(path)
}

fn bundle_field(bundle: &Path, key: &str) -> io::Result<String> {
    let result = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()?;
    if !result.status.success() {
        return Err(invalid("Missing application bundle metadata"));
    }
    Ok(String::from_utf8(result.stdout)
        .map_err(io::Error::other)?
        .trim_end()
        .into())
}

fn run(program: &mut Command) -> io::Result<()> {
    if !program
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?
        .success()
    {
        return Err(invalid("System update command failed"));
    }
    Ok(())
}

fn validate_bundle(bundle: &Path, update: &Verified) -> io::Result<()> {
    if bundle_field(bundle, "CFBundleIdentifier")? != "com.aiusage.lite"
        || bundle_field(bundle, "CFBundleShortVersionString")? != update.release.version
        || bundle_field(bundle, "CFBundleVersion")? != update.build.to_string()
    {
        return Err(invalid("Update bundle identity or version mismatch"));
    }
    let resources = bundle.join("Contents/Resources/QuotaBar_QuotaBar.bundle");
    let url = NSURL::fileURLWithPath(&NSString::from_str(
        resources
            .to_str()
            .ok_or_else(|| invalid("Invalid bundle path"))?,
    ));
    let key = NSBundle::bundleWithURL(&url)
        .and_then(|b| {
            b.URLForResource_withExtension(
                Some(&NSString::from_str("UpdatePublicKey")),
                Some(&NSString::from_str("pem")),
            )
        })
        .and_then(|u| u.path())
        .ok_or_else(|| invalid("Missing update public key"))?;
    if fs::read_to_string(key.to_string())?.trim() != PUBLIC_KEY.trim() {
        return Err(invalid("Update public key mismatch"));
    }
    run(Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", "--deep"])
        .arg(bundle))?;
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    run(Command::new("/usr/bin/lipo")
        .arg(bundle.join("Contents/MacOS/QuotaBar"))
        .args(["-verify_arch", arch]))
}

fn swap(first: &Path, second: &Path) -> io::Result<()> {
    let left = CString::new(first.as_os_str().as_bytes())?;
    let right = CString::new(second.as_os_str().as_bytes())?;
    // RENAME_SWAP makes the application path usable throughout replacement.
    if unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_SWAP,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct Replacement {
    target: PathBuf,
    incoming: PathBuf,
    backup: PathBuf,
}
impl Replacement {
    fn new(target: PathBuf, transaction: &str) -> io::Result<Self> {
        if target.extension() != Some(OsStr::new("app")) || fs::canonicalize(&target)? != target {
            return Err(invalid("Invalid bundle replacement path"));
        }
        let stem = target
            .file_stem()
            .ok_or_else(|| invalid("Missing bundle name"))?;
        let sibling = |kind: &str| {
            let mut name = OsString::from(".");
            name.push(stem);
            name.push(format!(".{kind}-{transaction}.app"));
            target.with_file_name(name)
        };
        Ok(Self {
            incoming: sibling("update"),
            backup: sibling("backup"),
            target,
        })
    }
    fn commit(&self) -> io::Result<()> {
        if self.backup.try_exists()? {
            return Err(invalid("Backup already exists"));
        }
        swap(&self.target, &self.incoming)?;
        if let Err(error) = fs::rename(&self.incoming, &self.backup) {
            swap(&self.target, &self.incoming)?;
            return Err(error);
        }
        Ok(())
    }
    fn rollback(&self) -> io::Result<()> {
        if self.target.try_exists()? {
            swap(&self.target, &self.backup)?;
            let _ = fs::remove_dir_all(&self.backup);
        } else {
            fs::rename(&self.backup, &self.target)?;
        }
        Ok(())
    }
}

fn open_app(path: &Path, transaction: Option<&str>) -> io::Result<()> {
    let mut open = Command::new("/usr/bin/open");
    open.arg("-n").arg(path).args(["--args", "--menu-bar-only"]);
    if let Some(id) = transaction {
        open.args(["--update-ack", id]);
    }
    run(&mut open)
}

pub fn apply(job: &Path) -> io::Result<()> {
    let root = std::env::home_dir()
        .ok_or_else(|| invalid("Missing home directory"))?
        .join("Library/Application Support/AIUsage Lite");
    let updates = root.join("Updates");
    let stage = job.parent().ok_or_else(|| invalid("Invalid job path"))?;
    if job.file_name() != Some(OsStr::new("install.json"))
        || stage.parent() != Some(&updates)
        || fs::canonicalize(job)? != job
    {
        return Err(invalid("Unexpected update job location"));
    }
    let info = fs::metadata(stage)?;
    if info.uid() != unsafe { libc::getuid() } || info.mode() & 0o077 != 0 {
        return Err(invalid("Update directory is not private"));
    }
    let transaction = stage
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| invalid("Invalid update transaction ID"))?
        .hyphenated()
        .to_string()
        .to_ascii_uppercase();
    let plan: Plan = serde_json::from_slice(&read_file(job, 64 * 1024)?)?;
    let target = local_url(&plan.target)?;
    if local_url(&plan.staged.directory)? != stage
        || bundle_field(&target, "CFBundleIdentifier")? != "com.aiusage.lite"
    {
        return Err(invalid("Unexpected update target"));
    }
    let paths = Replacement::new(target, &transaction)?;
    let update = &plan.staged.update;
    let result = |success, message: &str| -> io::Result<Vec<u8>> {
        Ok(serde_json::to_vec(&InstallResult {
            version: update.release.version.clone(),
            success,
            message: message.to_owned(),
        })?)
    };
    let history = updates.join("last-result.json");
    let mut lease = None;
    let mut committed = false;
    let attempt = (|| {
        let platform = if cfg!(target_arch = "aarch64") {
            Platform::MacArm64
        } else {
            Platform::MacX64
        };
        verified_stage(update, platform, &plan.current_version, stage)?;
        validate_bundle(&paths.incoming, update)?;
        write_private(&stage.join("installer-ready.json"), &result(true, "ready")?)?;
        lease = Some(acquire_lease(&root, Duration::from_secs(120))?);
        if bundle_field(&paths.target, "CFBundleShortVersionString")? != plan.current_version {
            return Err(invalid("Installed version changed"));
        }
        paths.commit()?;
        committed = true;
        drop(lease.take());
        open_app(&paths.target, Some(&transaction))?;
        let ack = stage.join("started.json");
        for _ in 0..200 {
            if let Ok(bytes) = read_file(&ack, 1023)
                && serde_json::from_slice::<InstallResult>(&bytes)
                    .is_ok_and(|r| r.success && r.version == update.release.version)
            {
                write_private(&history, &bytes)?;
                let _ = fs::remove_dir_all(&paths.backup);
                let _ = fs::remove_dir_all(stage);
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
        Err(invalid("Updated application did not acknowledge startup"))
    })();
    if attempt.is_err() {
        let restored = !committed || {
            if lease.is_none() {
                lease = acquire_lease(&root, Duration::from_secs(1)).ok();
            }
            lease.is_some() && paths.rollback().is_ok()
        };
        let message = if restored {
            "자동 업데이트 설치에 실패해 기존 버전을 유지했습니다. 업데이트를 다시 확인해 주세요."
        } else {
            "업데이트 시작을 확인하지 못했습니다. 복구용 이전 앱을 보관했습니다."
        };
        if let Ok(bytes) = result(false, message) {
            let _ = write_private(&history, &bytes);
        }
        drop(lease);
        if restored {
            let _ = open_app(&paths.target, None);
        }
    }
    attempt
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swift_file_urls_with_directory_hints_roundtrip() {
        let path = Path::new("/Applications/QuotaBar 한글.app");
        for suffix in ["", "/"] {
            assert_eq!(
                local_url(&format!(
                    "file:///Applications/QuotaBar%20%ED%95%9C%EA%B8%80.app{suffix}"
                ))
                .unwrap(),
                path
            );
        }
        assert!(local_url("https://example.invalid/QuotaBar.app").is_err());
    }

    #[test]
    fn swap_and_rollback_keep_the_bundle_present() {
        let directory = std::env::temp_dir().join(Uuid::new_v4().to_string());
        fs::create_dir(&directory).unwrap();
        let target = fs::canonicalize(&directory).unwrap().join("QuotaBar.app");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("old"), b"original").unwrap();
        let paths = Replacement::new(target, "test").unwrap();
        fs::create_dir(&paths.incoming).unwrap();
        fs::write(paths.incoming.join("new"), b"update").unwrap();
        paths.commit().unwrap();
        assert!(paths.target.join("new").is_file() && paths.backup.join("old").is_file());
        paths.rollback().unwrap();
        assert!(paths.target.join("old").is_file() && !paths.backup.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
