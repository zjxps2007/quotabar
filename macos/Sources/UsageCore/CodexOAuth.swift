import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

/// Credentials obtained by an isolated official Codex login, owned by this app.
public struct CodexOAuthCredentials: Sendable {
    public let data: Data
    public let identity: ImportedIdentity
    public let accessToken: String
    public let refreshToken: String?
    public let expiresAt: Date?
    private let lastRefresh: Date?

    private struct Parsed: Decodable {
        let identity: ImportedIdentity
        let accessToken: String
        let refreshToken: String?
        let expiresAt: Date?
        let lastRefresh: Date?
    }
    public init(data: Data) throws {
        let parsed = try NativeCredentials.decode(Parsed.self, data: data, provider: .codex, mode: 3)
        self.data = data
        identity = parsed.identity
        accessToken = parsed.accessToken
        refreshToken = parsed.refreshToken
        expiresAt = parsed.expiresAt
        lastRefresh = parsed.lastRefresh
    }

    public func needsRefresh(at now: Date = Date()) -> Bool {
        if let expiresAt { return expiresAt.timeIntervalSince(now) <= 300 }
        // With an opaque token, refresh at most hourly until an explicit expiry is available.
        return now.timeIntervalSince(lastRefresh ?? .distantPast) >= 3600
    }

}

public struct CodexOAuthClient: Sendable {
    public typealias Transport = @Sendable (URLRequest) async throws -> (Data, HTTPURLResponse)
    // Official Codex public client, also used by the upstream Codex provider.
    static let clientID = "app_EMoamEEZ73f0CkXaXp7hrann"
    private let transport: Transport?
    public init(transport: Transport? = nil) { self.transport = transport }

    public func refresh(_ current: Data, now: Date = Date()) async throws -> Data {
        guard let transport else {
            return try await NativeProviderQuery.native(.refresh, auth: current, timeout: 20, now: now)
        }
        return try await NativeProviderQuery.run(.refresh, auth: current, timeout: 20, now: now) { request in
            let (data, response) = try await transport(request)
            return (data, response)
        }
    }
}

/// One rotation per account. Save the replacement BEFORE making any quota request.
/// Query cancellation does not abandon a completed rotation. Deletion/reconnection
/// invalidates the generation and the compare-and-swap prevents credential resurrection.
@MainActor
public final class CodexSessionManager {
    public typealias Read = (UUID) throws -> Data?
    public typealias Write = (UUID, Data) throws -> Void
    private let client: CodexOAuthClient
    private let native: NativeCodexSessions
    private var jobs: [UInt64: Task<Data, Error>] = [:]
    public convenience init(client: CodexOAuthClient = CodexOAuthClient(), read: Read? = nil, write: Write? = nil) {
        self.init(client: client, vault: SecretVault(), read: read, write: write)
    }
    init(client: CodexOAuthClient, vault: SecretVault, read: Read? = nil, write: Write? = nil) {
        self.client = client
        native = NativeCodexSessions(vault: vault, read: read, write: write)
    }
    public func invalidate(_ id: UUID) {
        let ticket = native.notify(.invalidate, id: id)
        jobs.removeValue(forKey: ticket)?.cancel()
    }
    /// Call after stopping new account requests, before an automatic restart.
    public func finishPendingWrites() async throws {
        // Rotations can outlive a cancelled usage request (for example across sleep).
        for task in Array(jobs.values) { _ = try? await task.value }
        try native.flush()
    }
    public func credential(id: UUID, identity: String, rejected: Data? = nil) async throws -> Data {
        switch try native.begin(id, identity: identity, rejected: rejected) {
        case .ready(let data): return data
        case .join(let ticket):
            guard let task = jobs[ticket] else { throw CancellationError() }
            return try await task.value
        case .rotate(let ticket, let original):
            let task = Task { [native, client] in
                do {
                    let replacement = try await client.refresh(original)
                    try native.complete(id, ticket: ticket, replacement: replacement)
                    return replacement
                } catch {
                    if (error as? ProviderError)?.code == "reauth_required" { native.notify(.revoke, id: id, ticket: ticket) }
                    throw error
                }
            }
            jobs[ticket] = task
            defer {
                jobs.removeValue(forKey: ticket)
                native.notify(.end, id: id, ticket: ticket)
            }
            return try await task.value
        }
    }
}
