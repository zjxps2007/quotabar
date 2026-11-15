import CQuotaBar
import Foundation

public enum SettingsEdit: Encodable {
    case interval(seconds: Int), autoRefresh(enabled: Bool), automaticUpdates(enabled: Bool)
    case appearance(mode: AppearanceMode), showDockIcon(visible: Bool), menuBarDisplayMode(mode: MenuBarDisplayMode)
    case scanLocalLogs(enabled: Bool), toggle(id: UUID), menuVisibility(id: UUID, visible: Bool)
    case importLogin(provider: ProviderKind, path: String, alias: String)
    case managedLogin(alias: String, target: MonitoredAccount?)
    case claude(identity: String, sourcePath: String, label: String, alias: String)
    case bindClaude(directory: String, helper: String, alias: String), removeClaude(id: UUID, identity: String)
    case remove(id: UUID, deleteCredential: Bool)
}

public enum SettingsStorage: String, Encodable { case disk, memory, unavailable }

public struct SettingsChange: Decodable {
    public let settings: LiteSettings
    public let changed: Bool
    private let index: Int?
    public var account: MonitoredAccount? { index.map { settings.accounts[$0] } }
}

public struct SettingsFile: Sendable {
    public let url: URL
    public init(url: URL) { self.url = url }

    private enum Request: Encodable {
        case load
        case save(settings: LiteSettings)
        case edit(settings: LiteSettings, edit: SettingsEdit, storage: SettingsStorage)
    }

    public func load() throws -> LiteSettings { try send(.load).settings }
    public func save(_ value: LiteSettings) throws { _ = try send(.save(settings: value)) }

    public func apply(_ edit: SettingsEdit, to settings: LiteSettings, storage: SettingsStorage,
                      vault: SecretVault? = nil, credential: Data = Data()) throws -> SettingsChange {
        try send(.edit(settings: settings, edit: edit, storage: storage), vault: vault, credential: credential)
    }

    private func send(_ request: Request, vault: SecretVault? = nil, credential: Data = Data()) throws -> SettingsChange {
        let json = try JSONEncoder().encode(request)
        var output: UnsafeMutablePointer<CChar>?
        func call(_ directory: UnsafePointer<CChar>?) -> Int32 {
            json.withUnsafeBytes { args in
                credential.withUnsafeBytes { secret in
                    qb_mac_settings(url.path, directory, args.bindMemory(to: UInt8.self).baseAddress, args.count,
                        secret.bindMemory(to: UInt8.self).baseAddress, secret.count, &output)
                }
            }
        }
        let status = vault.map { $0.directory.path.withCString(call) } ?? call(nil)
        guard status == 0, let output else {
            var provider: ProviderKind = .codex
            if case .edit(_, .importLogin(let kind, let path, _), _) = request {
                provider = kind
                let code: CocoaError.Code?
                switch status {
                case -26: code = .fileReadNoSuchFile
                case -27: code = .fileReadNoPermission
                case -28: code = .fileReadInvalidFileName
                case -29: code = .fileReadUnknown
                default: code = nil
                }
                if let code {
                    throw CocoaError(code, userInfo: [NSFilePathErrorKey: path, NSURLErrorKey: URL(fileURLWithPath: path)])
                }
            }
            switch status {
            case -57 ... -40, ...(-10000): throw NativeClaude.failure(status)
            case -20: throw CocoaError(.coderReadCorrupt)
            case -21: throw ProviderError("settings_unavailable", "설정을 복구한 후 다시 시도하세요.")
            case -22: throw ProviderError("wrong_login_account", "선택한 계정과 로그인한 계정이 다릅니다. 기존 계정으로 로그인하거나 새 계정으로 연결하세요.")
            case -23: throw ProviderError("missing_refresh", "갱신 가능한 로그인이 필요합니다. 다시 로그인하세요.")
            case -24: throw ProviderError("credential_storage", "앱 전용 로그인 파일에 접근할 수 없습니다. 앱 데이터 폴더의 권한과 저장 공간을 확인하세요.")
            case -25: throw ProviderError("large_auth", "로그인 파일 크기가 너무 큽니다.")
            case -14, -7 ... -2: throw NativeCredentials.failure(status, provider: provider)
            default: throw NSError(domain: NSPOSIXErrorDomain, code: status > 0 ? Int(status) : Int(POSIXErrorCode.EIO.rawValue),
                                   userInfo: [NSFilePathErrorKey: url.path])
            }
        }
        defer { qb_string_free(output) }
        return try JSONDecoder().decode(SettingsChange.self, from: Data(bytes: output, count: strlen(output)))
    }
}
