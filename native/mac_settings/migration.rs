//! One-time import of pre-0.2.1 macOS logins. The disk marker and the
//! identity-checked replacement must both exist before retiring a keychain item.
use super::{Account, Error, Settings, same_identity};
use crate::{
    codex_sessions::Secret,
    credentials::{self, Platform, Provider},
    files, vault,
};
use std::{fs, path::Path};
use uuid::Uuid;

#[cfg(target_os = "macos")]
pub mod keychain;

pub trait LegacyItems {
    fn read(&mut self, id: Uuid) -> Option<Secret>;
    fn delete(&mut self, id: Uuid);
}

fn matches(account: &Account, data: &[u8]) -> bool {
    credentials::parse(data, account.provider, Platform::Mac)
        .is_ok_and(|login| same_identity(&login.identity, &account.identity))
}

fn cli_login(account: &Account) -> Option<Secret> {
    if account.provider == Provider::Codex && account.app_managed_login == Some(true)
        || account.source_path.is_empty()
    {
        return None;
    }
    let source = Path::new(&account.source_path);
    // Follow the user's existing CLI symlink, and bound the read even if the
    // file changes after metadata is read.
    let size = fs::metadata(source).ok()?;
    if !size.is_file() || size.len() > vault::MAX_SIZE as u64 {
        return None;
    }
    files::read_file(source, vault::MAX_SIZE)
        .ok()
        .map(Secret::new)
        .filter(|data| matches(account, data))
}

fn migrate(
    settings: &mut Settings,
    path: &Path,
    directory: &Path,
    legacy: &mut dyn LegacyItems,
) -> Result<(), Error> {
    if settings.credential_storage_version == Some(1) {
        return Ok(());
    }
    let mut migrated = Vec::new();
    for account in &settings.accounts {
        if account.provider == Provider::Claude {
            continue;
        }
        let id = account.id.0;
        let stored = || vault::load(directory, id).map_err(|_| Error::Vault);
        if stored()?.is_none() {
            let data = legacy
                .read(id)
                .filter(|data| matches(account, data))
                .or_else(|| cli_login(account));
            if let Some(data) = data {
                vault::save(directory, id, &data).map_err(|_| Error::Vault)?;
            }
        }
        if stored()?.is_some_and(|data| matches(account, &data)) {
            migrated.push(id);
        }
    }
    settings.credential_storage_version = Some(1);
    settings.save(path)?;
    for id in migrated {
        legacy.delete(id);
    }
    Ok(())
}

/// The Swift compatibility entry takes the settings already accepted by its
/// caller, including account edits that have not yet reached the file.
pub fn run(
    input: &[u8],
    path: &Path,
    directory: &Path,
    legacy: &mut dyn LegacyItems,
) -> Result<Vec<u8>, Error> {
    let mut settings: Settings = serde_json::from_slice(input)?;
    migrate(&mut settings, path, directory, legacy)?;
    Ok(serde_json::to_vec(&settings)?)
}
