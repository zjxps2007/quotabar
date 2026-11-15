use super::mac_connection::AUTH_KEYS;
pub(super) use super::mac_connection::Profile;
pub(super) use super::mac_snapshots::valid_session;
use super::mac_snapshots::{Snapshot, now};
use super::*;
use std::io::Read;

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

pub(super) fn routed() -> bool {
    AUTH_KEYS
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|v| !v.is_empty()))
}

impl Profile {
    pub(super) fn shell(&self, command: &str) -> Command {
        let mut process = Command::new("/bin/sh");
        process.arg("-c").arg(command);
        process
    }
}

pub(super) fn capture(
    folder: &Path,
    profile: &Profile,
    session: &str,
    limits: &Limits,
) -> io::Result<()> {
    Snapshot::from_limits(&profile.id, &profile.generation, session, limits, now()).save(folder)
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
    let latest = Snapshot::latest(folder, &profile.id, generation, time);
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
