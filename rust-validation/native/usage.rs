// Local usage aggregation follows the native QuotaBar/AIUsage file formats. See NOTICE.
mod projection;

use chrono::{DateTime, Days, NaiveDate, TimeZone, Utc};
use indexmap::{IndexMap, IndexSet};
use projection::{Projection, Row};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File, Metadata},
    io::{self, BufReader, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::SystemTime,
};

const MAX_EVENTS: usize = 250_000;
const PARTIAL: &str = "읽지 못한 로그 파일이 있어 통계가 불완전할 수 있습니다.";
const DIRECTORY: &str = "일부 로그 폴더에 접근할 수 없습니다.";
const INVALID: &str = if cfg!(windows) {
    "손상되거나 제한을 초과한 사용량 기록은 제외했습니다."
} else {
    "일부 로그의 손상되거나 너무 큰 사용량 기록은 제외했습니다."
};
const LIMIT: &str = if cfg!(windows) {
    "25만 사용량 기록 한도로 일부 통계만 표시합니다."
} else {
    "로그가 커서 일부 통계만 표시합니다 (25만 사용량 기록 한도)."
};
const ORPHAN: &str = if cfg!(windows) {
    "부모가 없는 분기 세션은 첫 누적 기록을 기준값으로 사용합니다."
} else {
    "원본이 없는 분기 세션은 첫 누적 기록을 기준값으로 사용합니다."
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub input: u64,
    pub cached: u64,
    pub output: u64,
}
impl Counts {
    pub fn total(self) -> u64 {
        self.input + self.output
    }
    fn add(self, other: Self) -> Self {
        Self {
            input: self.input + other.input,
            cached: self.cached + other.cached,
            output: self.output + other.output,
        }
    }
    fn sub(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_sub(other.input),
            cached: self.cached.saturating_sub(other.cached),
            output: self.output.saturating_sub(other.output),
        }
    }
    fn max(self, other: Self) -> Self {
        Self {
            input: self.input.max(other.input),
            cached: self.cached.max(other.cached),
            output: self.output.max(other.output),
        }
    }
    fn parse(value: &Value, provider: Provider) -> Self {
        let number = |key: &str| {
            let n = value[key].as_f64().unwrap_or(0.0);
            if cfg!(target_os = "macos") && n.fract() != 0.0 {
                0
            } else {
                n.clamp(0.0, 1_000_000_000.0) as u64
            }
        };
        let output = number("output_tokens");
        match provider {
            Provider::Codex => Self {
                input: number("input_tokens"),
                cached: number("cached_input_tokens"),
                output,
            },
            Provider::Claude => {
                let cached = number("cache_read_input_tokens");
                Self {
                    input: number("input_tokens") + cached + number("cache_creation_input_tokens"),
                    cached,
                    output,
                }
            }
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Event {
    time: DateTime<Utc>,
    model: Arc<str>,
    counts: Counts,
    cumulative: bool,
    message: Option<Arc<str>>,
}
#[derive(Default)]
struct Session {
    id: String,
    parent: Option<String>,
    fork: Option<DateTime<Utc>>,
    events: Vec<Event>,
    issue: bool,
}

fn timestamp(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|d| d.to_utc())
}
fn check(cancelled: &AtomicBool) -> io::Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        Err(io::ErrorKind::Interrupted.into())
    } else {
        Ok(())
    }
}

fn parse_file(
    file: &File,
    path: &Path,
    size: u64,
    provider: Provider,
    limit: usize,
    cancelled: &AtomicBool,
) -> io::Result<Session> {
    let mut result = Session {
        id: path.to_string_lossy().into_owned(),
        ..Session::default()
    };
    let mut reader = Projection::new(
        BufReader::with_capacity(64 * 1024, file.take(size)),
        cancelled,
    );
    let mut model: Arc<str> = "Unknown".into();
    loop {
        let row = match reader.row()? {
            Row::Data(row) => row,
            Row::Invalid => {
                result.issue = true;
                continue;
            }
            Row::End => break,
        };
        if result.events.len() >= limit {
            result.issue = true;
            break;
        }
        let payload = &row["payload"];
        let ty = row["type"].as_str();
        match provider {
            Provider::Codex => match ty {
                Some("session_meta") => {
                    if let Some(id) = payload["id"].as_str() {
                        result.id = id.to_owned();
                    }
                    result.parent = payload["forked_from_id"]
                        .as_str()
                        .or_else(|| payload["forkedFromId"].as_str())
                        .map(str::to_owned);
                    if cfg!(windows) {
                        if let Some(time) = timestamp(
                            payload
                                .get("timestamp")
                                .filter(|v| v.is_string())
                                .unwrap_or(&row["timestamp"]),
                        ) {
                            result.fork = Some(time);
                        }
                    } else {
                        result.fork = timestamp(&payload["timestamp"])
                            .or_else(|| timestamp(&row["timestamp"]));
                    }
                }
                Some("turn_context") => {
                    if let Some(m) = payload["model"].as_str() {
                        model = m.into();
                    }
                }
                Some("event_msg") if payload["type"] == "token_count" => {
                    let info = &payload["info"];
                    let Some(time) = timestamp(&row["timestamp"]) else {
                        continue;
                    };
                    let total = &info["total_token_usage"];
                    let cumulative = if cfg!(windows) {
                        total.is_object()
                    } else {
                        info.get("total_token_usage").is_some()
                    };
                    let counts = if cumulative {
                        total
                    } else {
                        &info["last_token_usage"]
                    };
                    if !counts.is_object() {
                        continue;
                    }
                    if let Some(m) = info["model"]
                        .as_str()
                        .or_else(|| info["model_name"].as_str())
                        && model.as_ref() != m
                    {
                        model = m.into();
                    }
                    result.events.push(Event {
                        time,
                        model: model.clone(),
                        counts: Counts::parse(counts, provider),
                        cumulative,
                        message: None,
                    });
                }
                _ => {}
            },
            Provider::Claude if ty == Some("assistant") => {
                let msg = &row["message"];
                let (Some(time), Some(id), Some(name)) = (
                    timestamp(&row["timestamp"]),
                    msg["id"].as_str(),
                    msg["model"].as_str(),
                ) else {
                    continue;
                };
                if !msg["usage"].is_object()
                    || name == "<synthetic>"
                    || (cfg!(target_os = "macos") && id.is_empty())
                {
                    continue;
                }
                if model.as_ref() != name {
                    model = name.into();
                }
                result.events.push(Event {
                    time,
                    model: model.clone(),
                    counts: Counts::parse(&msg["usage"], provider),
                    cumulative: false,
                    message: Some(id.into()),
                });
            }
            _ => {}
        }
        if cfg!(windows) && result.events.len() >= limit {
            result.issue = true;
            break;
        }
    }
    Ok(result)
}

#[derive(PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: SystemTime,
    device: u64,
    inode: u64,
}
impl Stamp {
    fn new(file: &File) -> io::Result<Self> {
        let m = file.metadata()?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (m.dev(), m.ino())
        };
        #[cfg(windows)]
        let identity = {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
            };
            let mut info = BY_HANDLE_FILE_INFORMATION::default();
            if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
                return Err(io::Error::last_os_error());
            }
            (
                u64::from(info.dwVolumeSerialNumber),
                u64::from(info.nFileIndexHigh) << 32 | u64::from(info.nFileIndexLow),
            )
        };
        Ok(Self {
            size: m.len(),
            modified: m.modified()?,
            device: identity.0,
            inode: identity.1,
        })
    }
}

fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(unix)]
    {
        meta.is_symlink()
    }
}
fn key_for(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(path.to_string_lossy().to_uppercase())
    }
    #[cfg(unix)]
    {
        path.to_owned()
    }
}

#[derive(Debug, Serialize)]
pub struct DailyUsage {
    pub day: NaiveDate,
    pub provider: Provider,
    pub model: Arc<str>,
    pub counts: Counts,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub days: Vec<DailyUsage>,
    pub scanned_files: usize,
    pub warnings: Vec<&'static str>,
    pub scanned_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct Scanner {
    cache: HashMap<(PathBuf, Provider), (Stamp, Arc<Session>)>,
}

struct Collected {
    sessions: IndexMap<String, Session>,
    messages: IndexMap<Arc<str>, Event>,
    warnings: IndexSet<&'static str>,
    visited: HashSet<PathBuf>,
    files: usize,
    events: usize,
}
impl Collected {
    fn merge(
        &mut self,
        file: &Session,
        provider: Provider,
        now: DateTime<Utc>,
        day: &impl Fn(DateTime<Utc>) -> NaiveDate,
        cutoff: NaiveDate,
    ) {
        self.files += 1;
        self.events += file.events.len();
        if file.issue {
            self.warnings.insert(INVALID);
        }
        match provider {
            Provider::Codex => {
                let session = self.sessions.entry(file.id.clone()).or_default();
                session.events.extend(file.events.iter().cloned());
                if session.parent.is_none() {
                    session.parent = file.parent.clone();
                }
                if session.fork.is_none() {
                    session.fork = file.fork;
                }
            }
            Provider::Claude => {
                for item in &file.events {
                    if item.time > now || (cfg!(target_os = "macos") && day(item.time) < cutoff) {
                        continue;
                    }
                    if let Some(id) = &item.message {
                        self.messages
                            .entry(id.clone())
                            .and_modify(|e| {
                                e.counts = e.counts.max(item.counts);
                                e.time = e.time.min(item.time);
                                e.model = item.model.clone();
                            })
                            .or_insert_with(|| item.clone());
                    }
                }
            }
        }
    }
}

impl Scanner {
    pub fn release_cache(&mut self) {
        self.cache.clear();
    }
    pub fn scan<T: TimeZone>(
        &mut self,
        codex: &[PathBuf],
        claude: &[PathBuf],
        now: DateTime<Utc>,
        zone: &T,
        history_days: u16,
        cancel: &AtomicBool,
    ) -> io::Result<Report> {
        let day = |time: DateTime<Utc>| time.with_timezone(zone).date_naive();
        let cutoff = day(now)
            .checked_sub_days(Days::new(u64::from(history_days.clamp(1, 366) - 1)))
            .unwrap_or(NaiveDate::MIN);
        let mut found = Collected {
            sessions: IndexMap::new(),
            messages: IndexMap::new(),
            warnings: IndexSet::new(),
            visited: HashSet::new(),
            files: 0,
            events: 0,
        };
        let roots = codex
            .iter()
            .map(|p| (p, Provider::Codex))
            .chain(claude.iter().map(|p| (p, Provider::Claude)));
        'roots: for (root, provider) in roots {
            check(cancel)?;
            let meta = match fs::symlink_metadata(root) {
                Ok(m) => m,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(_) => {
                    found.warnings.insert(DIRECTORY);
                    continue;
                }
            };
            if cfg!(windows) && is_link(&meta) {
                found.warnings.insert("링크된 로그 폴더는 제외했습니다.");
                continue;
            }
            if !root.is_dir() {
                continue;
            }
            let mut dirs = VecDeque::from([root.clone()]);
            while let Some(dir) = dirs.pop_front() {
                check(cancel)?;
                let entries = match fs::read_dir(dir) {
                    Ok(v) => v,
                    Err(_) => {
                        found.warnings.insert(DIRECTORY);
                        continue;
                    }
                };
                for item in entries {
                    check(cancel)?;
                    let Ok(entry) = item else {
                        found.warnings.insert(DIRECTORY);
                        continue;
                    };
                    let path = entry.path();
                    if cfg!(target_os = "macos")
                        && entry.file_name().to_string_lossy().starts_with('.')
                    {
                        continue;
                    }
                    let Ok(meta) = fs::symlink_metadata(&path) else {
                        found.warnings.insert(PARTIAL);
                        continue;
                    };
                    if is_link(&meta) && (cfg!(windows) || path.is_dir()) {
                        continue;
                    }
                    if meta.is_dir() {
                        dirs.push_back(path);
                        continue;
                    }
                    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    if if cfg!(windows) {
                        !ext.eq_ignore_ascii_case("jsonl")
                    } else {
                        ext != "jsonl"
                    } {
                        continue;
                    }
                    if found.events >= MAX_EVENTS {
                        found.warnings.insert(LIMIT);
                        break 'roots;
                    }
                    let path = match fs::canonicalize(&path) {
                        Ok(p) => p,
                        Err(_) => {
                            found.warnings.insert(PARTIAL);
                            continue;
                        }
                    };
                    if !fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                        continue;
                    }
                    let key = (key_for(&path), provider);
                    if !found.visited.insert(key.0.clone()) {
                        continue;
                    }
                    let read: io::Result<Option<Arc<Session>>> = (|| {
                        let file = File::open(&path)?;
                        if !file.metadata()?.is_file() {
                            return Ok(None);
                        }
                        let stamp = Stamp::new(&file)?;
                        if cfg!(target_os = "macos")
                            && provider == Provider::Claude
                            && day(stamp.modified.into()) < cutoff
                        {
                            return Ok(None);
                        }
                        let remaining = MAX_EVENTS - found.events;
                        if let Some((cached, session)) = self.cache.get(&key)
                            && *cached == stamp
                            && session.events.len() <= remaining
                        {
                            return Ok(Some(session.clone()));
                        }
                        let session = Arc::new(parse_file(
                            &file, &path, stamp.size, provider, remaining, cancel,
                        )?);
                        if session.events.len() < remaining
                            && (self.cache.len() < 128 || self.cache.contains_key(&key))
                            && Stamp::new(&file)? == stamp
                        {
                            self.cache.insert(key, (stamp, session.clone()));
                        }
                        Ok(Some(session))
                    })();
                    match read {
                        Ok(Some(file)) => found.merge(&file, provider, now, &day, cutoff),
                        Ok(None) => {}
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => return Err(e),
                        Err(_) => {
                            found.warnings.insert(PARTIAL);
                        }
                    }
                }
            }
        }
        self.cache
            .retain(|(path, _), _| found.visited.contains(path));
        if self
            .cache
            .values()
            .map(|(_, v)| v.events.len())
            .sum::<usize>()
            > 20_000
        {
            self.release_cache();
        }
        let mut buckets: IndexMap<(NaiveDate, Provider, Arc<str>), Counts> = IndexMap::new();
        let mut record = |event: &Event, counts: Counts, provider: Provider| {
            let date = day(event.time);
            if date >= cutoff && event.time <= now && counts.total() > 0 {
                let entry = buckets
                    .entry((date, provider, event.model.clone()))
                    .or_default();
                *entry = entry.add(counts);
            }
        };
        // One sorted cumulative timeline per parent avoids rescanning a whole session for each fork.
        let mut timelines = HashMap::new();
        for (id, session) in &mut found.sessions {
            check(cancel)?;
            session.events.sort_by_key(|e| e.time);
            timelines.insert(
                id.clone(),
                session
                    .events
                    .iter()
                    .filter(|e| e.cumulative)
                    .map(|e| (e.time, e.counts))
                    .collect::<Vec<_>>(),
            );
        }
        for s in found.sessions.values() {
            check(cancel)?;
            let mut inherited = Counts::default();
            if let Some(parent) = &s.parent {
                let baseline = s.fork.zip(timelines.get(parent)).and_then(|(fork, times)| {
                    let pos = times.partition_point(|(time, _)| *time <= fork);
                    pos.checked_sub(1).map(|i| times[i].1)
                });
                inherited = baseline.unwrap_or_else(|| {
                    found.warnings.insert(ORPHAN);
                    s.events
                        .iter()
                        .find(|e| e.cumulative)
                        .map(|e| e.counts)
                        .unwrap_or_default()
                });
            }
            let mut previous = inherited;
            let mut seen = HashSet::new();
            for event in &s.events {
                check(cancel)?;
                if !seen.insert(event) {
                    continue;
                }
                let delta = if event.cumulative {
                    let delta = event.counts.sub(previous);
                    previous = event.counts;
                    inherited = Counts::default();
                    delta
                } else {
                    let delta = event.counts.sub(inherited);
                    inherited = inherited.sub(event.counts);
                    previous = previous.add(delta);
                    delta
                };
                record(event, delta, Provider::Codex);
            }
        }
        for event in found.messages.values() {
            check(cancel)?;
            record(event, event.counts, Provider::Claude);
        }
        let mut days: Vec<_> = buckets
            .into_iter()
            .map(|((day, provider, model), counts)| DailyUsage {
                day,
                provider,
                model,
                counts,
            })
            .collect();
        if cfg!(windows) {
            days.sort_by_key(|entry| entry.day);
        } else {
            found.warnings.sort_unstable();
            days.sort_by(|a, b| {
                (&a.provider, &a.day, &a.model).cmp(&(&b.provider, &b.day, &b.model))
            });
        }
        Ok(Report {
            days,
            scanned_files: found.files,
            warnings: found.warnings.into_iter().collect(),
            scanned_at: now,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_token_contract_and_cancellation() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("shared/fixtures/local-usage");
        let expected: Value =
            serde_json::from_slice(&fs::read(folder.join("expected.json")).unwrap()).unwrap();
        let now = timestamp(&expected["now"]).unwrap();
        let mut scanner = Scanner::default();
        let cancel = AtomicBool::new(false);
        for _ in 0..2 {
            let result = scanner
                .scan(
                    &[folder.join("codex")],
                    &[folder.join("claude")],
                    now,
                    &Utc,
                    182,
                    &cancel,
                )
                .unwrap();
            assert_eq!(
                result.warnings.len() as u64,
                expected["warningCount"].as_u64().unwrap()
            );
            for (provider, key) in [(Provider::Codex, "codex"), (Provider::Claude, "claude")] {
                let sum = result.days.iter().filter(|r| r.provider == provider).fold(
                    Counts::default(),
                    |sum, row| {
                        assert_eq!(row.day.to_string(), expected["day"]);
                        sum.add(row.counts)
                    },
                );
                assert_eq!(sum.input, expected[key]["input"].as_u64().unwrap());
                assert_eq!(sum.cached, expected[key]["cached"].as_u64().unwrap());
                assert_eq!(sum.output, expected[key]["output"].as_u64().unwrap());
                assert_eq!(sum.total(), expected[key]["total"].as_u64().unwrap());
            }
        }
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            scanner
                .scan(&[folder], &[], now, &Utc, 182, &cancel)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Interrupted
        );
    }
}
