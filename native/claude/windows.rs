use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::os::windows::process::CommandExt;
use windows_sys::Win32::Globalization::{LCMAP_UPPERCASE, LCMapStringEx};
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
    const KEYS: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "CLAUDE_CODE_USE_BEDROCK",
        "CLAUDE_CODE_USE_VERTEX",
        "CLAUDE_CODE_USE_FOUNDRY",
    ];
    KEYS.iter().any(|key| {
        std::env::var(key).is_ok_and(|value| {
            !value.trim().is_empty()
                && (!key.starts_with("CLAUDE_CODE_USE_")
                    || value == "1"
                    || value.eq_ignore_ascii_case("true"))
        })
    })
}

/// .NET uses invariant, simple uppercase for the canonical Windows directory.
/// Map one character at a time and discard expansions (for example ß -> SS).
fn profile_hash(directory: &str) -> io::Result<String> {
    use std::path::{Component, Prefix};
    let path = Path::new(directory);
    if !path.is_absolute()
        || !matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
    {
        return Err(invalid("Expected a local drive path"));
    }
    let absolute = std::path::absolute(path)?;
    if absolute.parent().is_none() {
        return Err(invalid("Invalid profile root"));
    }
    crate::files::reject_links(&absolute)?;
    let directory = absolute
        .to_str()
        .ok_or_else(|| invalid("Invalid directory"))?
        .trim_end_matches(['\\', '/']);
    let mut result = String::new();
    for ch in directory.chars() {
        if ch.is_ascii() {
            result.push(ch.to_ascii_uppercase());
            continue;
        }
        // Windows invariant casing leaves the Turkish dotless i unchanged in .NET.
        if ch == '\u{131}' {
            result.push(ch);
            continue;
        }
        let mut source = [0u16; 2];
        let source = ch.encode_utf16(&mut source);
        let mut dest = [0u16; 8];
        let len = unsafe {
            LCMapStringEx(
                [0].as_ptr(),
                LCMAP_UPPERCASE,
                source.as_ptr(),
                source.len() as i32,
                dest.as_mut_ptr(),
                dest.len() as i32,
                std::ptr::null(),
                std::ptr::null(),
                0,
            )
        };
        if len <= 0 {
            return Err(io::Error::last_os_error());
        }
        let mut mapped = char::decode_utf16(dest[..len as usize].iter().copied());
        let upper = mapped
            .next()
            .and_then(Result::ok)
            .filter(|_| mapped.next().is_none())
            .unwrap_or(ch);
        result.push(upper);
    }
    Ok(digest(&result))
}

#[derive(Deserialize)]
pub(super) struct Profile {
    id: String,
    directory: String,
    pub(super) generation: String,
    original: Option<serde_json::Value>,
    shell: String,
}

impl Profile {
    pub(super) fn load(folder: &Path, id: &str) -> io::Result<Self> {
        let profile: Self =
            serde_json::from_slice(&read_file(&folder.join("profile.json"), 1_048_576)?)?;
        if profile.id != id || profile_hash(&profile.directory)? != id {
            return Err(invalid("Profile identity changed"));
        }
        Ok(profile)
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
struct QuotaWindow {
    title: String,
    remaining: f64,
    reset_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    profile_id: String,
    generation: String,
    session_id: String,
    received_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    windows: Vec<QuotaWindow>,
}

pub(super) fn capture(
    folder: &Path,
    profile: &Profile,
    session: &str,
    limits: &Limits,
) -> io::Result<()> {
    let mut windows = Vec::new();
    for (title, item) in [("5시간", &limits.five_hour), ("주간", &limits.seven_day)] {
        if let Some(item) = item {
            if item.resets_at >= 253_402_300_799.0 {
                return Err(invalid("Invalid reset time"));
            }
            let reset_at = DateTime::from_timestamp(item.resets_at as i64, 0)
                .ok_or_else(|| invalid("Invalid reset time"))?;
            windows.push(QuotaWindow {
                title: title.to_owned(),
                remaining: 100.0 - item.used_percentage,
                reset_at,
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
