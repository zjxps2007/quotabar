//! Windows HttpClient environment proxy rules and explicit proxy transport.
use super::{Reply, Request, TransportError};
use curl::{
    easy::{Auth, Easy2, Handler, HttpVersion, List, WriteError},
    multi::Multi,
};
use percent_encoding::percent_decode_str;
use std::{
    ffi::c_long,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use url::Url;
use zeroize::Zeroizing;

#[cfg(test)]
#[path = "windows_proxy_tests.rs"]
mod tests;

const NETWORK: TransportError = TransportError::Network(0x80004005u32 as i32);

pub(super) struct Environment {
    http: Option<Proxy>,
    https: Option<Proxy>,
    bypass: Vec<String>,
}

pub(super) enum Route<'a> {
    System,
    Direct,
    Explicit(&'a Proxy),
}

impl Environment {
    pub(super) fn from_process() -> Self {
        Self::read(|name| std::env::var(name).ok())
    }

    fn read(get: impl Fn(&str) -> Option<String>) -> Self {
        let get_proxy = |name| get(name).and_then(|s| Proxy::parse(&Zeroizing::new(s)));
        let http = get("GATEWAY_INTERFACE")
            .is_none()
            .then(|| get_proxy("HTTP_PROXY"))
            .flatten();
        let https = get_proxy("HTTPS_PROXY");
        let all = if http.is_none() || https.is_none() {
            get_proxy("ALL_PROXY")
        } else {
            None
        };
        let bypass = get("NO_PROXY")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        Self {
            http: http.or_else(|| all.clone()),
            https: https.or(all),
            bypass,
        }
    }

    pub(super) fn route(&self, target: &str) -> Result<Route<'_>, TransportError> {
        if self.http.is_none() && self.https.is_none() {
            return Ok(Route::System);
        }
        let url = Url::parse(target).map_err(|_| NETWORK)?;
        let host = url.host_str().ok_or(NETWORK)?;
        let proxy = match url.scheme() {
            "http" => &self.http,
            "https" => &self.https,
            _ => return Err(NETWORK),
        };
        let bypassed = self.bypass.iter().any(|pattern| {
            if let Some(domain) = pattern.strip_prefix('.') {
                host == domain || host.ends_with(pattern)
            } else {
                host == pattern
            }
        });
        Ok(match proxy {
            Some(proxy) if !bypassed => Route::Explicit(proxy),
            _ => Route::Direct,
        })
    }
}

#[derive(Clone)]
pub(super) struct Proxy {
    endpoint: String,
    socks: bool,
    credentials: Option<Credentials>,
}

#[derive(Clone)]
enum Credentials {
    WindowsLogon,
    Password {
        user: Zeroizing<String>,
        password: Zeroizing<String>,
    },
}

impl Proxy {
    fn parse(value: &str) -> Option<Self> {
        if value.is_empty() {
            return None;
        }
        let (scheme, authority) = value
            .split_once("://")
            .and_then(|(name, rest)| {
                ["http", "https", "socks4", "socks4a", "socks5"]
                    .into_iter()
                    .find(|s| name.eq_ignore_ascii_case(s))
                    .map(|s| (s, rest))
            })
            .unwrap_or(("http", value));
        let (userinfo, authority) = authority
            .rsplit_once('@')
            .map_or((None, authority), |(u, h)| (Some(u), h));
        let default_port = if scheme == "https" { 443 } else { 80 };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, tail)) if !tail.contains(']') => {
                let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
                (host, tail[..digits].parse::<u16>().ok()?)
            }
            _ => (authority, default_port),
        };
        // UriBuilder adds the port after the supplied host, which may already
        // include a trailing slash or path. Use only its effective authority.
        // Normalize international and IPv6 hostnames for every proxy protocol.
        let uri_scheme = if scheme == "https" { "https" } else { "http" };
        let url = Url::parse(&format!("{uri_scheme}://{host}:{port}/")).ok()?;
        let host = url.host_str()?;
        let port = url.port().unwrap_or(default_port);
        let socks = scheme.starts_with("socks");
        let scheme = if scheme == "socks5" {
            "socks5h"
        } else {
            scheme
        };
        let credentials = userinfo.filter(|s| !s.is_empty()).map(|raw| {
            let decoded = Zeroizing::new(percent_decode_str(raw).decode_utf8_lossy().into_owned());
            if decoded.as_str() == ":" {
                return Credentials::WindowsLogon;
            }
            let (user, password) = decoded.split_once(':').unwrap_or((&decoded, ""));
            let user = if socks {
                user.split_once('\\').map_or(user, |(_, user)| user)
            } else {
                user
            };
            Credentials::Password {
                user: Zeroizing::new(user.into()),
                password: Zeroizing::new(password.into()),
            }
        });
        Some(Self {
            endpoint: format!("{scheme}://{host}:{port}"),
            socks,
            credentials,
        })
    }

    pub(super) fn send(
        &self,
        request: &Request,
        discovery: bool,
        cancel: &AtomicBool,
    ) -> Result<Reply, TransportError> {
        if cancel.load(Ordering::Acquire) {
            return Err(TransportError::Cancelled);
        }
        let duration =
            Duration::try_from_secs_f64(request.timeout).map_err(|_| TransportError::Timeout)?;
        let deadline = Instant::now()
            .checked_add(duration)
            .ok_or(TransportError::Timeout)?;
        let mut easy = Easy2::new(Response {
            forward_proxy_auth: !self.socks
                && self.credentials.is_some()
                && request.url.starts_with("http://"),
            ..Response::default()
        });
        easy.url(&request.url)?;
        easy.proxy(&self.endpoint)?;
        easy.noproxy("")?;
        easy.follow_location(false)?;
        easy.http_version(HttpVersion::V11)?;
        easy.connect_timeout(duration)?;
        // The safe wrapper does not expose these two long-valued curl options.
        // Suppress CONNECT response headers so the origin header deadline
        // still applies after a proxy has accepted the tunnel.
        const SUPPRESS_CONNECT_HEADERS: curl_sys::CURLoption = 265;
        const SOCKS5_AUTH: curl_sys::CURLoption = 267;
        set_long(&mut easy, SUPPRESS_CONNECT_HEADERS, 1)?;
        if self.socks {
            // Windows HttpClient offers SOCKS no-auth and username/password.
            set_long(&mut easy, SOCKS5_AUTH, curl_sys::CURLAUTH_BASIC as c_long)?;
        }
        if let Some(credentials) = &self.credentials {
            let mut auth = Auth::new();
            auth.ntlm(true).gssnegotiate(true);
            let (user, pass) = match credentials {
                Credentials::WindowsLogon => ("", ""),
                Credentials::Password { user, password } => {
                    auth.basic(true).digest(true);
                    (user.as_str(), password.as_str())
                }
            };
            easy.proxy_username(user)?;
            easy.proxy_password(pass)?;
            if !self.socks {
                easy.proxy_auth(&auth)?;
            }
        }
        let mut headers = List::new();
        for (name, value) in &request.headers {
            if value.contains(['\r', '\n', '\0']) {
                return Err(NETWORK);
            }
            headers.append(&Zeroizing::new(format!("{name}: {value}")))?;
        }
        if !request.headers.contains_key("Accept") {
            headers.append("Accept:")?;
        }
        easy.http_headers(headers)?;
        let mut proxy_headers = List::new();
        proxy_headers.append("Proxy-Connection:")?;
        easy.proxy_headers(proxy_headers)?;
        if let Some(body) = &request.body {
            let bytes = Zeroizing::new(serde_json::to_vec(body).map_err(|_| NETWORK)?);
            easy.post_fields_copy(&bytes)?;
        }
        let multi = Multi::new();
        let mut handle = multi.add2(easy).map_err(|_| NETWORK)?;
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(TransportError::Cancelled);
            }
            let active = multi.perform().map_err(|_| NETWORK)?;
            let response = handle.get_mut();
            if response.oversized {
                return Err(TransportError::LargeResponse);
            }
            if response.finished_at_headers {
                return Ok(response.take_reply());
            }
            if active == 0 {
                break;
            }
            let mut wait = Duration::from_millis(50);
            if discovery || !response.headers_complete {
                wait = wait.min(deadline.saturating_duration_since(Instant::now()));
                if wait.is_zero() {
                    return Err(TransportError::Timeout);
                }
            }
            multi.poll(&mut [], wait).map_err(|_| NETWORK)?;
        }
        let mut result = None;
        multi.messages(|message| result = message.result_for2(&handle));
        result.ok_or(NETWORK)??;
        let response = handle.get_mut();
        if !response.headers_complete && !(response.forward_proxy_auth && response.status == 407) {
            return Err(NETWORK);
        }
        Ok(response.take_reply())
    }
}

fn set_long(
    handle: &mut Easy2<Response>,
    option: curl_sys::CURLoption,
    value: c_long,
) -> Result<(), TransportError> {
    // The caller owns this easy handle and both options take a C long.
    let code = unsafe { curl_sys::curl_easy_setopt(handle.raw(), option, value) };
    if code == curl_sys::CURLE_OK {
        Ok(())
    } else {
        Err(NETWORK)
    }
}

impl From<curl::Error> for TransportError {
    fn from(error: curl::Error) -> Self {
        if error.is_operation_timedout() {
            Self::Timeout
        } else {
            NETWORK
        }
    }
}

#[derive(Default)]
struct Response {
    forward_proxy_auth: bool,
    status: u16,
    headers_complete: bool,
    finished_at_headers: bool,
    oversized: bool,
    body: Zeroizing<Vec<u8>>,
}

impl Response {
    fn take_reply(&mut self) -> Reply {
        Reply {
            status: self.status,
            body: std::mem::take(&mut self.body),
        }
    }
}

impl Handler for Response {
    fn header(&mut self, line: &[u8]) -> bool {
        if line.starts_with(b"HTTP/") {
            self.status = line
                .split(|b| *b == b' ')
                .nth(1)
                .and_then(|s| std::str::from_utf8(s.trim_ascii()).ok()?.parse().ok())
                .unwrap_or(0);
            self.headers_complete = false;
        } else if line == b"\r\n" && self.status >= 200 {
            let negotiating = self.status == 407 && self.forward_proxy_auth;
            self.headers_complete = !negotiating;
            // A forward proxy's authentication challenge may be followed by a
            // retry on the same transfer. libcurl consumes the challenge body.
            if self.status >= 300 && !negotiating {
                self.finished_at_headers = true;
                return false;
            }
        }
        true
    }

    fn write(&mut self, bytes: &[u8]) -> Result<usize, WriteError> {
        if (200..300).contains(&self.status) {
            if bytes.len() > 2 * crate::vault::MAX_SIZE - self.body.len() {
                self.oversized = true;
                return Ok(0);
            }
            self.body.extend_from_slice(bytes);
        }
        Ok(bytes.len())
    }
}
