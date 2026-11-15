//! Credential changes are committed on the monitor's owner thread. Revoking a
//! pending rotation before replacing its vault entry keeps old replies from being saved.
use super::{Error, Settings, guid};
use crate::{
    credentials::{self, Platform, Provider},
    files, vault,
};
use std::{io, path::Path};
use uuid::Uuid;
use zeroize::Zeroizing;

fn vault_error(_: io::Error) -> Error {
    Error::Io(io::Error::other(
        "인증 파일을 보호하거나 열 수 없습니다. 다시 연결하세요.",
    ))
}

fn read_source(path: &Path) -> Result<Zeroizing<Vec<u8>>, Error> {
    files::reject_links(path)
        .map_err(|e| Error::File(e, "앱 데이터 경로에 링크를 사용할 수 없습니다."))?;
    files::read_file(path, vault::MAX_SIZE)
        .map(Zeroizing::new)
        .map_err(|e| Error::File(e, "파일을 읽을 수 없거나 크기 제한을 초과했습니다."))
}

pub enum Source<'a> {
    File { provider: Provider, path: &'a str },
    CodexLogin(&'a [u8]),
}

pub fn connect(
    root: &Path,
    mut settings: Settings,
    source: Source<'_>,
    revoke: impl FnOnce(Uuid) -> Result<(), Error>,
) -> Result<Vec<u8>, Error> {
    let imported;
    let (provider, path, managed, credential) = match source {
        Source::File {
            provider: Provider::Claude,
            ..
        } => {
            return Err(Error::Usage(
                "provider",
                "Claude는 설정 폴더의 status line으로 연결하세요.",
            ));
        }
        Source::File { provider, path } => {
            if !Path::new(path).is_absolute() || path.starts_with(r"\\") {
                return Err(Error::Usage("path", "Windows 로컬 파일을 선택하세요."));
            }
            let path = std::path::absolute(path)?;
            imported = read_source(&path)?;
            (
                provider,
                path.to_string_lossy().into_owned(),
                false,
                imported.as_slice(),
            )
        }
        Source::CodexLogin(bytes) => (Provider::Codex, String::new(), true, bytes),
    };
    let parsed =
        credentials::parse(credential, provider, Platform::Windows).map_err(Error::Credential)?;
    let existing = settings.connect(
        provider,
        parsed.label.clone(),
        parsed.identity.clone(),
        path,
        managed,
    )?;
    let id = guid(
        &settings
            .accounts
            .last()
            .expect("connect appends the account")
            .id,
    )?;
    let directory = root.join("Credentials");
    let previous = if existing {
        vault::load(&directory, id).map_err(vault_error)?
    } else {
        None
    };
    revoke(id)?;
    vault::save(&directory, id, credential).map_err(vault_error)?;
    match settings.save(root) {
        Ok(settings) => Ok(settings),
        Err(error) => {
            match previous {
                Some(bytes) => vault::save(&directory, id, &bytes).map_err(vault_error)?,
                None => vault::delete(&directory, id).map_err(vault_error)?,
            }
            Err(error)
        }
    }
}

pub fn imported(
    root: &Path,
    id: Uuid,
    provider: Provider,
    path: &str,
    identity: &str,
) -> Result<Zeroizing<Vec<u8>>, Error> {
    let directory = root.join("Credentials");
    let path = Path::new(path);
    let from_source = path.is_file();
    let bytes = if from_source {
        read_source(&std::path::absolute(path)?)?
    } else {
        vault::load(&directory, id)
            .map_err(vault_error)?
            .ok_or(Error::Usage(
                "missing_credentials",
                "로그인 파일을 다시 연결하세요.",
            ))?
    };
    let parsed =
        credentials::parse(&bytes, provider, Platform::Windows).map_err(Error::Credential)?;
    if parsed.identity != identity {
        return Err(Error::Usage(
            "account_mismatch",
            if from_source {
                "CLI에 다른 계정이 로그인되어 있습니다. 기존 계정과 섞지 않으려면 다시 연결하세요."
            } else {
                "인증 계정이 일치하지 않습니다."
            },
        ));
    }
    if from_source {
        vault::save(&directory, id, &bytes).map_err(vault_error)?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn managed_login_revokes_pending_rotation_before_replacing_the_saved_credential() {
        let root = std::env::temp_dir().join(format!("quotabar-managed-login-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let directory = root.join("Credentials");
        let login = br#"{"access_token":"old","account_id":"a","user_id":"u"}"#;
        let rotated = br#"{"access_token":"new","account_id":"a","user_id":"u"}"#;
        let first = connect(
            &root,
            Settings::default(),
            Source::CodexLogin(login),
            |id| {
                assert!(vault::load(&directory, id).unwrap().is_none());
                Ok(())
            },
        )
        .unwrap();
        let mut settings = Settings::read(&first).unwrap();
        let id = guid(&settings.accounts[0].id).unwrap();
        settings
            .edit(&id.to_string(), "내 계정", false, false)
            .unwrap();
        settings.save(&root).unwrap();
        let revoke = |candidate| {
            assert_eq!(candidate, id);
            assert_eq!(
                vault::load(&directory, id).unwrap().unwrap().as_slice(),
                login
            );
        };
        let aborted = connect(
            &root,
            Settings::load(&root).unwrap(),
            Source::CodexLogin(rotated),
            |id| {
                revoke(id);
                Err(Error::Usage("cancelled", "cancelled"))
            },
        );
        assert!(matches!(aborted, Err(Error::Usage("cancelled", _))));
        let current = connect(&root, settings, Source::CodexLogin(rotated), |id| {
            revoke(id);
            Ok(())
        })
        .unwrap();
        let state = Settings::read(&current).unwrap();
        let [account] = state.accounts.as_slice() else {
            panic!("unexpected account count")
        };
        assert_eq!(account.id, id.to_string());
        assert_eq!(account.name, "내 계정");
        assert!(account.app_managed_login && !account.enabled && !account.show_in_tray);
        assert_eq!(account.source_path.as_deref(), Some(""));
        assert_eq!(
            vault::load(&directory, id).unwrap().unwrap().as_slice(),
            rotated
        );
        fs::remove_dir_all(root).unwrap();
    }
}
