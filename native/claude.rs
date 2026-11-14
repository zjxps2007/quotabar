// The callback and stored data formats derive from sylearn/AIUsage and QuotaBar. See NOTICE.
use crate::files::{invalid, read_file, write_private};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use uuid::Uuid;

#[cfg(target_os = "macos")]
#[path = "claude/macos.rs"]
mod platform;
#[cfg(windows)]
#[path = "claude/windows.rs"]
mod platform;

const MAX_INPUT: usize = 2 * 1024 * 1024;
const MAX_SNAPSHOT: usize = 8192;

#[derive(Deserialize)]
struct Input {
    session_id: String,
    rate_limits: Option<Limits>,
}

#[derive(Deserialize)]
struct Limits {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
}

#[derive(Deserialize)]
struct Window {
    used_percentage: f64,
    resets_at: f64,
}

fn parse_input(bytes: &[u8]) -> io::Result<(String, Limits)> {
    if bytes.len() > MAX_INPUT {
        return Err(invalid("Oversized Claude callback"));
    }
    let input: Input = serde_json::from_slice(bytes)?;
    let session = input.session_id.as_str();
    if !platform::valid_session(session) {
        return Err(invalid("Invalid session ID"));
    }
    let limits = input
        .rate_limits
        .ok_or_else(|| invalid("Missing rate limits"))?;
    for item in [&limits.five_hour, &limits.seven_day].into_iter().flatten() {
        if !(0.0..=100.0).contains(&item.used_percentage) || item.resets_at <= 0.0 {
            return Err(invalid("Invalid quota window"));
        }
    }
    Ok((input.session_id, limits))
}

fn digest(path: &str) -> String {
    format!("{:x}", Sha256::digest(path.as_bytes()))
}

fn snapshot_files(folder: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("snapshot-") && name.ends_with(".json") && entry.file_type()?.is_file()
        {
            files.push(entry.path());
        }
    }
    Ok(files)
}

fn prune(folder: &Path) -> io::Result<()> {
    let mut files = snapshot_files(folder)?;
    if files.len() > 20 {
        files.sort_by_cached_key(|file| {
            std::cmp::Reverse(fs::metadata(file).and_then(|m| m.modified()).ok())
        });
        for file in &files[20..] {
            let _ = fs::remove_file(file);
        }
    }
    Ok(())
}

fn forward(mut command: Command, input: &[u8]) -> io::Result<i32> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(input);
    }
    let status = child.wait()?;
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        Ok(status.code().or_else(|| status.signal()).unwrap_or(1))
    }
    #[cfg(windows)]
    Ok(status.code().unwrap_or(1))
}

pub fn run(arguments: &[OsString]) -> io::Result<i32> {
    if arguments.first().and_then(|arg| arg.to_str()) != Some("--claude-statusline") {
        return Err(invalid("Expected a status line invocation"));
    }
    let id = arguments
        .get(1)
        .and_then(|a| a.to_str())
        .ok_or_else(|| invalid("Missing profile ID"))?;
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("Invalid profile ID"));
    }
    let generation = arguments
        .get(2)
        .and_then(|a| a.to_str())
        .ok_or_else(|| invalid("Missing generation"))?;
    let folder = platform::root(arguments)?.join(id);
    let profile = platform::Profile::load(&folder, id)?;
    let bytes = platform::read_input()?;
    let routed = platform::routed();
    if profile.generation == generation
        && !routed
        && let Ok((session, limits)) = parse_input(&bytes)
    {
        let _ = platform::capture(&folder, &profile, &session, &limits);
    }
    if let Some(command) = profile.original() {
        if command.contains("--claude-statusline") {
            #[cfg(target_os = "macos")]
            println!("Claude · reconnect quota feedback");
            return Ok(1);
        }
        return forward(profile.shell(&command), &bytes);
    }
    platform::display(&folder, &profile, generation, routed)?;
    Ok(0)
}
