use crate::{
    claude::mac_connection,
    credentials::{Error as Credential, Provider},
    mac_monitor::fetch::failure,
    mac_settings::Error,
};
use objc2::runtime::AnyObject;
use objc2_foundation::{
    NSCocoaErrorDomain, NSDictionary, NSError, NSFilePathErrorKey, NSPOSIXErrorDomain, NSString,
    NSURL, NSURLErrorKey,
};
use std::{
    io::{Error as IoError, ErrorKind},
    path::Path,
};

fn localized(domain: &NSString, code: isize, path: Option<&str>, include_url: bool) -> String {
    unsafe {
        let info = path.map(|path| {
            let name = NSString::from_str(path);
            if include_url {
                let url = NSURL::fileURLWithPath(&name);
                NSDictionary::<NSString, AnyObject>::from_slices(
                    &[NSFilePathErrorKey, NSURLErrorKey],
                    &[&name, &url],
                )
            } else {
                NSDictionary::<NSString, AnyObject>::from_slices(&[NSFilePathErrorKey], &[&name])
            }
        });
        NSError::errorWithDomain_code_userInfo(domain, code, info.as_deref())
            .localizedDescription()
            .to_string()
    }
}

fn cocoa(code: isize) -> String {
    localized(unsafe { NSCocoaErrorDomain }, code, None, false)
}
fn posix(error: IoError, path: Option<&str>) -> String {
    localized(
        unsafe { NSPOSIXErrorDomain },
        error.raw_os_error().unwrap_or(libc::EIO) as isize,
        path,
        false,
    )
}

pub(super) fn message(
    error: Error,
    settings: &Path,
    import: Option<&(Provider, String)>,
) -> String {
    match error {
        Error::Io(e) => posix(e,settings.to_str()),
        Error::Import(e) => {
            let code = match e.kind() { ErrorKind::NotFound => 260, ErrorKind::PermissionDenied => 257, ErrorKind::InvalidInput => 258, _ => 256 };
            localized(unsafe {NSCocoaErrorDomain},code,import.map(|(_,p)| p.as_str()),true)
        }
        Error::Json => cocoa(4864),
        Error::Unavailable => "설정을 복구한 후 다시 시도하세요.".into(),
        Error::WrongAccount => "선택한 계정과 로그인한 계정이 다릅니다. 기존 계정으로 로그인하거나 새 계정으로 연결하세요.".into(),
        Error::MissingRefresh => "갱신 가능한 로그인이 필요합니다. 다시 로그인하세요.".into(),
        Error::Vault => failure::STORAGE.into(),
        Error::LargeImport => "로그인 파일 크기가 너무 큽니다.".into(),
        Error::Credential(Credential::FoundationJson) => cocoa(3840),
        Error::Credential(e) => failure::credential(e, import.map_or(Provider::Codex, |(p,_)| *p)).into(),
        Error::Claude(e) => claude(e),
    }
}

fn claude(error: mac_connection::Error) -> String {
    use mac_connection::Error::*;
    match error {
        Io(e) => return posix(e,None), Json => return cocoa(4864), SettingsJson => return cocoa(3840),
        Foundation(code) => return cocoa(code),
        SettingsObject => "Claude settings.json 必须是 JSON 对象。",
        Directory => "请选择 Claude 专用配置目录，不能使用空路径、相对路径或根目录。",
        Root => "不能使用根目录作为 Claude 配置目录。",
        InvalidId => "Invalid Claude profile reference.",
        Identity => "Claude profile directory no longer matches its reference.",
        Backup => "回传备份无法读取，请先恢复 AIUsage 的 profile.json 备份。",
        Foreign => "现有回传命令属于其他配置，请先恢复原 statusLine。",
        Orphan => "回传配置的原状态栏备份不可用，请恢复原 statusLine 后重新连接。",
        Recursive => "原状态栏备份包含旧回传命令，请恢复原 statusLine 后重新连接。",
        UnsupportedType => "现有状态栏类型不支持串接；配置保持不变。",
        UnsupportedFormat => "现有状态栏格式不支持串接；配置保持不变。",
        Changed => "Claude 설정이 변경되었습니다. 다시 연결하세요.",
        RestoreNeeded => "statusLine이 수정되어 자동 복구하지 못했습니다. 설정에서 해당 연결 명령을 제거한 뒤 다시 삭제하세요.",
        LoginReference => "Claude subscription monitoring requires a configuration reference, not a token.",
        AuthStatus => "Claude Code returned an unsupported login status.",
        MissingDirectory => "Claude CLI에 로그인한 뒤 해당 설정 폴더를 선택하세요.",
    }.into()
}
