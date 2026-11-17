use crate::{
    claude::mac_connection,
    codex_sessions::{Error as Session, managed::FetchError},
    credentials::{Error as Credential, Provider},
    mac_settings::imported,
    oauth::RotationError,
    provider_http::{Failure, TransportError},
    provider_query::Error as Query,
};

const READ: &str = "조회에 실패했습니다. 연결한 파일과 앱의 접근 권한을 확인하세요.";
const LOGIN_JSON: &str = "로그인 파일을 읽을 수 없습니다. JSON 형식을 확인하세요.";
pub(in crate::mac_monitor) const STORAGE: &str =
    "앱 전용 로그인 파일에 접근할 수 없습니다. 앱 데이터 폴더의 권한과 저장 공간을 확인하세요.";
const NETWORK: &str = "네트워크 연결을 확인하세요. 마지막 성공 결과를 유지합니다.";

pub(super) enum Error {
    Managed(FetchError),
    Provider(Failure),
    Claude(mac_connection::Error),
    Import(imported::Error),
    Json,
}
impl From<Failure> for Error {
    fn from(error: Failure) -> Self {
        Self::Provider(error)
    }
}

pub(in crate::mac_monitor) fn credential(error: Credential, provider: Provider) -> &'static str {
    match error {
        Credential::TooLarge | Credential::InvalidJson => LOGIN_JSON,
        Credential::FoundationJson => READ,
        Credential::MissingToken => {
            "구독 계정의 CLI 로그인 파일이 필요합니다. API 키는 지원하지 않습니다."
        }
        Credential::MissingIdentity if provider == Provider::Codex => {
            "Codex의 워크스페이스와 사용자 정보를 확인할 수 없습니다. CLI에서 다시 로그인하세요."
        }
        Credential::MissingIdentity => {
            "Gemini 계정 식별 정보가 없습니다. CLI에서 다시 로그인하세요."
        }
        Credential::ClaudeDirectory => "Claude는 CLI 설정 폴더를 연결하세요.",
        Credential::SubscriptionRequired => "ChatGPT 구독 계정으로 로그인하세요.",
    }
}

fn access_denied(managed: bool) -> &'static str {
    if managed {
        "계정 접근이 거부되었습니다. 앱에서 다시 로그인하세요."
    } else {
        "로그인이 만료되었거나 접근이 거부되었습니다. Codex는 앱에서 다시 로그인하거나, 해당 CLI의 로그인 파일을 갱신하세요."
    }
}

fn provider_error(error: Failure, provider: Provider, managed: bool) -> String {
    match error {
        Failure::Transport(TransportError::Network(-1001) | TransportError::Timeout) =>
            "조회 시간이 초과되었습니다. 잠시 후 다시 시도하세요.",
        Failure::Transport(TransportError::Network(_) | TransportError::Cancelled) => NETWORK,
        Failure::Transport(TransportError::LargeResponse) => LOGIN_JSON,
        Failure::Query(Query::Credentials(e) | Query::Rotation(RotationError::Credentials(e))) => credential(e, provider),
        Failure::Query(Query::Rotation(RotationError::ReturnedAccountChanged)) =>
            "갱신된 인증의 계정이 달라 연결을 중단했습니다. 다시 로그인하세요.",
        Failure::Query(Query::Rotation(RotationError::StoredAccountChanged)) =>
            "갱신된 로그인 정보를 기존 계정에 연결할 수 없습니다.",
        Failure::Query(Query::Rotation(RotationError::MalformedResponse | RotationError::InvalidResponse)) =>
            "로그인 갱신 응답에 필요한 인증 정보가 없습니다.",
        Failure::Query(Query::QueryCredentials(-11)) if provider == Provider::Codex => "Codex login could not be parsed.",
        Failure::Query(Query::QueryCredentials(-11)) => "Gemini login must be a JSON object.",
        Failure::Query(Query::QueryCredentials(-12)) if provider == Provider::Codex => "Codex login exists but has no access token.",
        Failure::Query(Query::QueryCredentials(-12)) => access_denied(managed),
        Failure::Query(Query::QueryCredentials(-13)) => "ChatGPT subscription login is required; API keys do not expose subscription quota.",
        Failure::Query(Query::QueryCredentials(-14)) => READ,
        Failure::Query(Query::QueryCredentials(_) | Query::Json) => LOGIN_JSON,
        Failure::Query(Query::Provider("unauthorized" | "not_logged_in", _)) => access_denied(managed),
        Failure::Query(Query::Provider("gemini_individual_discontinued", _)) =>
            "이 계정의 Gemini CLI 접근이 지원되지 않습니다. Code Assist Standard 또는 Enterprise가 필요합니다.",
        Failure::Query(Query::Provider(_, message)) => return message,
    }.to_owned()
}

impl Error {
    pub(super) fn message(self, provider: Provider, managed: bool) -> Option<String> {
        let message = match self {
            Self::Managed(FetchError::Provider(e)) | Self::Provider(e) => {
                return Some(provider_error(e, provider, managed));
            }
            Self::Managed(FetchError::Session(Session::Missing)) => "앱에서 다시 로그인하세요.",
            Self::Managed(FetchError::Session(Session::Identity)) => {
                "저장된 계정 정보가 일치하지 않습니다."
            }
            Self::Managed(FetchError::Session(Session::Revoked)) => {
                "로그인이 만료되거나 해제되었습니다. 앱에서 다시 로그인하세요."
            }
            Self::Managed(FetchError::Session(Session::Superseded)) => return None,
            Self::Managed(FetchError::Session(Session::Storage | Session::WriteTooLarge))
            | Self::Import(imported::Error::Vault) => STORAGE,
            Self::Managed(FetchError::Session(Session::Credentials(e)))
            | Self::Import(imported::Error::Credential(e)) => credential(e, provider),
            Self::Managed(FetchError::Internal) => LOGIN_JSON,
            Self::Import(imported::Error::Missing) => {
                "저장된 로그인이 없습니다. 계정을 다시 연결하세요."
            }
            Self::Import(imported::Error::Mismatch) => {
                "저장된 로그인과 계정 정보가 일치하지 않습니다. 다시 연결하세요."
            }
            Self::Claude(mac_connection::Error::InvalidId) => "Invalid Claude profile reference.",
            Self::Claude(mac_connection::Error::Identity) => {
                "Claude profile directory no longer matches its reference."
            }
            Self::Claude(mac_connection::Error::SettingsObject) => {
                "Claude settings.json 必须是 JSON 对象。"
            }
            Self::Claude(mac_connection::Error::Backup) => {
                "回传备份无法读取，请先恢复 AIUsage 的 profile.json 备份。"
            }
            Self::Claude(_) | Self::Json => READ,
        };
        Some(message.to_owned())
    }
}
