use super::{State, now};
use crate::{
    credentials::Provider,
    login_item,
    mac_monitor::fetch::Snapshot,
    mac_settings::{AccountId, Settings},
};
use chrono::{Days, Local, TimeZone};
use serde::{Serialize, Serializer, ser::SerializeSeq};
use serde_json::{Value, json};
use std::{collections::BTreeMap, env, path::Path};
use uuid::Uuid;

// Foundation Codable encodes a dictionary with UUID keys as alternating keys and values.
fn pairs<S: Serializer, T: Serialize>(
    map: &BTreeMap<Uuid, T>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut list = serializer.serialize_seq(Some(map.len() * 2))?;
    for (id, value) in map {
        list.serialize_element(&AccountId(*id))?;
        list.serialize_element(value)?;
    }
    list.end()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct View<'a> {
    settings: &'a Settings,
    #[serde(serialize_with = "pairs")]
    snapshots: &'a BTreeMap<Uuid, Snapshot>,
    #[serde(serialize_with = "pairs")]
    errors: &'a BTreeMap<Uuid, String>,
    #[serde(serialize_with = "pairs")]
    notes: &'a BTreeMap<Uuid, &'static str>,
    loading: Vec<AccountId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    demo_report: Option<&'a Value>,
    launch_at_login: &'static str,
    app_update: &'a Value,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum Event<'a> {
    State {
        _0: View<'a>,
    },
    Reply {
        id: AccountId,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
    },
}

pub(super) fn snapshot(state: &State, dashboard: bool) -> Vec<u8> {
    serde_json::to_vec(&Event::State {
        _0: View {
            settings: &state.settings,
            snapshots: &state.snapshots,
            errors: &state.errors,
            notes: &state.notes,
            loading: state.queue.loading().map(AccountId).collect(),
            message: state.message.as_deref(),
            demo_report: state.report.as_ref().filter(|_| dashboard),
            launch_at_login: match state.login {
                login_item::State::Enabled => "enabled",
                login_item::State::Disabled => "disabled",
                login_item::State::Unavailable => "unavailable",
                login_item::State::RequiresApproval => "requiresApproval",
            },
            app_update: &state.update,
        },
    })
    .unwrap_or_default()
}

pub(super) fn reply(id: AccountId, error: Option<&str>) -> Vec<u8> {
    serde_json::to_vec(&Event::Reply { id, error }).unwrap_or_default()
}

pub(super) fn preview(state: &mut State, home: &Path) {
    let clock = now();
    let home = home.to_string_lossy();
    for (provider, name, remaining, plan) in [
        (Provider::Codex, "Personal", 72., "Plus"),
        (Provider::Codex, "Research lab", 38., "Business"),
        (Provider::Claude, "Writing & research", 16., "Max"),
        (Provider::Gemini, "Workspace", 84., "Standard"),
    ] {
        let source = match provider {
            Provider::Codex => format!(
                "{}/auth.json",
                env::var("CODEX_HOME").unwrap_or_else(|_| format!("{home}/.codex"))
            ),
            Provider::Claude => {
                env::var("CLAUDE_CONFIG_DIR").unwrap_or_else(|_| format!("{home}/.claude"))
            }
            Provider::Gemini => format!("{home}/.gemini/oauth_creds.json"),
        };
        let identity = String::from(AccountId(Uuid::new_v4()));
        let row = Settings::new_account(provider, &identity, name, &source);
        state.snapshots.insert(
            row.id.0,
            Snapshot::preview(provider, clock, remaining, plan),
        );
        state.settings.accounts.push(row);
    }
    let today = Local
        .timestamp_opt(clock.floor() as i64 + 978307200, 0)
        .single()
        .unwrap_or_else(Local::now)
        .date_naive();
    let mut points = Vec::with_capacity(26 * 7 * 2);
    for day in 0..26 * 7 {
        let date = today
            .checked_sub_days(Days::new(day))
            .expect("contemporary calendar date");
        let date = date.format("%Y-%m-%d").to_string();
        for (index, (provider, model)) in [("codex", "gpt-5"), ("claude", "claude-sonnet")]
            .into_iter()
            .enumerate()
        {
            let count = ((day * 7919 + index as u64 * 1777) % 34000 + 16000)
                * if day % 7 == 5 { 1 } else { 4 };
            points.push(json!({"day":date,"provider":provider,"model":model,
                "counts":{"input":count,"cached":count/3,"output":count/5}}));
        }
    }
    state.report = Some(json!({"days":points,"scannedFiles":42,"warnings":[],"scannedAt":clock}));
}
