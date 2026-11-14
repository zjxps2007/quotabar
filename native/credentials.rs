//! Existing CLI import and workspace identity rules. OAuth tokens are never logged.
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, FixedOffset, Local, NaiveDate, NaiveDateTime};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;
use zeroize::{Zeroize, Zeroizing};

pub(crate) const AUTH: &str = "https://api.openai.com/auth";
const PROFILE: &str = "https://api.openai.com/profile";

#[derive(Clone, Copy, PartialEq)]
pub enum Platform {
    Windows,
    Mac,
}
#[derive(Clone, Copy, PartialEq)]
pub enum Provider {
    Codex,
    Gemini,
    Claude,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Error {
    TooLarge,
    InvalidJson,
    MissingToken,
    MissingIdentity,
    ClaudeDirectory,
    SubscriptionRequired,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub identity: String,
    pub label: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub email: Option<String>,
    pub expires_at: Option<DateTime<FixedOffset>>,
    pub last_refresh: Option<DateTime<FixedOffset>>,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

pub fn claims(token: Option<&str>, platform: Platform) -> Value {
    let Some(token) = token else {
        return Value::Null;
    };
    let segment = if platform == Platform::Mac {
        token.split('.').filter(|p| !p.is_empty()).nth(1)
    } else {
        token.split('.').nth(1)
    };
    let Some(segment) = segment else {
        return Value::Null;
    };
    let segment: std::borrow::Cow<'_, str> =
        if platform == Platform::Windows && segment.bytes().any(|c| c.is_ascii_whitespace()) {
            segment
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect::<String>()
                .into()
        } else {
            segment.into()
        };
    [URL_SAFE_NO_PAD, URL_SAFE, STANDARD_NO_PAD, STANDARD]
        .iter()
        .find_map(|engine| engine.decode(segment.as_bytes()).ok())
        .and_then(|bytes| serde_json::from_slice(&Zeroizing::new(bytes)).ok())
        .unwrap_or(Value::Null)
}
pub(crate) fn trimmed(s: &str) -> Option<&str> {
    let s = s.trim();
    (!s.is_empty()).then_some(s)
}
fn norm(value: Option<&str>, platform: Platform) -> Option<String> {
    let s = value.and_then(trimmed)?.nfc().collect::<String>();
    // Windows uses invariant simple casing. Foundation applies full Unicode lowercase.
    Some(if platform == Platform::Windows {
        s.chars()
            .map(|c| {
                if c == 'İ' {
                    c
                } else {
                    c.to_lowercase().next().unwrap_or(c)
                }
            })
            .collect()
    } else {
        s.to_lowercase()
    })
}
fn field<'a>(object: &'a Value, keys: &[&str], platform: Platform) -> Option<&'a str> {
    match platform {
        Platform::Windows => keys
            .iter()
            .filter_map(|k| object[k].as_str())
            .find_map(trimmed),
        Platform::Mac => keys
            .iter()
            .find_map(|k| object.get(*k))
            .and_then(Value::as_str),
    }
}
fn selected<'a>(
    values: impl IntoIterator<Item = Option<&'a str>>,
    platform: Platform,
) -> Option<&'a str> {
    let mut values = values.into_iter().flatten();
    if platform == Platform::Mac {
        values.find_map(trimmed)
    } else {
        values.next().and_then(trimmed)
    }
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub account_id: Option<String>,
    pub user_id: Option<String>,
    pub email: Option<String>,
}

fn workspace(root: &Value, claims: &[Value; 2], platform: Platform) -> Workspace {
    let nested = &root["tokens"];
    let tokens = if platform == Platform::Windows && !nested.is_object() {
        root
    } else {
        nested
    };
    let group = |domain: Option<&str>, field: &str| {
        selected(
            claims
                .iter()
                .map(|c| domain.map_or(c, |d| &c[d])[field].as_str()),
            platform,
        )
    };
    let first =
        |list: &[Option<&str>]| norm(list.iter().find_map(|v| v.and_then(trimmed)), platform);
    let account_id = first(&[
        tokens["account_id"].as_str(),
        tokens["accountId"].as_str(),
        root["account_id"].as_str(),
        (platform == Platform::Mac)
            .then(|| root["accountId"].as_str())
            .flatten(),
        root["chatgpt_account_id"].as_str(),
        group(Some(AUTH), "chatgpt_account_id"),
        group(None, "chatgpt_account_id"),
    ]);
    let user_id = first(&[
        root["chatgpt_user_id"].as_str(),
        group(Some(AUTH), "chatgpt_user_id"),
        group(None, "chatgpt_user_id"),
        group(Some(AUTH), "user_id"),
        (platform == Platform::Mac)
            .then(|| group(None, "user_id"))
            .flatten(),
        root["user_id"].as_str(),
        group(None, "sub"),
    ]);
    let email = first(&[
        root["email"].as_str(),
        group(None, "email"),
        group(Some(PROFILE), "email"),
    ])
    .filter(|s| s.contains('@'));
    Workspace {
        account_id,
        user_id,
        email,
    }
}
fn token_claims(root: &Value, platform: Platform) -> [Value; 2] {
    let nested = &root["tokens"];
    let source = if nested.is_object() { nested } else { root };
    [["id_token", "idToken"], ["access_token", "accessToken"]].map(|keys| {
        let token = match platform {
            Platform::Windows => field(source, &keys, platform),
            // Foundation coalesces the raw fields first, before checking their type.
            Platform::Mac => [nested, root]
                .iter()
                .find_map(|obj| keys.iter().find_map(|key| obj.get(*key)))
                .and_then(Value::as_str),
        };
        claims(token, platform)
    })
}
pub fn mac_workspace(root: &Value) -> Workspace {
    workspace(root, &token_claims(root, Platform::Mac), Platform::Mac)
}

fn date(value: &Value, platform: Platform) -> Option<DateTime<FixedOffset>> {
    let text = value.as_str()?.trim();
    if let Ok(date) = DateTime::parse_from_rfc3339(text) {
        return Some(date);
    }
    if platform == Platform::Mac {
        return None;
    }
    if let Ok(date) = DateTime::parse_from_rfc2822(text) {
        return Some(date);
    }
    let mut local = None;
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(value) = NaiveDateTime::parse_from_str(text, format) {
            local = Some(value);
            break;
        }
    }
    let local = local
        .or_else(|| {
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)
        })
        .or_else(|| localized_date(text))?;
    local
        .and_local_timezone(Local)
        .latest()
        .map(|d| d.fixed_offset())
}

#[cfg(windows)]
fn localized_date(text: &str) -> Option<NaiveDateTime> {
    use windows_sys::Win32::{
        Foundation::SYSTEMTIME,
        Globalization::GetUserDefaultLCID,
        System::{Ole::VarDateFromStr, Variant::VariantTimeToSystemTime},
    };
    if text.contains('\0') {
        return None;
    }
    let text = text.encode_utf16().chain([0]).collect::<Vec<_>>();
    let mut value = 0.0;
    let mut date = SYSTEMTIME::default();
    unsafe {
        if VarDateFromStr(text.as_ptr(), GetUserDefaultLCID(), 0, &mut value) < 0
            || VariantTimeToSystemTime(value, &mut date) == 0
        {
            return None;
        }
    }
    NaiveDate::from_ymd_opt(date.wYear.into(), date.wMonth.into(), date.wDay.into())?
        .and_hms_milli_opt(
            date.wHour.into(),
            date.wMinute.into(),
            date.wSecond.into(),
            date.wMilliseconds.into(),
        )
}
#[cfg(not(windows))]
fn localized_date(_: &str) -> Option<NaiveDateTime> {
    None
}
fn label_prefix(value: &str, platform: Platform, count: usize) -> String {
    if platform == Platform::Mac {
        value.graphemes(true).take(count).collect()
    } else {
        String::from_utf16_lossy(&value.encode_utf16().take(count).collect::<Vec<_>>())
    }
}
pub fn parse(bytes: &[u8], provider: Provider, platform: Platform) -> Result<Credentials, Error> {
    if bytes.len() > crate::vault::MAX_SIZE {
        return Err(Error::TooLarge);
    }
    let root = document(bytes, platform)?;
    parse_object(&root, provider, platform)
}

pub(crate) fn document(bytes: &[u8], platform: Platform) -> Result<Value, Error> {
    if platform == Platform::Windows {
        return serde_json::from_slice(bytes).map_err(|_| Error::InvalidJson);
    }
    // JSONSerialization also accepts UTF-16/32 login files exported by shell tools.
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    let (width, little, skip) = match bytes {
        [0xff, 0xfe, 0, 0, ..] => (4, true, 4),
        [0, 0, 0xfe, 0xff, ..] => (4, false, 4),
        [0xff, 0xfe, ..] => (2, true, 2),
        [0xfe, 0xff, ..] => (2, false, 2),
        [0, 0, 0, _, ..] => (4, false, 0),
        [_, 0, 0, 0, ..] => (4, true, 0),
        [0, _, 0, _, ..] => (2, false, 0),
        [_, 0, _, 0, ..] => (2, true, 0),
        _ => return serde_json::from_slice(bytes).map_err(|_| Error::InvalidJson),
    };
    let bytes = &bytes[skip..];
    if !bytes.len().is_multiple_of(width) {
        return Err(Error::InvalidJson);
    }
    let mut decoded = Zeroizing::new(String::new());
    if width == 2 {
        let units = bytes.as_chunks::<2>().0.iter().map(|s| {
            if little {
                u16::from_le_bytes(*s)
            } else {
                u16::from_be_bytes(*s)
            }
        });
        for c in char::decode_utf16(units) {
            decoded.push(c.map_err(|_| Error::InvalidJson)?);
        }
    } else {
        for &unit in bytes.as_chunks::<4>().0 {
            let value = if little {
                u32::from_le_bytes(unit)
            } else {
                u32::from_be_bytes(unit)
            };
            decoded.push(char::from_u32(value).ok_or(Error::InvalidJson)?);
        }
    }
    serde_json::from_str(&decoded).map_err(|_| Error::InvalidJson)
}

pub(crate) fn parse_object(
    root: &Value,
    provider: Provider,
    platform: Platform,
) -> Result<Credentials, Error> {
    if platform == Platform::Mac && !root.is_object() {
        return Err(Error::InvalidJson);
    }
    let source = if root["tokens"].is_object() {
        &root["tokens"]
    } else {
        root
    };
    let access = field(source, &["access_token", "accessToken"], platform)
        .filter(|s| !s.is_empty())
        .ok_or(Error::MissingToken)?;
    if platform == Platform::Windows
        && root["OPENAI_API_KEY"]
            .as_str()
            .is_some_and(|x| !x.is_empty())
    {
        return Err(Error::MissingToken);
    }
    let refresh = field(source, &["refresh_token", "refreshToken"], platform).map(str::to_owned);
    let payloads = token_claims(root, platform);
    let ws = workspace(root, &payloads, platform);
    let (identity, label) = match (provider, platform) {
        (Provider::Codex, _) => {
            let account = ws.account_id.as_deref().ok_or(Error::MissingIdentity)?;
            let identity = if let Some(user) = &ws.user_id {
                format!("codex:account:{account}:user:{user}")
            } else if let Some(email) = &ws.email {
                format!("codex:account:{account}:email:{email}")
            } else {
                return Err(Error::MissingIdentity);
            };
            (
                identity,
                ws.email
                    .clone()
                    .unwrap_or_else(|| format!("Codex · {}", label_prefix(account, platform, 8))),
            )
        }
        (Provider::Claude, Platform::Mac) => return Err(Error::ClaudeDirectory),
        (_, Platform::Windows) => {
            let id = if let Some(sub) = payloads[0]["sub"].as_str() {
                format!("gemini:subject:{sub}")
            } else if let Some(email) = &ws.email {
                format!("gemini:email:{email}")
            } else if let Some(refresh) = &refresh {
                format!("gemini:credential:{:x}", Sha256::digest(refresh.as_bytes()))
            } else {
                return Err(Error::MissingIdentity);
            };
            let name = if provider == Provider::Claude {
                "Claude"
            } else {
                "Gemini"
            };
            let label = ws.email.clone().unwrap_or_else(|| {
                format!(
                    "{name} · {}",
                    label_prefix(ws.account_id.as_deref().unwrap_or(&id), platform, 8)
                )
            });
            (id, label)
        }
        (Provider::Gemini, Platform::Mac) => {
            let claims = claims(field(root, &["id_token", "idToken"], platform), platform);
            let email = claims["email"]
                .as_str()
                .or(root["email"].as_str())
                .map(str::to_lowercase);
            if let Some(sub) = claims["sub"].as_str().filter(|s| !s.is_empty()) {
                (
                    format!("gemini:subject:{sub}"),
                    email.unwrap_or_else(|| "Gemini account".into()),
                )
            } else if let Some(email) = email.filter(|s| !s.is_empty()) {
                (format!("gemini:email:{email}"), email)
            } else if let Some(token) = root["refresh_token"].as_str().filter(|s| !s.is_empty()) {
                let digest = format!("{:x}", Sha256::digest(token.as_bytes()));
                (
                    format!("gemini:credential:{digest}"),
                    format!("Gemini · {}", &digest[..6]),
                )
            } else {
                return Err(Error::MissingIdentity);
            }
        }
    };
    let number = |v: &Value| {
        v.as_f64().or_else(|| {
            (platform == Platform::Mac)
                .then(|| v.as_bool().map(|b| u8::from(b) as f64))
                .flatten()
        })
    };
    let expiry = number(&root["expires_at"]).or_else(|| {
        if platform == Platform::Mac && access.split('.').filter(|p| !p.is_empty()).count() != 3 {
            return None;
        }
        number(&payloads[1]["exp"])
    });
    let expires_at = expiry
        .filter(|s| platform == Platform::Mac || (*s > 0. && *s < 253402300799.))
        .and_then(|s| {
            if platform == Platform::Mac {
                DateTime::from_timestamp_micros((s * 1e6) as i64)
            } else {
                DateTime::from_timestamp(s as i64, 0)
            }
        })
        .map(|d| d.fixed_offset())
        .or_else(|| date(&root["expired"], platform));
    Ok(Credentials {
        identity,
        label,
        access_token: access.into(),
        refresh_token: refresh,
        account_id: ws.account_id,
        user_id: ws.user_id,
        email: ws.email,
        expires_at,
        last_refresh: date(&root["last_refresh"], platform),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth;
    use chrono::Utc;
    use serde_json::json;

    fn jwt(payload: Value) -> String {
        format!(
            "test.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }

    #[test]
    fn imported_identities_keep_platform_precedence_and_unicode() {
        let user = jwt(json!({"sub":"fallback",AUTH:{"chatgpt_user_id":"U-1"}}));
        let data = serde_json::to_vec(
            &json!({"tokens":{"access_token":user,"accountId":" E\u{301}QUIP "},
            "email":"MAIL@EXAMPLE.INVALID"}),
        )
        .unwrap();
        for platform in [Platform::Windows, Platform::Mac] {
            let parsed = parse(&data, Provider::Codex, platform).unwrap();
            assert_eq!(parsed.identity, "codex:account:équip:user:u-1");
            assert_eq!(parsed.label, "mail@example.invalid");
            assert!(matches!(
                parse(
                    br#"{"access_token":"test","email":"e@x"}"#,
                    Provider::Codex,
                    platform
                ),
                Err(Error::MissingIdentity)
            ));
        }
        assert_eq!(norm(Some("İΟΣ"), Platform::Mac).unwrap(), "i\u{307}ος");
        assert_eq!(norm(Some("İΟΣ"), Platform::Windows).unwrap(), "İοσ");
        let bytes = br#"{"access_token":"one","refresh_token":"stable"}"#;
        let win = parse(bytes, Provider::Gemini, Platform::Windows).unwrap();
        let mac = parse(bytes, Provider::Gemini, Platform::Mac).unwrap();
        assert_eq!(win.identity, mac.identity);
        assert_eq!(win.label, "Gemini · gemini:c");
        assert_eq!(
            mac.label,
            format!(
                "Gemini · {}",
                &mac.identity["gemini:credential:".len()..][..6]
            )
        );
        let flat = jwt(json!({"sub":"wrong"}));
        let case = json!({"tokens":{"access_token":null},"access_token":flat});
        assert!(mac_workspace(&case).user_id.is_none());
        let wide: Vec<_> = [0xfe, 0xff]
            .into_iter()
            .chain(
                std::str::from_utf8(bytes)
                    .unwrap()
                    .encode_utf16()
                    .flat_map(u16::to_be_bytes),
            )
            .collect();
        assert_eq!(
            parse(&wide, Provider::Gemini, Platform::Mac)
                .unwrap()
                .identity,
            mac.identity
        );
    }

    #[test]
    fn rotation_preserves_pinned_accounts_extensions_and_platform_expiry() {
        use oauth::{RotationError, rotate};
        let initial = serde_json::to_vec(
            &json!({"tokens":{"access_token":"old","account_id":"A","refresh_token":"stable"},
            "email":"e@x","expired":"2025-01-01T12:30:00+09:00","custom":{"keep":1}}),
        )
        .unwrap();
        let now = DateTime::<Utc>::from_timestamp(1735689600, 500000000).unwrap();
        for platform in [Platform::Windows, Platform::Mac] {
            let old = parse(&initial, Provider::Codex, platform).unwrap();
            assert_eq!(old.expires_at.unwrap().offset().local_minus_utc(), 32400);
            let update = rotate(
                &initial,
                br#"{"access_token":"replacement","expires_in":3600.25}"#,
                now,
                platform,
            )
            .unwrap();
            let new = parse(&update, Provider::Codex, platform).unwrap();
            let json: Value = serde_json::from_slice(&update).unwrap();
            assert_eq!(new.identity, old.identity);
            assert_eq!(new.access_token, "replacement");
            assert_eq!(new.refresh_token.as_deref(), Some("stable"));
            assert_eq!(json["custom"]["keep"], 1);
            assert!(json.get("expired").is_none());
            assert_eq!(new.expires_at.unwrap().timestamp(), now.timestamp() + 3600);
            assert_eq!(
                new.expires_at.unwrap().timestamp_subsec_millis(),
                if platform == Platform::Mac { 750 } else { 0 }
            );
            let wrong = jwt(json!({AUTH:{"chatgpt_account_id":"another"}}));
            let moved =
                serde_json::to_vec(&json!({"access_token":"opaque","id_token":wrong})).unwrap();
            assert_eq!(
                rotate(&initial, &moved, now, platform).unwrap_err(),
                RotationError::ReturnedAccountChanged
            );
            assert_eq!(
                rotate(
                    &initial,
                    br#"{"refresh_token":"lost-access"}"#,
                    now,
                    platform
                )
                .unwrap_err(),
                RotationError::InvalidResponse
            );
        }
        let key = br#"{"access_token":"token","account_id":"a","user_id":"u","OPENAI_API_KEY":""}"#;
        assert!(matches!(
            oauth::managed(key, Platform::Mac),
            Err(Error::SubscriptionRequired)
        ));
        assert!(oauth::managed(key, Platform::Windows).is_ok());
    }
}
