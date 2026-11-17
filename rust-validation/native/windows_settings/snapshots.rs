//! Quota windows cached by the Windows monitor between launches.
use super::{Error, Fields, check_json_depth, guid, malformed};
use crate::files;
use chrono::{Datelike, NaiveDate, TimeDelta};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::value::RawValue;
use std::{collections::HashSet, io, path::Path};

const LIMIT: usize = 2 * 1024 * 1024;
const DEFAULT_DATE: &str = "0001-01-01T00:00:00+00:00";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Window {
    title: Option<String>,
    remaining: f64,
    reset_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    fetched_at: String,
    windows: Vec<Window>,
    plan: Option<String>,
    email: Option<String>,
    observed_at: Option<String>,
    state: Option<String>,
    lowest: Option<f64>,
}

// DateTimeOffset accepts the extended ISO profile. Retaining the wire string
// preserves the offset and 100-nanosecond ticks across an application upgrade.
fn iso_date(value: &str) -> Option<()> {
    fn number(s: &str, start: usize, len: usize) -> Option<u32> {
        let digits = s.get(start..start + len)?;
        digits
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| digits.parse().ok())
            .flatten()
    }
    let b = value.as_bytes();
    let year = number(value, 0, 4)? as i32;
    if year == 0 || b.get(4) != Some(&b'-') || b.get(7) != Some(&b'-') {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(year, number(value, 5, 2)?, number(value, 8, 2)?)?;
    if b.len() == 10 {
        return Some(());
    }
    if b.get(10) != Some(&b'T') || b.get(13) != Some(&b':') {
        return None;
    }
    let mut end = 16;
    let second = if b.get(end) == Some(&b':') {
        let second = number(value, 17, 2)?;
        end = 19;
        if b.get(end) == Some(&b'.') {
            end += 1;
            let digits = b[end..].iter().take_while(|c| c.is_ascii_digit()).count();
            if !(1..=16).contains(&digits) {
                return None;
            }
            end += digits;
        }
        second
    } else {
        0
    };
    let local = date.and_hms_opt(number(value, 11, 2)?, number(value, 14, 2)?, second)?;
    let offset = match b.get(end..)? {
        [] | [b'Z'] => return Some(()),
        [sign @ (b'+' | b'-'), ..] => {
            let hours = number(value, end + 1, 2)?;
            let minutes = match b.get(end + 3..)? {
                [] => 0,
                [b':', _, _] => number(value, end + 4, 2)?,
                _ => return None,
            };
            if minutes > 59 || hours * 60 + minutes > 14 * 60 {
                return None;
            }
            i64::from(hours * 60 + minutes) * if *sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    let utc = local.checked_sub_signed(TimeDelta::minutes(offset))?;
    (1..=9999).contains(&utc.year()).then_some(())
}

fn timestamp(value: &RawValue) -> Result<Option<String>, Error> {
    let date = serde_json::from_str::<Option<String>>(value.get())?;
    if date.as_deref().is_some_and(|s| iso_date(s).is_none()) {
        return Err(malformed("Invalid DateTimeOffset"));
    }
    Ok(date)
}

fn window(fields: Fields<'_>) -> Result<Window, Error> {
    let mut window = Window {
        title: None,
        remaining: 0.,
        reset_at: None,
    };
    for (key, value) in fields.0 {
        match key.as_str() {
            "title" => window.title = serde_json::from_str(value.get())?,
            "remaining" => {
                let remaining = if value.get().starts_with('"') {
                    serde_json::from_str::<String>(value.get())?
                        .parse()
                        .map_err(|_| malformed("Invalid quota percentage"))?
                } else {
                    serde_json::from_str(value.get())?
                };
                if !f64::is_finite(remaining) {
                    return Err(malformed("Non-finite quota percentage"));
                }
                window.remaining = remaining;
            }
            "resetat" => window.reset_at = timestamp(value)?,
            _ => {}
        }
    }
    Ok(window)
}

fn snapshot(fields: Fields<'_>) -> Result<Option<Snapshot>, Error> {
    let mut fetched_at = DEFAULT_DATE.to_owned();
    let (mut windows, mut plan, mut email, mut observed_at, mut state) =
        (None, None, None, None, None);
    for (key, value) in fields.0 {
        match key.as_str() {
            "fetchedat" => {
                fetched_at = timestamp(value)?.ok_or_else(|| malformed("Missing fetch time"))?
            }
            "windows" => {
                let entries: Option<Vec<Fields>> = serde_json::from_str(value.get())?;
                windows = entries
                    .map(|w| w.into_iter().map(window).collect::<Result<Vec<_>, _>>())
                    .transpose()?;
            }
            "plan" => plan = serde_json::from_str(value.get())?,
            "email" => email = serde_json::from_str(value.get())?,
            "observedat" => observed_at = timestamp(value)?,
            "state" => state = serde_json::from_str(value.get())?,
            _ => {}
        }
    }
    Ok(windows.map(|windows| Snapshot {
        lowest: windows.iter().map(|w| w.remaining).reduce(f64::min),
        fetched_at,
        windows,
        plan,
        email,
        observed_at,
        state,
    }))
}

fn read(
    bytes: &[u8],
    accounts: Option<&HashSet<String>>,
) -> Result<IndexMap<String, Snapshot>, Error> {
    check_json_depth(bytes)?;
    let fields: Option<Fields> = serde_json::from_slice(bytes)?;
    let fields = fields.ok_or(Error::Usage("invalid_json", "데이터 형식을 확인하세요."))?;
    let mut result = IndexMap::new();
    for (key, raw) in fields.0 {
        let id = guid(&key)?.to_string();
        let fields: Option<Fields> = serde_json::from_str(raw.get())?;
        let value = fields.map(snapshot).transpose()?.flatten();
        // The original deserializes the entire cache before selecting configured accounts.
        if accounts.is_some_and(|ids| !ids.contains(&id)) {
            continue;
        }
        if let Some(value) = value {
            result.insert(id, value);
        } else {
            result.shift_remove(&id);
        }
    }
    Ok(result)
}

pub(crate) fn display(bytes: &[u8]) -> Result<serde_json::Value, Error> {
    let fields: Fields = serde_json::from_slice(bytes)?;
    let value = snapshot(fields)?.ok_or_else(|| malformed("Missing quota windows"))?;
    Ok(serde_json::to_value(value)?)
}

pub fn load(root: &Path, accounts: &HashSet<String>) -> Result<Vec<u8>, Error> {
    let path = root.join("snapshots.json");
    files::reject_links(&path)?;
    if path.is_dir() {
        return Ok(b"{}".to_vec());
    }
    match files::read_file(&path, LIMIT) {
        Ok(bytes) => Ok(serde_json::to_vec(&read(&bytes, Some(accounts))?)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(b"{}".to_vec()),
        Err(e) => Err(e.into()),
    }
}

pub fn save(root: &Path, bytes: &[u8]) -> Result<(), Error> {
    let data = serde_json::to_vec(&read(bytes, None)?)?;
    if data.len() > LIMIT {
        return Err(files::invalid("Snapshot cache exceeds size limit").into());
    }
    Ok(files::write_private(&root.join("snapshots.json"), &data)?)
}
