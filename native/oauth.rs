//! Validate each returned token before carrying forward any pinned identity fields.
use crate::credentials::{
    Error, Platform, Provider, Workspace, claims, document, mac_workspace, parse_object, trimmed,
};
use chrono::{DateTime, SecondsFormat, Timelike, Utc};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RotationError {
    Credentials(Error),
    InvalidResponse,
    ReturnedAccountChanged,
    StoredAccountChanged,
}

pub fn managed(bytes: &[u8], platform: Platform) -> Result<crate::credentials::Credentials, Error> {
    if bytes.len() > crate::vault::MAX_SIZE {
        return Err(Error::TooLarge);
    }
    let json = document(bytes, platform)?;
    managed_object(&json, platform)
}

fn managed_object(
    json: &Value,
    platform: Platform,
) -> Result<crate::credentials::Credentials, Error> {
    let mut credentials = parse_object(json, Provider::Codex, platform)?;
    if platform == Platform::Mac {
        if json["OPENAI_API_KEY"].is_string() || trimmed(&credentials.access_token).is_none() {
            return Err(Error::SubscriptionRequired);
        }
        if credentials
            .refresh_token
            .as_deref()
            .and_then(trimmed)
            .is_none()
        {
            credentials.refresh_token = None;
        }
    }
    Ok(credentials)
}

fn conflicts(old: &crate::credentials::Credentials, new: &Workspace) -> bool {
    if matches!((&old.account_id,&new.account_id), (Some(a),Some(b)) if a!=b) {
        return true;
    }
    if let (Some(a), Some(b)) = (&old.user_id, &new.user_id) {
        return a != b;
    }
    matches!((&old.email,&new.email), (Some(a),Some(b)) if a!=b)
}

#[cfg(windows)]
fn ordinal_equal(a: &str, b: &str) -> bool {
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    let a: Vec<_> = a.encode_utf16().collect();
    let b: Vec<_> = b.encode_utf16().collect();
    unsafe {
        CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
            == CSTR_EQUAL
    }
}
#[cfg(not(windows))]
fn ordinal_equal(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b) || a.to_uppercase() == b.to_uppercase()
}

fn windows_conflict(old: &crate::credentials::Credentials, token: &str) -> bool {
    let payload = claims(Some(token), Platform::Windows);
    let auth = &payload[crate::credentials::AUTH];
    let account = auth["chatgpt_account_id"]
        .as_str()
        .or(payload["chatgpt_account_id"].as_str());
    let user = auth["chatgpt_user_id"]
        .as_str()
        .or(payload["chatgpt_user_id"].as_str())
        .or(auth["user_id"].as_str())
        .or(payload["user_id"].as_str())
        .or(payload["sub"].as_str());
    account.is_some_and(|a| {
        !old.account_id
            .as_deref()
            .is_some_and(|b| ordinal_equal(a, b))
    }) || user
        .zip(old.user_id.as_deref())
        .is_some_and(|(a, b)| !ordinal_equal(a, b))
}

pub fn rotate(
    current: &[u8],
    response: &[u8],
    now: DateTime<Utc>,
    platform: Platform,
) -> Result<Vec<u8>, RotationError> {
    use RotationError::*;
    if current.len() > crate::vault::MAX_SIZE {
        return Err(Credentials(Error::TooLarge));
    }
    let mut root = document(current, platform).map_err(Credentials)?;
    let old = managed_object(&root, platform).map_err(Credentials)?;
    let limit = if platform == Platform::Mac {
        crate::vault::MAX_SIZE
    } else {
        2 * crate::vault::MAX_SIZE
    };
    if response.len() > limit {
        return Err(InvalidResponse);
    }
    let result = document(response, platform).map_err(|_| InvalidResponse)?;
    let access = result["access_token"]
        .as_str()
        .filter(|v| trimmed(v).is_some())
        .ok_or(InvalidResponse)?;
    for token in [Some(access), result["id_token"].as_str()]
        .into_iter()
        .flatten()
    {
        let mismatch = match platform {
            Platform::Mac => conflicts(&old, &mac_workspace(&json!({"access_token":token}))),
            Platform::Windows => windows_conflict(&old, token),
        };
        if mismatch {
            return Err(ReturnedAccountChanged);
        }
    }
    let nested = root["tokens"].is_object();
    let originally_nested = root.get("tokens").is_some();
    if platform == Platform::Mac && originally_nested && !nested {
        root["tokens"] = root.clone();
    }
    let target = if nested || (platform == Platform::Mac && originally_nested) {
        &mut root["tokens"]
    } else {
        &mut root
    };
    target["access_token"] = json!(access);
    for name in ["refresh_token", "id_token"] {
        if let Some(value) = result[name].as_str().filter(|s| {
            if platform == Platform::Mac {
                trimmed(s).is_some()
            } else {
                !s.is_empty()
            }
        }) {
            target[name] = json!(value);
        }
    }
    let obj = root
        .as_object_mut()
        .ok_or(Credentials(Error::InvalidJson))?;
    obj.remove("expires_at");
    obj.remove("expired");
    let now_seconds = now.timestamp() as f64 + f64::from(now.timestamp_subsec_nanos()) / 1e9;
    if let Some(seconds) = result["expires_in"]
        .as_f64()
        .or_else(|| {
            (platform == Platform::Mac)
                .then(|| {
                    result["expires_in"]
                        .as_bool()
                        .map(|v| f64::from(u8::from(v)))
                })
                .flatten()
        })
        .filter(|s| *s > 0. && (platform == Platform::Mac || *s < 315360000.))
    {
        let end = now_seconds + seconds;
        if !end.is_finite() {
            return Err(InvalidResponse);
        }
        root["expires_at"] = if platform == Platform::Windows {
            json!(end.floor() as i64)
        } else {
            json!(end)
        };
    }
    root["last_refresh"] = json!(match platform {
        Platform::Windows => format!(
            "{}.{:07}+00:00",
            now.format("%Y-%m-%dT%H:%M:%S"),
            now.nanosecond() / 100
        ),
        Platform::Mac => now.to_rfc3339_opts(SecondsFormat::Secs, true),
    });
    let bytes = serde_json::to_vec(&root).map_err(|_| InvalidResponse)?;
    if bytes.len() > crate::vault::MAX_SIZE {
        return Err(Credentials(Error::TooLarge));
    }
    if managed_object(&root, platform)
        .map_err(Credentials)?
        .identity
        != old.identity
    {
        return Err(StoredAccountChanged);
    }
    Ok(bytes)
}
