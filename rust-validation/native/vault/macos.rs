use super::MAX_SIZE;
use crate::files::invalid;
use std::{
    ffi::{CStr, CString},
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::Path,
};
use uuid::Uuid;
use zeroize::Zeroizing;

fn directory(path: &Path, create: bool) -> io::Result<File> {
    if create {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    let fd = File::options()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if fd.metadata()?.uid() != unsafe { libc::geteuid() } {
        return Err(invalid("Unexpected vault owner"));
    }
    fd.set_permissions(fs::Permissions::from_mode(0o700))?;
    Ok(fd)
}

fn name(id: Uuid) -> CString {
    CString::new(format!(
        "{}.json",
        id.hyphenated().to_string().to_ascii_uppercase()
    ))
    .expect("UUID filename")
}
fn open_at(folder: RawFd, name: &CStr, flags: i32) -> io::Result<File> {
    let raw = unsafe {
        libc::openat(
            folder,
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_int,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(raw) })
}

pub(super) fn load(path: &Path, id: Uuid) -> io::Result<Zeroizing<Vec<u8>>> {
    let folder = directory(path, false)?;
    let file = open_at(
        folder.as_raw_fd(),
        &name(id),
        libc::O_RDONLY | libc::O_NONBLOCK,
    )?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.len() > MAX_SIZE as u64
    {
        return Err(invalid("Unexpected credential file type, size or owner"));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    let mut data = Zeroizing::new(Vec::with_capacity(metadata.len() as usize));
    file.take(MAX_SIZE as u64 + 1).read_to_end(&mut data)?;
    if data.len() > MAX_SIZE {
        return Err(invalid("Credential exceeds size limit"));
    }
    Ok(data)
}
pub(super) fn save(path: &Path, id: Uuid, bytes: &[u8]) -> io::Result<()> {
    let folder = directory(path, true)?;
    let fd = folder.as_raw_fd();
    let temp = CString::new(format!(".{}.tmp", Uuid::new_v4())).expect("UUID filename");
    let mut file = open_at(fd, &temp, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?;
    let result = (|| {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        if unsafe { libc::renameat(fd, temp.as_ptr(), fd, name(id).as_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();
    if result.is_err() {
        unsafe {
            libc::unlinkat(fd, temp.as_ptr(), 0);
        }
    }
    result
}
pub(super) fn delete(path: &Path, id: Uuid) -> io::Result<()> {
    let folder = directory(path, false)?;
    if unsafe { libc::unlinkat(folder.as_raw_fd(), name(id).as_ptr(), 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
