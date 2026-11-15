use crate::{
    codex_sessions::{self, managed::FetchError},
    credentials,
    oauth::RotationError,
    provider_http::{Failure, TransportError},
    provider_query,
    windows_login::LoginError,
    windows_settings,
};
use std::io;

pub(super) const CANCELLED: &str = "작업을 취소했습니다.";
pub(super) const IO: &str =
    "파일을 읽거나 저장하지 못했습니다. 경로, 권한과 여유 공간을 확인하세요.";
pub(super) const INVALID: &str = "JSON 파일 형식을 확인하세요. 원본 파일은 보존했습니다.";
pub(super) const UNKNOWN: &str = "작업을 완료하지 못했습니다. 다시 시도하세요.";

pub(super) fn file(e: io::Error) -> String {
    if e.kind() == io::ErrorKind::PermissionDenied {
        "파일 접근 권한을 확인하세요."
    } else {
        IO
    }
    .into()
}

pub(super) fn claude(e: io::Error) -> String {
    if e.get_ref().is_some_and(|e| e.is::<serde_json::Error>()) {
        INVALID.into()
    } else if e.raw_os_error().is_none() && e.kind() == io::ErrorKind::InvalidInput {
        e.to_string()
    } else {
        file(e)
    }
}

fn credential(e: credentials::Error) -> &'static str {
    match e {
        credentials::Error::TooLarge => "로그인 파일이 너무 큽니다.",
        credentials::Error::MissingToken => "API 키 대신 구독 계정의 로그인 파일을 연결하세요.",
        credentials::Error::MissingIdentity => {
            "로그인 파일에서 계정과 워크스페이스를 확인할 수 없습니다."
        }
        _ => INVALID,
    }
}

pub(super) fn settings(e: windows_settings::Error) -> String {
    use windows_settings::Error::*;
    match e {
        Json(_) => INVALID.into(),
        Io(e) | File(e, _) => file(e),
        Credential(e) => credential(e).into(),
        Usage(_, message) => message.into(),
    }
}

pub(super) fn session(e: codex_sessions::Error) -> String {
    use codex_sessions::Error::*;
    match e {
        Credentials(e) => credential(e),
        Missing => "로그인을 다시 연결하세요.",
        Identity => "저장된 계정이 다릅니다.",
        Revoked => "앱에서 다시 로그인하세요.",
        Superseded => CANCELLED,
        Storage | WriteTooLarge => IO,
    }
    .into()
}

pub(super) fn managed(e: FetchError) -> String {
    match e {
        FetchError::Session(e) => session(e),
        FetchError::Provider(e) => provider(e),
        FetchError::Internal => UNKNOWN.into(),
    }
}

pub(super) fn provider(e: Failure) -> String {
    use provider_query::Error::*;
    match e {
        Failure::Transport(t) => match t {
            TransportError::Network(_) => "서버에 연결하지 못했습니다. 네트워크를 확인하세요.",
            TransportError::Cancelled | TransportError::Timeout => CANCELLED,
            TransportError::LargeResponse => "응답이 너무 큽니다.",
        },
        Failure::Query(q) => match q {
            Credentials(e) => credential(e),
            Rotation(RotationError::Credentials(e)) => credential(e),
            Rotation(RotationError::MalformedResponse | RotationError::InvalidResponse) => {
                "갱신 응답에 인증 정보가 없습니다."
            }
            Rotation(RotationError::ReturnedAccountChanged) => {
                "갱신된 계정이 다릅니다. 다시 연결하세요."
            }
            Rotation(RotationError::StoredAccountChanged) => "계정 정체성이 변경되었습니다.",
            Provider(_, message) => return message,
            Json | QueryCredentials(_) => INVALID,
        },
    }
    .into()
}

pub(super) fn login(e: LoginError) -> String {
    match e {
        LoginError::Cancelled | LoginError::Timeout => CANCELLED,
        LoginError::Failed => {
            "로그인이 완료되지 않았습니다. 다른 로그인 창을 닫고 다시 시도하세요."
        }
        LoginError::Launch(_) => "실행 파일을 찾거나 시작하지 못했습니다. CLI 설치를 확인하세요.",
        LoginError::Storage(_) => IO,
    }
    .into()
}
