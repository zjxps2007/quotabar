use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::Path;
use uuid::Uuid;

#[cfg(windows)]
pub(crate) mod windows;

pub fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn read_bounded(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(invalid("Input exceeds the size limit"));
    }
    Ok(bytes)
}

pub fn read_file(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    read_bounded(File::open(path)?, limit)
}

#[cfg(windows)]
pub(crate) use windows::wide;
#[cfg(windows)]
pub(crate) use windows::{create_private, create_private_directory_new, open_owned_credential};
#[cfg(windows)]
pub use windows::{ensure_private_directory, reject_links};

#[cfg(unix)]
pub fn ensure_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(unix)]
pub(crate) fn create_private_directory_new(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Missing staging parent"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(unix)]
pub(crate) fn create_private(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    File::options()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let folder = path
        .parent()
        .ok_or_else(|| invalid("Missing parent directory"))?;
    ensure_private_directory(folder)?;
    #[cfg(windows)]
    windows::reject_links(path)?;
    let temporary = folder.join(format!(".{}.tmp", Uuid::new_v4()));
    let mut file = create_private(&temporary)?;
    let result = (|| {
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
