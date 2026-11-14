use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use objc2_foundation::{NSString, NSURL};
use serde::Serialize;
use std::io::Read;

// Foundation's default Codable Date is seconds since 2001-01-01 UTC.
const FOUNDATION_EPOCH: f64 = 978_307_200.0;

fn now() -> f64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    unix.as_secs_f64() - FOUNDATION_EPOCH
}

pub(super) fn root(arguments: &[OsString]) -> io::Result<PathBuf> {
    if let Some(path) = arguments.get(3) {
        return Ok(path.into());
    }
    std::env::home_dir()
        .map(|home| home.join("Library/Application Support/AIUsage Lite/ClaudeProfiles"))
        .ok_or_else(|| invalid("Missing home directory"))
}

pub(super) fn read_input() -> io::Result<Vec<u8>> {
    // The callback size cap applies to capture. The original shell command still gets its complete input.
    let mut bytes = Vec::new();
    io::stdin().lock().read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn valid_session(session: &str) -> bool {
    session.len() == 36 && Uuid::parse_str(session).is_ok()
}

pub(super) fn routed() -> bool {
    const KEYS: &[&str] = &[
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
    KEYS.iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Profile {
    id: String,
    config_directory: String,
    #[serde(rename = "name")]
    _name: String,
    pub(super) generation: String,
    original_status_line: Option<String>,
}

impl Profile {
    pub(super) fn load(folder: &Path, id: &str) -> io::Result<Self> {
        let profile: Self = serde_json::from_slice(&fs::read(folder.join("profile.json"))?)?;
        let expanded = NSString::from_str(&profile.config_directory).stringByExpandingTildeInPath();
        let path = NSURL::fileURLWithPath(&expanded)
            .URLByStandardizingPath()
            .and_then(|u| u.URLByResolvingSymlinksInPath())
            .and_then(|u| u.path())
            .ok_or_else(|| invalid("Invalid profile directory"))?;
        if profile.id != id || digest(&path.to_string()) != id {
            return Err(invalid("Profile identity changed"));
        }
        if let Some(text) = &profile.original_status_line {
            STANDARD.decode(text).map_err(io::Error::other)?;
        }
        Ok(profile)
    }
    pub(super) fn original(&self) -> Option<String> {
        let bytes = STANDARD.decode(self.original_status_line.as_ref()?).ok()?;
        let object: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        Some(object.get("command")?.as_str()?.to_owned())
    }
    pub(super) fn shell(&self, command: &str) -> Command {
        let mut process = Command::new("/bin/sh");
        process.arg("-c").arg(command);
        process
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct QuotaWindow {
    used_percent: f64,
    reset_at: f64,
}

impl From<&Window> for QuotaWindow {
    fn from(value: &Window) -> Self {
        Self {
            used_percent: value.used_percentage,
            reset_at: value.resets_at - FOUNDATION_EPOCH,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    schema_version: u32,
    #[serde(rename = "profileID")]
    profile_id: String,
    generation: String,
    #[serde(rename = "sessionID")]
    session_id: String,
    received_at: f64,
    observed_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    five_hour: Option<QuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seven_day: Option<QuotaWindow>,
}

fn read_snapshot(path: &Path) -> Option<Snapshot> {
    serde_json::from_slice(&read_file(path, MAX_SNAPSHOT).ok()?).ok()
}

pub(super) fn capture(
    folder: &Path,
    profile: &Profile,
    session: &str,
    limits: &Limits,
) -> io::Result<()> {
    let five_hour = limits.five_hour.as_ref().map(QuotaWindow::from);
    let seven_day = limits.seven_day.as_ref().map(QuotaWindow::from);
    let received_at = now();
    let path = folder.join(format!("snapshot-{session}.json"));
    let observed_at = read_snapshot(&path)
        .filter(|s| {
            s.generation == profile.generation
                && s.five_hour == five_hour
                && s.seven_day == seven_day
        })
        .map_or(received_at, |s| s.observed_at);
    let sample = Snapshot {
        schema_version: 1,
        profile_id: profile.id.clone(),
        generation: profile.generation.clone(),
        session_id: session.to_owned(),
        received_at,
        observed_at,
        five_hour,
        seven_day,
    };
    write_private(&path, &serde_json::to_vec(&sample)?)?;
    prune(folder)
}

pub(super) fn display(
    folder: &Path,
    profile: &Profile,
    generation: &str,
    routed: bool,
) -> io::Result<()> {
    let mut output = io::stdout().lock();
    if routed {
        return writeln!(output, "Claude · API / proxy session");
    }
    if profile.generation != generation {
        return writeln!(output, "Claude · reconnect quota feedback");
    }
    let time = now();
    let latest = snapshot_files(folder)
        .unwrap_or_default()
        .iter()
        .filter_map(|p| read_snapshot(p))
        .filter(|s| {
            s.schema_version == 1
                && s.profile_id == profile.id
                && s.generation == generation
                && s.observed_at <= time + 60.0
                && (s.five_hour.is_some() || s.seven_day.is_some())
        })
        .max_by(|a, b| a.observed_at.total_cmp(&b.observed_at));
    let values: Vec<_> = latest
        .into_iter()
        .flat_map(|s| [("5h", s.five_hour), ("7d", s.seven_day)])
        .filter_map(|(name, w)| {
            w.filter(|w| w.reset_at > time)
                .map(|w| format!("{name} {:.0}%", (100.0 - w.used_percent).round()))
        })
        .collect();
    if values.is_empty() {
        writeln!(output, "Claude · waiting for quota")
    } else {
        writeln!(output, "{}", values.join(" · "))
    }
}
