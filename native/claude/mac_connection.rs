//! Claude Code subscription bindings, compatible with the original AIUsage macOS
//! profiles and statusLine backups. See NOTICE for upstream attribution.
mod backup_data;
mod config;
mod request;
#[cfg(test)]
mod tests;
mod usage;
pub use request::run;
#[cfg(target_os = "macos")]
pub(crate) use usage::fetch;

use crate::{
    files::{ensure_private_directory, write_private},
    foundation_json::keyed,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use config::{Config, Object};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

const ACTIVE: &str = "profile.json";
const PENDING: &str = "profile-pending.json";
const CALLBACK: &str = "--claude-statusline";
pub(super) const AUTH_KEYS: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_PROFILE",
    "ANTHROPIC_FEDERATION_RULE_ID",
    "ANTHROPIC_ORGANIZATION_ID",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_ANTHROPIC_FOUNDRY",
    "CLAUDE_CODE_GATEWAY_URL",
];

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json,
    SettingsJson,
    SettingsObject,
    Directory,
    Root,
    InvalidId,
    Identity,
    Backup,
    Foreign,
    Orphan,
    Recursive,
    UnsupportedType,
    UnsupportedFormat,
    Changed,
    RestoreNeeded,
    LoginReference,
    AuthStatus,
    MissingDirectory,
    #[cfg(target_os = "macos")]
    Foundation(isize),
}
pub type Result<T> = std::result::Result<T, Error>;
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self::Json
    }
}
impl From<Error> for io::Error {
    fn from(e: Error) -> Self {
        match e {
            Error::Io(e) => e,
            _ => io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Claude profile error ({})", e.status()),
            ),
        }
    }
}
impl Error {
    pub fn status(&self) -> i32 {
        match self {
            Self::Io(e) => e.raw_os_error().unwrap_or(-1),
            Self::Json => -40,
            Self::SettingsJson => -41,
            Self::SettingsObject => -42,
            Self::Directory => -43,
            Self::Root => -44,
            Self::InvalidId => -45,
            Self::Identity => -46,
            Self::Backup => -47,
            Self::Foreign => -48,
            Self::Orphan => -49,
            Self::Recursive => -50,
            Self::UnsupportedType => -51,
            Self::UnsupportedFormat => -52,
            Self::Changed => -53,
            Self::RestoreNeeded => -54,
            Self::LoginReference => -55,
            Self::AuthStatus => -56,
            Self::MissingDirectory => -57,
            #[cfg(target_os = "macos")]
            Self::Foundation(code) => -10000 - i32::try_from(*code).unwrap_or(512),
        }
    }
}

#[cfg(target_os = "macos")]
fn expand(path: &str) -> String {
    objc2_foundation::NSString::from_str(path)
        .stringByExpandingTildeInPath()
        .to_string()
}
#[cfg(target_os = "macos")]
fn canonical(path: &str) -> String {
    use objc2_foundation::{NSString, NSURL};
    let expanded = NSString::from_str(path).stringByExpandingTildeInPath();
    NSURL::fileURLWithPath(&expanded)
        .URLByStandardizingPath()
        .and_then(|url| url.URLByResolvingSymlinksInPath())
        .and_then(|url| url.path())
        .map_or_else(|| expanded.to_string(), |s| s.to_string())
}
#[cfg(not(target_os = "macos"))]
fn expand(path: &str) -> String {
    if (path == "~" || path.starts_with("~/"))
        && let Some(home) = std::env::home_dir()
    {
        return home
            .join(path.get(2..).unwrap_or(""))
            .to_string_lossy()
            .into_owned();
    }
    path.into()
}
#[cfg(not(target_os = "macos"))]
fn canonical(path: &str) -> String {
    let expanded = expand(path);
    let path = Path::new(&expanded);
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.into());
    // Resolve the existing prefix even when the final profile directory doesn't exist yet.
    let resolved = absolute
        .ancestors()
        .find_map(|p| {
            fs::canonicalize(p)
                .ok()
                .map(|c| c.join(absolute.strip_prefix(p).expect("ancestor")))
        })
        .unwrap_or(absolute);
    let path = resolved.to_string_lossy();
    #[cfg(windows)]
    let path = path
        .strip_prefix(r"\\?\")
        .unwrap_or(&path)
        .trim_end_matches('\\')
        .replace('\\', "/");
    path.to_string()
}

fn validated(directory: &str) -> Result<String> {
    let path = expand(directory.trim_matches(|c: char| c.is_whitespace() || c == '\u{200b}'));
    #[cfg(target_os = "macos")]
    let absolute = path.starts_with('/');
    #[cfg(not(target_os = "macos"))]
    let absolute = Path::new(&path).is_absolute();
    if !absolute || Path::new(&path).parent().is_none() {
        return Err(Error::Directory);
    }
    let path = canonical(&path);
    if Path::new(&path).parent().is_none() {
        return Err(Error::Root);
    }
    Ok(path)
}
fn generation() -> String {
    Uuid::new_v4().hyphenated().to_string().to_ascii_uppercase()
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn same(a: Option<&str>, b: Option<&str>) -> bool {
    a == b || a.zip(b).is_some_and(|(a, b)| a.nfc().eq(b.nfc()))
}
fn command(object: Option<&Object>) -> Option<String> {
    config::string(object?, "command")
}
fn profile_folder(root: &Path, id: &str) -> Result<PathBuf> {
    if id.chars().count() != 64
        || !id
            .chars()
            .all(|c| c.is_ascii_hexdigit() || matches!(c, '０'..='９' | 'Ａ'..='Ｆ' | 'ａ'..='ｆ'))
    {
        return Err(Error::InvalidId);
    }
    Ok(root.join(id))
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: String,
    pub config_directory: String,
    pub name: String,
    pub generation: String,
    #[serde(default, with = "backup_data", skip_serializing_if = "Option::is_none")]
    pub original_status_line: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
}
impl Profile {
    fn new(directory: &str, name: String) -> Self {
        let config_directory = canonical(directory);
        Self {
            id: super::digest(&config_directory),
            config_directory,
            name,
            generation: generation(),
            original_status_line: None,
            installed_command: None,
            email: None,
            organization_name: None,
            subscription_type: None,
        }
    }
    fn checked(&self, id: &str) -> Result<()> {
        if self.id != id || super::digest(&canonical(&self.config_directory)) != id {
            return Err(Error::Identity);
        }
        Ok(())
    }
    fn read(path: &Path, id: &str) -> Result<Self> {
        let data = fs::read(path)?;
        let utf8 = keyed::decoder_utf8(&data).ok_or(Error::Json)?;
        let bytes = keyed::trailing_commas(utf8.as_ref());
        let fields: keyed::Object<'_> = serde_json::from_slice(&bytes)?;
        let profile: Self = serde_json::from_slice(&serde_json::to_vec(&fields)?)?;
        profile.checked(id)?;
        Ok(profile)
    }
    fn backup(&self) -> Result<Option<Object>> {
        let Some(data) = &self.original_status_line else {
            return Ok(None);
        };
        config::decode(data)
    }
    pub fn original(&self) -> Option<String> {
        command(self.backup().ok()?.as_ref())
    }
    pub fn load(folder: &Path, id: &str) -> Result<Self> {
        if folder.join(PENDING).try_exists()? {
            let _guard = lock(folder)?;
            recover(folder, id)?;
        }
        Self::read(&folder.join(ACTIVE), id)
    }
    fn save(&self, root: &Path) -> Result<()> {
        self.checked(&self.id)?;
        let folder = profile_folder(root, &self.id)?;
        create_directory(root)?;
        let _guard = lock(&folder)?;
        ensure_private_directory(&folder)?;
        recover(&folder, &self.id)?;
        Ok(write_private(
            &folder.join(ACTIVE),
            &serde_json::to_vec(self)?,
        )?)
    }
}

fn lock(folder: &Path) -> Result<File> {
    // The store lock survives deletion and recreation of an individual profile.
    let path = folder
        .parent()
        .ok_or(Error::InvalidId)?
        .join(".connection.lock");
    let mut options = File::options();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    file.lock()?;
    Ok(file)
}
fn create_directory(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}
// Profile and CLI settings are separate atomic files. Persist the intended profile first,
// then replace the settings. Even the standalone callback can finish an interrupted write.
fn recover(folder: &Path, id: &str) -> Result<()> {
    let pending = folder.join(PENDING);
    let next = match Profile::read(&pending, id) {
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        result => result?,
    };
    let desired = next.installed_command.clone().or_else(|| next.original());
    if Config::read(&next.config_directory)?.matches_command(desired.as_deref()) {
        fs::rename(pending, folder.join(ACTIVE))?;
    } else {
        fs::remove_file(pending)?;
    }
    Ok(())
}
fn commit(folder: &Path, settings: &Config, next: &Profile) -> Result<()> {
    let pending = folder.join(PENDING);
    write_private(&pending, &serde_json::to_vec(next)?)?;
    if let Err(e) = settings.save(&next.config_directory) {
        let _ = fs::remove_file(pending);
        return Err(e);
    }
    Ok(fs::rename(pending, folder.join(ACTIVE))?)
}

pub(crate) struct Installed {
    pub profile: Profile,
    previous: Option<Profile>,
    folder: PathBuf,
    _guard: File,
}
impl Installed {
    pub fn undo(&self) -> Result<()> {
        rollback_locked(&self.folder, &self.profile, self.previous.as_ref())
    }
}

pub(crate) fn install_binding(
    root: &Path,
    directory: &str,
    name: String,
    helper: &str,
    renew: bool,
) -> Result<Installed> {
    let directory = validated(directory)?;
    create_directory(Path::new(&directory))?;
    let mut profile = Profile::new(&directory, name);
    let folder = profile_folder(root, &profile.id)?;
    create_directory(root)?;
    let guard = lock(&folder)?;
    ensure_private_directory(&folder)?;
    recover(&folder, &profile.id)?;
    let mut settings = Config::read(&profile.config_directory)?;
    let previous = if folder.join(ACTIVE).try_exists()? {
        Some(Profile::read(&folder.join(ACTIVE), &profile.id).map_err(|_| Error::Backup)?)
    } else {
        None
    };
    if let Some(p) = &previous {
        let name = profile.name;
        profile = p.clone();
        profile.name = name;
    }
    let mut current = settings.status();
    let command = command(current.as_ref());
    let wrapped = command.as_ref().is_some_and(|c| c.contains(CALLBACK));
    if wrapped && !command.as_ref().is_some_and(|c| c.contains(&profile.id)) {
        return Err(Error::Foreign);
    }
    if wrapped && previous.is_none() {
        return Err(Error::Orphan);
    }
    if wrapped
        || (profile.installed_command.is_some()
            && same(command.as_deref(), profile.installed_command.as_deref()))
    {
        if self::command(profile.backup()?.as_ref()).is_some_and(|c| c.contains(CALLBACK)) {
            return Err(Error::Recursive);
        }
        if renew {
            profile.generation = generation();
        }
    } else {
        profile.original_status_line = match current.as_ref() {
            Some(s)
                if config::string(s, "type").as_deref() == Some("command") && command.is_some() =>
            {
                Some(config::encode(s, false)?)
            }
            Some(_) => return Err(Error::UnsupportedType),
            None if settings.has_status() => return Err(Error::UnsupportedFormat),
            None => None,
        };
        profile.generation = generation();
    }
    let wrapper = [
        helper,
        CALLBACK,
        &profile.id,
        &profile.generation,
        root.to_str().ok_or(Error::Directory)?,
    ]
    .map(quote)
    .join(" ");
    let status = current.get_or_insert_with(Object::new);
    config::set_text(status, "type", "command")?;
    config::set_text(status, "command", &wrapper)?;
    profile.installed_command = Some(wrapper);
    settings.set_status(Some(status))?;
    commit(&folder, &settings, &profile)?;
    Ok(Installed {
        profile,
        previous,
        folder,
        _guard: guard,
    })
}

pub fn install(
    root: &Path,
    directory: &str,
    name: String,
    helper: &str,
    renew: bool,
) -> Result<Profile> {
    Ok(install_binding(root, directory, name, helper, renew)?.profile)
}

fn uninstall_locked(
    folder: &Path,
    mut profile: Profile,
    mut config: Config,
    for_removal: bool,
) -> Result<bool> {
    let mut status = config.status().unwrap_or_default();
    let cmd = command(Some(&status));
    let owned = profile.installed_command.is_some()
        && same(cmd.as_deref(), profile.installed_command.as_deref());
    let original = if owned { profile.backup()? } else { None };
    if !owned || (original.is_none() && status.keys().any(|k| k != "type" && k != "command")) {
        if for_removal && cmd.is_some_and(|c| c.contains(CALLBACK) && c.contains(&profile.id)) {
            return Err(Error::RestoreNeeded);
        }
        return Ok(false);
    }
    if let Some(mut original) = original {
        for key in ["type", "command"] {
            if let Some(v) = original.remove(key) {
                status.insert(key.into(), v);
            } else {
                status.remove(key);
            }
        }
        config.set_status(Some(&status))?;
    } else {
        config.set_status(None)?;
    }
    profile.generation = generation();
    profile.installed_command = None;
    commit(folder, &config, &profile)?;
    Ok(true)
}
pub fn uninstall(root: &Path, id: &str, for_removal: bool) -> Result<bool> {
    let folder = profile_folder(root, id)?;
    let _guard = lock(&folder)?;
    recover(&folder, id)?;
    let profile = Profile::read(&folder.join(ACTIVE), id)?;
    let config = Config::read(&profile.config_directory)?;
    uninstall_locked(&folder, profile, config, for_removal)
}
pub fn rollback(root: &Path, installed: &Profile, previous: Option<&Profile>) -> Result<()> {
    let folder = profile_folder(root, &installed.id)?;
    let _guard = lock(&folder)?;
    recover(&folder, &installed.id)?;
    rollback_locked(&folder, installed, previous)
}
fn rollback_locked(folder: &Path, installed: &Profile, previous: Option<&Profile>) -> Result<()> {
    let active = Profile::read(&folder.join(ACTIVE), &installed.id)?;
    if &active != installed {
        return Ok(());
    }
    let mut config = Config::read(&active.config_directory)?;
    let Some((previous, old_command)) = previous
        .filter(|p| p.id == active.id && p.original_status_line == installed.original_status_line)
        .and_then(|p| Some((p, p.installed_command.as_deref()?)))
    else {
        uninstall_locked(folder, active, config, false)?;
        return Ok(());
    };
    previous.checked(&installed.id)?;
    if config.matches_command(installed.installed_command.as_deref())
        && let Some(mut current) = config.status()
    {
        config::set_text(&mut current, "command", old_command)?;
        config.set_status(Some(&current))?;
        commit(folder, &config, previous)?;
    }
    Ok(())
}

/// Keep the profile lock through the account settings write. If that write fails,
/// restore the same wrapper and generation, provided the user still owns the restored command.
pub(crate) struct Removal {
    profile: Profile,
    wrapper: Option<Object>,
    folder: PathBuf,
    guard: File,
}
impl Removal {
    pub fn prepare(root: &Path, id: &str) -> Result<Self> {
        let folder = profile_folder(root, id)?;
        let guard = lock(&folder)?;
        recover(&folder, id)?;
        let profile = Profile::read(&folder.join(ACTIVE), id)?;
        let settings = Config::read(&profile.config_directory)?;
        let wrapper = settings.status();
        let restored = uninstall_locked(&folder, profile.clone(), settings, true)?;
        Ok(Self {
            profile,
            wrapper: if restored { wrapper } else { None },
            folder,
            guard,
        })
    }
    pub fn undo(&self) -> Result<()> {
        let Some(wrapper) = &self.wrapper else {
            return Ok(());
        };
        let mut config = Config::read(&self.profile.config_directory)?;
        let current = config.status();
        let original = self.profile.backup()?;
        if original.is_some() != current.is_some()
            || (original.is_none() && config.has_status())
            || !same(
                command(current.as_ref()).as_deref(),
                command(original.as_ref()).as_deref(),
            )
        {
            return Ok(());
        }
        let mut restored = current.unwrap_or_default();
        for key in ["type", "command"] {
            if let Some(v) = wrapper.get(key) {
                restored.insert(key.into(), v.clone());
            } else {
                restored.remove(key);
            }
        }
        config.set_status(Some(&restored))?;
        commit(&self.folder, &config, &self.profile)
    }
    pub fn finish(self) {
        // Detach the specific private directory before releasing its lock and deleting it.
        let trash = self
            .folder
            .with_file_name(format!(".removed-{}", Uuid::new_v4()));
        if fs::rename(&self.folder, &trash).is_ok() {
            drop(self.guard);
            let _ = fs::remove_dir_all(trash);
        }
    }
}
