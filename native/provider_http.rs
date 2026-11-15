//! Native HTTP transports for the fixed provider endpoints.
use crate::{
    provider_query::{Error, Query},
    provider_requests::Request,
};
use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicBool, Ordering};
use zeroize::Zeroizing;

#[cfg(target_os = "macos")]
#[path = "provider_http/macos.rs"]
mod platform;

#[cfg(windows)]
#[path = "provider_http/windows.rs"]
pub(crate) mod platform;
#[cfg(windows)]
#[path = "provider_http/windows_proxy.rs"]
pub(crate) mod proxy;
#[cfg(test)]
pub(crate) mod tests;

pub struct Reply {
    pub status: u16,
    pub body: Zeroizing<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub enum TransportError {
    Network(i32),
    Cancelled,
    Timeout,
    LargeResponse,
}

#[derive(Clone, Debug)]
pub enum Failure {
    Query(Error),
    Transport(TransportError),
}

pub fn run(
    query: Query,
    cancel: &AtomicBool,
    fixed_time: Option<DateTime<Utc>>,
) -> Result<Vec<u8>, Failure> {
    drive(
        query,
        cancel,
        fixed_time,
        |request, oauth, optional, cancel| platform::send(&request, oauth, optional, cancel),
    )
}

pub fn drive(
    mut query: Query,
    cancel: &AtomicBool,
    fixed_time: Option<DateTime<Utc>>,
    mut send: impl FnMut(Request, bool, bool, &AtomicBool) -> Result<Reply, TransportError>,
) -> Result<Vec<u8>, Failure> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(Failure::Transport(TransportError::Cancelled));
        }
        let request = query.request();
        let reply = match send(request, query.refreshing(), query.optional(), cancel) {
            Ok(reply) => reply,
            Err(_) if cancel.load(Ordering::Acquire) => {
                return Err(Failure::Transport(TransportError::Cancelled));
            }
            Err(_) if query.skip_discovery() => continue,
            Err(error) => return Err(Failure::Transport(error)),
        };
        if let Some(bytes) = query
            .accept(
                reply.status,
                &reply.body,
                fixed_time.unwrap_or_else(Utc::now),
            )
            .map_err(Failure::Query)?
        {
            return Ok(bytes);
        }
    }
}
