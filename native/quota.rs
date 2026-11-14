//! Quota response formats used by the existing Windows and macOS applications.
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io};

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Windows,
    Mac,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    provider: String,
    response: Value,
    email: Option<String>,
    account_id: Option<String>,
    user_id: Option<String>,
    jwt_plan: Option<String>,
    tier: Option<String>,
    project: Option<String>,
    source: Option<Value>,
}

fn trimmed(value: &Value) -> Option<&str> {
    value.as_str().map(str::trim).filter(|s| !s.is_empty())
}
fn number(value: &Value, format: Format) -> Option<f64> {
    value.as_f64().or_else(|| {
        if format == Format::Mac {
            value.as_bool().map(|v| f64::from(u8::from(v)))
        } else {
            None
        }
    })
}
fn utc(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn codex_label(seconds: Option<f64>, format: Format) -> Option<String> {
    let duration = seconds.filter(|s| *s > 0.0)?;
    let duration = if format == Format::Mac {
        duration.round()
    } else {
        duration
    };
    if duration == 604800.0 {
        return Some("Weekly Window".into());
    }
    for (unit, suffix) in [(86400.0, "d"), (3600.0, "h"), (60.0, "m")] {
        if format == Format::Windows && suffix == "m" {
            break;
        }
        if duration % unit == 0.0 {
            return Some(format!("{:.0}{suffix} Window", duration / unit));
        }
    }
    (format == Format::Mac).then(|| "Quota Window".into())
}
fn reset_description(time: Option<DateTime<Utc>>, now: DateTime<Utc>, show_days: bool) -> String {
    let Some(time) = time.filter(|t| *t > now) else {
        return "Resets soon".into();
    };
    let total = (time - now).num_minutes();
    let hours = total / 60;
    if show_days && hours >= 24 {
        format!("Resets in {}d {}h", hours / 24, hours % 24)
    } else if hours > 0 {
        format!("Resets in {hours}h {}m", total % 60)
    } else {
        format!("Resets in {total}m")
    }
}

pub fn mac_codex_plan(raw: &str) -> (&str, &'static str) {
    let key = raw.trim().to_lowercase().replace([' ', '-'], "_");
    match key.as_str() {
        "free" => ("Free", "Personal"),
        "go" => ("Go", "Personal"),
        "plus" => ("Plus", "Personal"),
        "pro" => ("Pro", "Personal"),
        "team" | "self_serve_business_usage_based" | "business" => ("Business", "Business"),
        "enterprise_cbp_usage_based" | "enterprise" | "hc" => ("Enterprise", "Enterprise"),
        "education" | "edu" => ("Edu", "Edu"),
        _ => (raw, "Personal"),
    }
}

fn codex(request: &Request, format: Format, now: DateTime<Utc>) -> Value {
    let data = &request.response;
    let limits = &data["rate_limit"];
    let mut windows = Vec::new();
    let mut mac =
        json!({"provider":"codex","label":"Codex","fetchedAt":utc(now),"source":request.source});
    for (key, slot, fallback) in [
        (&limits["primary_window"], "primary", "5시간"),
        (&limits["secondary_window"], "secondary", "주간"),
        (
            &data["code_review_rate_limit"]["primary_window"],
            "tertiary",
            "Code review",
        ),
    ] {
        let Some(used) = number(&key["used_percent"], format) else {
            continue;
        };
        let label = codex_label(number(&key["limit_window_seconds"], format), format);
        let reset = number(&key["reset_at"], format)
            .filter(|s| *s > 0.0)
            .filter(|s| format == Format::Mac || *s < 253402300799.0)
            .and_then(|s| DateTime::from_timestamp(s as i64, 0));
        if format == Format::Windows {
            let title = if slot == "tertiary" {
                fallback
            } else {
                label.as_deref().unwrap_or(fallback)
            };
            windows.push(
                json!({"title":title,"remaining":(100.-used).clamp(0.,100.),"resetAt":reset}),
            );
        } else {
            mac[slot] = json!({"label":label,"usedPercent":used,"remainingPercent":(100.-used).max(0.),
                "resetAt":reset.map(utc),"resetDescription":reset.map(|t| reset_description(Some(t),now,true))});
        }
    }
    if format == Format::Windows {
        return json!({"fetchedAt":now, "windows":windows, "plan":data["plan_type"].as_str(),
            "email":data["email"].as_str().or(request.email.as_deref())});
    }
    let account = request
        .account_id
        .as_deref()
        .or_else(|| trimmed(&data["account_id"]));
    let plan = trimmed(&data["plan_type"])
        .or(request.jwt_plan.as_deref())
        .map(mac_codex_plan);
    mac["accountEmail"] = json!(trimmed(&data["email"]).or(request.email.as_deref()));
    mac["accountId"] = json!(account);
    mac["accountPlan"] = json!(plan.map(|p| p.0));
    mac["extra"] = json!({"workspaceType":plan.map_or("Personal", |p| p.1)});
    if let Some(id) = account {
        mac["extra"]["workspaceId"] = json!(id);
    }
    if let Some(id) = &request.user_id {
        mac["extra"]["userId"] = json!(id);
    }
    mac
}

#[derive(Clone, Copy)]
struct Bucket<'a> {
    id: &'a str,
    fraction: f64,
    reset: Option<&'a str>,
}

fn capitalized(s: &str) -> String {
    let mut word = true;
    let mut output = String::new();
    for c in s.chars() {
        if word {
            output.extend(c.to_uppercase());
        } else {
            output.extend(c.to_lowercase());
        }
        word = !c.is_alphanumeric();
    }
    output
}
fn gemini(request: &Request, format: Format, now: DateTime<Utc>) -> Value {
    let buckets = request.response["buckets"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut by_model = BTreeMap::<&str, Bucket>::new();
    let mut ordered = Vec::new();
    for item in buckets {
        if let (Some(id), Some(fraction)) = (
            item["modelId"].as_str(),
            number(&item["remainingFraction"], format),
        ) {
            let entry = Bucket {
                id,
                fraction,
                reset: item["resetTime"].as_str(),
            };
            if format == Format::Mac {
                let previous = by_model.entry(id).or_insert(entry);
                if entry.fraction < previous.fraction {
                    *previous = entry;
                }
            } else {
                ordered.push(entry);
            }
        }
    }
    ordered.extend(by_model.into_values());
    let mut groups: [Option<Bucket>; 3] = [None; 3];
    for bucket in &ordered {
        let id = if format == Format::Mac {
            std::borrow::Cow::Owned(bucket.id.to_lowercase())
        } else {
            std::borrow::Cow::Borrowed(bucket.id)
        };
        let pro = id.contains("pro");
        let lite = id.contains("flash-lite");
        let flash = !lite && id.contains("flash");
        for (ix, matches) in [
            (0, pro && (format == Format::Mac || (!flash && !lite))),
            (1, flash),
            (2, lite),
        ] {
            if matches && groups[ix].is_none_or(|p| bucket.fraction < p.fraction) {
                groups[ix] = Some(*bucket);
            }
        }
    }
    let mut windows = Vec::new();
    let mut mac = json!({"provider":"gemini","label":"Gemini CLI","accountEmail":request.email,
        "accountId":request.project.as_ref().or(request.email.as_ref()),"fetchedAt":utc(now),"source":request.source});
    let title = ["Pro", "Flash", "Flash Lite"];
    for (i, bucket) in groups.into_iter().enumerate() {
        let Some(bucket) = bucket else { continue };
        let time = bucket
            .reset
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.to_utc());
        let left = bucket.fraction * 100.0;
        if format == Format::Windows {
            windows.push(json!({"title":title[i],"remaining":left.clamp(0.,100.),"resetAt":time}));
        } else {
            mac[["primary", "secondary", "tertiary"][i]] = json!({"remainingPercent":left,"usedPercent":(100.-left).max(0.),
                "resetAt":bucket.reset.unwrap_or(""),"resetDescription":reset_description(time,now,false)});
        }
    }
    if format == Format::Windows {
        return json!({"fetchedAt":now,"windows":windows});
    }
    let plan = request.tier.as_deref().map(capitalized).or_else(|| {
        request
            .email
            .as_ref()
            .filter(|e| e.ends_with(".edu"))
            .map(|_| "Education".into())
    });
    mac["accountPlan"] = json!(plan);
    mac["extra"] = json!({"projectId":request.project.as_deref().unwrap_or(""), "lowestPercentLeft":ordered.iter()
        .map(|b| b.fraction*100.).reduce(f64::min).unwrap_or(100.)});
    mac
}

pub fn parse(request: &Request, format: Format, now: DateTime<Utc>) -> io::Result<Value> {
    match request.provider.as_str() {
        "codex" => Ok(codex(request, format, now)),
        "gemini" => Ok(gemini(request, format, now)),
        _ => Err(crate::files::invalid("Unknown quota provider")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn original_provider_response_contracts_on_both_platforms() {
        let now = DateTime::from_timestamp(1792152000, 0).unwrap();
        let fixtures = [
            (
                "codex",
                include_bytes!("../shared/fixtures/codex-quota.json").as_slice(),
            ),
            (
                "gemini",
                include_bytes!("../shared/fixtures/gemini-quota.json").as_slice(),
            ),
        ];
        for (provider, body) in fixtures {
            let request: Request = serde_json::from_value(json!({"provider":provider,"response":serde_json::from_slice::<Value>(body).unwrap(),
                "accountId":"current-workspace","email":"student@example.edu"})).unwrap();
            let win = parse(&request, Format::Windows, now).unwrap();
            let mac = parse(&request, Format::Mac, now).unwrap();
            assert_eq!(win["windows"].as_array().unwrap().len(), 3);
            if provider == "codex" {
                assert_eq!(win["windows"][1]["remaining"], 47.);
                assert_eq!(win["plan"], "plus");
                assert_eq!(win["windows"][2]["title"], "Code review");
                assert_eq!(mac["accountId"], "current-workspace");
                assert_eq!(mac["accountPlan"], "Plus");
                assert_eq!(mac["primary"]["label"], "5h Window");
                assert_eq!(mac["secondary"]["remainingPercent"], 47.);
            } else {
                assert_eq!(win["windows"][0]["remaining"], 40.);
                assert_eq!(win["windows"][1]["remaining"], 90.);
                assert_eq!(mac["tertiary"]["remainingPercent"], 100.);
                assert_eq!(mac["extra"]["lowestPercentLeft"], 40.);
                assert_eq!(mac["accountPlan"], "Education");
            }
        }
    }
}
