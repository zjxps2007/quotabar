#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum State {
    Disabled = 0,
    Enabled = 1,
    RequiresApproval = 2,
    Unavailable = 3,
}

#[cfg(windows)]
pub mod windows {
    use super::State;
    use crate::files::{invalid, wide};
    use std::{io, path::Path, ptr::null_mut};
    use windows_sys::{
        Win32::{
            Foundation::{
                ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS,
                ERROR_UNSUPPORTED_TYPE,
            },
            Globalization::{CSTR_EQUAL, CompareStringOrdinal},
            System::Registry::{
                HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW,
                RegGetValueW, RegSetKeyValueW,
            },
        },
        core::w,
    };

    fn command(exe: &Path) -> Option<Vec<u16>> {
        let path = wide(exe).ok()?;
        if !exe.is_absolute()
            || !exe
                .file_name()?
                .eq_ignore_ascii_case("QuotaBar.Monitor.exe")
            || path.contains(&34)
            || !exe.is_file()
        {
            return None;
        }
        let mut cmd = vec![34];
        cmd.extend_from_slice(&path[..path.len() - 1]);
        cmd.extend("\" --background\0".encode_utf16());
        (cmd.len() <= 261).then_some(cmd)
    }
    pub fn state(exe: &Path, key: &Path) -> io::Result<State> {
        let Some(command) = command(exe) else {
            return Ok(State::Unavailable);
        };
        let key = wide(key)?;
        let mut buffer = [0u16; 261];
        let mut size = size_of_val(&buffer) as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                w!("QuotaBar"),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        match rc {
            ERROR_FILE_NOT_FOUND
            | ERROR_PATH_NOT_FOUND
            | ERROR_UNSUPPORTED_TYPE
            | ERROR_MORE_DATA => Ok(State::Disabled),
            ERROR_SUCCESS => Ok(
                if unsafe { CompareStringOrdinal(buffer.as_ptr(), -1, command.as_ptr(), -1, 1) }
                    == CSTR_EQUAL
                {
                    State::Enabled
                } else {
                    State::Disabled
                },
            ),
            other => Err(io::Error::from_raw_os_error(other as i32)),
        }
    }
    pub fn set(exe: &Path, key: &Path, enabled: bool) -> io::Result<()> {
        let key = wide(key)?;
        let status = if enabled {
            let value = command(exe).ok_or_else(|| invalid("Unavailable startup executable"))?;
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    w!("QuotaBar"),
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            }
        } else {
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), w!("QuotaBar")) }
        };
        if status == ERROR_SUCCESS
            || (!enabled && matches!(status, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND))
        {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        }
    }
}

#[cfg(target_os = "macos")]
pub mod macos {
    use super::State;
    use objc2_foundation::NSBundle;
    use objc2_service_management::{SMAppService, SMAppServiceStatus};
    use std::io;

    pub fn state() -> State {
        let bundle = NSBundle::mainBundle();
        if !bundle
            .bundleIdentifier()
            .is_some_and(|id| id.to_string() == "com.aiusage.lite")
            || !bundle
                .bundleURL()
                .pathExtension()
                .is_some_and(|ext| ext.to_string() == "app")
        {
            return State::Unavailable;
        }
        match unsafe { SMAppService::mainAppService().status() } {
            SMAppServiceStatus::NotRegistered => State::Disabled,
            SMAppServiceStatus::Enabled => State::Enabled,
            SMAppServiceStatus::RequiresApproval => State::RequiresApproval,
            _ => State::Unavailable,
        }
    }
    pub fn set(enabled: bool) -> io::Result<()> {
        let state = state();
        if state == State::Unavailable {
            return Err(io::Error::other("Login item unavailable"));
        }
        if (state != State::Disabled) == enabled {
            return Ok(());
        }
        let service = unsafe { SMAppService::mainAppService() };
        let result = unsafe {
            if enabled {
                service.registerAndReturnError()
            } else {
                service.unregisterAndReturnError()
            }
        };
        result.map_err(|error| io::Error::other(error.to_string()))
    }
    pub fn open_settings() {
        unsafe {
            SMAppService::openSystemSettingsLoginItems();
        }
    }
}
