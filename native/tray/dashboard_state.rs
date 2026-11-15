use super::{MacPeriod, MacPopoverAccount, MacPopoverSource, mac_popover_view};
use crate::credentials::Provider;
use serde::{
    Deserialize, Deserializer,
    de::{Error, IgnoredAny, SeqAccess, Visitor},
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    marker::PhantomData,
};

// Foundation encodes dictionaries keyed by UUID as alternating keys and values.
fn uuid_dictionary<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    input: D,
) -> Result<HashMap<String, T>, D::Error> {
    struct Pairs<T>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Pairs<T> {
        type Value = HashMap<String, T>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("alternating UUIDs and values")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut result = HashMap::new();
            while let Some(id) = seq.next_element()? {
                let value = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::custom("missing UUID dictionary value"))?;
                result.insert(id, value);
            }
            Ok(result)
        }
    }
    input.deserialize_seq(Pairs(PhantomData))
}

#[derive(Deserialize)]
struct Event {
    state: Payload,
}
#[derive(Deserialize)]
struct Payload {
    _0: State,
}
#[derive(Deserialize)]
struct State {
    settings: Settings,
    #[serde(deserialize_with = "uuid_dictionary")]
    snapshots: HashMap<String, Snapshot>,
    #[serde(deserialize_with = "uuid_dictionary")]
    errors: HashMap<String, IgnoredAny>,
    loading: HashSet<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    accounts: Vec<Account>,
    appearance: Option<String>,
    menu_bar_display_mode: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Account {
    id: String,
    name: String,
    provider: Provider,
    enabled: bool,
    show_in_menu_bar: Option<bool>,
}
#[derive(Default, Deserialize)]
struct Snapshot {
    state: Option<String>,
    windows: Vec<MacPeriod>,
}

/// Only the resident menu's fields are deserialized. The dashboard receives the
/// same frame, including its activity report, unchanged.
pub fn mac_dashboard_popover(bytes: &[u8], demo: bool) -> serde_json::Result<serde_json::Value> {
    let mut state = serde_json::from_slice::<Event>(bytes)?.state._0;
    let accounts = state
        .settings
        .accounts
        .into_iter()
        .map(|account| {
            let snapshot = state.snapshots.remove(&account.id).unwrap_or_default();
            let provider = match account.provider {
                Provider::Codex => "Codex",
                Provider::Claude => "Claude",
                Provider::Gemini => "Gemini",
            };
            MacPopoverAccount {
                name: account.name,
                provider: provider.into(),
                enabled: account.enabled,
                visible: account.show_in_menu_bar.unwrap_or(true),
                error: state.errors.contains_key(&account.id),
                loading: state.loading.contains(&account.id),
                state: snapshot.state,
                windows: snapshot.windows,
            }
        })
        .collect();
    Ok(mac_popover_view(MacPopoverSource {
        accounts,
        demo,
        appearance: state.settings.appearance.unwrap_or_else(|| "system".into()),
        mode: state
            .settings
            .menu_bar_display_mode
            .unwrap_or_else(|| "providerAndRemaining".into()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn resident_menu_projects_foundation_dictionaries_in_account_order() {
        let mut event = json!({"state":{"_0":{
            "settings":{"accounts":[
                {"id":"B","provider":"gemini","name":"Second","enabled":true},
                {"id":"A","provider":"codex","name":"First","enabled":true},
                {"id":"C","provider":"claude","name":"Hidden","enabled":true,"showInMenuBar":false}]},
            "snapshots":["A",{"windows":[{"title":"5시간","remaining":40.0,"resetAt":123.0}]}],
            "loading":["B"],"errors":["A","sample"],"demoReport":{"unused":true}
        }}});
        let view = mac_dashboard_popover(&serde_json::to_vec(&event).unwrap(), true).unwrap();
        let card = &view["accounts"];
        assert_eq!(card.as_array().unwrap().len(), 2);
        assert_eq!(card[0]["name"], "Second");
        assert_eq!(card[0]["loading"], true);
        assert_eq!(card[1]["provider"], "Codex");
        assert_eq!(card[1]["stale"], true);
        assert_eq!(card[1]["windows"][0]["resetAt"], 123.0);
        assert_eq!(view["demo"], true);
        assert_eq!(view["mode"], "providerAndRemaining");
        assert_eq!(view["appearance"], "system");
        event["state"]["_0"]["snapshots"] = json!(["A"]);
        assert!(mac_dashboard_popover(&serde_json::to_vec(&event).unwrap(), false).is_err());
    }
}
