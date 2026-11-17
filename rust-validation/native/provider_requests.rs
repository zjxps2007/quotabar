//! Provider requests and discovery decisions. Each request owns its headers
//! so concurrent accounts sharing the same endpoint cannot share a token.
use crate::credentials::{self, Platform};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Endpoint {
    Codex,
    Refresh,
    GeminiAssist,
    GeminiProjects,
    GeminiQuota,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parameters {
    pub endpoint: Endpoint,
    pub token: String,
    pub account: Option<String>,
    pub project: Option<String>,
    pub version: Option<String>,
    pub timeout: f64,
}
impl Drop for Parameters {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}
#[derive(Serialize)]
pub struct Request {
    pub url: String,
    pub method: &'static str,
    pub headers: BTreeMap<&'static str, String>,
    pub body: Option<Value>,
    pub timeout: f64,
}
impl Drop for Request {
    fn drop(&mut self) {
        if let Some(bearer) = self.headers.get_mut("Authorization") {
            bearer.zeroize();
        }
        if let Some(Value::String(refresh)) =
            self.body.as_mut().and_then(|b| b.get_mut("refresh_token"))
        {
            refresh.zeroize();
        }
    }
}
pub const CODEX_CLIENT: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub fn build(params: &Parameters, platform: Platform) -> Request {
    use Endpoint::*;
    let (url, body) = match params.endpoint {
        Codex => ("https://chatgpt.com/backend-api/wham/usage", None),
        Refresh => (
            "https://auth.openai.com/oauth/token",
            Some(
                json!({"grant_type":"refresh_token","client_id":CODEX_CLIENT,"refresh_token":params.token}),
            ),
        ),
        GeminiAssist => (
            "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist",
            Some(json!({"metadata":{"ideType":"GEMINI_CLI","pluginType":"GEMINI"}})),
        ),
        GeminiProjects => (
            "https://cloudresourcemanager.googleapis.com/v1/projects",
            None,
        ),
        GeminiQuota => (
            "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota",
            Some(
                params
                    .project
                    .as_ref()
                    .map_or_else(|| json!({}), |p| json!({"project":p})),
            ),
        ),
    };
    let mut headers = BTreeMap::new();
    if platform == Platform::Windows {
        headers.insert(
            "User-Agent",
            format!(
                "QuotaBar/{}",
                params
                    .version
                    .as_deref()
                    .unwrap_or(env!("QUOTABAR_VERSION"))
            ),
        );
        headers.insert("Accept", "application/json".into());
        headers.insert("Cache-Control", "no-store, no-cache".into());
        headers.insert("Pragma", "no-cache".into());
    } else {
        if matches!(params.endpoint, Codex | Refresh | GeminiProjects) {
            headers.insert("Cache-Control", "no-cache, no-store, max-age=0".into());
            headers.insert("Pragma", "no-cache".into());
        }
        if params.endpoint == Codex {
            headers.insert("User-Agent", "QuotaBar".into());
            headers.insert("Accept", "application/json".into());
        }
    }
    if params.endpoint != Refresh {
        headers.insert("Authorization", format!("Bearer {}", params.token));
    }
    if params.endpoint == Codex
        && let Some(id) = &params.account
    {
        headers.insert("ChatGPT-Account-Id", id.clone());
    }
    if body.is_some() {
        headers.insert("Content-Type", "application/json".into());
    }
    Request {
        url: url.into(),
        method: if body.is_some() { "POST" } else { "GET" },
        headers,
        body,
        timeout: if matches!(params.endpoint, GeminiAssist | GeminiProjects) {
            5.
        } else {
            params.timeout
        },
    }
}

#[derive(Serialize)]
pub struct Discovery<'a> {
    pub project: Option<&'a str>,
    pub tier: Option<&'a str>,
}
pub fn discovery(data: &Value, list: bool, platform: Platform) -> Discovery<'_> {
    if list {
        let projects = data["projects"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut project = None;
        if platform == Platform::Windows || projects.iter().all(Value::is_object) {
            for entry in projects {
                let id = entry["projectId"].as_str();
                if (id.is_some() || platform == Platform::Windows)
                    && (id.is_some_and(|s| s.starts_with("gen-lang-client"))
                        || entry["labels"].get("generative-language").is_some())
                {
                    project = id;
                    break;
                }
            }
        }
        return Discovery {
            project,
            tier: None,
        };
    }
    let field = &data["cloudaicompanionProject"];
    let project = field
        .as_str()
        .filter(|s| platform == Platform::Windows || !s.is_empty())
        .or(field["id"].as_str())
        .or_else(|| {
            (platform == Platform::Mac)
                .then(|| field["projectId"].as_str())
                .flatten()
        });
    Discovery {
        project,
        tier: data["currentTier"]["id"].as_str(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryCredentials {
    pub access_token: String,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    pub user_id: Option<String>,
}
impl Drop for QueryCredentials {
    fn drop(&mut self) {
        self.access_token.zeroize();
    }
}
pub fn mac_query(bytes: &[u8], codex: bool) -> Result<QueryCredentials, i32> {
    let json =
        credentials::document(bytes, Platform::Mac).map_err(|_| if codex { -11 } else { -14 })?;
    if !json.is_object() {
        return Err(if codex || json.is_array() { -11 } else { -14 });
    }
    if !codex {
        let access = json["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(-12)?;
        let claims = credentials::claims(json["id_token"].as_str(), Platform::Mac);
        return Ok(QueryCredentials {
            access_token: access.into(),
            account_id: None,
            user_id: None,
            plan: None,
            email: claims["email"]
                .as_str()
                .or(json["email"].as_str())
                .map(str::to_owned),
        });
    }
    let string = |v: &Value| v.as_str().and_then(credentials::trimmed).map(str::to_owned);
    if string(&json["OPENAI_API_KEY"]).is_some() {
        return Err(-13);
    }
    let sources = [&json["tokens"], &json];
    let token = |snake: &str, camel: &str| {
        sources
            .iter()
            .find_map(|v| string(&v[snake]).or_else(|| string(&v[camel])))
    };
    let access = token("access_token", "accessToken").ok_or(-12)?;
    let id = token("id_token", "idToken");
    let claims = [
        credentials::claims(id.as_deref(), Platform::Mac),
        credentials::claims(Some(&access), Platform::Mac),
    ];
    let workspace = credentials::mac_workspace(&json);
    let email = string(&json["email"]).or_else(|| {
        claims.iter().find_map(|c| {
            string(&c["email"]).or_else(|| string(&c["https://api.openai.com/profile"]["email"]))
        })
    });
    let plan = claims
        .iter()
        .find_map(|c| string(&c[credentials::AUTH]["chatgpt_plan_type"]));
    Ok(QueryCredentials {
        access_token: access,
        account_id: workspace.account_id,
        email,
        plan,
        user_id: workspace.user_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adapters_keep_bearer_and_refresh_separate_and_discover_projects_in_order() {
        let mut p = Parameters {
            endpoint: Endpoint::Codex,
            token: "first".into(),
            account: Some("workspace".into()),
            project: None,
            version: Some("1.2.3".into()),
            timeout: 20.,
        };
        let first = build(&p, Platform::Windows);
        p.token = "second".into();
        p.account = Some("other".into());
        let second = build(&p, Platform::Windows);
        assert_eq!(first.headers["Authorization"], "Bearer first");
        assert_eq!(second.headers["ChatGPT-Account-Id"], "other");
        assert!(!first.headers.contains_key("Content-Type"));
        for platform in [Platform::Mac, Platform::Windows] {
            p.endpoint = Endpoint::Refresh;
            let request = build(&p, platform);
            assert!(!request.headers.contains_key("Authorization"));
            assert!(!request.headers.contains_key("ChatGPT-Account-Id"));
            assert_eq!(request.body.as_ref().unwrap()["refresh_token"], "second");
            p.endpoint = Endpoint::GeminiAssist;
            assert_eq!(build(&p, platform).timeout, 5.);
        }
        let list = json!({"projects":[{"labels":{"generative-language":null}},{"projectId":"gen-lang-client-selected"}]});
        assert_eq!(discovery(&list, true, Platform::Windows).project, None);
        assert_eq!(
            discovery(&list, true, Platform::Mac).project,
            Some("gen-lang-client-selected")
        );
        let query = mac_query(
            br#"{"tokens":{"access_token":" token "},"email":"Display@Example.invalid"}"#,
            true,
        )
        .unwrap();
        assert_eq!(query.access_token, "token");
        assert_eq!(query.email.as_deref(), Some("Display@Example.invalid"));
    }
}
