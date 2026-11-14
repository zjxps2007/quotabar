// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

public struct GeminiProvider: Sendable {
    let timeoutSeconds: Double
    public init(timeoutSeconds: Double = 15) { self.timeoutSeconds = timeoutSeconds }

    /// CLI-managed OAuth: no token rotation or writes to the user's login files.
    public func fetch(authData: Data) async throws -> ProviderUsage {
        let creds = try NativeCredentials.decode(NativeProviderRequest.Credentials.self, data: authData, provider: .gemini, mode: 4)
        let assist = await discover(.geminiAssist, token: creds.accessToken)
        let project: String?
        if let id = assist?.project { project = id }
        else { project = await discover(.geminiProjects, token: creds.accessToken)?.project }
        let response = try await retrieveQuota(accessToken: creds.accessToken, projectId: project)
        return try NativeQuota.parse(provider: "gemini", response: response, source: SourceInfo(mode: "read-only", type: "gemini-cli"),
            email: creds.email, project: project, tier: assist?.tier)
    }

    private func discover(_ endpoint: NativeProviderRequest.Endpoint, token: String) async -> NativeProviderRequest.Discovery? {
        guard let request = try? NativeProviderRequest.make(endpoint, token: token, timeout: timeoutSeconds),
              let (data, _) = try? await QuotaHTTP.data(for: request) else { return nil }
        return try? NativeProviderRequest.discovery(data, list: endpoint == .geminiProjects)
    }

    private func retrieveQuota(accessToken: String, projectId: String?) async throws -> [String: Any] {
        let request = try NativeProviderRequest.make(.geminiQuota, token: accessToken, project: projectId, timeout: timeoutSeconds)
        let (data, response) = try await QuotaHTTP.data(for: request)
        if let http = response as? HTTPURLResponse, (http.statusCode == 401 || http.statusCode == 403) {
            // 不是令牌问题：重新登录也无效，所以不能再提示用户重新认证，也不触发刷新重试。
            if http.statusCode == 403, Self.isIndividualTierDiscontinued(responseBody: data) {
                throw ProviderError("gemini_individual_discontinued", Self.individualTierDiscontinuedMessage)
            }
            throw ProviderError("not_logged_in", "Gemini OAuth token is invalid or expired. Re-authenticate with `gemini`.")
        }
        if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
            throw ProviderError("api_error", "Gemini quota API returned HTTP \(http.statusCode).")
        }
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw ProviderError("parse_failed", "Gemini quota API returned invalid JSON.")
        }
        return json
    }

    // MARK: - Individual Tier Shutdown
    // Google 自 2026-06-18 起停止为个人账号（免费 / Google AI Pro / Ultra）提供 Gemini CLI 与 Code Assist，
    // 后端对这些账号返回 403 UNSUPPORTED_CLIENT："This client is no longer supported for Gemini Code
    // Assist for individuals…"。只有 Code Assist Standard / Enterprise 许可仍可使用。
    // https://developers.google.com/gemini-code-assist/docs/deprecations/code-assist-individuals

    static let individualTierDiscontinuedMessage =
        "Google stopped serving Gemini CLI for personal Google accounts (free, Google AI Pro and Ultra) on June 18, 2026. Only Gemini Code Assist Standard or Enterprise licenses still work — track personal accounts with Antigravity instead."

    static func isIndividualTierDiscontinued(responseBody data: Data) -> Bool {
        let body = String(decoding: data, as: UTF8.self).lowercased()
        return body.contains("unsupported_client")
            || body.contains("no longer supported for gemini code assist for individuals")
    }

}
