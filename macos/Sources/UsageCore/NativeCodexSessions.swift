import CQuotaBar
import Foundation

final class NativeCodexSessions {
    enum Decision { case ready(Data), rotate(UInt64, Data), join(UInt64) }
    enum Event: Int32 { case invalidate, revoke, end }
    private let handle: OpaquePointer

    // Keep a custom vault's original Swift error for this synchronous call.
    private final class StoreCall {
        let read: CodexSessionManager.Read
        let write: CodexSessionManager.Write
        var error: Error?
        init(read: @escaping CodexSessionManager.Read, write: @escaping CodexSessionManager.Write) {
            self.read = read
            self.write = write
        }
    }
    private let custom: (CodexSessionManager.Read, CodexSessionManager.Write)?

    init(vault: SecretVault, read: CodexSessionManager.Read?, write: CodexSessionManager.Write?) {
        if read == nil && write == nil {
            handle = qb_sessions_new(1, vault.directory.path)!
            custom = nil
        } else {
            handle = qb_sessions_new(1, nil)!
            custom = (read ?? { try vault.load($0) }, write ?? { try vault.save($1, id: $0) })
        }
    }
    deinit { qb_sessions_free(handle) }

    private func withStore(_ body: (UnsafePointer<QBSessionStore>?) -> Int32) throws -> Int32 {
        let status: Int32
        if let (read, write) = custom {
            let call = StoreCall(read: read, write: write)
            var store = QBSessionStore(context: Unmanaged.passUnretained(call).toOpaque(), access: { context, key, value, length, receiver in
                guard let context, let key, let id = UUID(uuidString: String(cString: key)) else { return -1 }
                let call = Unmanaged<StoreCall>.fromOpaque(context).takeUnretainedValue()
                do {
                    if let receiver {
                        guard let data = try call.read(id) else { return 1 }
                        return data.withUnsafeBytes { raw in
                            qb_sessions_loaded(receiver, raw.bindMemory(to: UInt8.self).baseAddress, raw.count)
                        }
                    }
                    let data = value.map { Data(bytes: $0, count: length) } ?? Data()
                    try call.write(id, data)
                    return 0
                } catch { call.error = error; return -1 }
            })
            status = body(&store)
            if let error = call.error { throw error }
        } else {
            status = body(nil)
        }
        switch status {
        case 0...: return status
        case -50: throw ProviderError("missing_credentials", "앱에서 다시 로그인하세요.")
        case -51: throw ProviderError("account_mismatch", "저장된 계정 정보가 일치하지 않습니다.")
        case -52: throw ProviderError("reauth_required", "로그인이 만료되거나 해제되었습니다. 앱에서 다시 로그인하세요.")
        case -53: throw CancellationError()
        case -54, -55: throw SecretVault.failure()
        default: throw NativeCredentials.failure(status, provider: .codex)
        }
    }

    func begin(_ id: UUID, identity: String, rejected: Data?) throws -> Decision {
        var ticket: UInt64 = 0
        var output: UnsafeMutablePointer<UInt8>?
        var length = 0
        let status = try withStore { store in
            Array(identity.utf8).withUnsafeBufferPointer { key in
                func start(_ data: UnsafePointer<UInt8>?, _ count: Int) -> Int32 {
                    qb_sessions_begin(handle, id.uuidString, key.baseAddress, key.count, data, count,
                                      store, &ticket, &output, &length)
                }
                guard let rejected else { return start(nil, 0) }
                return rejected.withUnsafeBytes { start($0.bindMemory(to: UInt8.self).baseAddress, $0.count) }
            }
        }
        if status == 2 { return .join(ticket) }
        let data = NativeCredentials.take(output, count: length)
        return status == 1 ? .rotate(ticket, data) : .ready(data)
    }

    func complete(_ id: UUID, ticket: UInt64, replacement: Data) throws {
        _ = try withStore { store in
            replacement.withUnsafeBytes { raw in
                qb_sessions_complete(handle, id.uuidString, ticket, raw.bindMemory(to: UInt8.self).baseAddress, raw.count, store)
            }
        }
    }
    func flush() throws { _ = try withStore { qb_sessions_flush(handle, $0) } }
    @discardableResult func notify(_ event: Event, id: UUID, ticket: UInt64 = 0) -> UInt64 {
        qb_sessions_event(handle, id.uuidString, ticket, event.rawValue)
    }
}
