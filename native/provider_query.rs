//! One account's quota lookup or OAuth exchange, from credential selection to the
//! final snapshot. Each call to accept consumes the response for the current request.
use crate::{
    credentials::{self, Platform, Provider},
    oauth::{self, RotationError},
    provider_requests::{self, Endpoint, Parameters, Request},
    quota,
};
use chrono::{DateTime, Utc};
use serde_json::json;
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq)]
pub enum Operation {
    Codex,
    Gemini,
    Refresh,
}

#[derive(Debug)]
pub enum Error {
    Credentials(credentials::Error),
    QueryCredentials(i32),
    Rotation(RotationError),
    Json,
    Provider(&'static str, String),
}

impl Error {
    fn provider(code: &'static str, message: impl Into<String>) -> Self {
        Self::Provider(code, message.into())
    }
}

pub const GEMINI_DISCONTINUED: &str = "Google stopped serving Gemini CLI for personal Google accounts (free, Google AI Pro and Ultra) on June 18, 2026. Only Gemini Code Assist Standard or Enterprise licenses still work — track personal accounts with Antigravity instead.";

pub fn discontinued(body: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(body).to_ascii_lowercase();
    lower.contains("unsupported_client")
        || lower.contains("no longer supported for gemini code assist for individuals")
}

pub struct Query {
    platform: Platform,
    parameters: Parameters,
    email: Option<String>,
    user_id: Option<String>,
    jwt_plan: Option<String>,
    tier: Option<String>,
    original: Option<Zeroizing<Vec<u8>>>,
}

impl Query {
    pub fn new(
        op: Operation,
        auth: &[u8],
        platform: Platform,
        timeout: f64,
    ) -> Result<Self, Error> {
        if auth.len() > crate::vault::MAX_SIZE
            && (platform == Platform::Windows || op == Operation::Refresh)
        {
            return Err(Error::Credentials(credentials::Error::TooLarge));
        }
        let codex = op != Operation::Gemini;
        let provider = if codex {
            Provider::Codex
        } else {
            Provider::Gemini
        };
        let (token, account, email, user_id, jwt_plan) =
            if platform == Platform::Mac && op != Operation::Refresh {
                let mut parsed =
                    provider_requests::mac_query(auth, codex).map_err(Error::QueryCredentials)?;
                (
                    std::mem::take(&mut parsed.access_token),
                    parsed.account_id.take(),
                    parsed.email.take(),
                    parsed.user_id.take(),
                    parsed.plan.take(),
                )
            } else {
                let mut parsed = if op == Operation::Refresh {
                    oauth::managed(auth, platform)
                } else {
                    credentials::parse(auth, provider, platform)
                }
                .map_err(Error::Credentials)?;
                let token = if op == Operation::Refresh {
                    parsed.refresh_token.take().ok_or_else(|| {
                        Error::provider(
                            "reauth_required",
                            match platform {
                                Platform::Mac => {
                                    "자동 갱신할 수 없는 로그인입니다. 앱에서 다시 로그인하세요."
                                }
                                Platform::Windows => "앱에서 다시 로그인하세요.",
                            },
                        )
                    })?
                } else {
                    std::mem::take(&mut parsed.access_token)
                };
                (
                    token,
                    parsed.account_id.take(),
                    parsed.email.take(),
                    parsed.user_id.take(),
                    None,
                )
            };
        let endpoint = match op {
            Operation::Codex => Endpoint::Codex,
            Operation::Gemini => Endpoint::GeminiAssist,
            Operation::Refresh => Endpoint::Refresh,
        };
        Ok(Self {
            platform,
            parameters: Parameters {
                endpoint,
                token,
                account,
                project: None,
                version: None,
                timeout,
            },
            email,
            user_id,
            jwt_plan,
            tier: None,
            original: (op == Operation::Refresh).then(|| Zeroizing::new(auth.to_vec())),
        })
    }

    pub fn request(&self) -> Request {
        provider_requests::build(&self.parameters, self.platform)
    }

    pub fn refreshing(&self) -> bool {
        self.original.is_some()
    }

    pub fn optional(&self) -> bool {
        matches!(
            self.parameters.endpoint,
            Endpoint::GeminiAssist | Endpoint::GeminiProjects
        )
    }

    fn next_gemini_request(&mut self) {
        self.parameters.endpoint = if self.parameters.endpoint == Endpoint::GeminiAssist
            && self.parameters.project.is_none()
        {
            Endpoint::GeminiProjects
        } else {
            Endpoint::GeminiQuota
        };
    }

    /// A transport error is recoverable only during optional project discovery.
    pub fn skip_discovery(&mut self) -> bool {
        if !self.optional() {
            return false;
        }
        self.next_gemini_request();
        true
    }

    fn http_error(&self, status: u16, body: &[u8]) -> Error {
        let refresh = self.parameters.endpoint == Endpoint::Refresh;
        let revoked = refresh && matches!(status, 400 | 401 | 403);
        let denied = matches!(status, 401 | 403);
        if self.platform == Platform::Windows {
            if revoked {
                return Error::provider(
                    "reauth_required",
                    "로그인이 해제되었습니다. 다시 로그인하세요.",
                );
            }
            return Error::provider(
                if denied { "unauthorized" } else { "http" },
                format!("사용량 서버 응답: HTTP {status}. 마지막 성공 값을 표시합니다."),
            );
        }
        if refresh {
            return if revoked {
                Error::provider(
                    "reauth_required",
                    "로그인이 만료되거나 해제되었습니다. 앱에서 다시 로그인하세요.",
                )
            } else {
                Error::provider(
                    "oauth_unavailable",
                    "로그인 갱신이 일시적으로 실패했습니다. 다음 조회에서 다시 시도합니다.",
                )
            };
        }
        let codex = self.parameters.endpoint == Endpoint::Codex;
        if denied {
            return if codex {
                Error::provider("unauthorized", "Codex OAuth token is invalid or expired.")
            } else if status == 403 && discontinued(body) {
                Error::provider("gemini_individual_discontinued", GEMINI_DISCONTINUED)
            } else {
                Error::provider(
                    "not_logged_in",
                    "Gemini OAuth token is invalid or expired. Re-authenticate with `gemini`.",
                )
            };
        }
        let service = if codex { "Codex usage" } else { "Gemini quota" };
        Error::provider(
            "api_error",
            format!("{service} API returned HTTP {status}."),
        )
    }

    pub fn accept(
        &mut self,
        status: u16,
        body: &[u8],
        now: DateTime<Utc>,
    ) -> Result<Option<Vec<u8>>, Error> {
        if status == 0 && self.platform == Platform::Mac && self.refreshing() {
            return Err(Error::provider(
                "oauth_response",
                "로그인 갱신 응답을 확인할 수 없습니다.",
            ));
        }
        let ok = (200..300).contains(&status) || (status == 0 && self.platform == Platform::Mac);
        if self.optional() {
            if ok || self.platform == Platform::Mac {
                match credentials::document(body, self.platform) {
                    Ok(json) => {
                        let found = provider_requests::discovery(
                            &json,
                            self.parameters.endpoint == Endpoint::GeminiProjects,
                            self.platform,
                        );
                        self.parameters.project = found.project.map(str::to_owned);
                        if self.parameters.endpoint == Endpoint::GeminiAssist {
                            self.tier = found.tier.map(str::to_owned);
                        }
                    }
                    Err(_) if self.platform == Platform::Windows => return Err(Error::Json),
                    Err(_) => {}
                }
            }
            self.next_gemini_request();
            return Ok(None);
        }
        if !ok {
            return Err(self.http_error(status, body));
        }
        if let Some(original) = &self.original {
            return oauth::rotate(original, body, now, self.platform)
                .map(Some)
                .map_err(|error| match error {
                    RotationError::MalformedResponse if self.platform == Platform::Windows => {
                        Error::Json
                    }
                    error => Error::Rotation(error),
                });
        }
        let codex = self.parameters.endpoint == Endpoint::Codex;
        let response = credentials::document(body, self.platform)
            .ok()
            .filter(|v| self.platform == Platform::Windows || v.is_object())
            .ok_or_else(|| {
                if self.platform == Platform::Windows {
                    Error::Json
                } else {
                    Error::provider(
                        "parse_failed",
                        if codex {
                            "Codex usage API returned invalid JSON."
                        } else {
                            "Gemini quota API returned invalid JSON."
                        },
                    )
                }
            })?;
        let request = quota::Request {
            provider: if codex { "codex" } else { "gemini" }.into(),
            response,
            email: self.email.clone(),
            account_id: self.parameters.account.clone(),
            user_id: self.user_id.clone(),
            jwt_plan: self.jwt_plan.clone(),
            tier: self.tier.clone(),
            project: self.parameters.project.clone(),
            source: Some(
                json!({"mode":"read-only","type":if codex { "codex-auth-json" } else { "gemini-cli" }}),
            ),
        };
        let format = if self.platform == Platform::Windows {
            quota::Format::Windows
        } else {
            quota::Format::Mac
        };
        let mut output = quota::parse(&request, format, now).map_err(|_| Error::Json)?;
        if !codex && self.platform == Platform::Windows {
            output["email"] = json!(self.email);
            output["plan"] = json!(self.tier.as_deref().unwrap_or("Code Assist"));
        }
        serde_json::to_vec(&output)
            .map(Some)
            .map_err(|_| Error::Json)
    }
}
