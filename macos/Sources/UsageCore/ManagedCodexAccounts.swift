import CQuotaBar
import Foundation

public struct MonitorFetchResult: Decodable, Sendable {
    public let snapshot: AccountSnapshot?
    public let error: String?
    public let note: String?
    public fileprivate(set) var updatedCredential: Data?

    private enum CodingKeys: String, CodingKey { case snapshot, error, note }
}

/// The resident monitor receives quota data. Rust owns the vault, exchanges, and rejected-token retry.
public final class ManagedCodexAccounts: @unchecked Sendable {
    private let handle: OpaquePointer

    public init(vault: SecretVault = SecretVault()) { handle = qb_managed_accounts_new(vault.directory.path)! }
    deinit { qb_managed_accounts_free(handle) }

    public func invalidate(_ id: UUID) { qb_managed_accounts_invalidate(handle, id.uuidString) }

    public func refresh(_ account: MonitoredAccount, root: URL) async -> MonitorFetchResult {
        let cancel = NativeCancellation()
        return await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                DispatchQueue.global(qos: .utility).async { [self] in
                    do {
                        let input = try JSONEncoder().encode(account)
                        var result = QBMacRefreshResult()
                        let status = input.withUnsafeBytes {
                            qb_mac_fetch_account(handle, root.path, $0.bindMemory(to: UInt8.self).baseAddress,
                                                 $0.count, cancel.handle, &result)
                        }
                        let state = NativeCredentials.take(result.state, count: result.stateLength)
                        let credential = result.credential.map { NativeCredentials.take($0, count: result.credentialLength) }
                        guard status == 0 else { throw CocoaError(.coderReadCorrupt) }
                        var outcome = try JSONDecoder().decode(MonitorFetchResult.self, from: state)
                        outcome.updatedCredential = credential
                        continuation.resume(returning: outcome)
                    } catch {
                        continuation.resume(returning: MonitorFetchResult(snapshot: nil,
                            error: "조회에 실패했습니다. 연결한 파일과 앱의 접근 권한을 확인하세요.", note: nil))
                    }
                }
            }
        } onCancel: { cancel.cancel() }
    }

    public func finishPendingWrites() async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            DispatchQueue.global(qos: .utility).async { [self] in
                continuation.resume(with: Result { _ = try NativeCodexSessions.check(qb_managed_accounts_flush(handle)) })
            }
        }
    }

    public func fetch(id: UUID, identity: String) async throws -> ProviderUsage {
        let cancel = NativeCancellation()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                DispatchQueue.global(qos: .utility).async { [self] in
                    var output: UnsafeMutablePointer<UInt8>?
                    var count = 0
                    var osError: Int32 = 0
                    let status = Array(identity.utf8).withUnsafeBufferPointer {
                        qb_managed_accounts_fetch(handle, id.uuidString, $0.baseAddress, $0.count, cancel.handle,
                                                  nil, nil, &osError, &output, &count)
                    }
                    let bytes = NativeCredentials.take(output, count: count)
                    continuation.resume(with: Result {
                        if (-55 ... -50).contains(status) { _ = try NativeCodexSessions.check(status) }
                        let data = try NativeProviderQuery.nativeResult(status, osError: osError, data: bytes, operation: .codex)
                        return try JSONDecoder().decode(ProviderUsage.self, from: data)
                    })
                }
            }
        } onCancel: { cancel.cancel() }
    }
}
