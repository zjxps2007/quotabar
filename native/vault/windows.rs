use crate::files::{
    ensure_private_directory, invalid, open_owned_credential, read_bounded, reject_links,
    write_private,
};
use std::{
    fs, io,
    path::Path,
    ptr::{null, null_mut},
};
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
};
use zeroize::{Zeroize, Zeroizing};

const MAX_ENCRYPTED: usize = 1_100_000;

// DPAPI allocates its output with LocalAlloc, including plaintext on unprotect.
struct LocalBlob(CRYPT_INTEGER_BLOB);
impl Drop for LocalBlob {
    fn drop(&mut self) {
        if !self.0.pbData.is_null() {
            unsafe {
                std::slice::from_raw_parts_mut(self.0.pbData, self.0.cbData as usize).zeroize();
                LocalFree(self.0.pbData.cast());
            }
        }
    }
}

fn transform(bytes: &[u8], protect: bool) -> io::Result<Zeroizing<Vec<u8>>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into().map_err(io::Error::other)?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = LocalBlob(CRYPT_INTEGER_BLOB::default());
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.0,
            )
        }
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let n = output.0.cbData as usize;
    if n > MAX_ENCRYPTED {
        return Err(invalid("DPAPI output exceeds size limit"));
    }
    let data = if n == 0 {
        Vec::new()
    } else {
        if output.0.pbData.is_null() {
            return Err(invalid("Missing DPAPI data"));
        }
        unsafe { std::slice::from_raw_parts(output.0.pbData, n) }.to_vec()
    };
    Ok(Zeroizing::new(data))
}

pub(super) fn save(directory: &Path, id: Uuid, bytes: &[u8]) -> io::Result<()> {
    write_private(
        &directory.join(format!("{id}.bin")),
        &transform(bytes, true)?,
    )
}
pub(super) fn load(directory: &Path, id: Uuid) -> io::Result<Zeroizing<Vec<u8>>> {
    let path = directory.join(format!("{id}.bin"));
    fs::symlink_metadata(&path)?;
    ensure_private_directory(directory)?;
    reject_links(&path)?;
    transform(
        &read_bounded(open_owned_credential(&path)?, MAX_ENCRYPTED)?,
        false,
    )
}
pub(super) fn delete(directory: &Path, id: Uuid) -> io::Result<()> {
    let path = directory.join(format!("{id}.bin"));
    reject_links(&path)?;
    fs::remove_file(path)
}
