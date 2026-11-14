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
        guard status == 0 else { throw failure() }
        defer { qb_secret_free(bytes, count) }
        if count == 0 { return Data() }
        guard let bytes else { throw failure() }
        return Data(bytes: bytes, count: count)
    }

    public func save(_ data: Data, id: UUID) throws {
        let status = data.withUnsafeBytes { bytes in
            qb_vault_save(directory.path, id.uuidString, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count)
        }
        guard status == 0 else { throw failure() }
    }

    public func delete(_ id: UUID) throws {
        guard qb_vault_delete(directory.path, id.uuidString) == 0 else { throw failure() }
    }

    private func failure() -> ProviderError {
        ProviderError("credential_storage", "앱 전용 로그인 파일에 접근할 수 없습니다. 앱 데이터 폴더의 권한과 저장 공간을 확인하세요.")
    }
}
