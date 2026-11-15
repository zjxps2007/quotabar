//! The Claude quota cache shared by the macOS monitor and status line.
use super::{Limits, MAX_SNAPSHOT, Window, parse_input, prune, snapshot_files};
use crate::files::{invalid, read_file, write_private};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};
use uuid::Uuid;

// Foundation's default Codable Date counts seconds from 2001-01-01 UTC.
const FOUNDATION_EPOCH: f64 = 978_307_200.0;

pub fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        - FOUNDATION_EPOCH
}

pub(super) fn valid_session(session: &str) -> bool {
    session.len() == 36 && Uuid::parse_str(session).is_ok()
}

#[derive(Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub used_percent: f64,
    pub reset_at: f64,
}

impl From<&Window> for QuotaWindow {
    fn from(w: &Window) -> Self {
        Self {
            used_percent: w.used_percentage,
            reset_at: w.resets_at - FOUNDATION_EPOCH,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    schema_version: i64,
    #[serde(rename = "profileID")]
    profile_id: String,
    generation: String,
    #[serde(rename = "sessionID")]
    session_id: String,
    pub(super) received_at: f64,
    pub(super) observed_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<QuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<QuotaWindow>,
}

impl Snapshot {
    pub(super) fn from_limits(
        id: &str,
        generation: &str,
        session: &str,
        limits: &Limits,
        time: f64,
    ) -> Self {
        Self {
            schema_version: 1,
            profile_id: id.to_owned(),
            generation: generation.to_owned(),
            session_id: session.to_owned(),
            received_at: time,
            observed_at: time,
            five_hour: limits.five_hour.as_ref().map(QuotaWindow::from),
            seven_day: limits.seven_day.as_ref().map(QuotaWindow::from),
        }
    }

    pub fn parse(input: &[u8], id: &str, generation: &str, time: f64) -> io::Result<Option<Self>> {
        Ok(parse_input(input, valid_session)?
            .map(|(session, limits)| Self::from_limits(id, generation, &session, &limits, time)))
    }

    fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&read_file(path, MAX_SNAPSHOT).ok()?).ok()
    }

    pub fn save(mut self, folder: &Path) -> io::Result<()> {
        if !valid_session(&self.session_id) {
            return Err(invalid("Claude session ID is not valid."));
        }
        let path = folder.join(format!("snapshot-{}.json", self.session_id));
        if let Some(previous) = Self::read(&path)
            && previous.generation == self.generation
            && previous.five_hour == self.five_hour
            && previous.seven_day == self.seven_day
        {
            self.observed_at = previous.observed_at;
        }
        write_private(&path, &serde_json::to_vec(&self)?)?;
        prune(folder)
    }

    pub fn latest(folder: &Path, id: &str, generation: &str, time: f64) -> Option<Self> {
        snapshot_files(folder)
            .ok()?
            .iter()
            .filter_map(|path| Self::read(path))
            .filter(|s| {
                s.schema_version == 1
                    && s.profile_id == id
                    && s.generation == generation
                    && s.observed_at <= time + 60.0
                    && (s.five_hour.is_some() || s.seven_day.is_some())
            })
            .max_by(|a, b| a.observed_at.total_cmp(&b.observed_at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{fs, path::PathBuf};

    const SESSION: &str = "ABCDEF01-0000-4000-8000-000000000001";
    const NOW: f64 = 805_000_000.125;
    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("quotabar-mac-snapshots-{}", Uuid::new_v4()));
            crate::files::ensure_private_directory(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn input(session: &str, used: f64) -> Vec<u8> {
        serde_json::to_vec(&json!({"session_id":session, "unrelated":"omitted",
            "rate_limits":{"five_hour":{"used_percentage":used, "resets_at":1_800_000_000.25}}}))
        .unwrap()
    }

    #[test]
    fn foundation_status_line_input() {
        let s = Snapshot::parse(&input(SESSION, 23.5), "profile", "binding", NOW)
            .unwrap()
            .unwrap();
        let encoded = serde_json::to_value(s).unwrap();
        assert_eq!(
            encoded,
            json!({"schemaVersion":1, "profileID":"profile", "generation":"binding",
            "sessionID":SESSION, "receivedAt":NOW,"observedAt":NOW,
            "fiveHour":{"usedPercent":23.5, "resetAt":821692800.25}})
        );
        let decode = |bytes: &[u8]| Snapshot::parse(bytes, "a", "b", NOW);
        for bytes in [
            input(&SESSION.replace('-', ""), 40.),
            format!(r#"{{"session_id":"{SESSION}"}}"#).into_bytes(),
        ] {
            assert!(decode(&bytes).unwrap().is_none());
        }
        let missing = format!(r#"{{"session_id":"{SESSION}","rate_limits":{{}}}}"#);
        assert!(
            decode(missing.as_bytes())
                .unwrap()
                .unwrap()
                .five_hour
                .is_none()
        );
        assert!(decode(b"{}").is_err());
        assert!(decode(&input(SESSION, 101.)).is_err());
        let mut window: Value = serde_json::from_slice(&input(SESSION, 100.)).unwrap();
        window["rate_limits"]["five_hour"]["resets_at"] = json!(-1);
        assert!(decode(&serde_json::to_vec(&window).unwrap()).is_err());
        assert!(decode(&vec![b' '; super::super::MAX_INPUT + 1]).is_err());
    }

    #[test]
    fn foundation_fractional_reset_dates_survive_json_round_trips() {
        // Date and quota values are compared exactly when deciding whether to renew observedAt.
        for i in 1..=128 {
            let decimal = format!("1800000000.{:08}", i * 117_139);
            let original: f64 = decimal.parse().unwrap();
            let payload = format!(
                r#"{{"session_id":"{SESSION}","rate_limits":{{"five_hour":{{"used_percentage":22.8,"resets_at":{decimal}}}}}}}"#
            );
            let sample = Snapshot::parse(payload.as_bytes(), "p", "g", NOW)
                .unwrap()
                .unwrap();
            let restored: Snapshot =
                serde_json::from_slice(&serde_json::to_vec(&sample).unwrap()).unwrap();
            assert_eq!(
                sample.five_hour.as_ref().unwrap().reset_at,
                original - FOUNDATION_EPOCH
            );
            assert_eq!(
                restored.five_hour.unwrap().reset_at,
                original - FOUNDATION_EPOCH
            );
        }
    }

    #[test]
    fn foundation_cache_keeps_original_observation_and_filters_unusable_files() {
        let folder = Folder::new();
        let path = &folder.0;
        let stored = path.join(format!("snapshot-{SESSION}.json"));
        // Includes a null optional window as well as Foundation's numeric dates.
        let mut fixture = json!({"schemaVersion":1,"profileID":"p","generation":"g","sessionID":SESSION,
            "receivedAt":NOW-20.0,"observedAt":NOW-600.25,
            "fiveHour":{"usedPercent":0.0,"resetAt":821692800.25},"sevenDay":null});
        fs::write(&stored, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let save = |used: f64| {
            Snapshot::parse(&input(SESSION, used), "p", "g", NOW)
                .unwrap()
                .unwrap()
                .save(path)
                .unwrap()
        };
        save(0.);
        let latest = || Snapshot::latest(path, "p", "g", NOW).unwrap();
        assert_eq!(
            (latest().observed_at, latest().received_at),
            (NOW - 600.25, NOW)
        );
        assert!(Snapshot::latest(path, "p", "other", NOW).is_none());
        assert!(Snapshot::latest(path, "other", "g", NOW).is_none());
        save(100.);
        assert_eq!(latest().observed_at, NOW);
        fixture["observedAt"] = json!(NOW + 61.);
        fs::write(
            path.join("snapshot-future.json"),
            serde_json::to_vec(&fixture).unwrap(),
        )
        .unwrap();
        fs::write(path.join("snapshot-corrupt.json"), b"{").unwrap();
        fs::write(
            path.join("snapshot-oversized.json"),
            vec![b' '; MAX_SNAPSHOT + 1],
        )
        .unwrap();
        fs::remove_file(&stored).unwrap();
        assert!(Snapshot::latest(path, "p", "g", NOW).is_none());
        fixture["observedAt"] = json!(NOW + 60.);
        fs::write(&stored, serde_json::to_vec(&fixture).unwrap()).unwrap();
        assert_eq!(latest().observed_at, NOW + 60.);
    }

    #[test]
    fn foundation_cache_prunes_only_quota_snapshots() {
        let folder = Folder::new();
        let path = &folder.0;
        fs::write(path.join("profile.json"), b"profile").unwrap();
        for index in 1..=22 {
            let session = Uuid::from_u128(index).to_string();
            Snapshot::parse(&input(&session, 42.), "p", "g", NOW)
                .unwrap()
                .unwrap()
                .save(path)
                .unwrap();
        }
        assert_eq!(snapshot_files(path).unwrap().len(), 20);
        assert_eq!(fs::read(path.join("profile.json")).unwrap(), b"profile");
        let mut invalid = Snapshot::parse(&input(SESSION, 0.), "p", "g", NOW)
            .unwrap()
            .unwrap();
        invalid.session_id = "../profile".into();
        assert!(invalid.save(path).is_err());
    }
}
