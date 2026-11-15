use chrono::{DateTime, Local, Utc};
use serde_json::{Value, json};

pub const MAC: bool = cfg!(target_os = "macos");
pub fn text<'a>(v: &'a Value, key: &str, default: &'a str) -> &'a str {
    v[key].as_str().unwrap_or(default)
}
pub fn list(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
pub fn provider(key: &str) -> &'static str {
    match key.to_ascii_lowercase().as_str() {
        "claude" => "Claude",
        "gemini" => "Gemini",
        _ => "Codex",
    }
}
pub fn local_date(v: &Value) -> Option<DateTime<Local>> {
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Local));
    }
    let secs = v.as_f64()? + 978307200.;
    DateTime::<Utc>::from_timestamp(secs.floor() as i64, (secs.fract() * 1e9).max(0.) as u32)
        .map(|d| d.with_timezone(&Local))
}
pub fn number(n: u64) -> String {
    for (factor, s) in [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")] {
        if n >= factor {
            return format!("{:.1}{s}", n as f64 / factor as f64);
        }
    }
    n.to_string()
}
pub fn count(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or(0)
}
pub fn total(v: &Value) -> u64 {
    count(v, "input") + count(v, "output")
}
pub fn pending(state: &str) -> &'static str {
    match state {
        "waiting" => "Claude에서 첫 메시지를 보내면 잔여량이 표시됩니다.",
        "api-routed" => "API 연결 세션은 구독 사용량을 제공하지 않습니다.",
        "disconnected" => "Claude 연결이 해제되었습니다. 다시 연결하세요.",
        "stale" => "새 데이터가 없습니다. Claude에서 메시지를 보내 연결을 확인하세요.",
        _ => "아직 조회된 잔여량이 없습니다.",
    }
}
#[derive(Clone, Debug)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub provider: &'static str,
    pub path: String,
    pub enabled: bool,
    pub tray: bool,
    pub managed: bool,
    pub snapshot: Value,
    pub error: Option<String>,
    pub note: Option<String>,
    pub loading: bool,
}
impl Account {
    pub fn quota_message(&self) -> &'static str {
        let state = text(&self.snapshot, "state", "");
        if MAC {
            return match state {
                "api-routed" => "API 연결 세션은 구독 quota를 제공하지 않습니다.",
                "disconnected" | "waiting" => pending(state),
                _ if self.loading => "사용량을 확인하고 있습니다…",
                _ => pending(""),
            };
        }
        pending(state)
    }

    pub fn connection_message(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| {
            let state = text(&self.snapshot, "state", "");
            match state {
                "api-routed" if MAC => Some("API 연결 세션은 구독 잔여량을 제공하지 않습니다."),
                "disconnected" if MAC => {
                    Some("Claude 연결이 해제되었습니다. 계정 연결에서 설정 폴더를 다시 연결하세요.")
                }
                "waiting" if MAC => {
                    Some("Claude에서 첫 메시지를 보내면 데이터를 받을 수 있습니다.")
                }
                "stale" | "waiting" | "disconnected" | "api-routed" => Some(pending(state)),
                _ => None,
            }
        })
    }

    pub fn control_label(&self, control: &str) -> String {
        if MAC {
            format!("{}({}) {control}", self.provider, self.name)
        } else {
            format!("{} {} {control}", self.provider, self.name)
        }
    }
    pub fn windows(&self) -> &[Value] {
        list(&self.snapshot["windows"])
    }
    pub fn attention(&self) -> bool {
        self.error.is_some()
            || matches!(
                self.snapshot["state"].as_str(),
                Some("stale" | "disconnected" | "api-routed")
            )
            || MAC && self.snapshot["state"] == "waiting"
    }
    pub fn lowest(&self) -> Option<f64> {
        self.windows()
            .iter()
            .filter_map(|w| w["remaining"].as_f64())
            .reduce(f64::min)
    }
}
pub struct State {
    pub raw: Value,
    pub accounts: Vec<Account>,
    pub demo: bool,
    pub report: Value,
}
impl State {
    pub fn new(demo: bool) -> Self {
        Self {
            raw: json!({}),
            accounts: vec![],
            demo,
            report: Value::Null,
        }
    }
    pub fn apply(&mut self, mut value: Value) {
        fn lookup<'a>(v: &'a Value, id: &str) -> &'a Value {
            if let Some(obj) = v.as_object()
                && let Some((_, s)) = obj.iter().find(|(k, _)| k.eq_ignore_ascii_case(id))
            {
                return s;
            }
            if let Some(a) = v.as_array() {
                for pair in a.as_chunks::<2>().0 {
                    if pair[0].as_str().is_some_and(|s| s.eq_ignore_ascii_case(id)) {
                        return &pair[1];
                    }
                }
            }
            &Value::Null
        }
        self.accounts = list(&value["settings"]["accounts"])
            .iter()
            .map(|a| {
                let id = text(a, "id", "");
                Account {
                    id: id.into(),
                    name: text(a, "name", "").into(),
                    provider: provider(text(a, "provider", "codex")),
                    path: text(a, "sourcePath", "").into(),
                    enabled: a["enabled"].as_bool().unwrap_or(true),
                    tray: a["showInTray"]
                        .as_bool()
                        .or(a["showInMenuBar"].as_bool())
                        .unwrap_or(true),
                    managed: a["appManagedLogin"] == true,
                    snapshot: lookup(&value["snapshots"], id).clone(),
                    error: lookup(&value["errors"], id).as_str().map(str::to_owned),
                    note: lookup(&value["notes"], id).as_str().map(str::to_owned),
                    loading: list(&value["loading"])
                        .iter()
                        .any(|v| v.as_str().is_some_and(|k| k.eq_ignore_ascii_case(id))),
                }
            })
            .collect();
        if let Some(d) = value["demo"].as_bool() {
            self.demo = d;
        }
        if !MAC {
            self.report = value["activity"].take();
        } else if !value["demoReport"].is_null() {
            self.report = value["demoReport"].take();
        }
        self.raw = value;
    }
    pub fn setting<'a>(&'a self, k: &str, default: &'a str) -> &'a str {
        text(&self.raw["settings"], k, default)
    }
    pub fn flag(&self, win: &str, mac: &str) -> bool {
        self.raw["settings"][if MAC { mac } else { win }]
            .as_bool()
            .unwrap_or(true)
    }
    pub fn defaults(&self, p: &str) -> String {
        if let Some(path) = self.raw["defaultPaths"][p.to_ascii_lowercase()].as_str() {
            return path.to_owned();
        }
        if !MAC {
            return if p == "Claude" {
                "Claude 설정 폴더"
            } else {
                "로그인 JSON 파일"
            }
            .into();
        }
        let home = std::path::PathBuf::from(
            std::env::var_os(if MAC { "HOME" } else { "USERPROFILE" }).unwrap_or_default(),
        );
        let path = match p {
            "Claude" => std::env::var_os("CLAUDE_CONFIG_DIR")
                .map(Into::into)
                .unwrap_or_else(|| home.join(".claude")),
            "Gemini" => home.join(".gemini").join("oauth_creds.json"),
            _ => std::env::var_os("CODEX_HOME")
                .map(Into::into)
                .unwrap_or_else(|| home.join(".codex"))
                .join("auth.json"),
        };
        path.to_string_lossy().into_owned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn swift_uuid_pairs_and_dotnet_dictionary_restore_previous_values() {
        for snapshots in [
            json!(["ABC",{"windows":[{"remaining":14}]}]),
            json!({"aBc":{"windows":[{"remaining":14}]}}),
        ] {
            let mut state = State::new(false);
            state.apply(json!({"settings":{"accounts":[{"id":"abc","provider":"claude","showInMenuBar":false}]},"snapshots":snapshots,"errors":{"ABC":"오프라인"},"loading":["ABC"]}));
            let a = &state.accounts[0];
            assert!(!a.tray && a.loading && a.attention());
            assert_eq!(a.lowest(), Some(14.));
        }
        assert_eq!(local_date(&json!(0)).unwrap().timestamp(), 978307200);
        assert_eq!(total(&json!({"input":10,"cached":7,"output":3})), 13);
    }
}
