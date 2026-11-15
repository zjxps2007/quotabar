import CQuotaBar
import Foundation

/// The resident monitor receives quota data. Rust owns the vault, exchanges, and rejected-token retry.
public final class ManagedCodexAccounts: @unchecked Sendable {
    private let handle: OpaquePointer

    public init(vault: SecretVault = SecretVault()) { handle = qb_managed_accounts_new(vault.directory.path)! }
    deinit { qb_managed_accounts_free(handle) }

    public func invalidate(_ id: UUID) { qb_managed_accounts_invalidate(handle, id.uuidString) }

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
