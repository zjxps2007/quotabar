use quotabar::credentials::{self, Error, Platform, Provider};
use quotabar::oauth::{self, RotationError};
use serde::Serialize;
use std::{ptr, slice};
use zeroize::Zeroizing;

pub(super) fn error_code(error: Error) -> i32 {
    match error {
        Error::TooLarge => -2,
        Error::InvalidJson => -3,
        Error::FoundationJson => -14,
        Error::MissingToken => -4,
        Error::MissingIdentity => -5,
        Error::ClaudeDirectory => -6,
        Error::SubscriptionRequired => -7,
    }
}
#[derive(Serialize)]
struct Identity<'a> {
    key: &'a str,
    label: &'a str,
}

fn platform() -> Platform {
    if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Mac
    }
}

pub(super) fn publish(bytes: Vec<u8>, output: &mut *mut u8, length: &mut usize) {
    publish_secret(&Zeroizing::new(bytes), output, length);
}

pub(super) fn publish_secret(bytes: &[u8], output: &mut *mut u8, length: &mut usize) {
    let result: Box<[u8]> = bytes.into();
    *length = result.len();
    *output = Box::into_raw(result).cast();
}

/// # Safety
/// The input must be length readable bytes, and both output locations must
/// remain valid. Release the returned bytes with qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_credentials(
    data: *const u8,
    length: usize,
    provider: i32,
    mode: i32,
    output: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    let (Some(output), Some(out_len)) = (unsafe { output.as_mut() }, unsafe { out_len.as_mut() })
    else {
        return -1;
    };
    *output = ptr::null_mut();
    *out_len = 0;
    if length > quotabar::vault::MAX_SIZE {
        return -2;
    }
    if data.is_null() && length != 0 {
        return -3;
    }
    let provider = match provider {
        0 => Provider::Codex,
        1 => Provider::Claude,
        2 => Provider::Gemini,
        _ => return -1,
    };
    let platform = if mode == 0 { platform() } else { Platform::Mac };
    let encoded = std::panic::catch_unwind(|| -> Result<Vec<u8>, i32> {
        let input = if length == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(data, length) }
        };
        if mode == 4 {
            let parsed =
                quotabar::provider_requests::mac_query(input, provider == Provider::Codex)?;
            return serde_json::to_vec(&parsed).map_err(|_| -11);
        }
        if mode == 2 {
            let json = serde_json::from_slice(input).map_err(|_| -3)?;
            return serde_json::to_vec(&credentials::mac_workspace(&json)).map_err(|_| -3);
        }
        let parsed = if mode == 3 {
            oauth::managed(input, platform)
        } else {
            credentials::parse(input, provider, platform)
        }
        .map_err(error_code)?;
        match mode {
            0 => serde_json::to_vec(&parsed).map_err(|_| -3),
            3 => serde_json::to_vec(&serde_json::json!({
                "identity":{"key":parsed.identity,"label":parsed.label},
                "accessToken":parsed.access_token,"refreshToken":parsed.refresh_token,
                "expiresAt":parsed.expires_at.map(|d|d.timestamp_micros() as f64/1e6),
                "lastRefresh":parsed.last_refresh.map(|d|d.timestamp_micros() as f64/1e6),
            }))
            .map_err(|_| -3),
            1 => serde_json::to_vec(&Identity {
                key: &parsed.identity,
                label: &parsed.label,
            })
            .map_err(|_| -3),
            _ => Err(-1),
        }
    });
    match encoded {
        Ok(Ok(bytes)) => {
            publish(bytes, output, out_len);
            0
        }
        Ok(Err(code)) => code,
        Err(_) => -1,
    }
}

/// # Safety
/// Both inputs must contain the stated lengths, and output locations must be valid.
/// Release returned credentials exactly once with qb_secret_free.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qb_rotate_credentials(
    current: *const u8,
    current_len: usize,
    response: *const u8,
    response_len: usize,
    now_micros: i64,
    output: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    let (Some(output), Some(out_len)) = (unsafe { output.as_mut() }, unsafe { out_len.as_mut() })
    else {
        return -1;
    };
    *output = ptr::null_mut();
    *out_len = 0;
    if current_len > quotabar::vault::MAX_SIZE {
        return -2;
    }
    if current.is_null() || current_len == 0 {
        return -3;
    }
    if response.is_null() || response_len == 0 || response_len > 2 * quotabar::vault::MAX_SIZE {
        return -8;
    }
    let Some(now) = chrono::DateTime::from_timestamp_micros(now_micros) else {
        return -1;
    };
    let result = std::panic::catch_unwind(|| {
        oauth::rotate(
            unsafe { slice::from_raw_parts(current, current_len) },
            unsafe { slice::from_raw_parts(response, response_len) },
            now,
            platform(),
        )
    });
    match result {
        Ok(Ok(bytes)) => {
            publish(bytes, output, out_len);
            0
        }
        Ok(Err(RotationError::Credentials(e))) => error_code(e),
        Ok(Err(RotationError::MalformedResponse | RotationError::InvalidResponse)) => -8,
        Ok(Err(RotationError::ReturnedAccountChanged)) => -9,
        Ok(Err(RotationError::StoredAccountChanged)) => -10,
        Err(_) => -1,
    }
}
