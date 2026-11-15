//! Windows monitor settings, including files written by the original .NET application.
pub mod accounts;
pub mod snapshots;
use crate::{credentials::Provider, files, schedule};
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self, MapAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{collections::HashSet, fmt, io, path::Path};
use uuid::Uuid;

pub const SETTINGS_LIMIT: usize = 4 * 1024 * 1024;
const UNSUPPORTED: &str = "설정 형식이 지원되지 않습니다. 원본 파일을 보존했습니다.";

#[derive(Debug)]
pub enum Error {
    Json(serde_json::Error),
    Io(io::Error),
    File(io::Error, &'static str),
    Credential(crate::credentials::Error),
    Usage(&'static str, &'static str),
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

// System.Text.Json binds property names case-insensitively and processes repeated
// properties in input order. Keep each value borrowed until its field is decoded.
struct Fields<'a>(Vec<(String, &'a RawValue)>);

impl<'de> Deserialize<'de> for Fields<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Object;
        impl<'de> Visitor<'de> for Object {
            type Value = Fields<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = Vec::new();
                while let Some((mut key, value)) = map.next_entry::<String, &RawValue>()? {
                    key.make_ascii_lowercase();
                    fields.push((key, value));
                }
                Ok(Fields(fields))
            }
        }
        d.deserialize_map(Object)
    }
}

fn malformed(message: &'static str) -> Error {
    Error::Json(de::Error::custom(message))
}
fn unsupported() -> Error {
    Error::Io(files::invalid(UNSUPPORTED))
}

// RawValue skips unknown fields without descending into them. The old reader
// applies its depth limit to the whole document, including skipped extensions.
fn check_json_depth(bytes: &[u8]) -> Result<(), Error> {
    if crate::json_depth::allows(bytes, 64) {
        Ok(())
    } else {
        Err(malformed("JSON exceeds the maximum depth of 64"))
    }
}

fn integer(raw: &RawValue) -> Result<i32, Error> {
    if raw.get().starts_with('"') {
        let text: String = serde_json::from_str(raw.get())?;
        // Utf8Parser accepts decimal digits with an optional leading sign.
        return text
            .parse()
            .map_err(|_| malformed("Expected a 32-bit integer"));
    }
    Ok(serde_json::from_str(raw.get())?)
}

fn provider(raw: &RawValue) -> Result<Option<Provider>, Error> {
    let ordinal = if raw.get().starts_with('"') {
        let value: String = serde_json::from_str(raw.get())?;
        let value = value.trim();
        if let Ok(n) = value.parse::<i32>() {
            n
        } else {
            let mut ordinal = 0;
            for name in value.split(',').map(str::trim) {
                ordinal |= match name.to_ascii_lowercase().as_str() {
                    "codex" => 0,
                    "claude" => 1,
                    "gemini" => 2,
                    _ => return Err(malformed("Unknown provider")),
                };
            }
            ordinal
        }
    } else {
        serde_json::from_str(raw.get())?
    };
    Ok(match ordinal {
        0 => Some(Provider::Codex),
        1 => Some(Provider::Claude),
        2 => Some(Provider::Gemini),
        _ => None,
    })
}

fn guid(value: &str) -> Result<Uuid, Error> {
    if value.len() != 36 {
        return Err(malformed("Expected a hyphenated GUID"));
    }
    Uuid::parse_str(value).map_err(|_| malformed("Invalid GUID"))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub name: String,
    pub identity: String,
    pub source_path: Option<String>,
    pub enabled: bool,
    pub show_in_tray: bool,
    pub app_managed_login: bool,
}

impl Account {
    fn read(fields: Fields<'_>) -> Result<Option<Self>, Error> {
        let mut id = Uuid::nil();
        let mut service = Some(Provider::Codex);
        let (mut name, mut identity, mut source_path) = (None::<String>, None::<String>, None);
        let (mut enabled, mut show_in_tray, mut app_managed_login) = (true, true, false);
        for (key, raw) in fields.0 {
            let value = raw.get();
            match key.as_str() {
                "id" => id = guid(&serde_json::from_str::<String>(value)?)?,
                "provider" => service = provider(raw)?,
                "name" => name = serde_json::from_str(value)?,
                "identity" => identity = serde_json::from_str(value)?,
                "sourcepath" => source_path = serde_json::from_str(value)?,
                "enabled" => enabled = serde_json::from_str(value)?,
                "showintray" => show_in_tray = serde_json::from_str(value)?,
                "appmanagedlogin" => app_managed_login = serde_json::from_str(value)?,
                _ => {}
            }
        }
        let (Some(service), Some(name), Some(identity)) = (service, name, identity) else {
            return Ok(None);
        };
        if id.is_nil() || name.trim().is_empty() || identity.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            id: id.to_string(),
            provider: service,
            name,
            identity,
            source_path,
            enabled,
            show_in_tray,
            app_managed_login,
        }))
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: i32,
    pub accounts: Vec<Account>,
    pub refresh_seconds: i32,
    pub auto_refresh: bool,
    pub automatic_updates: bool,
    pub scan_local_logs: bool,
    pub appearance: String,
    pub show_taskbar_icon: bool,
    pub tray_display_mode: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            accounts: vec![],
            refresh_seconds: 300,
            auto_refresh: true,
            automatic_updates: true,
            scan_local_logs: true,
            appearance: "system".into(),
            show_taskbar_icon: true,
            tray_display_mode: "remaining".into(),
        }
    }
}

fn theme(value: &str) -> bool {
    matches!(value, "system" | "light" | "dark")
}
fn tray(value: &str) -> bool {
    matches!(value, "remaining" | "icon")
}

impl Settings {
    pub fn read(bytes: &[u8]) -> Result<Self, Error> {
        check_json_depth(bytes)?;
        let fields: Option<Fields> = serde_json::from_slice(bytes)?;
        let fields = fields.ok_or(Error::Usage("invalid_json", "데이터 형식을 확인하세요."))?;
        let mut settings = Self::default();
        let mut valid_accounts = true;
        for (key, raw) in fields.0 {
            let value = raw.get();
            match key.as_str() {
                "schemaversion" => settings.schema_version = integer(raw)?,
                "accounts" => {
                    let entries: Option<Vec<Option<Fields>>> = serde_json::from_str(value)?;
                    settings.accounts.clear();
                    valid_accounts = entries.as_ref().is_some_and(|a| a.len() <= 50);
                    for entry in entries.unwrap_or_default() {
                        match entry.map(Account::read).transpose()?.flatten() {
                            Some(account) => settings.accounts.push(account),
                            None => valid_accounts = false,
                        }
                    }
                }
                "refreshseconds" => settings.refresh_seconds = integer(raw)?,
                "autorefresh" => settings.auto_refresh = serde_json::from_str(value)?,
                "automaticupdates" => settings.automatic_updates = serde_json::from_str(value)?,
                "scanlocallogs" => settings.scan_local_logs = serde_json::from_str(value)?,
                "appearance" => {
                    settings.appearance =
                        serde_json::from_str::<Option<String>>(value)?.unwrap_or_default()
                }
                "showtaskbaricon" => settings.show_taskbar_icon = serde_json::from_str(value)?,
                "traydisplaymode" => {
                    settings.tray_display_mode =
                        serde_json::from_str::<Option<String>>(value)?.unwrap_or_default()
                }
                _ => {}
            }
        }
        let mut ids = HashSet::new();
        if settings.schema_version != 1
            || !valid_accounts
            || !settings.accounts.iter().all(|a| ids.insert(&a.id))
        {
            return Err(unsupported());
        }
        if !schedule::INTERVALS.contains(&settings.refresh_seconds) {
            settings.refresh_seconds = 300;
        }
        if !theme(&settings.appearance) {
            settings.appearance = "system".into();
        }
        if !tray(&settings.tray_display_mode) {
            settings.tray_display_mode = "remaining".into();
        }
        Ok(settings)
    }

    pub fn load(root: &Path) -> Result<Self, Error> {
        let path = root.join("settings.json");
        files::reject_links(&path)?;
        if path.is_dir() {
            return Ok(Self::default());
        }
        match files::read_file(&path, SETTINGS_LIMIT) {
            Ok(data) => Self::read(&data),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, root: &Path) -> Result<Vec<u8>, Error> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > SETTINGS_LIMIT {
            return Err(files::invalid("Settings exceed size limit").into());
        }
        files::write_private(&root.join("settings.json"), &bytes)?;
        Ok(bytes)
    }

    pub fn preferences(&mut self, options: Preferences) -> Result<(), Error> {
        let mode = options
            .tray_display
            .unwrap_or_else(|| self.tray_display_mode.clone());
        let appearance = options.theme.unwrap_or_default();
        if !schedule::INTERVALS.contains(&options.seconds) || !theme(&appearance) || !tray(&mode) {
            return Err(Error::Usage("settings", "지원하는 설정 값을 선택하세요."));
        }
        self.refresh_seconds = options.seconds;
        self.appearance = appearance;
        self.scan_local_logs = options.scan;
        self.show_taskbar_icon = options.taskbar;
        self.tray_display_mode = mode;
        self.auto_refresh = options.auto_refresh.unwrap_or(self.auto_refresh);
        self.automatic_updates = options.automatic_updates.unwrap_or(self.automatic_updates);
        Ok(())
    }

    pub fn edit(&mut self, id: &str, name: &str, enabled: bool, tray: bool) -> Result<(), Error> {
        if name.trim().is_empty() || name.encode_utf16().count() > 100 {
            return Err(Error::Usage("name", "이름은 1~100자로 입력하세요."));
        }
        if let Some(account) = self.accounts.iter_mut().find(|a| a.id == id) {
            account.name = name.trim().into();
            account.enabled = enabled;
            account.show_in_tray = tray;
        }
        Ok(())
    }

    /// Reconnecting preserves user edits and moves the account to the end, matching
    /// the order presented by both the legacy Dashboard and its notification menu.
    pub fn connect(
        &mut self,
        service: Provider,
        name: String,
        identity: String,
        path: String,
        managed: bool,
    ) -> Result<bool, Error> {
        let index = self.accounts.iter().position(|a| {
            a.identity == identity && (service == Provider::Claude || a.provider == service)
        });
        let account = if let Some(index) = index {
            let mut account = self.accounts.remove(index);
            if service != Provider::Claude {
                account.source_path = Some(path);
                account.app_managed_login = managed;
            }
            account
        } else {
            if self.accounts.len() >= 50 {
                return Err(Error::Usage(
                    "accounts",
                    "계정은 최대 50개까지 연결할 수 있습니다.",
                ));
            }
            Account {
                id: Uuid::new_v4().to_string(),
                provider: service,
                name,
                identity,
                source_path: Some(path),
                enabled: true,
                show_in_tray: true,
                app_managed_login: managed,
            }
        };
        self.accounts.push(account);
        Ok(index.is_some())
    }

    pub fn remove(&mut self, id: &str) {
        self.accounts.retain(|a| a.id != id);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    seconds: i32,
    theme: Option<String>,
    scan: bool,
    taskbar: bool,
    tray_display: Option<String>,
    auto_refresh: Option<bool>,
    automatic_updates: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_at_capacity_preserves_the_windows_account_and_user_edits() {
        let mut settings = Settings::default();
        for i in 0..50 {
            assert!(
                !settings
                    .connect(
                        Provider::Codex,
                        format!("Name {i}"),
                        format!("id:{i}"),
                        "before".into(),
                        false
                    )
                    .unwrap()
            );
        }
        let id = settings.accounts[0].id.clone();
        settings
            .edit(&id, "　My workspace　", false, false)
            .unwrap();
        assert!(matches!(
            settings.connect(
                Provider::Gemini,
                "Other".into(),
                "id:0".into(),
                "same path".into(),
                false
            ),
            Err(Error::Usage("accounts", _))
        ));
        assert!(
            settings
                .connect(
                    Provider::Codex,
                    "Different label".into(),
                    "id:0".into(),
                    "new path".into(),
                    true
                )
                .unwrap()
        );
        let last = settings.accounts.last().unwrap();
        assert_eq!(
            (&last.id, last.name.as_str(), last.source_path.as_deref()),
            (&id, "My workspace", Some("new path"))
        );
        assert!(!last.enabled && !last.show_in_tray && last.app_managed_login);
        assert_eq!(settings.accounts.len(), 50);
        settings.remove(&id);
        assert_eq!(settings.accounts.len(), 49);
    }
}
