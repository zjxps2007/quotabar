//! Windows release transport. The caller supplies the sink so even a portable
//! package is streamed straight to its newly created file.

use super::{Error, Resource, Transfer, check_cancel};

use std::{
    io,
    time::{Duration, Instant},
};
use url::Url;

const IDLE: Duration = Duration::from_secs(30);
pub(crate) const NETWORK: i32 = 0x80004005u32 as i32;
pub const TIMEOUT: i32 = 0x800705b4u32 as i32;

enum Head {
    Body,
    NoRelease,
    Redirect(String),
}

/// The ordinary Windows client and explicitly supplied HTTP handlers use the
/// same status rules, byte limits, progress reporting and redirect deadlines.
pub struct Response<'a> {
    pub(crate) transfer: Transfer<'a>,
    head: Option<Head>,
    end: Instant,
    pub(crate) deadline: Instant,
    received: u64,
    last_percent: Option<i32>,
}

impl Response<'_> {
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    pub(crate) fn running(&self) -> Result<(), Error> {
        check_cancel(self.transfer.cancel)?;
        if self.remaining().is_zero() {
            return Err(Error::Network(TIMEOUT));
        }
        Ok(())
    }

    pub fn headers(
        &mut self,
        status: u16,
        length: Option<u64>,
        location: Option<&str>,
    ) -> Result<bool, Error> {
        self.running()?;
        let head = match (self.transfer.resource, status) {
            (Resource::Feed | Resource::OptionalAsset, 404) => Head::NoRelease,
            (Resource::Feed, 403 | 429) => return Err(Error::RateLimited),
            (Resource::Asset | Resource::OptionalAsset, 301 | 302 | 307 | 308) => Head::Redirect(
                location
                    .ok_or_else(|| io::Error::other("Update redirect missing"))?
                    .into(),
            ),
            (_, 200) if length.is_none_or(|len| len <= self.transfer.limit) => Head::Body,
            (Resource::Feed, _) => return Err(Error::Network(NETWORK)),
            (Resource::Asset | Resource::OptionalAsset, _) => return Err(Error::Download),
        };
        let read = matches!(head, Head::Body);
        self.head = Some(head);
        self.deadline = self.end.min(Instant::now() + IDLE);
        Ok(read)
    }

    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.running()?;
        if !matches!(self.head, Some(Head::Body)) {
            return Err(Error::Network(NETWORK));
        }
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() as u64 > self.transfer.limit - self.received {
            return Err(match self.transfer.resource {
                Resource::Feed => Error::Network(NETWORK),
                Resource::Asset | Resource::OptionalAsset => {
                    io::Error::other("Update exceeds declared size").into()
                }
            });
        }
        self.transfer.output.write_all(bytes)?;
        self.received += bytes.len() as u64;
        let percent = (self.received * 100 / self.transfer.limit.max(1)) as i32;
        if self.last_percent != Some(percent) {
            (self.transfer.progress)(percent);
            self.last_percent = Some(percent);
        }
        self.deadline = self.end.min(Instant::now() + IDLE);
        Ok(())
    }
}

pub fn fetch_with(
    transfer: Transfer<'_>,
    mut exchange: impl FnMut(&Url, &mut Response<'_>) -> Result<(), Error>,
) -> Result<bool, Error> {
    let mut url = Url::parse(transfer.url).map_err(|_| io::Error::other("Invalid release URL"))?;
    let end = Instant::now()
        + match transfer.resource {
            Resource::Feed => IDLE,
            Resource::Asset | Resource::OptionalAsset => Duration::from_secs(20 * 60),
        };
    let mut response = Response {
        transfer,
        head: None,
        end,
        deadline: end,
        received: 0,
        last_percent: None,
    };
    for _ in 0..=5 {
        if url.scheme() != "https" {
            return Err(io::Error::other("Insecure update URL").into());
        }
        response.deadline = end.min(Instant::now() + IDLE);
        response.running()?;
        exchange(&url, &mut response)?;
        response.running()?;
        match response.head.take() {
            Some(Head::NoRelease) => return Ok(false),
            Some(Head::Body) => return Ok(true),
            Some(Head::Redirect(next)) => {
                url = url
                    .join(&next)
                    .map_err(|_| io::Error::other("Invalid update redirect"))?;
            }
            None => return Err(Error::Network(NETWORK)),
        }
    }
    Err(io::Error::other("Too many update redirects").into())
}
