//! The running Windows monitor prepares the update and hands it to its private helper.
use super::package::{check_cancel, copy_contents, copy_tree, overlay_zip};
use super::{
    InstallResult, Plan, Replacement, Staged, absolute, installed, parent, process_started, same,
    validate_version,
};
use crate::files::{ensure_private_directory, invalid, read_file, reject_links, write_private};
use crate::updates::{PUBLIC_KEY, Platform, read_update_manifest, verify_file_while};
use serde::Deserialize;
use std::{
    fs, io,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, GetCurrentProcess};

#[derive(Debug)]
pub enum PreparationError {
    Signature,
    File(io::Error),
}

impl From<io::Error> for PreparationError {
    fn from(value: io::Error) -> Self {
        Self::File(value)
    }
}

fn integrity(error: io::Error) -> PreparationError {
    if error.kind() == io::ErrorKind::InvalidData {
        PreparationError::Signature
    } else {
        error.into()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Request {
    staged: Staged,
    target: PathBuf,
    data_root: PathBuf,
    current_version: String,
}

fn transaction(stage: &Path, data_root: &Path) -> io::Result<Uuid> {
    if !same(parent(stage)?, &data_root.join("Updates")) {
        return Err(invalid("Unexpected update stage"));
    }
    stage
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| invalid("Invalid update transaction"))
}

pub fn prepare(request: &[u8], cancel: &AtomicBool) -> Result<(), PreparationError> {
    check_cancel(cancel)?;
    let request: Request = serde_json::from_slice(request).map_err(io::Error::from)?;
    prepare_job(
        request.staged,
        &request.target,
        &request.data_root,
        request.current_version,
        cancel,
    )
}

pub(super) fn prepare_job(
    mut staged: Staged,
    target: &Path,
    data_root: &Path,
    current_version: String,
    cancel: &AtomicBool,
) -> Result<(), PreparationError> {
    check_cancel(cancel)?;
    staged.directory = absolute(&staged.directory)?.components().collect();
    let data_root: PathBuf = absolute(data_root)?.components().collect();
    let target: PathBuf = absolute(target)?.components().collect();
    let id = transaction(&staged.directory, &data_root)?;
    if data_root.ancestors().any(|p| same(p, &target)) {
        return Err(invalid("Invalid update target").into());
    }
    ensure_private_directory(&staged.directory)?;
    let plan = Plan {
        staged,
        installer: installed(&target)?,
        data_root,
        target,
        current_version,
        parent_pid: std::process::id(),
        parent_started: process_started(unsafe { GetCurrentProcess() })?,
    };
    let update = &plan.staged.update;
    let stage = &plan.staged.directory;
    let offered = Platform::for_asset(&update.release).map_err(integrity)?;
    let checked = read_update_manifest(
        &update.release,
        offered,
        &plan.current_version,
        stage,
        PUBLIC_KEY,
    )
    .map_err(integrity)?;
    if checked != *update
        || update.asset.name != plan.platform().asset_name(update.release.version()?)
    {
        return Err(invalid("Update platform mismatch").into());
    }
    let archive = plan.archive();
    reject_links(&archive)?;
    verify_file_while(&archive, &update.asset, || cancel.load(Ordering::Acquire))
        .map_err(integrity)?;
    let replacement = Replacement::new(&plan.target, id)?;
    let job = stage.join("install.json");
    if replacement.incoming.try_exists()? || replacement.backup.try_exists()? || job.try_exists()? {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists).into());
    }
    let helper = stage.join("Helper");
    fs::create_dir(&helper)?;
    // The helper directory also reserves this stage while it is being prepared.
    let mut made_incoming = false;
    let result = (|| -> io::Result<()> {
        if !plan.installer {
            copy_tree(&plan.target, &replacement.incoming, cancel)?;
            made_incoming = true;
            overlay_zip(&archive, &replacement.incoming, cancel)?;
            validate_version(&replacement.incoming, &update.release.version)?;
        }
        copy_contents(&plan.target.join("StatusLine"), &helper, cancel)?;
        let bytes = serde_json::to_vec(&plan)?;
        if bytes.len() > 64 * 1024 {
            return Err(invalid("Update job exceeds helper limit"));
        }
        check_cancel(cancel)?;
        write_private(&job, &bytes)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(helper);
        if made_incoming {
            let _ = fs::remove_dir_all(replacement.incoming);
        }
    }
    result.map_err(Into::into)
}

fn read_plan(job: &Path) -> io::Result<Plan> {
    reject_links(job)?;
    let plan: Plan = serde_json::from_slice(&read_file(job, 64 * 1024)?)?;
    let stage = parent(job)?;
    if job.file_name().and_then(|n| n.to_str()) != Some("install.json")
        || !same(stage, &absolute(&plan.staged.directory)?)
    {
        return Err(invalid("Unexpected update plan"));
    }
    transaction(stage, &absolute(&plan.data_root)?)?;
    Ok(plan)
}

pub struct Installer {
    child: Child,
    ready: PathBuf,
    version: String,
    detached: bool,
}
impl Installer {
    pub fn launch(job: &Path) -> io::Result<Self> {
        let job = absolute(job)?;
        let plan = read_plan(&job)?;
        let stage = parent(&job)?;
        let helper = stage.join("Helper/QuotaBar.StatusLine.exe");
        reject_links(&helper)?;
        let ready = stage.join("installer-ready.json");
        match fs::remove_file(&ready) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let child = Command::new(helper)
            .arg("--apply-update")
            .arg(&job)
            .current_dir(stage)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        Ok(Self {
            child,
            ready,
            version: plan.staged.update.release.version,
            detached: false,
        })
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
    pub fn ready(&mut self) -> io::Result<bool> {
        if self.child.try_wait()?.is_some() {
            return Err(invalid("Update helper exited"));
        }
        Ok(read_result(&self.ready).is_some_and(|r| r.success && r.version == self.version))
    }
    pub fn detach(&mut self) {
        self.detached = true;
    }
}
impl Drop for Installer {
    fn drop(&mut self) {
        if !self.detached && matches!(self.child.try_wait(), Ok(None)) && self.child.kill().is_ok()
        {
            let _ = self.child.wait();
        }
    }
}

pub(super) fn read_result(path: &Path) -> Option<InstallResult> {
    reject_links(path).ok()?;
    serde_json::from_slice(&read_file(path, 4096).ok()?).ok()
}

pub fn discard(data_root: &Path, target: &Path, stage: &Path) -> io::Result<()> {
    let stage = absolute(stage)?;
    let id = transaction(&stage, &absolute(data_root)?)?;
    let paths = Replacement::new(&absolute(target)?.components().collect::<PathBuf>(), id)?;
    if !paths.backup.try_exists()? && paths.incoming.try_exists()? {
        reject_links(&paths.incoming)?;
        fs::remove_dir_all(paths.incoming)?;
    }
    fs::remove_dir_all(stage)
}

pub fn clean_completed(data_root: &Path) -> io::Result<()> {
    let root = absolute(data_root)?.join("Updates");
    reject_links(&root)?;
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let dir = entry.path();
        if entry.file_type()?.is_dir()
            && Uuid::parse_str(&entry.file_name().to_string_lossy()).is_ok()
            && read_result(&dir.join("completed.json")).is_some_and(|r| r.success)
        {
            let _ = fs::remove_dir_all(dir);
        }
    }
    Ok(())
}

pub fn acknowledge(
    data_root: &Path,
    target: &Path,
    transaction_id: Uuid,
    current: &str,
) -> io::Result<()> {
    let data_root: PathBuf = absolute(data_root)?.components().collect();
    let target: PathBuf = absolute(target)?.components().collect();
    let stage = data_root.join("Updates").join(transaction_id.to_string());
    let plan = read_plan(&stage.join("install.json"))?;
    if !same(
        &target,
        &absolute(&plan.target)?.components().collect::<PathBuf>(),
    ) || !same(
        &data_root,
        &absolute(&plan.data_root)?.components().collect::<PathBuf>(),
    ) || plan.staged.update.release.version != current
    {
        return Err(invalid("Unexpected update startup acknowledgement"));
    }
    plan.result(
        &stage.join("started.json"),
        true,
        "업데이트를 설치했습니다.",
    )
}
