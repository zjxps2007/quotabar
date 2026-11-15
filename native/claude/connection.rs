//! Windows Claude Code connection and the profile format shared with installed callbacks.
pub use super::platform::{API_KEYS, Profile, QuotaWindow, Snapshot};
use super::{MAX_SNAPSHOT, digest, parse_input, platform, snapshot_files};
use crate::files::{
    ensure_private_directory, invalid, read_bounded, read_file, reject_links, write_private,
};
use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{
    fs::{self, File},
    io::{self, Write},
    os::windows::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf, Prefix},
};
use uuid::Uuid;
use windows_sys::Win32::{
    Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, MoveFileW},
    UI::Shell::{FOLDERID_LocalAppData, FOLDERID_ProgramFiles},
};

const MAX_CONFIG: usize = 1_048_576;
const ACTIVE: &str = "profile.json";
const PENDING: &str = "profile-pending.json";
const CALLBACK: &str = "--claude-statusline";

pub fn canonical(path: &Path) -> io::Result<String> {
    if !path.is_absolute()
        || !matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows 로컬 절대경로를 선택하세요. WSL/네트워크 경로는 아직 지원하지 않습니다.",
        ));
    }
    let absolute = std::path::absolute(path)?;
    if absolute.parent().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "드라이브 루트는 설정 폴더로 사용할 수 없습니다.",
        ));
    }
    reject_links(&absolute)?;
    Ok(absolute
        .to_str()
        .ok_or_else(|| invalid("Invalid profile path"))?
        .trim_end_matches(['\\', '/'])
        .into())
}

pub fn profile_id(directory: &str) -> String {
    digest(&crate::windows_format::invariant_uppercase(directory))
}

pub fn bash() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| {
            crate::windows_format::known_folder(&FOLDERID_ProgramFiles)
                .map(|p| p.join("Git/bin/bash.exe"))
                .filter(|p| p.is_file())
        })
        .or_else(|| {
            crate::windows_format::known_folder(&FOLDERID_LocalAppData)
                .map(|p| p.join("Programs/Git/bin/bash.exe"))
                .filter(|p| p.is_file())
        })
}

pub fn quote(text: &str, bash: bool) -> String {
    format!(
        "'{}'",
        text.replace('\'', if bash { "'\"'\"'" } else { "''" })
    )
}

fn wrapper(
    helper: &Path,
    root: &Path,
    id: &str,
    generation: &str,
    shell: &str,
) -> io::Result<String> {
    let executable = std::path::absolute(helper)?;
    let slash = |p: &Path| {
        p.to_str()
            .map(|s| s.replace('\\', "/"))
            .ok_or_else(|| invalid("Invalid callback path"))
    };
    let exe = slash(&executable)?;
    let root = slash(root)?;
    let args = [CALLBACK, id, generation, &root];
    if shell != "powershell" {
        let quoted = std::iter::once(exe.as_str())
            .chain(args)
            .map(|s| quote(s, true))
            .collect::<Vec<_>>();
        return Ok(format!("exec {}", quoted.join(" ")));
    }
    let native_args = args
        .map(|s| format!("\"{}\"", s.replace('"', "\\\"")))
        .join(" ");
    Ok(format!(
        concat!(
            "$quotaStart = New-Object Diagnostics.ProcessStartInfo; $quotaStart.FileName = {}; $quotaStart.Arguments = {}; ",
            "$quotaStart.UseShellExecute = $false; $quotaStart.CreateNoWindow = $true; $quotaStart.RedirectStandardInput = $true; $quotaStart.RedirectStandardOutput = $true; $quotaStart.RedirectStandardError = $true; ",
            "$quotaChild = [Diagnostics.Process]::Start($quotaStart); $quotaOut = $quotaChild.StandardOutput.BaseStream.CopyToAsync([Console]::OpenStandardOutput()); $quotaErr = $quotaChild.StandardError.BaseStream.CopyToAsync([Console]::OpenStandardError()); ",
            "try {{ [Console]::OpenStandardInput().CopyTo($quotaChild.StandardInput.BaseStream) }} catch [IO.IOException] {{}} finally {{ $quotaChild.StandardInput.Close() }}; ",
            "$quotaChild.WaitForExit(); $quotaOut.Wait(); $quotaErr.Wait(); exit $quotaChild.ExitCode"
        ),
        quote(&exe, false),
        quote(&native_args, false)
    ))
}

struct Config {
    path: PathBuf,
    original: Option<Vec<u8>>,
    data: Map<String, Value>,
}
impl Config {
    fn read(directory: &str) -> io::Result<Self> {
        let path = Path::new(directory).join("settings.json");
        reject_links(&path)?;
        let original = match read_file(&path, MAX_CONFIG) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let data = match original.as_deref() {
            None => Map::new(),
            Some(bytes) => match serde_json::from_slice(bytes)? {
                Value::Object(data) => data,
                _ => return Err(invalid("Claude 설정이 JSON 객체가 아닙니다.")),
            },
        };
        Ok(Self {
            path,
            original,
            data,
        })
    }

    fn command(&self) -> Option<&str> {
        self.data.get("statusLine")?.get("command")?.as_str()
    }

    /// Only replace the CLI file we read. Its own DACL and parent directory belong to Claude Code.
    fn save(&self) -> io::Result<()> {
        reject_links(&self.path)?;
        let mut options = File::options();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
        let old = match options.open(&self.path) {
            Ok(file) if Some(read_bounded(&file, MAX_CONFIG)?) == self.original => Some(file),
            Err(e) if e.kind() == io::ErrorKind::NotFound && self.original.is_none() => None,
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => return Err(invalid("Claude 설정이 변경되었습니다. 다시 연결하세요.")),
        };
        let bytes = serde_json::to_vec(&self.data)?;
        if bytes.len() > MAX_CONFIG {
            return Err(invalid("Claude 설정 파일의 크기 제한을 초과했습니다."));
        }
        let temp = self
            .path
            .with_file_name(format!("settings.json.quotabar-{}.tmp", Uuid::new_v4()));
        let mut file = match &old {
            Some(original) => crate::files::windows::create_with_dacl(&temp, original)?,
            None => File::options()
                .create_new(true)
                .write(true)
                .share_mode(0)
                .open(&temp)?,
        };
        let result = (|| {
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            if old.is_some() {
                fs::rename(&temp, &self.path)
            } else {
                // Another program may have created a settings file since the read.
                let from = crate::files::windows::wide_file_path(&temp)?;
                let to = crate::files::windows::wide_file_path(&self.path)?;
                if unsafe { MoveFileW(from.as_ptr(), to.as_ptr()) } == 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(())
                }
            }
        })();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }
}

fn folder(root: &Path, id: &str) -> io::Result<PathBuf> {
    if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("잘못된 프로필입니다."));
    }
    let path = root.join("ClaudeProfiles").join(id);
    reject_links(&path)?;
    Ok(path)
}

pub(super) fn lock(folder: &Path) -> io::Result<File> {
    let path = folder.join("connection.lock");
    reject_links(&path)?;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

fn read_profile(path: &Path, id: &str) -> io::Result<Profile> {
    reject_links(path)?;
    let p: Profile = serde_json::from_slice(&read_file(path, MAX_CONFIG)?)?;
    if p.id != id || profile_id(&canonical(Path::new(&p.directory))?) != id {
        return Err(invalid("Claude 프로필 경로가 변경되었습니다."));
    }
    Ok(p)
}

/// Resolve an interrupted two-file installation while holding the profile lock.
/// The wrapper actually present in Claude Code's settings determines which generation won.
fn recover(folder: &Path, id: &str) -> io::Result<()> {
    let pending = folder.join(PENDING);
    let next = match read_profile(&pending, id) {
        Ok(p) => p,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let current = Config::read(&next.directory)?;
    let committed = if next.command.is_empty() {
        current.command() != Some(&read_profile(&folder.join(ACTIVE), id)?.command)
    } else {
        current.command() == Some(&next.command)
    };
    if committed {
        fs::rename(pending, folder.join(ACTIVE))
    } else {
        fs::remove_file(pending)
    }
}

pub(super) fn active(folder: &Path, id: &str) -> io::Result<Profile> {
    recover(folder, id)?;
    read_profile(&folder.join(ACTIVE), id)
}

pub(super) fn load_folder(folder: &Path, id: &str) -> io::Result<Profile> {
    if folder.join(PENDING).try_exists()? {
        let _lock = lock(folder)?;
        return active(folder, id);
    }
    read_profile(&folder.join(ACTIVE), id)
}

pub fn load(root: &Path, id: &str) -> io::Result<Profile> {
    load_folder(&folder(root, id)?, id)
}

fn commit(folder: &Path, config: &Config, profile: &Profile) -> io::Result<()> {
    let bytes = serde_json::to_vec(profile)?;
    if bytes.len() > MAX_CONFIG {
        return Err(invalid("Claude 프로필의 크기 제한을 초과했습니다."));
    }
    let pending = folder.join(PENDING);
    write_private(&pending, &bytes)?;
    if let Err(error) = config.save() {
        let _ = fs::remove_file(pending);
        return Err(error);
    }
    fs::rename(pending, folder.join(ACTIVE))
}

pub fn install(
    root: &Path,
    directory: &Path,
    helper: &Path,
    shell: Option<&str>,
) -> io::Result<Profile> {
    let directory = canonical(directory)?;
    let id = profile_id(&directory);
    let folder = folder(root, &id)?;
    fs::create_dir_all(&directory)?;
    ensure_private_directory(&folder)?;
    let _lock = lock(&folder)?;
    recover(&folder, &id)?;
    let mut config = Config::read(&directory)?;
    let current = config.data.get("statusLine").filter(|v| !v.is_null());
    let command = config.command();
    let wrapped = command.is_some_and(|s| s.contains(CALLBACK));
    let previous = match read_profile(&folder.join(ACTIVE), &id) {
        Ok(p) => Some(p),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if wrapped && previous.as_ref().map(|p| p.command.as_str()) != command {
        return Err(invalid(
            "이전 status line 백업을 복구한 뒤 다시 연결하세요.",
        ));
    }
    if current.is_some_and(|v| v["type"] != "command" || command.is_none()) {
        return Err(invalid(
            "기존 status line 형식을 지원하지 않습니다. 설정은 보존했습니다.",
        ));
    }
    let original = if wrapped {
        previous.as_ref().and_then(|p| p.original.clone())
    } else {
        current.cloned()
    };
    if original
        .as_ref()
        .and_then(|v| v["command"].as_str())
        .is_some_and(|s| s.contains(CALLBACK))
    {
        return Err(invalid("중복 callback은 연결할 수 없습니다."));
    }
    let shell = shell.map(str::to_owned).unwrap_or_else(|| {
        bash().map_or_else(|| "powershell".into(), |p| p.to_string_lossy().into_owned())
    });
    let generation = Uuid::new_v4().to_string();
    let command = wrapper(helper, root, &id, &generation, &shell)?;
    let original_shell = previous.filter(|_| wrapped).map_or(shell, |p| p.shell);
    let profile = Profile {
        id,
        directory,
        generation,
        command,
        original,
        shell: original_shell,
    };
    let entry = config.data.entry("statusLine").or_insert(Value::Null);
    if entry.is_null() {
        *entry = json!({});
    }
    entry["type"] = json!("command");
    entry["command"] = json!(profile.command);
    commit(&folder, &config, &profile)?;
    Ok(profile)
}

pub fn uninstall(root: &Path, id: &str) -> io::Result<bool> {
    let folder = folder(root, id)?;
    let _lock = lock(&folder)?;
    let mut profile = active(&folder, id)?;
    let mut config = Config::read(&profile.directory)?;
    if profile.command.is_empty() || config.command() != Some(&profile.command) {
        return Ok(false);
    }
    let current = config
        .data
        .get_mut("statusLine")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| invalid("기존 status line 형식을 지원하지 않습니다."))?;
    if let Some(original) = &profile.original {
        current.insert("command".into(), original["command"].clone());
        current.insert("type".into(), original["type"].clone());
    } else if current.keys().all(|k| k == "type" || k == "command") {
        config.data.remove("statusLine");
    } else {
        return Ok(false);
    }
    profile.generation = Uuid::new_v4().to_string();
    profile.command.clear();
    commit(&folder, &config, &profile)?;
    Ok(true)
}

pub fn latest(root: &Path, id: &str, generation: &str) -> io::Result<Option<Snapshot>> {
    let cutoff = Utc::now() + TimeDelta::minutes(1);
    let mut best: Option<Snapshot> = None;
    for path in snapshot_files(&folder(root, id)?)? {
        let Some(s) = read_file(&path, MAX_SNAPSHOT)
            .ok()
            .and_then(|b| serde_json::from_slice::<Snapshot>(&b).ok())
        else {
            continue;
        };
        if s.profile_id == id
            && s.generation == generation
            && s.observed_at <= cutoff
            && best
                .as_ref()
                .is_none_or(|old| s.observed_at > old.observed_at)
        {
            best = Some(s);
        }
    }
    Ok(best)
}

pub fn capture(root: &Path, id: &str, generation: &str, bytes: &[u8]) -> io::Result<()> {
    let folder = folder(root, id)?;
    let profile = load_folder(&folder, id)?;
    if profile.generation == generation {
        match parse_input(bytes, platform::valid_session) {
            Ok(Some((session, limits))) => platform::capture(&folder, &profile, &session, &limits)?,
            Ok(None) => {}
            Err(_) => {
                serde_json::from_slice::<serde::de::IgnoredAny>(bytes)?;
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub fetched_at: DateTime<Utc>,
    pub observed_at: Option<DateTime<Utc>>,
    pub windows: Vec<QuotaWindow>,
    pub state: &'static str,
}

pub fn fetch(root: &Path, directory: &Path) -> io::Result<Usage> {
    let directory = canonical(directory)?;
    let id = profile_id(&directory);
    let folder = folder(root, &id)?;
    let _lock = lock(&folder)?;
    let profile = active(&folder, &id)?;
    let config = Config::read(&directory)?;
    let now = Utc::now();
    let mut usage = Usage {
        fetched_at: now,
        observed_at: None,
        windows: vec![],
        state: "waiting",
    };
    if profile.command.is_empty() || config.command() != Some(&profile.command) {
        usage.state = "disconnected";
    } else if config
        .data
        .get("apiKeyHelper")
        .is_some_and(|v| !v.is_null())
        || platform::routed_with(config.data.get("env").unwrap_or(&Value::Null))
    {
        usage.state = "api-routed";
    } else if let Some(sample) = latest(root, &id, &profile.generation)? {
        let reset = |w: &QuotaWindow| w.reset_at.is_some_and(|t| t <= now);
        usage.state = if sample.windows.iter().all(reset) {
            "reset"
        } else if sample.observed_at < now - TimeDelta::minutes(5) {
            "stale"
        } else {
            "snapshot"
        };
        usage.observed_at = Some(sample.observed_at);
        usage.windows = sample.windows;
        for w in &mut usage.windows {
            if reset(w) {
                w.remaining = 100.;
                w.reset_at = None;
            }
        }
    }
    Ok(usage)
}
