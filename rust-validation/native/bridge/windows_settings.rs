use quotabar::{
    credentials::{Error as CredentialError, Provider},
    vault,
    windows_settings::{Error, Preferences, SETTINGS_LIMIT, Settings, accounts, snapshots},
};
use serde::Deserialize;
use serde_json::{json, to_vec};
use std::{collections::HashSet, ffi::c_char, io, path::Path, ptr, slice};
use uuid::Uuid;

fn failure(error: Error) -> (i32, String) {
    fn usage(code: &str, message: &str) -> (i32, String) {
        (-2, json!({ "code": code, "message": message }).to_string())
    }
    match error {
        Error::Io(e) => (e.raw_os_error().unwrap_or(-1), e.to_string()),
        Error::File(e, message) => (e.raw_os_error().unwrap_or(-1), message.into()),
        Error::Json(e) => (-3, e.to_string()),
        Error::Usage(code, message) => usage(code, message),
        Error::Credential(e) => match e {
            CredentialError::TooLarge => usage("invalid_auth", "로그인 파일이 너무 큽니다."),
            CredentialError::MissingToken => usage(
                "missing_token",
                "API 키 대신 구독 계정의 로그인 파일을 연결하세요.",
            ),
            CredentialError::MissingIdentity => usage(
                "missing_identity",
                "로그인 파일에서 계정과 워크스페이스를 확인할 수 없습니다.",
            ),
            _ => (-3, "로그인 파일의 JSON 형식을 확인하세요.".into()),
        },
    }
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Request {
    Load,
    LoadSnapshots {
        accounts: HashSet<String>,
    },
    SaveSnapshots,
    Save,
    Preferences {
        options: Preferences,
    },
    Edit {
        id: String,
        name: Option<String>,
        enabled: bool,
        tray: bool,
    },
    Prepare {
        provider: Provider,
        name: String,
        identity: String,
        path: String,
        managed: bool,
    },
    Remove {
        id: String,
    },
}

fn perform(root: &Path, request: &[u8], current: &[u8]) -> Result<Vec<u8>, Error> {
    let request = serde_json::from_slice(request)?;
    match request {
        Request::Load => return Ok(to_vec(&Settings::load(root)?)?),
        Request::LoadSnapshots { accounts } => return snapshots::load(root, &accounts),
        Request::SaveSnapshots => {
            snapshots::save(root, current)?;
            return Ok(to_vec(&true)?);
        }
        _ => {}
    }
    let mut settings = Settings::read(current)?;
    match request {
        Request::Load | Request::LoadSnapshots { .. } | Request::SaveSnapshots => unreachable!(),
        Request::Save => {}
        Request::Preferences { options } => settings.preferences(options)?,
        Request::Edit {
            id,
            name,
            enabled,
            tray,
        } => settings.edit(&id, name.as_deref().unwrap_or_default(), enabled, tray)?,
        Request::Prepare {
            provider,
            name,
            identity,
            path,
            managed,
        } => {
            let existing = settings.connect(provider, name, identity, path, managed)?;
            return Ok(to_vec(
                &json!({ "settings": settings, "existing": existing }),
            )?);
        }
        Request::Remove { id } => settings.remove(&id),
    }
    // The managed display model is replaced only after the file has been committed.
    settings.save(root)
}

/// # Safety
/// Root is a NUL-terminated UTF-8 path. Both buffers are readable for the supplied
/// lengths. Release the returned settings or error with qb_string_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_settings(
    root: *const c_char,
    request: *const u8,
    request_len: usize,
    current: *const u8,
    current_len: usize,
    output: *mut *mut c_char,
) -> i32 {
    // System.Text.Json may escape a single UTF-8 byte into six ASCII characters
    // when the host sends a copy of an already-loaded document.
    unsafe {
        super::windows_operation::invoke(
            root,
            (request, request_len),
            (current, current_len),
            (SETTINGS_LIMIT * 8, SETTINGS_LIMIT * 8),
            output,
            |root, request, current| perform(root, request, current).map_err(failure),
        )
    }
}

#[derive(Deserialize)]
struct Connection {
    provider: i32,
    path: String,
    managed: bool,
}
fn provider(value: i32) -> Result<Provider, Error> {
    match value {
        0 => Ok(Provider::Codex),
        1 => Ok(Provider::Claude),
        2 => Ok(Provider::Gemini),
        _ => Err(Error::Usage("provider", "지원하지 않는 서비스입니다.")),
    }
}

/// The buffers and synchronous owner-thread callback are valid until return.
/// Only settings or an error message cross the string boundary.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_connect(
    root: *const c_char,
    request: *const u8,
    request_len: usize,
    current: *const u8,
    current_len: usize,
    credential: *const u8,
    credential_len: usize,
    revoke: extern "C" fn(*const c_char) -> i32,
    output: *mut *mut c_char,
) -> i32 {
    unsafe {
        super::windows_operation::invoke(
            root,
            (request, request_len),
            (current, current_len),
            (SETTINGS_LIMIT, SETTINGS_LIMIT * 8),
            output,
            |root, request, current| {
                let result = (|| {
                    let req: Connection = serde_json::from_slice(request)?;
                    let source = if req.managed {
                        if credential_len > vault::MAX_SIZE {
                            return Err(Error::Credential(CredentialError::TooLarge));
                        }
                        let bytes = if credential_len == 0 {
                            &[]
                        } else {
                            if credential.is_null() {
                                return Err(Error::Credential(CredentialError::InvalidJson));
                            }
                            slice::from_raw_parts(credential, credential_len)
                        };
                        accounts::Source::CodexLogin(bytes)
                    } else {
                        accounts::Source::File {
                            provider: provider(req.provider)?,
                            path: &req.path,
                        }
                    };
                    accounts::connect(root, Settings::read(current)?, source, |id| {
                        let mut name = [0; 37];
                        id.hyphenated().encode_lower(&mut name);
                        if revoke(name.as_ptr().cast()) != 0 {
                            Ok(())
                        } else {
                            Err(Error::Io(io::Error::other(
                                "Account refresh could not be cancelled",
                            )))
                        }
                    })
                })();
                result.map_err(failure)
            },
        )
    }
}

#[derive(Deserialize)]
struct Imported {
    id: String,
    provider: i32,
    path: Option<String>,
    identity: String,
}

/// On success the credential is returned as a wipeable buffer. Error strings
/// use the same ownership and error codes as other Windows settings operations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_windows_imported(
    root: *const c_char,
    request: *const u8,
    request_len: usize,
    secret: *mut *mut u8,
    length: *mut usize,
    output: *mut *mut c_char,
) -> i32 {
    if secret.is_null() || length.is_null() {
        return -1;
    }
    unsafe {
        *secret = ptr::null_mut();
        *length = 0;
        super::windows_operation::invoke(
            root,
            (request, request_len),
            (ptr::null(), 0),
            (SETTINGS_LIMIT * 8, 0),
            output,
            |root, request, _| {
                let req: Imported =
                    serde_json::from_slice(request).map_err(|e| failure(e.into()))?;
                let bytes = accounts::imported(
                    root,
                    Uuid::parse_str(&req.id).map_err(|_| (-3, "Invalid account ID".into()))?,
                    provider(req.provider).map_err(failure)?,
                    req.path.as_deref().unwrap_or_default(),
                    &req.identity,
                )
                .map_err(failure)?;
                super::credentials::publish_secret(&bytes, &mut *secret, &mut *length);
                Ok(Vec::new())
            },
        )
    }
}
