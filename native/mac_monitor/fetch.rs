//! One refresh of a resident account. A replacement CLI login travels separately
//! from the dashboard state, and is saved only if the caller still owns the job.
mod failure;

use crate::{
    claude::{mac_connection, mac_snapshots},
    codex_sessions::managed::ManagedAccounts,
    credentials::{Platform, Provider},
    mac_settings::{
        Account,
        imported::{self, Selection},
    },
    provider_http,
    provider_query::{Operation, Query},
};
use failure::Error;
use objc2_foundation::{NSISO8601DateFormatOptions as Options, NSISO8601DateFormatter, NSString};
use serde::Serialize;
use serde_json::Value;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroizing;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    title: String,
    remaining: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    reset_at: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    fetched_at: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    observed_at: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    windows: Vec<Window>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
}

#[derive(Default, Serialize)]
pub struct Outcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
    #[serde(skip)]
    pub updated_credential: Option<Zeroizing<Vec<u8>>>,
}

fn date(value: &Value) -> Option<f64> {
    let value = value.as_str().filter(|s| !s.is_empty())?;
    let fractional = value.contains('.');
    let value = NSString::from_str(value);
    let formatter = NSISO8601DateFormatter::new();
    for fractions in [fractional, !fractional] {
        let mut options = Options::WithInternetDateTime;
        if fractions {
            options |= Options::WithFractionalSeconds;
        }
        formatter.setFormatOptions(options);
        if let Some(date) = formatter.dateFromString(&value) {
            return Some(date.timeIntervalSinceReferenceDate());
        }
    }
    None
}

impl Snapshot {
    fn from_usage(usage: Value, provider: Provider) -> Self {
        let labels = if provider == Provider::Gemini {
            ["Pro", "Flash", "Flash Lite"]
        } else {
            ["5시간", "주간", "Code review"]
        };
        let windows = ["primary", "secondary", "tertiary"]
            .into_iter()
            .zip(labels)
            .filter_map(|(key, label)| {
                let raw = &usage[key];
                let remaining = raw["remainingPercent"]
                    .as_f64()
                    .or_else(|| raw["usedPercent"].as_f64().map(|used| 100. - used))?;
                Some(Window {
                    title: raw["label"].as_str().unwrap_or(label).to_owned(),
                    remaining: remaining.clamp(0., 100.),
                    reset_at: date(&raw["resetAt"]),
                })
            })
            .collect();
        Self {
            fetched_at: mac_snapshots::now(),
            observed_at: date(&usage["fetchedAt"]),
            plan: usage["accountPlan"].as_str().map(str::to_owned),
            email: usage["accountEmail"].as_str().map(str::to_owned),
            windows,
            state: usage["extra"]["snapshotState"].as_str().map(str::to_owned),
        }
    }
}

fn query(
    root: &Path,
    sessions: &ManagedAccounts,
    account: &Account,
    managed: bool,
    cancel: &AtomicBool,
) -> Result<Outcome, Error> {
    let mut result = Outcome::default();
    let usage = if managed {
        let bytes = sessions
            .fetch(account.id.0, &account.identity, cancel)
            .map_err(Error::Managed)?;
        serde_json::from_slice(&bytes).map_err(|_| Error::Json)?
    } else if account.provider == Provider::Claude {
        mac_connection::fetch(
            &root.join("ClaudeProfiles"),
            &account.source_path,
            mac_snapshots::now(),
        )
        .map_err(Error::Claude)?
    } else {
        let (data, selected) = imported::select(
            sessions.vault_directory(),
            account.id.0,
            account.provider,
            &account.source_path,
            &account.identity,
        )
        .map_err(Error::Import)?;
        let (op, timeout) = if account.provider == Provider::Gemini {
            (Operation::Gemini, 15.)
        } else {
            (Operation::Codex, 20.)
        };
        let query =
            Query::new(op, &data, Platform::Mac, timeout).map_err(provider_http::Failure::Query)?;
        let bytes = provider_http::run(query, cancel, None)?;
        match selected {
            Selection::Updated => result.updated_credential = Some(data),
            Selection::OtherAccount => {
                result.note = Some("CLI가 다른 계정으로 전환되어 보관된 로그인을 사용합니다.")
            }
            Selection::Stored => {}
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Json)?
    };
    result.snapshot = Some(Snapshot::from_usage(usage, account.provider));
    Ok(result)
}

pub fn fetch(
    root: &Path,
    sessions: &ManagedAccounts,
    account: &Account,
    cancel: &AtomicBool,
) -> Outcome {
    let managed = account.provider == Provider::Codex && account.app_managed_login == Some(true);
    let result = query(root, sessions, account, managed, cancel);
    if cancel.load(Ordering::Acquire) {
        return Outcome::default();
    }
    result.unwrap_or_else(|error| Outcome {
        error: error.message(account.provider, managed),
        ..Outcome::default()
    })
}
