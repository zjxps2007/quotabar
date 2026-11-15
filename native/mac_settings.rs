//! Settings and account changes made by the macOS monitor. The on-disk format
//! is shared with the original app's Foundation Codable models.
mod codec;
pub mod imported;
#[cfg(test)]
mod tests;

use crate::{
    claude::mac_connection,
    credentials::{self, Platform, Provider},
    files, oauth, schedule, vault,
};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::Path,
    time::{Duration, SystemTime},
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Debug)]
pub enum Error {
    Json,
    Io(io::Error),
    Import(io::Error),
    Credential(credentials::Error),
    WrongAccount,
    MissingRefresh,
    Unavailable,
    Vault,
    LargeImport,
    Claude(mac_connection::Error),
}
impl From<mac_connection::Error> for Error {
    fn from(error: mac_connection::Error) -> Self {
        Self::Claude(error)
    }
}
impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::Json
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
struct AccountId(Uuid);
impl TryFrom<String> for AccountId {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() == 36
            && let Ok(uuid) = Uuid::parse_str(&value)
        {
            return Ok(Self(uuid));
        }
        Err("Expected a hyphenated UUID")
    }
}
impl From<AccountId> for String {
    fn from(id: AccountId) -> Self {
        id.0.hyphenated().to_string().to_ascii_uppercase()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    id: AccountId,
    #[serde(deserialize_with = "codec::provider")]
    provider: Provider,
    name: String,
    identity: String,
    source_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_managed_login: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_in_menu_bar: Option<bool>,
    enabled: bool,
    created_at: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "String")]
enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}
impl From<String> for Appearance {
    fn from(value: String) -> Self {
        match value.as_str() {
            "light" => Self::Light,
            "dark" => Self::Dark,
            _ => Self::System,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", from = "String")]
enum MenuDisplay {
    #[default]
    ProviderAndRemaining,
    RemainingOnly,
    IconOnly,
}
impl From<String> for MenuDisplay {
    fn from(value: String) -> Self {
        match value.as_str() {
            "remainingOnly" => Self::RemainingOnly,
            "iconOnly" => Self::IconOnly,
            _ => Self::ProviderAndRemaining,
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    accounts: Vec<Account>,
    #[serde(deserialize_with = "codec::integer")]
    refresh_seconds: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_refresh: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    automatic_updates: Option<bool>,
    scan_local_logs: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    appearance: Option<Appearance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    show_dock_icon: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    menu_bar_display_mode: Option<MenuDisplay>,
    #[serde(
        default,
        deserialize_with = "codec::optional_integer",
        skip_serializing_if = "Option::is_none"
    )]
    credential_storage_version: Option<i64>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            refresh_seconds: 300,
            auto_refresh: None,
            automatic_updates: None,
            scan_local_logs: true,
            appearance: None,
            show_dock_icon: None,
            menu_bar_display_mode: None,
            credential_storage_version: None,
        }
    }
}
impl Settings {
    fn load(path: &Path) -> Result<Self, Error> {
        match fs::read(path) {
            Ok(bytes) => codec::decode(&bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }
    fn save(&self, path: &Path) -> Result<(), Error> {
        Ok(files::write_private(path, &serde_json::to_vec(self)?)?)
    }
    fn connect_claude(
        &mut self,
        identity: &str,
        source_path: &str,
        label: &str,
        alias: &str,
    ) -> Option<usize> {
        if let Some(i) = self.matching(Provider::Claude, identity) {
            let a = &mut self.accounts[i];
            if !alias.is_empty() {
                a.name = alias.to_owned();
            }
            a.enabled = true;
        } else {
            let name = if alias.is_empty() { label } else { alias };
            self.accounts.push(Settings::new_account(
                Provider::Claude,
                identity,
                name,
                source_path,
            ));
        }
        self.accounts
            .iter()
            .position(|a| same_identity(&a.identity, identity))
    }
    fn index(&self, id: AccountId) -> Option<usize> {
        self.accounts.iter().position(|a| a.id == id)
    }
    fn matching(&self, provider: Provider, identity: &str) -> Option<usize> {
        self.accounts
            .iter()
            .position(|a| a.provider == provider && same_identity(&a.identity, identity))
    }
    fn new_account(provider: Provider, identity: &str, label: &str, path: &str) -> Account {
        let epoch = SystemTime::UNIX_EPOCH + Duration::from_secs(978_307_200);
        let created_at = match SystemTime::now().duration_since(epoch) {
            Ok(d) => d.as_secs_f64(),
            Err(e) => -e.duration().as_secs_f64(),
        };
        Account {
            id: AccountId(Uuid::new_v4()),
            provider,
            name: label.to_owned(),
            identity: identity.to_owned(),
            source_path: path.to_owned(),
            app_managed_login: None,
            show_in_menu_bar: None,
            enabled: true,
            created_at,
        }
    }
    fn connect(
        &mut self,
        provider: Provider,
        credentials: &credentials::Credentials,
        alias: &str,
        path: &str,
        managed: bool,
    ) -> usize {
        let mut account = self
            .matching(provider, &credentials.identity)
            .map(|i| self.accounts[i].clone())
            .unwrap_or_else(|| {
                Self::new_account(provider, &credentials.identity, &credentials.label, path)
            });
        if !alias.is_empty() {
            account.name = alias.to_owned();
        }
        account.source_path = path.to_owned();
        account.app_managed_login = Some(managed);
        account.enabled = true;
        // The original monitor replaces the first row with this UUID, even if
        // the matching provider/identity came from a later duplicate row.
        if let Some(i) = self.index(account.id) {
            self.accounts[i] = account;
            i
        } else {
            self.accounts.push(account);
            self.accounts.len() - 1
        }
    }
}
fn same_identity(a: &str, b: &str) -> bool {
    a == b || a.nfc().eq(b.nfc())
}

#[derive(Deserialize)]
struct Target {
    id: AccountId,
    identity: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Edit {
    Interval {
        seconds: i64,
    },
    AutoRefresh {
        enabled: bool,
    },
    AutomaticUpdates {
        enabled: bool,
    },
    Appearance {
        mode: Appearance,
    },
    ShowDockIcon {
        visible: bool,
    },
    MenuBarDisplayMode {
        mode: MenuDisplay,
    },
    ScanLocalLogs {
        enabled: bool,
    },
    Toggle {
        id: AccountId,
    },
    MenuVisibility {
        id: AccountId,
        visible: bool,
    },
    ImportLogin {
        provider: Provider,
        path: String,
        alias: String,
    },
    ManagedLogin {
        alias: String,
        target: Option<Target>,
    },
    Claude {
        identity: String,
        source_path: String,
        label: String,
        alias: String,
    },
    BindClaude {
        directory: String,
        helper: String,
        alias: String,
    },
    RemoveClaude {
        id: AccountId,
        identity: String,
    },
    Remove {
        id: AccountId,
        delete_credential: bool,
    },
}
#[derive(Clone, Copy, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Storage {
    Disk,
    Memory,
    Unavailable,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum Request {
    Load {},
    Save {
        settings: Settings,
    },
    Edit {
        settings: Settings,
        edit: Edit,
        storage: Storage,
    },
}
#[derive(Serialize)]
struct Reply {
    settings: Settings,
    changed: bool,
    index: Option<usize>,
}

fn preference<T: PartialEq + Copy>(field: &mut Option<T>, default: T, value: T) -> bool {
    if field.unwrap_or(default) == value {
        return false;
    }
    *field = Some(value);
    true
}

/// Commit the credential before publishing settings. A failed settings write
/// restores the original vault entry, including its absence on first connect.
fn save_credential(
    settings: &Settings,
    path: &Path,
    directory: Option<&Path>,
    id: AccountId,
    data: Option<&[u8]>,
) -> Result<(), Error> {
    let directory = directory.ok_or(Error::Vault)?;
    let replace = |value: Option<&[u8]>| match value {
        Some(bytes) => vault::save(directory, id.0, bytes),
        None => vault::delete(directory, id.0),
    };
    let previous = vault::load(directory, id.0).map_err(|_| Error::Vault)?;
    replace(data).map_err(|_| Error::Vault)?;
    if let Err(error) = settings.save(path) {
        replace(previous.as_ref().map(|v| v.as_slice())).map_err(|_| Error::Vault)?;
        return Err(error);
    }
    Ok(())
}

fn apply(
    mut settings: Settings,
    edit: Edit,
    storage: Storage,
    path: &Path,
    directory: Option<&Path>,
    credential: &[u8],
) -> Result<Reply, Error> {
    let mut index = None;
    let mut vault_change = None;
    let imported;
    let changed = match edit {
        Edit::Interval { seconds } => {
            let valid = schedule::INTERVALS.iter().any(|&s| i64::from(s) == seconds);
            let changed = valid && settings.refresh_seconds != seconds;
            if changed {
                settings.refresh_seconds = seconds;
            }
            changed
        }
        Edit::AutoRefresh { enabled } => preference(&mut settings.auto_refresh, true, enabled),
        Edit::AutomaticUpdates { enabled } => {
            settings.automatic_updates = Some(enabled);
            true
        }
        Edit::Appearance { mode } => preference(&mut settings.appearance, Appearance::System, mode),
        Edit::ShowDockIcon { visible } => preference(&mut settings.show_dock_icon, true, visible),
        Edit::MenuBarDisplayMode { mode } => preference(
            &mut settings.menu_bar_display_mode,
            MenuDisplay::ProviderAndRemaining,
            mode,
        ),
        Edit::ScanLocalLogs { enabled } => {
            let changed = settings.scan_local_logs != enabled;
            settings.scan_local_logs = enabled;
            changed
        }
        Edit::Toggle { id } => {
            index = settings.index(id);
            if let Some(i) = index {
                settings.accounts[i].enabled = !settings.accounts[i].enabled;
            }
            index.is_some()
        }
        Edit::MenuVisibility { id, visible } => settings
            .index(id)
            .is_some_and(|i| preference(&mut settings.accounts[i].show_in_menu_bar, true, visible)),
        Edit::ImportLogin {
            provider,
            path: source,
            alias,
        } => {
            let file = fs::File::open(&source).map_err(Error::Import)?;
            if file.metadata().map_err(Error::Import)?.len() > vault::MAX_SIZE as u64 {
                return Err(Error::LargeImport);
            }
            imported = Zeroizing::new(files::read_bounded(file, vault::MAX_SIZE).map_err(|e| {
                if e.kind() == io::ErrorKind::InvalidData {
                    Error::LargeImport
                } else {
                    Error::Import(e)
                }
            })?);
            let parsed = credentials::parse(&imported, provider, Platform::Mac)
                .map_err(Error::Credential)?;
            let i = settings.connect(provider, &parsed, &alias, &source, false);
            index = Some(i);
            vault_change = Some((settings.accounts[i].id, Some(imported.as_slice())));
            true
        }
        Edit::ManagedLogin { alias, target } => {
            let parsed = oauth::managed(credential, Platform::Mac).map_err(Error::Credential)?;
            if parsed.refresh_token.is_none() {
                return Err(Error::MissingRefresh);
            }
            if let Some(t) = target
                && (settings.index(t.id).is_none() || !same_identity(&t.identity, &parsed.identity))
            {
                return Err(Error::WrongAccount);
            }
            let i = settings.connect(Provider::Codex, &parsed, &alias, "", true);
            index = Some(i);
            vault_change = Some((settings.accounts[i].id, Some(credential)));
            true
        }
        Edit::Claude {
            identity,
            source_path,
            label,
            alias,
        } => {
            index = settings.connect_claude(&identity, &source_path, &label, &alias);
            true
        }
        Edit::BindClaude {
            directory: source,
            helper,
            alias,
        } => {
            if storage != Storage::Disk {
                return Err(Error::Unavailable);
            }
            let dir = Path::new(&source);
            if !dir.is_dir() {
                return Err(mac_connection::Error::MissingDirectory.into());
            }
            let profiles = path.with_file_name("ClaudeProfiles");
            let profile_name = if alias.is_empty() {
                "Claude profile"
            } else {
                &alias
            };
            let binding = mac_connection::install_binding(
                &profiles,
                &source,
                profile_name.to_owned(),
                &helper,
                false,
            )?;
            let label = format!(
                "Claude · {}",
                dir.file_name().unwrap_or_default().to_string_lossy()
            );
            index = settings.connect_claude(&binding.profile.id, &source, &label, &alias);
            if let Err(e) = settings.save(path) {
                binding.undo()?;
                return Err(e);
            }
            return Ok(Reply {
                settings,
                changed: true,
                index,
            });
        }
        Edit::RemoveClaude { id, identity } => {
            if storage != Storage::Disk {
                return Err(Error::Unavailable);
            }
            let removing = mac_connection::Removal::prepare(
                &path.with_file_name("ClaudeProfiles"),
                &identity,
            )?;
            settings.accounts.retain(|a| a.id != id);
            if let Err(e) = settings.save(path) {
                removing.undo()?;
                return Err(e);
            }
            removing.finish();
            return Ok(Reply {
                settings,
                changed: true,
                index: None,
            });
        }
        Edit::Remove {
            id,
            delete_credential,
        } => {
            settings.accounts.retain(|a| a.id != id);
            if delete_credential {
                vault_change = Some((id, None));
            }
            true
        }
    };
    if changed {
        match storage {
            Storage::Unavailable => return Err(Error::Unavailable),
            Storage::Memory => (),
            Storage::Disk => match vault_change {
                Some((id, data)) => save_credential(&settings, path, directory, id, data)?,
                None => settings.save(path)?,
            },
        }
    }
    Ok(Reply {
        settings,
        changed,
        index,
    })
}

pub fn run(
    path: &Path,
    directory: Option<&Path>,
    request: &[u8],
    credential: &[u8],
) -> Result<Vec<u8>, Error> {
    let reply = match serde_json::from_slice(request)? {
        Request::Load {} => Reply {
            settings: Settings::load(path)?,
            changed: false,
            index: None,
        },
        Request::Save { settings } => {
            settings.save(path)?;
            Reply {
                settings,
                changed: true,
                index: None,
            }
        }
        Request::Edit {
            settings,
            edit,
            storage,
        } => apply(settings, edit, storage, path, directory, credential)?,
    };
    Ok(serde_json::to_vec(&reply)?)
}
