use super::{Reply, Request, TransportError};
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows::{
    Foundation::Uri,
    Storage::Streams::{Buffer, DataReader, DataWriter, InputStreamOptions},
    Web::Http::{
        Filters::{
            HttpBaseProtocolFilter, HttpCacheReadBehavior, HttpCacheWriteBehavior,
            HttpCookieUsageBehavior,
        },
        HttpBufferContent, HttpClient, HttpCompletionOption, HttpMethod, HttpRequestMessage,
        HttpVersion,
    },
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::{HSTRING, RuntimeType},
};
use windows_future::{
    AsyncOperationWithProgressCompletedHandler, AsyncStatus, IAsyncOperationWithProgress,
};
use zeroize::Zeroizing;

impl From<windows::core::Error> for TransportError {
    fn from(value: windows::core::Error) -> Self {
        Self::Network(value.code().0)
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

fn client(use_system_proxy: bool) -> windows::core::Result<&'static HttpClient> {
    static SYSTEM: OnceLock<HttpClient> = OnceLock::new();
    static DIRECT: OnceLock<HttpClient> = OnceLock::new();
    let shared = if use_system_proxy { &SYSTEM } else { &DIRECT };
    if let Some(client) = shared.get() {
        return Ok(client);
    }
    let filter = HttpBaseProtocolFilter::new()?;
    filter.SetUseProxy(use_system_proxy)?;
    filter.SetAllowAutoRedirect(false)?;
    filter.SetAllowUI(false)?;
    filter.SetAutomaticDecompression(false)?;
    filter.SetMaxVersion(HttpVersion::Http11)?;
    filter.SetCookieUsageBehavior(HttpCookieUsageBehavior::NoCookies)?;
    let cache = filter.CacheControl()?;
    cache.SetReadBehavior(HttpCacheReadBehavior::NoCache)?;
    cache.SetWriteBehavior(HttpCacheWriteBehavior::NoCache)?;
    let _ = shared.set(HttpClient::Create(&filter)?);
    Ok(shared.get().expect("client was initialized"))
}

fn wait<T: RuntimeType, P: RuntimeType>(
    op: IAsyncOperationWithProgress<T, P>,
    cancel: &AtomicBool,
    deadline: Option<Instant>,
) -> Result<T, TransportError> {
    if op.Status()? == AsyncStatus::Started {
        let (tx, rx) = mpsc::channel();
        op.SetCompleted(&AsyncOperationWithProgressCompletedHandler::new(
            move |_, _| {
                let _ = tx.send(());
                Ok(())
            },
        ))?;
        loop {
            if cancel.load(Ordering::Acquire) {
                let _ = op.Cancel();
                return Err(TransportError::Cancelled);
            }
            let mut poll = Duration::from_millis(50);
            if let Some(end) = deadline {
                poll = poll.min(end.saturating_duration_since(Instant::now()));
                if poll.is_zero() {
                    let _ = op.Cancel();
                    return Err(TransportError::Timeout);
                }
            }
            match rx.recv_timeout(poll) {
                Ok(()) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(TransportError::Network(-1));
                }
            }
        }
    }
    Ok(op.GetResults()?)
}

pub(super) fn send(
    description: &Request,
    _: bool,
    discovery: bool,
    cancel: &AtomicBool,
) -> Result<Reply, TransportError> {
    use super::proxy::{Environment, Route};
    static PROXIES: OnceLock<Environment> = OnceLock::new();
    let proxies = PROXIES.get_or_init(Environment::from_process);
    match proxies.route(&description.url)? {
        Route::Explicit(proxy) => proxy.send(description, discovery, cancel),
        Route::System => send_winrt(description, discovery, cancel, true),
        Route::Direct => send_winrt(description, discovery, cancel, false),
    }
}

pub(super) fn send_winrt(
    description: &Request,
    discovery: bool,
    cancel: &AtomicBool,
    use_system_proxy: bool,
) -> Result<Reply, TransportError> {
    // The background caller owns this apartment until every local WinRT object is dropped.
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
    }
    let _apartment = Apartment;
    let client = client(use_system_proxy)?;
    let method = HttpMethod::Create(&HSTRING::from(description.method))?;
    let request =
        HttpRequestMessage::Create(&method, &Uri::CreateUri(&HSTRING::from(&description.url))?)?;
    if let Some(body) = &description.body {
        let bytes =
            Zeroizing::new(serde_json::to_vec(body).map_err(|_| TransportError::Network(-1))?);
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bytes)?;
        let content = HttpBufferContent::CreateFromBuffer(&writer.DetachBuffer()?)?;
        content
            .Headers()?
            .Append(&"Content-Type".into(), &"application/json".into())?;
        request.SetContent(&content)?;
    }
    let headers = request.Headers()?;
    for (name, value) in &description.headers {
        if *name != "Content-Type" {
            headers.Append(&HSTRING::from(*name), &HSTRING::from(value))?;
        }
    }
    let deadline = Instant::now().checked_add(
        Duration::try_from_secs_f64(description.timeout).map_err(|_| TransportError::Timeout)?,
    );
    let response = wait(
        client.SendRequestWithOptionAsync(&request, HttpCompletionOption::ResponseHeadersRead)?,
        cancel,
        deadline,
    )?;
    let status = response.StatusCode()?.0 as u16;
    let mut body = Zeroizing::new(Vec::new());
    if (200..300).contains(&status) {
        // HttpClient's ordinary timeout ends at the headers. The five-second
        // discovery deadline and the caller's cancellation also cover the body.
        let deadline = if discovery { deadline } else { None };
        let stream = wait(
            response.Content()?.ReadAsInputStreamAsync()?,
            cancel,
            deadline,
        )?;
        let buffer = Buffer::Create(8192)?;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(TransportError::Cancelled);
            }
            let read = wait(
                stream.ReadAsync(&buffer, 8192, InputStreamOptions::Partial)?,
                cancel,
                deadline,
            )?;
            let count = read.Length()? as usize;
            if count == 0 {
                break;
            }
            let start = body.len();
            if start + count > 2 * crate::vault::MAX_SIZE {
                return Err(TransportError::LargeResponse);
            }
            body.resize(start + count, 0);
            DataReader::FromBuffer(&read)?.ReadBytes(&mut body[start..])?;
        }
    }
    Ok(Reply { status, body })
}
