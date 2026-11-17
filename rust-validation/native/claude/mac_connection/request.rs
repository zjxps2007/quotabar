use super::*;
use serde_json::{json, to_vec};

const MODEL_KEYS: &[&str] = &[
    "ANTHROPIC_MODEL",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_DEFAULT_FABLE_MODEL",
    "ANTHROPIC_DEFAULT_OPUS_MODEL",
    "ANTHROPIC_DEFAULT_SONNET_MODEL",
    "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    "ANTHROPIC_CUSTOM_MODEL_OPTION",
    "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Request {
    Reference {
        directory: String,
        name: String,
    },
    Validate {
        directory: String,
    },
    Folder {
        id: String,
    },
    Load {
        id: String,
    },
    Save {
        profile: Profile,
    },
    Connected {},
    Install {
        directory: String,
        name: String,
        helper: String,
        renew: bool,
    },
    Uninstall {
        id: String,
        for_removal: bool,
    },
    Rollback {
        installed: Profile,
        previous: Option<Box<Profile>>,
    },
    Preserve {
        directory: String,
        active_object: bool,
        incoming_command: Option<String>,
    },
    Routes {
        directory: String,
    },
    Fetch {
        directory: String,
        auth_method: String,
        source_kind: Option<String>,
        now: f64,
    },
    Plan {
        raw: Option<String>,
    },
    AuthStatus {
        data: String,
    },
    Quote {
        value: String,
    },
    Keys {
        model: bool,
    },
    SettingsOverride {},
    DefaultDirectory {
        directory: String,
        home: String,
    },
    Terminal {
        directory: String,
        home: String,
    },
}
fn default_directory(directory: &str, home: &str) -> bool {
    let default = Path::new(home).join(".claude");
    same(
        Some(&canonical(directory)),
        Some(&canonical(&default.to_string_lossy())),
    )
}
fn connected(root: &Path) -> Vec<Profile> {
    let mut profiles: Vec<_> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let id = name.to_str()?;
            Profile::load(&profile_folder(root, id).ok()?, id).ok()
        })
        .filter(|p| p.installed_command.is_some())
        .collect();
    profiles.sort_by(|a, b| a.id.cmp(&b.id));
    profiles
}
fn preserve(
    root: &Path,
    directory: &str,
    active_object: bool,
    incoming: Option<&str>,
) -> Result<bool> {
    let id = super::super::digest(&canonical(directory));
    let folder = profile_folder(root, &id)?;
    let p = match Profile::load(&folder, &id) {
        Err(Error::Io(e)) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        result => result?,
    };
    Ok((p.installed_command.is_some() && active_object)
        || incoming.is_some_and(|cmd| cmd.contains(CALLBACK) && cmd.contains(&id)))
}
pub fn run(root: &Path, request: &[u8]) -> Result<Vec<u8>> {
    let request: Request = serde_json::from_slice(request)?;
    Ok(match request {
        Request::Reference { directory, name } => to_vec(&Profile::new(&directory, name))?,
        Request::Validate { directory } => to_vec(&validated(&directory)?)?,
        Request::Folder { id } => to_vec(&profile_folder(root, &id)?)?,
        Request::Load { id } => to_vec(&Profile::load(&profile_folder(root, &id)?, &id)?)?,
        Request::Save { profile } => {
            profile.save(root)?;
            to_vec(&true)?
        }
        Request::Connected {} => to_vec(&connected(root))?,
        Request::Install {
            directory,
            name,
            helper,
            renew,
        } => to_vec(&install(root, &directory, name, &helper, renew)?)?,
        Request::Uninstall { id, for_removal } => to_vec(&uninstall(root, &id, for_removal)?)?,
        Request::Rollback {
            installed,
            previous,
        } => {
            rollback(root, &installed, previous.as_deref())?;
            to_vec(&true)?
        }
        Request::Preserve {
            directory,
            active_object,
            incoming_command,
        } => to_vec(&preserve(
            root,
            &directory,
            active_object,
            incoming_command.as_deref(),
        )?)?,
        Request::Routes { directory } => {
            to_vec(&Config::read(&directory).is_ok_and(|c| c.routes_through_api()))?
        }
        Request::Fetch {
            directory,
            auth_method,
            source_kind,
            now,
        } => {
            if auth_method != "auto" || source_kind.as_deref() != Some("claude-cli-profile") {
                return Err(Error::LoginReference);
            }
            to_vec(&usage::fetch(root, &directory, now)?)?
        }
        Request::Plan { raw } => to_vec(&usage::plan(raw.as_deref()))?,
        Request::AuthStatus { data } => {
            if data.len() > 65536_usize.div_ceil(3) * 4 {
                return Err(Error::AuthStatus);
            }
            to_vec(&usage::auth_status(
                &STANDARD.decode(data).map_err(|_| Error::Json)?,
            )?)?
        }
        Request::Quote { value } => to_vec(&quote(&value))?,
        Request::Keys { model } => to_vec(if model { MODEL_KEYS } else { AUTH_KEYS })?,
        Request::SettingsOverride {} => {
            let env: std::collections::BTreeMap<_, _> = AUTH_KEYS
                .iter()
                .chain(MODEL_KEYS)
                .map(|k| (k, ""))
                .collect();
            let fields: Object = serde_json::from_slice(&to_vec(
                &json!({"env":env, "apiKeyHelper":"", "model":"sonnet"}),
            )?)?;
            to_vec(&String::from_utf8(config::encode(&fields, false)?).map_err(|_| Error::Json)?)?
        }
        Request::DefaultDirectory { directory, home } => {
            to_vec(&default_directory(&directory, &home))?
        }
        Request::Terminal { directory, home } => {
            let cmd = if default_directory(&directory, &home) {
                "claude".into()
            } else {
                format!("CLAUDE_CONFIG_DIR={} claude", quote(&directory))
            };
            to_vec(&cmd)?
        }
    })
}
