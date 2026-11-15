//! Unauthenticated release discovery and bounded, signed update staging.
use crate::updates::{
    MAX_FEED, MAX_MANIFEST, Platform, REPOSITORY, Release, Verified, Version, parse_feed,
    verify_file_while, verify_manifest,
};
use block2::DynBlock;
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSObject, NSObjectProtocol, NSString,
    NSURL, NSURLRequest, NSURLResponse, NSURLSession, NSURLSessionConfiguration,
    NSURLSessionDataDelegate, NSURLSessionDataTask, NSURLSessionDelegate,
    NSURLSessionResponseDisposition as Disposition, NSURLSessionTask, NSURLSessionTaskDelegate,
};
use std::{
    fs::{self, DirBuilder, File},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    time::Duration,
};

const LATEST: &str = "https://api.github.com/repos/zjxps2007/quotabar/releases/latest";

#[derive(Debug)]
pub enum Error {
    Development,
    RateLimited,
    InvalidFeed,
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

#[derive(Copy, Clone)]
enum Resource {
    Feed,
    Asset,
}

enum Event {
    Headers {
        status: i32,
        length: i64,
        https: bool,
    },
    Bytes(Vec<u8>),
    Complete(Option<i32>),
}

struct Transfer {
    resource: Resource,
    tx: SyncSender<Event>,
}

define_class!(
    #[unsafe(super=NSObject)]
    #[ivars=Transfer]
    #[name = "QuotaBarReleaseTransfer"]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl NSURLSessionDelegate for Delegate {}
    unsafe impl NSURLSessionTaskDelegate for Delegate {
        #[unsafe(method(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:))]
        unsafe fn redirect(
            &self,
            _: &NSURLSession,
            _: &NSURLSessionTask,
            _: &NSHTTPURLResponse,
            request: &NSURLRequest,
            complete: &DynBlock<dyn Fn(*mut NSURLRequest)>,
        ) {
            let allowed = matches!(self.ivars().resource, Resource::Asset)
                && request.URL().is_some_and(|u| https(&u));
            complete.call((if allowed {
                ptr::from_ref(request).cast_mut()
            } else {
                ptr::null_mut()
            },));
        }

        #[unsafe(method(URLSession:task:didCompleteWithError:))]
        fn finished(&self, _: &NSURLSession, _: &NSURLSessionTask, error: Option<&NSError>) {
            let _ = self
                .ivars()
                .tx
                .send(Event::Complete(error.map(|e| e.code() as i32)));
        }
    }
    unsafe impl NSURLSessionDataDelegate for Delegate {
        #[unsafe(method(URLSession:dataTask:didReceiveResponse:completionHandler:))]
        unsafe fn response(
            &self,
            _: &NSURLSession,
            _: &NSURLSessionDataTask,
            response: &NSURLResponse,
            complete: &DynBlock<dyn Fn(Disposition)>,
        ) {
            let status = response
                .downcast_ref::<NSHTTPURLResponse>()
                .map_or(0, |r| r.statusCode() as i32);
            let delivered = self
                .ivars()
                .tx
                .send(Event::Headers {
                    status,
                    length: response.expectedContentLength(),
                    https: response.URL().is_some_and(|u| https(&u)),
                })
                .is_ok();
            complete.call((if delivered && status == 200 {
                Disposition::Allow
            } else {
                Disposition::Cancel
            },));
        }

        #[unsafe(method(URLSession:dataTask:didReceiveData:))]
        fn data(&self, _: &NSURLSession, task: &NSURLSessionDataTask, data: &NSData) {
            // Foundation owns immutable data throughout this callback. Keep a bounded
            // number of small chunks in memory even when the destination is a large ZIP.
            for chunk in unsafe { data.as_bytes_unchecked() }.chunks(64 * 1024) {
                if self.ivars().tx.send(Event::Bytes(chunk.to_vec())).is_err() {
                    task.cancel();
                    break;
                }
            }
        }
    }
);

fn https(url: &NSURL) -> bool {
    url.scheme()
        .is_some_and(|s| s.to_string().eq_ignore_ascii_case("https"))
}

struct Session(Retained<NSURLSession>);
impl Drop for Session {
    fn drop(&mut self) {
        self.0.invalidateAndCancel();
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Acquire) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

impl Resource {
    /// Returns false for a missing public release. Other HTTP failures are delivered
    /// immediately, even if the server never finishes sending its error page.
    fn fetch(
        self,
        url: &str,
        agent: &str,
        limit: u64,
        cancel: &AtomicBool,
        output: &mut dyn Write,
        mut progress: impl FnMut(i32),
    ) -> Result<bool, Error> {
        check_cancel(cancel)?;
        let (request_timeout, resource_timeout) = match self {
            Self::Feed => (20., 30.),
            Self::Asset => (30., 20. * 60.),
        };
        autoreleasepool(|_| {
            let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
            config.setHTTPCookieStorage(None);
            config.setURLCredentialStorage(None);
            config.setURLCache(None);
            config.setTimeoutIntervalForRequest(request_timeout);
            config.setTimeoutIntervalForResource(resource_timeout);
            let url = NSURL::URLWithString(&NSString::from_str(url))
                .filter(|u| https(u))
                .ok_or(Error::Network(-1000))?;
            let request = NSMutableURLRequest::requestWithURL(&url);
            request.setTimeoutInterval(request_timeout);
            request.setValue_forHTTPHeaderField(
                Some(&NSString::from_str(agent)),
                &NSString::from_str("User-Agent"),
            );
            if let Self::Feed = self {
                request.setValue_forHTTPHeaderField(
                    Some(&NSString::from_str("application/vnd.github+json")),
                    &NSString::from_str("Accept"),
                );
            }
            let (tx, rx) = mpsc::sync_channel(2);
            let delegate: Retained<Delegate> = unsafe {
                msg_send![
                    super(Delegate::alloc().set_ivars(Transfer { resource: self, tx })),
                    init
                ]
            };
            let session = Session(unsafe {
                NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
                    &config,
                    Some(ProtocolObject::from_ref(&*delegate)),
                    None,
                )
            });
            let task = session.0.dataTaskWithRequest(&request);
            // Disconnect any producer blocked on a full channel before invalidating its session.
            let events = rx;
            task.resume();
            let mut received = 0_u64;
            let mut headers = false;
            let mut last_progress = None;
            loop {
                check_cancel(cancel)?;
                match events.recv_timeout(Duration::from_millis(50)) {
                    Ok(Event::Headers {
                        status,
                        length,
                        https,
                    }) => {
                        match (self, status) {
                            (Self::Feed, 404) => return Ok(false),
                            (Self::Feed, 403 | 429) => return Err(Error::RateLimited),
                            (Self::Feed, 200) if length <= limit as i64 => {}
                            (Self::Feed, _) => return Err(Error::Network(-1011)),
                            (Self::Asset, 200) if https => {
                                if length > limit as i64 {
                                    return Err(Error::Network(-1103));
                                }
                            }
                            (Self::Asset, _) => return Err(Error::Download),
                        }
                        headers = true;
                    }
                    Ok(Event::Bytes(bytes)) if headers => {
                        received += bytes.len() as u64;
                        if received > limit {
                            return Err(Error::Network(-1103));
                        }
                        output.write_all(&bytes)?;
                        let percent = (received * 100 / limit) as i32;
                        if last_progress != Some(percent) {
                            progress(percent);
                            last_progress = Some(percent);
                        }
                    }
                    Ok(Event::Complete(Some(code))) => return Err(Error::Network(code)),
                    Ok(Event::Complete(None)) if headers => return Ok(true),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    _ => return Err(Error::Network(-1011)),
                }
            }
        })
    }
}

pub fn latest(
    platform: Platform,
    current: &str,
    cancel: &AtomicBool,
) -> Result<Option<Release>, Error> {
    current.parse::<Version>().map_err(|_| Error::Development)?;
    let mut data = Vec::new();
    if !Resource::Feed.fetch(
        LATEST,
        &format!("QuotaBar/{current}"),
        MAX_FEED as u64,
        cancel,
        &mut data,
        |_| {},
    )? {
        return Ok(None);
    }
    check_cancel(cancel)?;
    parse_feed(&data, platform).map_err(|_| Error::InvalidFeed)
}

pub fn stage(
    release: &Release,
    current: &str,
    key: &str,
    directory: &Path,
    cancel: &AtomicBool,
    progress: impl FnMut(i32),
) -> Result<Verified, Error> {
    check_cancel(cancel)?;
    let platform = Platform::for_asset(release).map_err(|_| Error::Signature)?;
    release
        .validate_asset(platform)
        .map_err(|_| Error::Signature)?;
    let parent = directory
        .parent()
        .ok_or_else(|| io::Error::other("Missing staging parent"))?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    // Only the caller which created this directory may remove it on failure.
    DirBuilder::new().mode(0o700).create(directory)?;
    let attempt = (|| {
        let base = format!("{REPOSITORY}/releases/download/{}/", release.tag);
        let fetch = |name: &str, limit| -> Result<Vec<u8>, Error> {
            let mut bytes = Vec::new();
            Resource::Asset.fetch(
                &format!("{base}{name}"),
                "QuotaBar-Updater",
                limit,
                cancel,
                &mut bytes,
                |_| {},
            )?;
            crate::files::write_private(&directory.join(name), &bytes)?;
            Ok(bytes)
        };
        let manifest = fetch("update-manifest.json", MAX_MANIFEST as u64)?;
        let signature = fetch("update-manifest.sig", 128)?;
        let verified = verify_manifest(release, platform, current, &manifest, &signature, key)
            .map_err(|_| Error::Signature)?;
        let asset = &verified.asset;
        let archive = directory.join(&asset.name);
        let mut payload = File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&archive)?;
        Resource::Asset.fetch(
            &asset.url,
            "QuotaBar-Updater",
            asset.bytes,
            cancel,
            &mut payload,
            progress,
        )?;
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
