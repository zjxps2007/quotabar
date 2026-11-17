use super::{Config, Error, Profile, Result, canonical, config, profile_folder};
use crate::claude::{
    digest,
    mac_snapshots::{QuotaWindow, Snapshot},
};
use serde_json::{Value, json};
use std::path::Path;
use unicode_segmentation::UnicodeSegmentation;

fn trimmed(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || c == '\u{200b}')
}
pub(super) fn plan(raw: Option<&str>) -> Option<String> {
    let raw = trimmed(raw?);
    let first = raw.graphemes(true).next()?;
    Some(match raw.to_lowercase().as_str() {
        "pro" => "Pro".into(),
        "max" => "Max".into(),
        "team" => "Team".into(),
        "enterprise" => "Enterprise".into(),
        "free" => "Free".into(),
        _ => first.to_uppercase() + &raw[first.len()..],
    })
}
pub(super) fn auth_status(data: &[u8]) -> Result<Value> {
    if data.len() >= 64 * 1024 {
        return Err(Error::AuthStatus);
    }
    let obj = config::decode(data)?.ok_or(Error::AuthStatus)?;
    let text = |key| {
        config::string(&obj, key)
            .map(|s| trimmed(&s).to_owned())
            .filter(|s| !s.is_empty())
    };
    let logged_in = obj
        .get("loggedIn")
        .and_then(|v| serde_json::from_str::<Value>(v.get()).ok())
        .is_some_and(|v| v.as_bool() == Some(true) || v.as_f64() == Some(1.0));
    Ok(
        json!({"loggedIn":logged_in,"authMethod":text("authMethod"),"apiProvider":text("apiProvider"),
        "configDirectory":text("configDirectory"),"email":text("email"),"organizationID":text("orgId"),
        "organizationName":text("orgName"),"subscriptionType":text("subscriptionType")}),
    )
}

#[cfg(target_os = "macos")]
fn iso(date: f64) -> String {
    use objc2_foundation::{NSDate, NSISO8601DateFormatOptions, NSISO8601DateFormatter};
    let f = NSISO8601DateFormatter::new();
    f.setFormatOptions(NSISO8601DateFormatOptions::WithInternetDateTime);
    f.stringFromDate(&NSDate::dateWithTimeIntervalSinceReferenceDate(date))
        .to_string()
}
#[cfg(not(target_os = "macos"))]
fn iso(date: f64) -> String {
    chrono::DateTime::from_timestamp((date + 978307200.).floor() as i64, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}
fn window(w: Option<&QuotaWindow>, now: f64) -> Option<Value> {
    let w = w.filter(|w| (0.0..=100.).contains(&w.used_percent))?;
    Some(if w.reset_at > now {
        json!({"usedPercent":w.used_percent,"remainingPercent":100.-w.used_percent,"resetAt":iso(w.reset_at)})
    } else {
        json!({"usedPercent":0,"remainingPercent":100,"resetDescription":"Starts with next message"})
    })
}
pub(crate) fn fetch(root: &Path, directory: &str, now: f64) -> Result<Value> {
    let id = digest(&canonical(directory));
    let folder = profile_folder(root, &id)?;
    let profile = Profile::load(&folder, &id)?;
    let config = Config::read(&profile.config_directory).ok();
    let mut usage = json!({"provider":"claude-subscription","label":"Claude Subscription","accountId":id,
        "fetchedAt":"","accountName":profile.email.as_ref().unwrap_or(&profile.name), "accountEmail":profile.email,
        "accountPlan":plan(profile.subscription_type.as_deref()), "source":{"mode":"local","type":"claude-code-statusline"},
        "extra":{"profileDirectory":profile.config_directory,"identityState":if profile.email.is_some() {"cli-reported"} else {"unverified"}}});
    if let Some(name) = profile.organization_name {
        usage["extra"]["organizationName"] = name.into();
    }
    let connected = profile.installed_command.is_some()
        && config
            .as_ref()
            .is_some_and(|c| c.matches_command(profile.installed_command.as_deref()));
    let state = if !connected {
        "disconnected"
    } else if let Some(sample) = Snapshot::latest(&folder, &id, &profile.generation, now) {
        usage["fetchedAt"] = iso(sample.observed_at).into();
        usage["extra"]["receivedAt"] = iso(sample.received_at).into();
        usage["primary"] = window(sample.five_hour.as_ref(), now).into();
        usage["secondary"] = window(sample.seven_day.as_ref(), now).into();
        let live = [sample.five_hour, sample.seven_day]
            .into_iter()
            .flatten()
            .any(|w| w.reset_at > now);
        if !live {
            "reset"
        } else if now - sample.observed_at > 300.0 {
            "stale"
        } else {
            "snapshot"
        }
    } else if config.is_some_and(|c| c.routes_through_api()) {
        "api-routed"
    } else {
        "waiting"
    };
    usage["extra"]["snapshotState"] = state.into();
    Ok(usage)
}
