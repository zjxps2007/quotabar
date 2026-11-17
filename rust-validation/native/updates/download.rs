//! Release discovery and signed streaming downloads for the installed applications.
pub mod windows;
use super::{
    FeedError, MAX_FEED, MAX_MANIFEST, Platform, REPOSITORY, Release, Verified, Version,
    parse_feed, verify_file_while, verify_manifest,
};
use crate::files;
use std::{
    fs,
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const LATEST: &str = "https://api.github.com/repos/zjxps2007/quotabar/releases/latest";

#[derive(Debug)]
pub enum Error {
    Development,
    RateLimited,
    Feed(FeedError),
    Download,
    Signature,
    Cancelled,
    Network(i32),
    File(io::Error),
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::File(error)
    }
}

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum Resource {
    Feed,
    Asset,
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// A bounded public resource. The platform client writes only the final successful
/// response to the sink; a missing release is the one successful empty result.
pub struct Transfer<'a> {
    pub resource: Resource,
    pub url: &'a str,
    pub agent: &'a str,
    pub limit: u64,
    pub cancel: &'a AtomicBool,
    pub output: &'a mut dyn Write,
    pub progress: &'a mut dyn FnMut(i32),
}

#[cfg(any(windows, target_os = "macos"))]
impl Transfer<'_> {
    pub fn send(self) -> Result<bool, Error> {
        self.resource.fetch(
            self.url,
            self.agent,
            self.limit,
            self.cancel,
            self.output,
            self.progress,
        )
    }
}

#[cfg(any(windows, target_os = "macos"))]
pub fn latest(
    platform: Platform,
    current: &str,
    cancel: &AtomicBool,
) -> Result<Option<Release>, Error> {
    latest_with(platform, current, cancel, |t| t.send())
}

pub fn latest_with(
    platform: Platform,
    current: &str,
    cancel: &AtomicBool,
    mut send: impl FnMut(Transfer<'_>) -> Result<bool, Error>,
) -> Result<Option<Release>, Error> {
    current.parse::<Version>().map_err(|_| Error::Development)?;
    let mut data = Vec::new();
    if !send(Transfer {
        resource: Resource::Feed,
        url: LATEST,
        agent: &format!("QuotaBar/{current}"),
        limit: MAX_FEED as u64,
        cancel,
        output: &mut data,
        progress: &mut |_| {},
    })? {
        return Ok(None);
    }
    check_cancel(cancel)?;
    parse_feed(&data, platform).map_err(Error::Feed)
}

#[cfg(any(windows, target_os = "macos"))]
pub fn stage(
    release: &Release,
    current: &str,
    key: &str,
    directory: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(i32),
) -> Result<Verified, Error> {
    stage_with(release, current, key, directory, cancel, progress, |t| {
        t.send()
    })
}

pub fn stage_with(
    release: &Release,
    current: &str,
    key: &str,
    directory: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(i32),
    mut send: impl FnMut(Transfer<'_>) -> Result<bool, Error>,
) -> Result<Verified, Error> {
    check_cancel(cancel)?;
    let platform = Platform::for_asset(release).map_err(|_| Error::Signature)?;
    release
        .validate_asset(platform)
        .map_err(|_| Error::Signature)?;
    files::create_private_directory_new(directory)?;
    let attempt = (|| {
        let base = format!("{REPOSITORY}/releases/download/{}/", release.tag);
        let mut fetch = |name: &str, limit| -> Result<Vec<u8>, Error> {
            let mut bytes = Vec::new();
            send(Transfer {
                resource: Resource::Asset,
                url: &format!("{base}{name}"),
                agent: "QuotaBar-Updater",
                limit,
                cancel,
                output: &mut bytes,
                progress: &mut |_| {},
            })?;
            crate::files::write_private(&directory.join(name), &bytes)?;
            Ok(bytes)
        };
        let manifest = fetch("update-manifest.json", MAX_MANIFEST as u64)?;
        let signature = fetch("update-manifest.sig", 128)?;
        let verified = verify_manifest(release, platform, current, &manifest, &signature, key)
            .map_err(|_| Error::Signature)?;
        let asset = &verified.asset;
        let archive = directory.join(&asset.name);
        let mut payload = files::create_private(&archive)?;
        send(Transfer {
            resource: Resource::Asset,
            url: &asset.url,
            agent: "QuotaBar-Updater",
            limit: asset.bytes,
            cancel,
            output: &mut payload,
            progress: &mut progress,
        })?;
        payload.sync_all()?;
        drop(payload);
        verify_file_while(&archive, asset, || cancel.load(Ordering::Acquire)).map_err(|error| {
            if error.kind() == io::ErrorKind::Interrupted {
                Error::Cancelled
            } else if error.raw_os_error().is_some() {
                Error::File(error)
            } else {
                Error::Signature
            }
        })?;
        check_cancel(cancel)?;
        Ok(verified)
    })();
    if attempt.is_err() {
        let _ = fs::remove_dir_all(directory);
    }
    attempt
}
