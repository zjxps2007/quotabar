use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::os::windows::process::CommandExt;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

pub(super) fn root(arguments: &[OsString]) -> io::Result<PathBuf> {
    if arguments.len() != 4 {
        return Err(invalid("Invalid status line arguments"));
    }
    Ok(Path::new(&arguments[3]).join("ClaudeProfiles"))
}

pub(super) fn read_input() -> io::Result<Vec<u8>> {
    crate::files::read_bounded(io::stdin().lock(), MAX_INPUT)
}

pub(super) fn valid_session(session: &str) -> bool {
    let session = session.trim();
    let text = session
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(session);
    Uuid::parse_str(text).is_ok() && !text.starts_with("urn:")
}

pub(super) fn routed() -> bool {
    routed_with(&serde_json::Value::Null)
}

pub const API_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
];

pub(super) fn routed_with(config: &serde_json::Value) -> bool {
    API_KEYS.iter().any(|key| {
        let enabled = |value: &str| {
            !value.trim().is_empty()
                && (!key.starts_with("CLAUDE_CODE_USE_")
                    || value == "1"
                    || value.eq_ignore_ascii_case("true"))
        };
        let value = &config[*key];
        let configured = match value {
            serde_json::Value::Null => false,
            serde_json::Value::String(s) => enabled(s),
            v => enabled(&v.to_string()),
        };
        configured || std::env::var(key).is_ok_and(|v| enabled(&v))
    })
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub directory: String,
    pub generation: String,
    #[serde(default)]
    pub command: String,
    pub original: Option<serde_json::Value>,
    pub shell: String,
}

impl Profile {
    pub(super) fn load(folder: &Path, id: &str) -> io::Result<Self> {
        connection::load_folder(folder, id)
    }
    pub(super) fn original(&self) -> Option<String> {
        Some(self.original.as_ref()?.get("command")?.as_str()?.to_owned())
    }
    pub(super) fn shell(&self, command: &str) -> Command {
        let mut process;
        if self.shell == "powershell" {
            let bytes: Vec<_> = command.encode_utf16().flat_map(u16::to_le_bytes).collect();
            process = Command::new("powershell.exe");
            process
                .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
                .arg(STANDARD.encode(bytes));
        } else {
            process = Command::new(&self.shell);
            process.arg("-c").arg(command);
        }
        process.creation_flags(CREATE_NO_WINDOW);
        process
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub title: String,
    pub remaining: f64,
    pub reset_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub profile_id: String,
    pub generation: String,
    pub session_id: String,
    pub received_at: DateTime<Utc>,
    pub observed_at: DateTime<Utc>,
    pub windows: Vec<QuotaWindow>,
}

pub(super) fn capture(
    folder: &Path,
    profile: &Profile,
    session: &str,
    limits: &Limits,
) -> io::Result<()> {
    let _lock = connection::lock(folder)?;
    // A callback can have read the old profile just before a reinstallation.
    if connection::active(folder, &profile.id)?.generation != profile.generation {
        return Ok(());
    }
    let mut windows = Vec::new();
    for (title, item) in [("5시간", &limits.five_hour), ("주간", &limits.seven_day)] {
        if let Some(item) = item {
            if item.resets_at >= 253_402_300_799.0 {
                return Ok(());
            }
            let reset_at = DateTime::from_timestamp(item.resets_at as i64, 0)
                .ok_or_else(|| invalid("Invalid reset time"))?;
            windows.push(QuotaWindow {
                title: title.to_owned(),
                remaining: 100.0 - item.used_percentage,
                reset_at: Some(reset_at),
            });
        }
    }
    if windows.is_empty() {
        return Ok(());
    }
    // DateTimeOffset stores 100-nanosecond ticks in the existing snapshot format.
    let now = Utc::now();
    let now = DateTime::from_timestamp(now.timestamp(), now.timestamp_subsec_nanos() / 100 * 100)
        .unwrap();
    let path = folder.join(format!("snapshot-{session}.json"));
    let previous = read_file(&path, MAX_SNAPSHOT)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Snapshot>(&bytes).ok());
    let observed_at = previous
        .filter(|s| s.generation == profile.generation && s.windows == windows)
        .map_or(now, |s| s.observed_at);
    let sample = Snapshot {
        profile_id: profile.id.clone(),
        generation: profile.generation.clone(),
        session_id: session.to_owned(),
        received_at: now,
        observed_at,
        windows,
    };
    write_private(&path, &serde_json::to_vec(&sample)?)?;
    prune(folder)
}

pub(super) fn display(_: &Path, profile: &Profile, generation: &str, _: bool) -> io::Result<()> {
    let message = if profile.generation == generation {
        "quota connected"
    } else {
        "reconnect quota feedback"
    };
    write!(io::stdout().lock(), "Claude · {message}")
}
