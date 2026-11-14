// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

public struct CodexProvider: Sendable {
    let timeoutSeconds: Double
    static let defaultBaseURL = "https://chatgpt.com/backend-api/"
    public init(timeoutSeconds: Double = 20) { self.timeoutSeconds = timeoutSeconds }

    /// Read access tokens in memory. The CLI retains ownership of OAuth refresh.
    public func fetch(authData: Data) async throws -> ProviderUsage {
        let creds = try loadCredentials(from: "Codex login", data: authData)
        guard !creds.isApiKeyMode else {
            throw ProviderError("api_key_unsupported", "ChatGPT subscription login is required; API keys do not expose subscription quota.")
        }
        let response = try await requestUsage(creds: creds, url: Self.resolveUsageEndpointURL(homeDirectory: ""))
        return try parseResponse(response, accountId: creds.accountId,
            source: SourceInfo(mode: "read-only", type: "codex-auth-json"),
            fallbackEmail: creds.accountEmail, jwtPlanType: creds.jwtPlanType, jwtUserId: creds.jwtUserId)
    }
    struct Credentials: Sendable {
        let authFile: String
        let accessToken: String
        let refreshToken: String?
        let idToken: String?
        let accountId: String?
        let accountEmail: String?
        let jwtPlanType: String?
        let jwtUserId: String?
        let needsRefresh: Bool
        let isApiKeyMode: Bool
    }

    private func loadCredentials(from path: String, data: Data) throws -> Credentials {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw ProviderError("invalid_auth_file", "\(path) could not be parsed.")
        }

        if let apiKey = stringValue(json["OPENAI_API_KEY"]) {
            return Credentials(
                authFile: path,
                accessToken: apiKey,
                refreshToken: nil,
                idToken: nil,
                accountId: nil,
                accountEmail: stringValue(json["email"]),
                jwtPlanType: nil,
                jwtUserId: nil,
                needsRefresh: false,
                isApiKeyMode: true
            )
        }

        let tokens = json["tokens"] as? [String: Any] ?? [:]
        let accessToken = stringValue(tokens["access_token"])
            ?? stringValue(tokens["accessToken"])
            ?? stringValue(json["access_token"])
            ?? stringValue(json["accessToken"])

        guard let accessToken else {
            throw ProviderError("missing_tokens", "\(path) exists but has no access token.")
        }

        let refreshToken = stringValue(tokens["refresh_token"])
            ?? stringValue(tokens["refreshToken"])
            ?? stringValue(json["refresh_token"])
            ?? stringValue(json["refreshToken"])
        let idToken = stringValue(tokens["id_token"])
            ?? stringValue(tokens["idToken"])
            ?? stringValue(json["id_token"])
            ?? stringValue(json["idToken"])
        let identity = CodexAccountIdentity(authJSON: json)
        let accountId = identity.accountId
        let accountEmail = stringValue(json["email"])
            ?? decodeEmail(fromJWT: idToken)
            ?? decodeEmail(fromJWT: accessToken)
        let jwtPlanType = decodePlanType(fromJWT: idToken) ?? decodePlanType(fromJWT: accessToken)
        let jwtUserId = identity.userId

        let lastRefresh = stringValue(json["last_refresh"]).flatMap(parseDate)
        let expiryDate = stringValue(json["expired"]).flatMap(parseDate)
        var needsRefresh: Bool
        if let expiryDate {
            needsRefresh = expiryDate.timeIntervalSinceNow <= 300
        } else if let lastRefresh {
            needsRefresh = Date().timeIntervalSince(lastRefresh) > 8 * 86400
        } else {
            needsRefresh = true
        }

        if !needsRefresh, let payload = decodeJWTPayload(token: accessToken),
           let exp = payload["exp"] as? Double, Date(timeIntervalSince1970: exp).timeIntervalSinceNow <= 60 {
            needsRefresh = true
        }

        return Credentials(
            authFile: path,
            accessToken: accessToken,
            refreshToken: refreshToken,
            idToken: idToken,
            accountId: accountId,
            accountEmail: accountEmail,
            jwtPlanType: jwtPlanType,
            jwtUserId: jwtUserId,
            needsRefresh: needsRefresh,
            isApiKeyMode: false
        )
    }

    static func resolveUsageEndpointURL(homeDirectory _: String) throws -> URL {
        guard let url = URL(string: "\(Self.defaultBaseURL)wham/usage") else {
            throw ProviderError("invalid_url", "Codex usage URL is invalid.")
        }
        return url
    }

    private func requestUsage(creds: Credentials, url: URL) async throws -> [String: Any] {
        // The usage URL is identical for every account; identity lives in the
        // headers below. Cached responses would leak across accounts, so this
        // goes through the cache-free quota session.
        var request = QuotaHTTP.request(url: url, timeout: timeoutSeconds)
        request.setValue("Bearer \(creds.accessToken)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("QuotaBar", forHTTPHeaderField: "User-Agent")
        if let accountId = creds.accountId {
            request.setValue(accountId, forHTTPHeaderField: "ChatGPT-Account-Id")
        }

        let (data, response) = try await QuotaHTTP.data(for: request)
        if let http = response as? HTTPURLResponse {
            if http.statusCode == 401 || http.statusCode == 403 {
                throw ProviderError("unauthorized", "Codex OAuth token is invalid or expired.")
            }
            if !((200..<300).contains(http.statusCode)) {
                throw ProviderError("api_error", "Codex usage API returned HTTP \(http.statusCode).")
            }
        }

        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw ProviderError("parse_failed", "Codex usage API returned invalid JSON.")
        }
        return json
    }

    func parseResponse(_ response: [String: Any], accountId: String?, source: SourceInfo, fallbackEmail: String?,
                       jwtPlanType: String? = nil, jwtUserId: String? = nil) throws -> ProviderUsage {
        try NativeQuota.parse(provider: "codex", response: response, source: source, email: fallbackEmail,
                              accountID: accountId, jwtPlan: jwtPlanType, userID: jwtUserId)
    }

    private func parseDate(_ value: String) -> Date? {
        SharedFormatters.parseISO8601(value)
    }

    private func stringValue(_ value: Any?) -> String? {
        guard let string = value as? String else { return nil }
        let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private func decodeJWTPayload(token: String?) -> [String: Any]? {
        guard let token, token.contains(".") else { return nil }
        let segments = token.split(separator: ".")
        guard segments.count >= 2 else { return nil }

        var payload = String(segments[1])
        let remainder = payload.count % 4
        if remainder > 0 {
            payload += String(repeating: "=", count: 4 - remainder)
        }
        payload = payload
            .replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")

        guard let data = Data(base64Encoded: payload),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return nil
        }
        return json
    }

    private func decodeEmail(fromJWT token: String?) -> String? {
        guard let payload = decodeJWTPayload(token: token) else { return nil }
        if let email = stringValue(payload["email"]) {
            return email
        }
        if let profile = payload["https://api.openai.com/profile"] as? [String: Any] {
            return stringValue(profile["email"])
        }
        return nil
    }

    private func decodePlanType(fromJWT token: String?) -> String? {
        guard let payload = decodeJWTPayload(token: token) else { return nil }
        if let auth = payload["https://api.openai.com/auth"] as? [String: Any] {
            return stringValue(auth["chatgpt_plan_type"])
        }
        return nil
    }

    // MARK: - ChatGPT / Codex Plan Mapping
    // 基于 openai/codex 的 KnownPlan（codex-rs/login/src/token_data.rs）做映射，但展示名贴合
    // ChatGPT 现行方案命名。要点：
    //   - 旧的 "team" 已更名为 "ChatGPT Business"，统一展示为 Business（旧账号 JWT 仍可能下发 team）；
    //   - "hc" → Enterprise；含 *_usage_based 的按需计费变体归并到 Business / Enterprise；
    //   - 个人计划：free / go / plus / pro。

    /// 规范化原始 plan 键：小写、去空白、空格/连字符→下划线，
    /// 以便同时兼容原始值（"self_serve_business_usage_based"）与展示名（"Self Serve Business Usage Based"）。
    private static func normalizePlanKey(_ raw: String) -> String {
        raw.lowercased()
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: " ", with: "_")
            .replacingOccurrences(of: "-", with: "_")
    }

    /// 展示名（贴合 ChatGPT 现行方案命名）。未知值原样返回。
    public static func planDisplayName(forRaw raw: String?) -> String? {
        guard let raw, !raw.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
        switch normalizePlanKey(raw) {
        case "free": return "Free"
        case "go": return "Go"
        case "plus": return "Plus"
        case "pro": return "Pro"
        // ChatGPT Team 已更名为 Business，旧 team 值统一展示为 Business。
        case "team", "self_serve_business_usage_based", "business": return "Business"
        case "enterprise_cbp_usage_based", "enterprise", "hc": return "Enterprise"
        case "education", "edu": return "Edu"
        default: return raw
        }
    }
}
