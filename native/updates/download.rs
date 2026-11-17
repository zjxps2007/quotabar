//! Release discovery and verified streaming downloads for the installed applications.
pub mod windows;
use super::{
    FeedError, MAX_FEED, MAX_MANIFEST, Platform, REPOSITORY, Release, Verified, Version,
    parse_feed, verify_file_while, verify_manifest, verify_unsigned_manifest,
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
    OptionalAsset,
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
        let mut fetch = |name: &str, limit, optional| -> Result<Option<Vec<u8>>, Error> {
            let mut bytes = Vec::new();
            let found = send(Transfer {
                resource: if optional {
                    Resource::OptionalAsset
                } else {
                    Resource::Asset
                },
                url: &format!("{base}{name}"),
                agent: "QuotaBar-Updater",
                limit,
                cancel,
                output: &mut bytes,
                progress: &mut |_| {},
            })?;
            check_cancel(cancel)?;
            if !found {
                return if optional {
                    Ok(None)
                } else {
                    Err(Error::Download)
                };
            }
            crate::files::write_private(&directory.join(name), &bytes)?;
            Ok(Some(bytes))
        };
        let manifest =
            fetch("update-manifest.json", MAX_MANIFEST as u64, false)?.ok_or(Error::Download)?;
        let signature = fetch("update-manifest.sig", 128, true)?;
        let verified = match signature {
            Some(signature) => {
                verify_manifest(release, platform, current, &manifest, &signature, key)
            }
            None => verify_unsigned_manifest(release, platform, current, &manifest),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::updates::{Asset, verified_stage};
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    fn unsigned_download(corrupt: bool, signature_status: u16, cancel_midway: bool) {
        let directory =
            std::env::temp_dir().join(format!("quotabar-unsigned-update-{}", Uuid::new_v4()));
        let payload = b"exact published archive bytes";
        let name = "QuotaBar-1.1.0-Windows-x64.zip";
        let asset = Asset {
            name: name.into(),
            url: format!("{REPOSITORY}/releases/download/v1.1.0/{name}"),
            bytes: payload.len() as u64,
            sha256: format!("{:x}", Sha256::digest(payload)),
        };
        let release = Release {
            tag: "v1.1.0".into(),
            version: "1.1.0".into(),
            page: format!("{REPOSITORY}/releases/tag/v1.1.0"),
            asset: Some(asset.clone()),
        };
        let manifest = serde_json::to_vec(&serde_json::json!({
            "schemaVersion":1,"version":"1.1.0","build":12,
            "assets":[{"name":name,"bytes":asset.bytes,"sha256":asset.sha256}],
        }))
        .unwrap();
        let cancel = AtomicBool::new(false);
        let mut requests = Vec::new();
        let result = stage_with(
            &release,
            "1.0.0",
            "",
            &directory,
            &cancel,
            |_| {},
            |transfer| {
                requests.push(transfer.url.to_owned());
                let resource = transfer.resource;
                windows::fetch_with(transfer, |url, response| {
                    if url.path().ends_with("update-manifest.sig") {
                        assert!(resource == Resource::OptionalAsset);
                        response.headers(signature_status, Some(0), None)?;
                    } else {
                        let body = if url.path().ends_with("update-manifest.json") {
                            manifest.clone()
                        } else if corrupt {
                            vec![0; payload.len()]
                        } else {
                            payload.to_vec()
                        };
                        if response.headers(200, Some(body.len() as u64), None)? {
                            response.write(&body)?;
                            if cancel_midway && url.path().ends_with(".zip") {
                                cancel.store(true, Ordering::Release);
                            }
                        }
                    }
                    Ok(())
                })
            },
        );
        if cancel_midway {
            assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
            assert!(!directory.exists());
        } else if signature_status != 404 {
            assert!(matches!(result, Err(Error::Download)), "{result:?}");
            assert!(!directory.exists());
            assert_eq!(requests.len(), 2);
        } else if corrupt {
            assert!(matches!(result, Err(Error::Signature)), "{result:?}");
            assert!(!directory.exists());
        } else {
            let update = result.unwrap();
            assert_eq!(update.build, 12);
            assert!(!directory.join("update-manifest.sig").exists());
            assert_eq!(fs::read(directory.join(name)).unwrap(), payload);
            verified_stage(&update, Platform::WindowsPortable, "1.0.0", &directory, "").unwrap();
            fs::write(directory.join(name), vec![0; payload.len()]).unwrap();
            assert!(
                verified_stage(&update, Platform::WindowsPortable, "1.0.0", &directory, "")
                    .is_err()
            );
            fs::remove_dir_all(&directory).unwrap();
        }
    }

    #[test]
    fn unsigned_update_downloads_and_revalidates_without_any_key() {
        unsigned_download(false, 404, false);
    }
    #[test]
    fn unsigned_update_rejects_changed_archive_and_removes_the_stage() {
        unsigned_download(true, 404, false);
    }
    #[test]
    fn signature_server_errors_cannot_silently_remove_verification() {
        unsigned_download(false, 500, false);
    }
    #[test]
    fn unsigned_update_cancellation_removes_the_stage() {
        unsigned_download(false, 404, true);
    }
}
