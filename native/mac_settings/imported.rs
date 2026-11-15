use super::{AccountId, same_identity};
use crate::{
    credentials::{self, Platform, Provider},
    files, vault,
};
use serde::Deserialize;
use std::{fs, path::Path};
use zeroize::Zeroizing;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Import {
    id: AccountId,
    provider: Provider,
    source_path: String,
    identity: String,
}

#[derive(Debug)]
pub enum Error {
    Vault,
    Missing,
    Mismatch,
    Credential(credentials::Error),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    Stored,
    Updated,
    OtherAccount,
}

impl Import {
    /// The monitor saves a fresh CLI credential only after the fetch succeeds
    /// and the account's generation still matches. Selecting it never writes the vault.
    pub fn select(&self, directory: &Path) -> Result<(Zeroizing<Vec<u8>>, Selection), Error> {
        let stored = vault::load(directory, self.id.0)
            .map_err(|_| Error::Vault)?
            .ok_or(Error::Missing)?;
        let path = Path::new(&self.source_path);
        let fresh = fs::metadata(path)
            .ok()
            .filter(|info| info.is_file() && info.len() <= vault::MAX_SIZE as u64)
            .and_then(|_| files::read_file(path, vault::MAX_SIZE).ok())
            .map(Zeroizing::new);
        let mut selection = Selection::Stored;
        if let Some(bytes) = fresh
            && let Ok(parsed) = credentials::parse(&bytes, self.provider, Platform::Mac)
        {
            if same_identity(&parsed.identity, &self.identity) {
                let selection = if bytes == stored {
                    Selection::Stored
                } else {
                    Selection::Updated
                };
                return Ok((bytes, selection));
            }
            selection = Selection::OtherAccount;
        }
        let parsed =
            credentials::parse(&stored, self.provider, Platform::Mac).map_err(Error::Credential)?;
        if !same_identity(&parsed.identity, &self.identity) {
            return Err(Error::Mismatch);
        }
        Ok((stored, selection))
    }
}
