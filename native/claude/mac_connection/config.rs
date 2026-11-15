//! Only statusLine belongs to the connection. Keep unrelated settings and arbitrary
//! JSON numbers intact while changing its command. The containing folder belongs to the CLI.
use super::{AUTH_KEYS, Error, Result, command, same};
use serde_json::value::{RawValue, to_raw_value};
use std::{collections::BTreeMap, fs, io, path::Path};
use zeroize::Zeroizing;

pub(super) type Object = BTreeMap<String, Box<RawValue>>;

pub(super) fn string(object: &Object, key: &str) -> Option<String> {
    serde_json::from_str(object.get(key)?.get()).ok()
}

pub(super) fn set_text(object: &mut Object, key: &str, value: &str) -> Result<()> {
    object.insert(key.into(), to_raw_value(value)?);
    Ok(())
}

// Use exactly the same Foundation JSON reader, number formatting, collation and slash
// escaping as the installed application when writing backups and the CLI's settings.
#[cfg(target_os = "macos")]
fn formatted(bytes: &[u8], pretty: bool) -> Result<Vec<u8>> {
    use objc2_foundation::{
        NSData, NSJSONReadingOptions, NSJSONSerialization, NSJSONWritingOptions,
    };
    let obj = NSJSONSerialization::JSONObjectWithData_options_error(
        &NSData::with_bytes(bytes),
        NSJSONReadingOptions::empty(),
    )
    .map_err(|_| Error::SettingsJson)?;
    let mut flags = NSJSONWritingOptions::SortedKeys;
    if pretty {
        flags |= NSJSONWritingOptions::PrettyPrinted;
    }
    // Foundation produced this top-level array or dictionary using the non-fragment reader.
    unsafe { NSJSONSerialization::dataWithJSONObject_options_error(&obj, flags) }
        .map(|d| d.to_vec())
        .map_err(|_| Error::SettingsJson)
}

// Swift Foundation orders numbered keys numerically. Use the invariant system collator
// when running its compatibility suite on Windows, with a deterministic tie-break.
#[cfg(windows)]
fn compare_keys(left: &str, right: &str) -> std::cmp::Ordering {
    use windows_sys::Win32::Globalization::{
        CSTR_GREATER_THAN, CSTR_LESS_THAN, CompareStringEx, LOCALE_NAME_INVARIANT, NORM_IGNORECASE,
        SORT_DIGITSASNUMBERS,
    };
    let a: Vec<_> = left.encode_utf16().collect();
    let b: Vec<_> = right.encode_utf16().collect();
    let order = unsafe {
        CompareStringEx(
            LOCALE_NAME_INVARIANT,
            NORM_IGNORECASE | SORT_DIGITSASNUMBERS,
            a.as_ptr(),
            a.len() as i32,
            b.as_ptr(),
            b.len() as i32,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    match order {
        CSTR_LESS_THAN => std::cmp::Ordering::Less,
        CSTR_GREATER_THAN => std::cmp::Ordering::Greater,
        _ => left.cmp(right),
    }
}

// The compatibility build exercises file transactions on hosts without Apple Foundation.
#[cfg(not(target_os = "macos"))]
fn formatted(bytes: &[u8], _: bool) -> Result<Vec<u8>> {
    use crate::foundation_json::{keyed::trailing_commas, utf8};
    fn compact(value: &RawValue) -> Result<String> {
        match value.get().as_bytes()[0] {
            b'{' => {
                let object: Object = serde_json::from_str(value.get())?;
                #[cfg(windows)]
                let fields = {
                    let mut fields: Vec<_> = object.into_iter().collect();
                    fields.sort_by(|a, b| compare_keys(&a.0, &b.0));
                    fields
                };
                #[cfg(not(windows))]
                let fields = object;
                let mut parts = Vec::with_capacity(fields.len());
                for (k, v) in fields {
                    parts.push(format!(
                        "{}:{}",
                        serde_json::to_string(&k)?.replace('/', "\\/"),
                        compact(&v)?
                    ));
                }
                Ok(format!("{{{}}}", parts.join(",")))
            }
            b'[' => {
                let items: Vec<&RawValue> = serde_json::from_str(value.get())?;
                let values: Result<Vec<_>> = items.into_iter().map(compact).collect();
                Ok(format!("[{}]", values?.join(",")))
            }
            b'"' => Ok(
                serde_json::to_string(&serde_json::from_str::<String>(value.get())?)?
                    .replace('/', "\\/"),
            ),
            _ => Ok(value.get().into()),
        }
    }
    let utf8 = utf8(bytes).ok_or(Error::SettingsJson)?;
    let normalized = trailing_commas(utf8.as_ref());
    let value: &RawValue = serde_json::from_slice(&normalized).map_err(|_| Error::SettingsJson)?;
    if !matches!(value.get().as_bytes()[0], b'{' | b'[') {
        return Err(Error::SettingsJson);
    }
    Ok(compact(value)?.into_bytes())
}

pub(super) fn encode(object: &Object, pretty: bool) -> Result<Vec<u8>> {
    formatted(&Zeroizing::new(serde_json::to_vec(object)?), pretty)
}

pub(super) fn decode(bytes: &[u8]) -> Result<Option<Object>> {
    let normalized = Zeroizing::new(formatted(bytes, false)?);
    if normalized.first() == Some(&b'[') {
        return Ok(None);
    }
    serde_json::from_slice(&normalized)
        .map(Some)
        .map_err(|_| Error::SettingsJson)
}

fn read_existing(path: &Path) -> io::Result<Option<Zeroizing<Vec<u8>>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(super) struct Config {
    original: Option<Zeroizing<Vec<u8>>>,
    data: Object,
}
impl Config {
    pub fn read(directory: &str) -> Result<Self> {
        let original = read_existing(&Path::new(directory).join("settings.json"))?;
        let data = match original.as_ref() {
            Some(bytes) => decode(bytes)?.ok_or(Error::SettingsObject)?,
            None => Object::new(),
        };
        Ok(Self { original, data })
    }
    pub fn status(&self) -> Option<Object> {
        serde_json::from_str(self.data.get("statusLine")?.get()).ok()
    }
    pub fn has_status(&self) -> bool {
        self.data.contains_key("statusLine")
    }
    pub fn matches_command(&self, expected: Option<&str>) -> bool {
        same(command(self.status().as_ref()).as_deref(), expected)
    }
    pub fn set_status(&mut self, status: Option<&Object>) -> Result<()> {
        match status {
            Some(s) => {
                self.data.insert("statusLine".into(), to_raw_value(s)?);
            }
            None => {
                self.data.remove("statusLine");
            }
        }
        Ok(())
    }
    pub fn routes_through_api(&self) -> bool {
        // CharacterSet.whitespaces includes horizontal whitespace and excludes line breaks.
        fn horizontal(c: char) -> bool {
            matches!(
                c,
                '\t' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'
                    ..='\u{200b}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
            )
        }
        if string(&self.data, "apiKeyHelper")
            .is_some_and(|s| !s.trim_matches(horizontal).is_empty())
        {
            return true;
        }
        let Some(env) = self
            .data
            .get("env")
            .and_then(|v| serde_json::from_str::<Object>(v.get()).ok())
        else {
            return false;
        };
        AUTH_KEYS.iter().any(|k| {
            env.get(*k).is_some_and(|v| {
                serde_json::from_str::<String>(v.get()).map_or(true, |s| {
                    let s = s.trim_matches(horizontal);
                    !s.is_empty() && s != "0" && !s.eq_ignore_ascii_case("false")
                })
            })
        })
    }
    pub fn save(&self, directory: &str) -> Result<()> {
        let path = Path::new(directory).join("settings.json");
        let bytes = Zeroizing::new(encode(&self.data, true)?);
        if read_existing(&path)? != self.original {
            return Err(Error::Changed);
        }
        atomic_write(&path, &bytes)
    }
}

#[cfg(target_os = "macos")]
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use objc2_foundation::{NSData, NSDataWritingOptions, NSString, NSURL};
    let url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().ok_or(Error::Directory)?));
    NSData::with_bytes(bytes)
        .writeToURL_options_error(&url, NSDataWritingOptions::Atomic)
        .map_err(|e| Error::Foundation(e.code()))
}

#[cfg(not(target_os = "macos"))]
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let tmp = path.with_file_name(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut file = fs::File::create_new(&tmp)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(Into::into)
}
