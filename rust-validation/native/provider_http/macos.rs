use super::{Reply, Request, TransportError};
use block2::{DynBlock, RcBlock};
use objc2::{
    ClassType, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::ProtocolObject,
};
use objc2_foundation::{
    NSData, NSError, NSHTTPURLResponse, NSMutableURLRequest, NSObject, NSObjectProtocol, NSString,
    NSURL, NSURLRequest, NSURLRequestCachePolicy, NSURLResponse, NSURLSession,
    NSURLSessionConfiguration, NSURLSessionDelegate, NSURLSessionTask, NSURLSessionTaskDelegate,
};
use std::{
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};
use zeroize::Zeroizing;

define_class!(
    #[unsafe(super=NSObject)]
    #[name = "QuotaBarOAuthRedirectDelegate"]
    struct NoRedirects;
    unsafe impl NSObjectProtocol for NoRedirects {}
    unsafe impl NSURLSessionDelegate for NoRedirects {}
    unsafe impl NSURLSessionTaskDelegate for NoRedirects {
        #[unsafe(method(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:))]
        unsafe fn redirect(
            &self,
            _: &NSURLSession,
            _: &NSURLSessionTask,
            _: &NSHTTPURLResponse,
            _: &NSURLRequest,
            completion: &DynBlock<dyn Fn(*mut NSURLRequest)>,
        ) {
            completion.call((ptr::null_mut(),));
        }
    }
);

fn session(oauth: bool) -> &'static NSURLSession {
    static QUOTA: OnceLock<Retained<NSURLSession>> = OnceLock::new();
    static OAUTH: OnceLock<Retained<NSURLSession>> = OnceLock::new();
    let storage = if oauth { &OAUTH } else { &QUOTA };
    storage.get_or_init(|| {
        let config = NSURLSessionConfiguration::ephemeralSessionConfiguration();
        config.setURLCache(None);
        if oauth {
            config.setHTTPCookieStorage(None);
            let delegate: Retained<NoRedirects> = unsafe { msg_send![NoRedirects::class(), new] };
            unsafe {
                NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
                    &config,
                    Some(ProtocolObject::from_ref(&*delegate)),
                    None,
                )
            }
        } else {
            config.setRequestCachePolicy(NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData);
            NSURLSession::sessionWithConfiguration(&config)
        }
    })
}

#[cfg(test)]
#[test]
fn provider_sessions_do_not_cache_another_accounts_response() {
    let quota = session(false).configuration();
    assert!(quota.URLCache().is_none());
    assert_eq!(
        quota.requestCachePolicy(),
        NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData
    );
    let oauth = session(true).configuration();
    assert!(oauth.URLCache().is_none());
    assert!(oauth.HTTPCookieStorage().is_none());
}

pub(super) fn send(
    description: &Request,
    oauth: bool,
    _: bool,
    cancel: &AtomicBool,
) -> Result<Reply, TransportError> {
    autoreleasepool(|_| {
        let url = NSURL::URLWithString(&NSString::from_str(&description.url))
            .ok_or(TransportError::Network(-1000))?;
        let policy = if description.headers.contains_key("Cache-Control") {
            NSURLRequestCachePolicy::ReloadIgnoringLocalCacheData
        } else {
            NSURLRequestCachePolicy::UseProtocolCachePolicy
        };
        let request = NSMutableURLRequest::requestWithURL_cachePolicy_timeoutInterval(
            &url,
            policy,
            description.timeout,
        );
        request.setHTTPMethod(&NSString::from_str(description.method));
        for (name, value) in &description.headers {
            request.setValue_forHTTPHeaderField(
                Some(&NSString::from_str(value)),
                &NSString::from_str(name),
            );
        }
        if let Some(body) = &description.body {
            let bytes = Zeroizing::new(
                serde_json::to_vec(body).map_err(|_| TransportError::Network(-1017))?,
            );
            request.setHTTPBody(Some(&NSData::with_bytes(&bytes)));
        }
        let (tx, rx) = mpsc::channel();
        let completion = RcBlock::new(
            move |data: *mut NSData, response: *mut NSURLResponse, error: *mut NSError| {
                // Foundation owns these pointers only for the duration of its callback.
                let reply = if let Some(error) = unsafe { error.as_ref() } {
                    Err(TransportError::Network(error.code() as i32))
                } else {
                    let status = unsafe { response.as_ref() }
                        .and_then(|r| r.downcast_ref::<NSHTTPURLResponse>())
                        .map_or(0, |r| r.statusCode() as u16);
                    let bytes = unsafe { data.as_ref() }
                        .map(NSData::to_vec)
                        .unwrap_or_default();
                    Ok(Reply {
                        status,
                        body: Zeroizing::new(bytes),
                    })
                };
                let _ = tx.send(reply);
            },
        );
        let task =
            unsafe { session(oauth).dataTaskWithRequest_completionHandler(&request, &completion) };
        task.resume();
        loop {
            if cancel.load(Ordering::Acquire) {
                task.cancel();
                return Err(TransportError::Cancelled);
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(reply) => return reply,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(TransportError::Network(-1005));
                }
            }
        }
    })
}
