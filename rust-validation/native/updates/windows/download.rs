use crate::provider_http::{
    TransportError,
    platform::{Apartment, client, wait},
    proxy::{Environment, Proxy, Route},
};
use crate::updates::download::{
    Error, Resource, Transfer,
    windows::{NETWORK, Response, TIMEOUT, fetch_with},
};
use curl::{
    easy::{Easy2, Handler, List, WriteError},
    multi::Multi,
};
use std::time::Duration;
use std::{io::Write, sync::atomic::AtomicBool};
use url::Url;
#[cfg(test)]
mod tests;
use windows::{
    Foundation::Uri,
    Storage::Streams::{Buffer, DataReader, InputStreamOptions},
    Web::Http::{HttpCompletionOption, HttpMethod, HttpRequestMessage},
    core::HSTRING,
};

impl From<windows::core::Error> for Error {
    fn from(error: windows::core::Error) -> Self {
        Self::Network(error.code().0)
    }
}
impl From<curl::Error> for Error {
    fn from(error: curl::Error) -> Self {
        TransportError::from(error).into()
    }
}

fn get(url: &Url, response: &mut Response<'_>) -> Result<(), Error> {
    match Environment::process().route(url.as_str())? {
        Route::Explicit(proxy) => proxied(url, response, proxy),
        Route::System => winrt(url, response, true),
        Route::Direct => winrt(url, response, false),
    }
}

fn winrt(url: &Url, response: &mut Response<'_>, system_proxy: bool) -> Result<(), Error> {
    let _apartment = Apartment::mta()?;
    let request = HttpRequestMessage::Create(
        &HttpMethod::Get()?,
        &Uri::CreateUri(&HSTRING::from(url.as_str()))?,
    )?;
    let headers = request.Headers()?;
    headers.Append(&"User-Agent".into(), &response.transfer.agent.into())?;
    if response.transfer.resource == Resource::Feed {
        headers.Append(&"Accept".into(), &"application/vnd.github+json".into())?;
    }
    let cancel = response.transfer.cancel;
    let reply = wait(
        client(system_proxy)?
            .SendRequestWithOptionAsync(&request, HttpCompletionOption::ResponseHeadersRead)?,
        cancel,
        Some(response.deadline),
    )?;
    let content = reply.Content()?;
    let length = content
        .Headers()?
        .ContentLength()
        .ok()
        .and_then(|v| v.Value().ok());
    let location = reply
        .Headers()?
        .Lookup(&"Location".into())
        .ok()
        .map(|s| s.to_string());
    if !response.headers(reply.StatusCode()?.0 as u16, length, location.as_deref())? {
        return Ok(());
    }
    let stream = wait(
        content.ReadAsInputStreamAsync()?,
        cancel,
        Some(response.deadline),
    )?;
    let mut bytes = vec![0; 65536];
    let buffer = Buffer::Create(bytes.len() as u32)?;
    loop {
        response.running()?;
        let read = wait(
            stream.ReadAsync(&buffer, bytes.len() as u32, InputStreamOptions::Partial)?,
            cancel,
            Some(response.deadline),
        )?;
        let count = read.Length()? as usize;
        if count == 0 {
            return Ok(());
        }
        DataReader::FromBuffer(&read)?.ReadBytes(&mut bytes[..count])?;
        response.write(&bytes[..count])?;
    }
}

struct ProxyReply<'a, 'b> {
    receiver: &'a mut Response<'b>,
    status: u16,
    length: Option<u64>,
    location: Option<String>,
    complete: bool,
    error: Option<Error>,
}

impl Handler for ProxyReply<'_, '_> {
    fn header(&mut self, line: &[u8]) -> bool {
        if line.starts_with(b"HTTP/") {
            self.status = std::str::from_utf8(line)
                .ok()
                .and_then(|s| s.split_ascii_whitespace().nth(1)?.parse().ok())
                .unwrap_or_default();
            self.length = None;
            self.location = None;
        } else if line == b"\r\n" && self.status >= 200 {
            // Trailers after a chunked body aren't a second response head.
            let status = std::mem::take(&mut self.status);
            match self
                .receiver
                .headers(status, self.length, self.location.as_deref())
            {
                Ok(true) => {}
                Ok(false) => {
                    self.complete = true;
                    return false;
                }
                Err(error) => {
                    self.error = Some(error);
                    return false;
                }
            }
        } else if let Some(colon) = line.iter().position(|b| *b == b':') {
            let name = &line[..colon];
            let value = line[colon + 1..].trim_ascii();
            if name.eq_ignore_ascii_case(b"content-length") {
                self.length = std::str::from_utf8(value).ok().and_then(|s| s.parse().ok());
            } else if name.eq_ignore_ascii_case(b"location") {
                self.location = std::str::from_utf8(value).ok().map(str::to_owned);
            }
        }
        true
    }

    fn write(&mut self, bytes: &[u8]) -> Result<usize, WriteError> {
        match self.receiver.write(bytes) {
            Ok(()) => Ok(bytes.len()),
            Err(error) => {
                self.error = Some(error);
                Ok(0)
            }
        }
    }
}

fn proxied(url: &Url, response: &mut Response<'_>, proxy: &Proxy) -> Result<(), Error> {
    let duration = response.remaining();
    let mut headers = List::new();
    headers.append(if response.transfer.resource == Resource::Feed {
        "Accept: application/vnd.github+json"
    } else {
        "Accept:"
    })?;
    let agent = response.transfer.agent;
    let mut easy = Easy2::new(ProxyReply {
        receiver: response,
        status: 0,
        length: None,
        location: None,
        complete: false,
        error: None,
    });
    easy.url(url.as_str())?;
    proxy.configure(&mut easy, duration)?;
    easy.useragent(agent)?;
    easy.http_headers(headers)?;
    let multi = Multi::new();
    let mut handle = multi.add2(easy).map_err(|_| Error::Network(NETWORK))?;
    loop {
        let active = multi.perform().map_err(|_| Error::Network(NETWORK))?;
        let reply = handle.get_mut();
        if let Some(error) = reply.error.take() {
            return Err(error);
        }
        if reply.complete {
            return Ok(());
        }
        reply.receiver.running()?;
        if active == 0 {
            break;
        }
        multi
            .poll(
                &mut [],
                Duration::from_millis(50).min(reply.receiver.remaining()),
            )
            .map_err(|_| Error::Network(NETWORK))?;
    }
    let mut result = None;
    multi.messages(|message| result = message.result_for2(&handle));
    result.ok_or(Error::Network(NETWORK))??;
    Ok(())
}

impl From<TransportError> for Error {
    fn from(error: TransportError) -> Self {
        match error {
            TransportError::Cancelled => Self::Cancelled,
            TransportError::Timeout => Self::Network(TIMEOUT),
            TransportError::Network(code) => Self::Network(code),
            TransportError::LargeResponse => Self::Network(NETWORK),
        }
    }
}

impl Resource {
    pub(crate) fn fetch(
        self,
        url: &str,
        agent: &str,
        limit: u64,
        cancel: &AtomicBool,
        output: &mut dyn Write,
        mut progress: impl FnMut(i32),
    ) -> Result<bool, Error> {
        fetch_with(
            Transfer {
                resource: self,
                url,
                agent,
                limit,
                cancel,
                output,
                progress: &mut progress,
            },
            get,
        )
    }
}
