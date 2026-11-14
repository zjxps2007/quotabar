use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct Account {
    pub provider: String,
    pub name: String,
    pub enabled: bool,
    pub visible: bool,
    pub remaining: Vec<f64>,
    pub error: bool,
    pub state: Option<String>,
}
#[derive(Default, Deserialize)]
pub struct State {
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub demo: bool,
    #[serde(default)]
    pub icon: bool,
}

impl Account {
    fn lowest(&self) -> Option<f64> {
        self.remaining
            .iter()
            .copied()
            .filter(|n| n.is_finite())
            .map(|n| n.clamp(0., 100.))
            .reduce(f64::min)
    }
    fn windows_attention(&self) -> bool {
        self.error
            || matches!(
                self.state.as_deref(),
                Some("stale" | "reset" | "disconnected" | "api-routed")
            )
    }
    fn mac_attention(&self) -> bool {
        self.error
            || self.lowest().is_none()
            || matches!(
                self.state.as_deref(),
                Some("waiting" | "stale" | "disconnected" | "api-routed")
            )
    }
}
fn clip_utf16(text: &str, max: usize) -> String {
    if text.encode_utf16().count() <= max {
        return text.to_owned();
    }
    let mut remaining = max - 1;
    let mut result = String::new();
    for c in text.chars() {
        if c.len_utf16() > remaining {
            break;
        }
        remaining -= c.len_utf16();
        result.push(c);
    }
    result.push('…');
    result
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsSummary {
    text: String,
    use_logo: bool,
    warning: bool,
    tooltip: String,
}
pub fn windows(state: &State) -> WindowsSummary {
    let visible: Vec<_> = state
        .accounts
        .iter()
        .filter(|a| a.enabled && a.visible)
        .collect();
    let lowest = visible.iter().filter_map(|a| a.lowest()).reduce(f64::min);
    let uncertain = visible.iter().any(|a| a.windows_attention());
    let missing = visible.iter().any(|a| a.lowest().is_none());
    let text = lowest
        .map(|n| n.round().to_string())
        .unwrap_or_else(|| if uncertain { "!" } else { "—" }.to_owned());
    let mut tooltip = String::from("QuotaBar");
    if state.demo {
        tooltip.push_str(" · 미리보기");
    }
    if lowest.is_some() {
        tooltip.push_str(&format!(" · 최소 잔여 {text}%"));
    } else {
        tooltip.push_str(if visible.is_empty() {
            " · 표시할 계정 없음"
        } else if uncertain {
            " · 연결 확인 필요"
        } else {
            " · 사용량 조회 대기"
        });
    }
    if uncertain {
        tooltip.push_str("\n연결 확인 필요 · 이전 값/갱신 대기 포함");
    } else if missing && lowest.is_some() {
        tooltip.push_str("\n일부 계정 조회 대기");
    }
    for a in &visible {
        let name = clip_utf16(&a.name.replace(['\r', '\n'], " "), 18);
        let percent = a
            .lowest()
            .map(|n| n.round().to_string())
            .unwrap_or_else(|| "—".into());
        tooltip.push_str(&format!("\n{} {name}: {percent}%", a.provider));
    }
    WindowsSummary {
        text,
        use_logo: state.icon || visible.is_empty(),
        warning: uncertain || lowest.is_some_and(|n| n <= 20.) || missing && lowest.is_some(),
        tooltip: clip_utf16(&tooltip, 127),
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacSummary {
    title: String,
    remaining: String,
    help: String,
    empty_message: &'static str,
}

pub fn mac(state: &State) -> MacSummary {
    let empty_message = if state.accounts.is_empty() {
        "Dashboard에서 계정을 연결하세요."
    } else if state.accounts.iter().any(|a| a.visible) {
        "표시할 계정의 모니터링이 일시 정지되어 있습니다."
    } else {
        "Dashboard의 계정 카드에서 ‘상단바에 표시’를 켜세요."
    };
    let visible: Vec<_> = state
        .accounts
        .iter()
        .filter(|a| a.enabled && a.visible)
        .collect();
    if visible.is_empty() {
        let title = if state.accounts.is_empty() {
            "계정 연결"
        } else if state.accounts.iter().any(|a| a.enabled) {
            "QuotaBar"
        } else {
            "일시 정지"
        };
        return MacSummary {
            title: title.into(),
            remaining: "—".into(),
            help: format!("QuotaBar · {empty_message}"),
            empty_message,
        };
    }
    let mut labels = Vec::new();
    let mut values = Vec::new();
    for name in ["Codex", "Claude", "Gemini"] {
        let accounts: Vec<_> = visible
            .iter()
            .copied()
            .filter(|a| a.provider == name)
            .collect();
        if accounts.is_empty() {
            continue;
        }
        let percent = accounts
            .iter()
            .filter_map(|a| a.lowest())
            .reduce(f64::min)
            .map_or_else(
                || "—".into(),
                |n| {
                    let stale = if accounts.iter().any(|a| a.mac_attention()) {
                        "*"
                    } else {
                        ""
                    };
                    format!("{}%{stale}", n.round())
                },
            );
        labels.push(format!("{name} {percent}"));
        values.push(percent);
    }
    let mut lines = vec![String::from(
        "남은 사용량 · 상단바에 표시한 계정과 사용 기간 중 Provider별 가장 낮은 잔여율",
    )];
    for a in &visible {
        let value = a.lowest().map_or_else(
            || "잔여량 확인 중".to_owned(),
            |n| {
                format!(
                    "{}% 남음{}",
                    n.round(),
                    if a.mac_attention() {
                        " · 이전 값"
                    } else {
                        ""
                    }
                )
            },
        );
        lines.push(format!("{}({}): {value}", a.provider, a.name));
    }
    if visible.iter().any(|a| a.mac_attention()) {
        lines.push("* 일부 계정의 최신 잔여량 확인 필요 · — 조회된 잔여량 없음".into());
    }
    MacSummary {
        title: labels.join(" · "),
        remaining: values.join(" · "),
        help: lines.join("\n"),
        empty_message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_specific_rounding_visibility_and_stale_states() {
        let request = serde_json::json!({"accounts":[
            {"provider":"Claude","name":"개인","enabled":true,"visible":true,"remaining":[54.5,40.0],"error":true},
            {"provider":"Codex","name":"🚀".repeat(13),"enabled":true,"visible":true,"remaining":[16.5],"error":false},
            {"provider":"Gemini","name":"숨김","enabled":true,"visible":false,"remaining":[1.],"error":false}
        ]});
        let view: State = serde_json::from_value(request).unwrap();
        assert_eq!(mac(&view).title, "Codex 17% · Claude 40%*");
        let win = windows(&view);
        assert_eq!(win.text, "17");
        assert!(win.warning && !win.tooltip.contains("숨김"));
        assert!(win.tooltip.contains("🚀🚀🚀🚀🚀🚀🚀🚀…"));
    }
}
