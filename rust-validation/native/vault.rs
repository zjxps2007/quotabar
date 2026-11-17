//! Storage used by existing QuotaBar installations. Callers supply the account's UUID,
//! never a filename derived from credentials or an import path.
use std::{io, path::Path};
use uuid::Uuid;
use zeroize::Zeroizing;

#[cfg(windows)]
#[path = "vault/windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "vault/macos.rs"]
mod platform;

pub const MAX_SIZE: usize = 1_048_576;

pub fn load(directory: &Path, id: Uuid) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
    match platform::load(directory, id) {
        Ok(data) => Ok(Some(data)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(other) => Err(other),
    }
}
pub fn save(directory: &Path, id: Uuid, data: &[u8]) -> io::Result<()> {
    if data.len() > MAX_SIZE {
        return Err(crate::files::invalid("Credential exceeds size limit"));
    }
    platform::save(directory, id, data)
}
pub fn delete(directory: &Path, id: Uuid) -> io::Result<()> {
    match platform::delete(directory, id) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}
