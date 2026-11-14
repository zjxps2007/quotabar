// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

public struct CodexProvider: Sendable {
    let timeoutSeconds: Double
    static let defaultBaseURL = "https://chatgpt.com/backend-api/"
    public init(timeoutSeconds: Double = 20) { self.timeoutSeconds = timeoutSeconds }

    /// Read access tokens in memory. The CLI retains ownership of OAuth refresh.
    public func fetch(authData: Data) async throws -> ProviderUsage {
        let creds = try NativeCredentials.decode(NativeProviderRequest.Credentials.self, data: authData, provider: .codex, mode: 4)
        let request = try NativeProviderRequest.make(.codex, token: creds.accessToken, account: creds.accountId, timeout: timeoutSeconds)
        let response = try await requestUsage(request)
        return try parseResponse(response, accountId: creds.accountId, source: SourceInfo(mode: "read-only", type: "codex-auth-json"),
            fallbackEmail: creds.email, jwtPlanType: creds.plan, jwtUserId: creds.userId)
    }

    static func resolveUsageEndpointURL(homeDirectory _: String) throws -> URL {
        guard let url = URL(string: "\(Self.defaultBaseURL)wham/usage") else {
            throw ProviderError("invalid_url", "Codex usage URL is invalid.")
        }
        return url
    }

    private func requestUsage(_ request: URLRequest) async throws -> [String: Any] {
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
