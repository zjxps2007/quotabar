import Foundation

/// Credentials obtained by an isolated official Codex login, owned by this app.
public struct CodexOAuthCredentials: Sendable {
    public let data: Data
    public let identity: ImportedIdentity
    public let accessToken: String
    public let refreshToken: String?
    public let expiresAt: Date?
    private let lastRefresh: Date?

    public init(data: Data) throws {
        identity = try ImportedIdentity.parse(data, provider: .codex)
        let json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        let tokens = json["tokens"] as? [String: Any] ?? json
        guard json["OPENAI_API_KEY"] as? String == nil,
              let access = Self.string(tokens["access_token"] ?? tokens["accessToken"]) else {
            throw ProviderError("invalid_auth", "ChatGPT 구독 계정으로 로그인하세요.")
        }
        self.data = data; accessToken = access
        refreshToken = Self.string(tokens["refresh_token"] ?? tokens["refreshToken"])
        let expiry = (json["expires_at"] as? Double).map(Date.init(timeIntervalSince1970:))
        expiresAt = expiry ?? (Self.claims(access)?["exp"] as? Double).map(Date.init(timeIntervalSince1970:))
            ?? (json["expired"] as? String).flatMap(SharedFormatters.parseISO8601)
        lastRefresh = (json["last_refresh"] as? String).flatMap(SharedFormatters.parseISO8601)
    }

    public func needsRefresh(at now: Date = Date()) -> Bool {
        if let expiresAt { return expiresAt.timeIntervalSince(now) <= 300 }
        // With an opaque token, refresh at most hourly until an explicit expiry is available.
        return now.timeIntervalSince(lastRefresh ?? .distantPast) >= 3600
    }

    static func string(_ value: Any?) -> String? {
        guard let value = value as? String, !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        return value
    }
    static func claims(_ token: String) -> [String: Any]? {
        let pieces = token.split(separator: ".")
        guard pieces.count == 3 else { return nil }
        var base64 = String(pieces[1]).replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        base64 += String(repeating: "=", count: (4 - base64.count % 4) % 4)
        guard let data = Data(base64Encoded: base64) else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }
}

/// Refuses redirects so a token POST cannot be forwarded to another endpoint.
private final class NoOAuthRedirects: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}

public struct CodexOAuthClient: Sendable {
    public typealias Transport = @Sendable (URLRequest) async throws -> (Data, HTTPURLResponse)
    // Official Codex public client, also used by the upstream Codex provider.
    static let clientID = "app_EMoamEEZ73f0CkXaXp7hrann"
    private let transport: Transport
    private static let session: URLSession = {
        let config = URLSessionConfiguration.ephemeral
        config.urlCache = nil; config.httpCookieStorage = nil
        return URLSession(configuration: config, delegate: NoOAuthRedirects(), delegateQueue: nil)
    }()
    public init(transport: Transport? = nil) {
        self.transport = transport ?? { request in
            let (data, response) = try await Self.session.data(for: request)
            guard let response = response as? HTTPURLResponse else {
                throw ProviderError("oauth_response", "로그인 갱신 응답을 확인할 수 없습니다.")
            }
            return (data, response)
        }
    }

    public func refresh(_ current: Data, now: Date = Date()) async throws -> Data {
        let old = try CodexOAuthCredentials(data: current)
        guard let refresh = old.refreshToken else {
            throw ProviderError("reauth_required", "자동 갱신할 수 없는 로그인입니다. 앱에서 다시 로그인하세요.")
        }
        var request = QuotaHTTP.request(url: URL(string: "https://auth.openai.com/oauth/token")!, timeout: 20)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: [
            "grant_type": "refresh_token", "client_id": Self.clientID, "refresh_token": refresh
        ])
        let (data, response) = try await transport(request)
        guard (200..<300).contains(response.statusCode) else {
            if [400, 401, 403].contains(response.statusCode) {
                throw ProviderError("reauth_required", "로그인이 만료되거나 해제되었습니다. 앱에서 다시 로그인하세요.")
            }
            throw ProviderError("oauth_unavailable", "로그인 갱신이 일시적으로 실패했습니다. 다음 조회에서 다시 시도합니다.")
        }
        guard data.count <= 1_048_576,
              let result = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let access = CodexOAuthCredentials.string(result["access_token"]) else {
            throw ProviderError("oauth_response", "로그인 갱신 응답에 필요한 인증 정보가 없습니다.")
        }
        var json = try JSONSerialization.jsonObject(with: current) as! [String: Any]
        var tokens = json["tokens"] as? [String: Any] ?? json
        let original = CodexAccountIdentity(authJSON: json)
        // Check each returned token before carrying forward the pinned workspace.
        for token in [access, result["id_token"] as? String].compactMap({ $0 }) {
            let returned = CodexAccountIdentity(authJSON: ["access_token": token])
            guard !original.conflicts(with: returned) else {
                throw ProviderError("account_mismatch", "갱신된 인증의 계정이 달라 연결을 중단했습니다. 다시 로그인하세요.")
            }
        }
        tokens["access_token"] = access
        if let value = CodexOAuthCredentials.string(result["refresh_token"]) { tokens["refresh_token"] = value }
        if let value = CodexOAuthCredentials.string(result["id_token"]) { tokens["id_token"] = value }
        if json["tokens"] != nil { json["tokens"] = tokens } else { json = tokens }
        json.removeValue(forKey: "expires_at"); json.removeValue(forKey: "expired")
        if let seconds = result["expires_in"] as? Double, seconds.isFinite, seconds > 0 {
            json["expires_at"] = now.addingTimeInterval(seconds).timeIntervalSince1970
        }
        json["last_refresh"] = SharedFormatters.iso8601String(from: now)
        let updated = try JSONSerialization.data(withJSONObject: json)
        guard try CodexOAuthCredentials(data: updated).identity.key == old.identity.key else {
            throw ProviderError("account_mismatch", "갱신된 로그인 정보를 기존 계정에 연결할 수 없습니다.")
        }
        return updated
    }
}

/// One rotation per account. Save the replacement BEFORE making any quota request.
/// Query cancellation does not abandon a completed rotation. Deletion/reconnection
/// invalidates the generation and the compare-and-swap prevents credential resurrection.
@MainActor
public final class CodexSessionManager {
    public typealias Read = (UUID) throws -> Data?
    public typealias Write = (UUID, Data) throws -> Void
    private let read: Read
    private let write: Write
    private let client: CodexOAuthClient
    private var generations: [UUID: UUID] = [:]
    private var jobs: [UUID: Task<Data, Error>] = [:]
    private var pendingWrites: [UUID: (Data, Data)] = [:]
    private var reauthRequired: Set<UUID> = []
    public init(client: CodexOAuthClient = CodexOAuthClient(), read: Read? = nil, write: Write? = nil) {
        let vault = SecretVault()
        self.client = client; self.read = read ?? { try vault.load($0) }
        self.write = write ?? { try vault.save($1, id: $0) }
    }
    public func invalidate(_ id: UUID) {
        generations[id] = UUID(); jobs.removeValue(forKey: id)?.cancel()
        pendingWrites.removeValue(forKey: id); reauthRequired.remove(id)
    }
    public func credential(id: UUID, identity: String, rejected: Data? = nil) async throws -> Data {
        guard !reauthRequired.contains(id) else {
            throw ProviderError("reauth_required", "로그인이 만료되거나 해제되었습니다. 앱에서 다시 로그인하세요.")
        }
        if let task = jobs[id] { return try await task.value }
        guard var current = try read(id) else { throw ProviderError("missing_credentials", "앱에서 다시 로그인하세요.") }
        if let (previous, replacement) = pendingWrites[id] {
            if current == previous { try write(id, replacement); current = replacement }
            pendingWrites.removeValue(forKey: id)
        }
        let credentials = try CodexOAuthCredentials(data: current)
        guard credentials.identity.key == identity else { throw ProviderError("account_mismatch", "저장된 계정 정보가 일치하지 않습니다.") }
        guard credentials.needsRefresh() || rejected == current else { return current }
        let generation = generations[id] ?? UUID(); generations[id] = generation
        let original = current
        let task = Task { [self, client] in
            do {
                let replacement = try await client.refresh(original)
                guard generations[id] == generation, try read(id) == original else { throw CancellationError() }
                pendingWrites[id] = (original, replacement)
                try write(id, replacement)
                pendingWrites.removeValue(forKey: id)
                return replacement
            } catch {
                if generations[id] == generation, (error as? ProviderError)?.code == "reauth_required" { reauthRequired.insert(id) }
                throw error
            }
        }
        jobs[id] = task
        defer { if generations[id] == generation { jobs.removeValue(forKey: id) } }
        return try await task.value
    }
}
