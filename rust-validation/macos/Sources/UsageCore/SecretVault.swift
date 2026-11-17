import CQuotaBar
import Foundation

/// App-owned login files protected by the current user's filesystem permissions.
public struct SecretVault: Sendable {
    public let directory: URL

    public init(directory: URL = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/AIUsage Lite/Credentials")) {
        self.directory = directory
    }

    public func load(_ id: UUID) throws -> Data? {
        var bytes: UnsafeMutablePointer<UInt8>?
        var count = 0
        let status = qb_vault_load(directory.path, id.uuidString, &bytes, &count)
        if status == 1 { return nil }
        guard status == 0 else { throw Self.failure() }
        defer { qb_secret_free(bytes, count) }
        if count == 0 { return Data() }
        guard let bytes else { throw Self.failure() }
        return Data(bytes: bytes, count: count)
    }

    public func save(_ data: Data, id: UUID) throws {
        let status = data.withUnsafeBytes { bytes in
            qb_vault_save(directory.path, id.uuidString, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count)
        }
        guard status == 0 else { throw Self.failure() }
    }

    public func imported(_ account: MonitoredAccount) throws -> (data: Data, updated: Bool, note: String?) {
        let request = try JSONEncoder().encode(account)
        var bytes: UnsafeMutablePointer<UInt8>?
        var count = 0
        let status = request.withUnsafeBytes {
            qb_mac_imported(directory.path, $0.bindMemory(to: UInt8.self).baseAddress, $0.count, &bytes, &count)
        }
        switch status {
        case 0...2:
            return (NativeCredentials.take(bytes, count: count), status == 1,
                    status == 2 ? "CLI가 다른 계정으로 전환되어 보관된 로그인을 사용합니다." : nil)
        case -50: throw ProviderError("missing_credentials", "저장된 로그인이 없습니다. 계정을 다시 연결하세요.")
        case -51: throw ProviderError("account_mismatch", "저장된 로그인과 계정 정보가 일치하지 않습니다. 다시 연결하세요.")
        case -24: throw Self.failure()
        default: throw NativeCredentials.failure(status, provider: account.provider)
        }
    }

    public func delete(_ id: UUID) throws {
        guard qb_vault_delete(directory.path, id.uuidString) == 0 else { throw Self.failure() }
    }

    static func failure() -> ProviderError {
        ProviderError("credential_storage", "앱 전용 로그인 파일에 접근할 수 없습니다. 앱 데이터 폴더의 권한과 저장 공간을 확인하세요.")
    }
}
