import CQuotaBar
import Foundation

enum NativeClaude {
    enum Request: Encodable {
        case reference(directory: String, name: String), validate(directory: String)
        case folder(id: String), load(id: String), save(profile: ClaudeSubscriptionProfile), connected
        case install(directory: String, name: String, helper: String, renew: Bool)
        case uninstall(id: String, forRemoval: Bool)
        case rollback(installed: ClaudeSubscriptionProfile, previous: ClaudeSubscriptionProfile?)
        case preserve(directory: String, activeObject: Bool, incomingCommand: String?)
        case routes(directory: String)
        case fetch(directory: String, authMethod: String, sourceKind: String?, now: Double)
        case plan(raw: String?), authStatus(data: Data)
        case quote(value: String), keys(model: Bool), settingsOverride
        case defaultDirectory(directory: String, home: String), terminal(directory: String, home: String)
    }

    static func call<T: Decodable>(_ request: Request, root: String = "") throws -> T {
        let bytes = try JSONEncoder().encode(request)
        var output: UnsafeMutablePointer<CChar>?
        let status = bytes.withUnsafeBytes {
            qb_mac_claude(root, $0.bindMemory(to: UInt8.self).baseAddress, $0.count, &output)
        }
        guard status == 0, let output else { throw failure(status) }
        defer { qb_string_free(output) }
        return try JSONDecoder().decode(T.self, from: Data(bytes: output, count: strlen(output)))
    }

    // These pure projections accept every Swift String. A decoding failure here is an ABI mismatch.
    static func value<T: Decodable>(_ request: Request) -> T { try! call(request) }

    static func failure(_ status: Int32) -> Error {
        switch status {
        case -40: return DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "Invalid Claude profile."))
        case -41: return NSError(domain: NSCocoaErrorDomain, code: 3840)
        case -42: return ProviderError("invalid_settings", "Claude settings.json 必须是 JSON 对象。")
        case -43: return ProviderError("invalid_directory", "请选择 Claude 专用配置目录，不能使用空路径、相对路径或根目录。")
        case -44: return ProviderError("invalid_directory", "不能使用根目录作为 Claude 配置目录。")
        case -45: return ProviderError("invalid_profile", "Invalid Claude profile reference.")
        case -46: return ProviderError("invalid_profile", "Claude profile directory no longer matches its reference.")
        case -47: return ProviderError("statusline_recovery_required", "回传备份无法读取，请先恢复 AIUsage 的 profile.json 备份。")
        case -48: return ProviderError("statusline_recovery_required", "现有回传命令属于其他配置，请先恢复原 statusLine。")
        case -49: return ProviderError("statusline_recovery_required", "回传配置的原状态栏备份不可用，请恢复原 statusLine 后重新连接。")
        case -50: return ProviderError("statusline_recovery_required", "原状态栏备份包含旧回传命令，请恢复原 statusLine 后重新连接。")
        case -51: return ProviderError("unsupported_statusline", "现有状态栏类型不支持串接；配置保持不变。")
        case -52: return ProviderError("unsupported_statusline", "现有状态栏格式不支持串接；配置保持不变。")
        case -53: return ProviderError("settings_changed", "Claude 설정이 변경되었습니다. 다시 연결하세요.")
        case -54: return ProviderError("restore_needed", "statusLine이 수정되어 자동 복구하지 못했습니다. 설정에서 해당 연결 명령을 제거한 뒤 다시 삭제하세요.")
        case -55: return ProviderError("invalid_profile", "Claude subscription monitoring requires a configuration reference, not a token.")
        case -56: return ProviderError("invalid_auth_status", "Claude Code returned an unsupported login status.")
        case -57: return ProviderError("missing_directory", "Claude CLI에 로그인한 뒤 해당 설정 폴더를 선택하세요.")
        case ...(-10000): return NSError(domain: NSCocoaErrorDomain, code: -10000 - Int(status))
        default: return NSError(domain: NSPOSIXErrorDomain, code: status > 0 ? Int(status) : Int(POSIXErrorCode.EIO.rawValue))
        }
    }
}
