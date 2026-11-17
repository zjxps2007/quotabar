//! Foundation transport for public release assets.
pub use crate::updates::download::{Error, latest, stage};
use crate::updates::download::{Resource, check_cancel};
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
    io::Write,
    ptr,
    sync::{
        atomic::AtomicBool,
        mpsc::{self, SyncSender},
    },
    time::Duration,
};

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
            let allowed = matches!(
                self.ivars().resource,
                Resource::Asset | Resource::OptionalAsset
            ) && request.URL().is_some_and(|u| https(&u));
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

impl Resource {
    /// Returns false for a missing public release. Other HTTP failures are delivered
    /// immediately, even if the server never finishes sending its error page.
    pub(crate) fn fetch(
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
            Self::Asset | Self::OptionalAsset => (30., 20. * 60.),
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
                            (Self::Feed | Self::OptionalAsset, 404) => return Ok(false),
                            (Self::Feed, 403 | 429) => return Err(Error::RateLimited),
                            (Self::Feed, 200) if length <= limit as i64 => {}
                            (Self::Feed, _) => return Err(Error::Network(-1011)),
                            (Self::Asset | Self::OptionalAsset, 200) if https => {
                                if length > limit as i64 {
                                    return Err(Error::Network(-1103));
                                }
                            }
                            (Self::Asset | Self::OptionalAsset, _) => return Err(Error::Download),
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
