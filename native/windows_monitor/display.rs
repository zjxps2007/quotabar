use crate::{
    credentials::Provider,
    tray,
    windows_settings::{Account, Settings},
};
use chrono::{Days, Duration, Local, TimeZone, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

pub(super) fn summary(state: &Value) -> tray::WindowsSummary {
    let accounts = state["settings"]["accounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| {
            let id = a["id"].as_str().unwrap_or_default();
            let snapshot = &state["snapshots"][id];
            tray::Account {
                provider: match a["provider"].as_str() {
                    Some("codex") => "Codex",
                    Some("claude") => "Claude",
                    Some("gemini") => "Gemini",
                    _ => "",
                }
                .into(),
                name: a["name"].as_str().unwrap_or_default().into(),
                enabled: a["enabled"] == true,
                visible: a["showInTray"] == true,
                remaining: snapshot["windows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| w["remaining"].as_f64())
                    .collect(),
                error: state["errors"].get(id).is_some(),
                state: snapshot["state"].as_str().map(str::to_owned),
            }
        })
        .collect();
    tray::windows(&tray::State {
        accounts,
        demo: state["demo"] == true,
        icon: state["settings"]["trayDisplayMode"] == "icon",
    })
}

pub(super) fn demo(probe: bool) -> Value {
    let now = if probe {
        Utc.with_ymd_and_hms(2099, 1, 15, 12, 0, 0).unwrap()
    } else {
        Utc::now()
    };
    let mut settings = Settings::default();
    let mut snapshots = Map::new();
    for (i, (provider, name)) in [
        (Provider::Codex, "개인 계정 · 샘플"),
        (Provider::Claude, "작업 계정 · 샘플"),
        (Provider::Gemini, "Code Assist · 샘플"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = Uuid::new_v4().to_string();
        settings.accounts.push(Account {
            id: id.clone(),
            provider,
            name: name.into(),
            identity: format!("demo-{}", i + 1),
            source_path: Some(String::new()),
            enabled: true,
            show_in_tray: true,
            app_managed_login: false,
        });
        let weekly = 16 + i * 23;
        snapshots.insert(id,json!({"fetchedAt":now,"windows":[{"title":"5시간","remaining":38+i*18,"resetAt":now+Duration::hours(3)},
            {"title":"주간","remaining":weekly,"resetAt":now+Duration::days(4)}],"plan":"샘플","email":null,"observedAt":null,"state":null,"lowest":weekly}));
    }
    let clock = Local::now();
    let mut days = vec![];
    for i in 0..182 {
        if i % 7 == 3 || i % 11 == 0 {
            continue;
        }
        let day = clock.date_naive().checked_sub_days(Days::new(i)).unwrap();
        let input = 10000 + i * 7927 % 320000;
        let output = 3000 + i * 70;
        for (provider, model) in [("codex", "gpt-5.4"), ("claude", "claude-sonnet-4-6")] {
            days.push(json!({"day":day,"provider":provider,"model":model,"counts":{"input":input,"cached":i*200,"output":output,"total":input+output}}));
        }
    }
    json!({"settings":settings,"snapshots":snapshots,"errors":{},"loading":[],"message":null,"demo":true,
        "activity":{"days":days,"scannedFiles":247,"warnings":[],"scannedAt":clock},"scanning":false,"loggingIn":false,
        "dataDirectory":null,"defaultPaths":null,"launchAtLogin":"disabled","appUpdate":null})
}
