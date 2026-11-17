use super::{
    InstallResult, PUBLIC_KEY, Plan, Replacement, bundle_field, data_root, invalid, local_url,
    platform, read_file, read_plan, run, same_location, transaction_for, validate_bundle,
    verified_stage, write_private,
};
use objc2_foundation::NSBundle;
use std::{
    env, fs, io,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
};
use uuid::Uuid;

pub fn prepare(plan: &[u8]) -> io::Result<()> {
    prepare_in(&data_root().join("Updates"), plan, PUBLIC_KEY)
}

fn prepare_in(updates: &Path, bytes: &[u8], key: &str) -> io::Result<()> {
    if bytes.len() > 64 * 1024 {
        return Err(invalid("Update job exceeds the helper's limit"));
    }
    let plan: Plan = serde_json::from_slice(bytes)?;
    let stage = local_url(&plan.staged.directory)?;
    let id = transaction_for(updates, &stage)?;
    let paths = Replacement::new(local_url(&plan.target)?, &id)?;
    let update = &plan.staged.update;
    verified_stage(update, platform(), &plan.current_version, &stage, key)?;
    if paths.incoming.try_exists()? || paths.backup.try_exists()? {
        return Err(io::ErrorKind::AlreadyExists.into());
    }

    let unpacked = stage.join("unpacked");
    let installer = stage.join("Installer.app");
    // Reserve this transaction's workspace and each destination before copying.
    // A failed or concurrent attempt must never clean up another attempt's bundle.
    fs::DirBuilder::new().mode(0o700).create(&unpacked)?;
    let mut made_incoming = false;
    let mut made_installer = false;
    let result = (|| {
        run(Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(stage.join(&update.asset.name))
            .arg(&unpacked))?;
        let application = unpacked.join("QuotaBar.app");
        validate_bundle(&application, update, key)?;
        fs::create_dir(&paths.incoming)?;
        made_incoming = true;
        run(Command::new("/usr/bin/ditto")
            .arg(&application)
            .arg(&paths.incoming))?;
        validate_bundle(&paths.incoming, update, key)?;
        fs::create_dir(&installer)?;
        made_installer = true;
        run(Command::new("/usr/bin/ditto")
            .arg(&paths.target)
            .arg(&installer))?;
        write_private(&stage.join("install.json"), bytes)
    })();
    if result.is_err() {
        if made_installer {
            let _ = fs::remove_dir_all(&installer);
        }
        if made_incoming {
            let _ = fs::remove_dir_all(&paths.incoming);
        }
    }
    let _ = fs::remove_dir_all(unpacked);
    result
}

pub fn discard(stage: &Path, target: &Path) -> io::Result<()> {
    let transaction = transaction_for(&data_root().join("Updates"), stage)?;
    if let Ok(paths) = Replacement::new(target.to_owned(), &transaction)
        && paths.backup.try_exists().is_ok_and(|exists| !exists)
    {
        let _ = fs::remove_dir_all(paths.incoming);
    }
    fs::remove_dir_all(stage)
}

pub struct Installer {
    child: Child,
    ready: PathBuf,
    version: String,
}

impl Installer {
    pub fn launch(job: &Path) -> io::Result<Self> {
        Self::launch_in(job, &data_root().join("Updates"))
    }

    fn launch_in(job: &Path, updates: &Path) -> io::Result<Self> {
        let (plan, stage, _) = read_plan(job, updates)?;
        let ready = stage.join("installer-ready.json");
        match fs::remove_file(&ready) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let child = Command::new(stage.join("Installer.app/Contents/MacOS/QuotaBar"))
            .arg("--apply-update")
            .arg(job)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(Self {
            child,
            ready,
            version: plan.staged.update.release.version,
        })
    }

    pub fn ready(&mut self) -> io::Result<bool> {
        if self.child.try_wait()?.is_some() {
            return Err(invalid("Update helper exited"));
        }
        Ok(read_file(&self.ready, 1023)
            .ok()
            .and_then(|data| serde_json::from_slice::<InstallResult>(&data).ok())
            .is_some_and(|result| result.success && result.version == self.version))
    }

    pub fn finish(mut self, terminate: bool) {
        if self.child.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }
        if terminate {
            // This unreaped Child owns the PID until wait returns.
            unsafe {
                libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
            }
        }
        let _ = thread::Builder::new()
            .name("update helper exit".into())
            .spawn(move || {
                let _ = self.child.wait();
            });
    }
}

pub fn acknowledge_launch() {
    let mut args = env::args_os().skip_while(|a| a != "--update-ack").skip(1);
    let Some(transaction) = args.next().and_then(|s| Uuid::parse_str(s.to_str()?).ok()) else {
        return;
    };
    let target = PathBuf::from(NSBundle::mainBundle().bundlePath().to_string());
    let updates = data_root().join("Updates");
    if let Err(error) = acknowledge(&updates, &target, transaction) {
        eprintln!(
            "Cannot acknowledge update {transaction} in {}: {error}",
            updates.display()
        );
    }
}

fn acknowledge(updates: &Path, target: &Path, transaction: Uuid) -> io::Result<()> {
    let stage = updates.join(transaction.hyphenated().to_string().to_ascii_uppercase());
    let (plan, _, _) = read_plan(&stage.join("install.json"), updates)?;
    let version = bundle_field(target, "CFBundleShortVersionString")?;
    if !same_location(&local_url(&plan.target)?, target)?
        || plan.staged.update.release.version != version
    {
        return Err(invalid("Unexpected application startup acknowledgement"));
    }
    let result = InstallResult {
        version,
        success: true,
        message: "업데이트를 설치했습니다.".to_owned(),
    };
    write_private(&stage.join("started.json"), &serde_json::to_vec(&result)?)
}

#[cfg(test)]
mod tests;
