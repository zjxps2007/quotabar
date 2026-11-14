use super::invalid;
use std::ffi::c_void;
use std::fs::{self, File};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, ERROR_INSUFFICIENT_BUFFER, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorDacl, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(crate) fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut units: Vec<_> = path.as_os_str().encode_wide().collect();
    if units.contains(&0) {
        return Err(invalid("Invalid path"));
    }
    units.push(0);
    Ok(units)
}

pub fn reject_links(path: &Path) -> io::Result<()> {
    for ancestor in path.ancestors().filter(|p| !p.as_os_str().is_empty()) {
        match fs::symlink_metadata(ancestor) {
            Ok(info) if info.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
                return Err(invalid("Reparse point in private path"));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn succeeded(value: i32) -> io::Result<()> {
    if value == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn status(value: u32) -> io::Result<()> {
    if value == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(value as i32))
    }
}

struct CurrentUser(Vec<usize>);

impl CurrentUser {
    fn get() -> io::Result<Self> {
        unsafe {
            let mut raw = null_mut();
            succeeded(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw))?;
            let token = OwnedHandle::from_raw_handle(raw);
            let mut size = 0;
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut size);
            if size == 0
                || io::Error::last_os_error().raw_os_error()
                    != Some(ERROR_INSUFFICIENT_BUFFER as i32)
            {
                return Err(io::Error::last_os_error());
            }
            let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
            succeeded(GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                storage.as_mut_ptr().cast(),
                size,
                &mut size,
            ))?;
            Ok(Self(storage))
        }
    }

    fn sid(&self) -> *mut c_void {
        // GetTokenInformation placed TOKEN_USER and its SID in this aligned buffer.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn descriptor(&self, directory: bool) -> io::Result<Local> {
        unsafe {
            let mut text = null_mut();
            succeeded(ConvertSidToStringSidW(self.sid(), &mut text))?;
            let _allocation = Local(text.cast());
            let mut len = 0;
            while *text.add(len) != 0 {
                len += 1;
            }
            let sid = String::from_utf16(std::slice::from_raw_parts(text, len))
                .map_err(io::Error::other)?;
            let inherit = if directory { "OICI" } else { "" };
            let sddl = format!("O:{sid}D:P(A;{inherit};FA;;;{sid})\0")
                .encode_utf16()
                .collect::<Vec<_>>();
            let mut sd = null_mut();
            succeeded(ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut sd,
                null_mut(),
            ))?;
            Ok(Local(sd))
        }
    }

    fn check_owner(&self, handle: &OwnedHandle) -> io::Result<()> {
        unsafe {
            let mut owner = null_mut();
            let mut sd = null_mut();
            status(GetSecurityInfo(
                handle.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut sd,
            ))?;
            let _allocation = Local(sd);
            if EqualSid(owner, self.sid()) == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Unexpected private file owner",
                ));
            }
        }
        Ok(())
    }
}

fn attributes(descriptor: &Local) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}

pub(crate) fn open_owned_credential(path: &Path) -> io::Result<File> {
    use windows_sys::Win32::Foundation::GENERIC_READ;
    let path = wide(path)?;
    unsafe {
        let raw = CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        );
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let handle = OwnedHandle::from_raw_handle(raw);
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        succeeded(GetFileInformationByHandle(
            handle.as_raw_handle(),
            &mut info,
        ))?;
        if info.nNumberOfLinks != 1
            || info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                != 0
        {
            return Err(invalid("Unexpected credential file type or link count"));
        }
        CurrentUser::get()?.check_owner(&handle)?;
        Ok(File::from(handle))
    }
}

pub fn ensure_private_directory(path: &Path) -> io::Result<()> {
    reject_links(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let user = CurrentUser::get()?;
    let sd = user.descriptor(true)?;
    let name = wide(path)?;
    unsafe {
        if CreateDirectoryW(name.as_ptr(), &attributes(&sd)) != 0 {
            return Ok(());
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(err);
        }
        let raw = CreateFileW(
            name.as_ptr(),
            READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        );
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let handle = OwnedHandle::from_raw_handle(raw);
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        succeeded(GetFileInformationByHandle(
            handle.as_raw_handle(),
            &mut info,
        ))?;
        if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
            != FILE_ATTRIBUTE_DIRECTORY
        {
            return Err(invalid("Expected a private directory"));
        }
        user.check_owner(&handle)?;
        let (mut present, mut defaulted, mut dacl) = (0, 0, null_mut());
        succeeded(GetSecurityDescriptorDacl(
            sd.0,
            &mut present,
            &mut dacl,
            &mut defaulted,
        ))?;
        if present == 0 || dacl.is_null() {
            return Err(invalid("Private ACL is missing"));
        }
        status(SetSecurityInfo(
            handle.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            dacl,
            null(),
        ))
    }
}

pub(super) fn create_private(path: &Path) -> io::Result<File> {
    let name = wide(path)?;
    let sd = CurrentUser::get()?.descriptor(false)?;
    unsafe {
        let raw = CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes(&sd),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        );
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(File::from_raw_handle(raw))
    }
}
