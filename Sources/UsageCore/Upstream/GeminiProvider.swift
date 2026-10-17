// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

public struct GeminiProvider: Sendable {
    let timeoutSeconds: Double
    static let quotaURL = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota"
    static let loadCodeAssistURL = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist"
    static let projectsURL = "https://cloudresourcemanager.googleapis.com/v1/projects"
    public init(timeoutSeconds: Double = 15) { self.timeoutSeconds = timeoutSeconds }

    /// CLI-managed OAuth: no token rotation or writes to the user's login files.
    public func fetch(authData: Data) async throws -> ProviderUsage {
        let creds = try loadCredentials(data: authData)
        guard let token = creds.accessToken, !token.isEmpty else {
            throw ProviderError("not_logged_in", "Gemini login has no access token. Sign in through Gemini CLI and reconnect.")
        }
        let assist = await loadCodeAssist(accessToken: token)
        let project: String?
        if let value = assist.projectId { project = value }
        else { project = await discoverProjectId(accessToken: token) }
        let response = try await retrieveQuota(accessToken: token, projectId: project)
        return parseQuotaResponse(response, creds: creds, projectId: project, tierId: assist.tierId,
            source: SourceInfo(mode: "read-only", type: "gemini-cli"))
    }
    private struct GeminiCredentials {
        let accessToken: String?
        let idToken: String?
        let refreshToken: String?
        let expiryDate: Date?
        let directEmail: String?
        let clientId: String?
        let clientSecret: String?

        var accountEmail: String? {
            if let jwtEmail = jwtEmail() { return jwtEmail }
            return directEmail
        }

        private func jwtEmail() -> String? {
            guard let token = idToken, token.contains(".") else { return nil }
            let parts = token.split(separator: ".")
            guard parts.count >= 2 else { return nil }
            var payload = String(parts[1])
            let rem = payload.count % 4
            if rem > 0 { payload += String(repeating: "=", count: 4 - rem) }
            payload = payload.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
            guard let data = Data(base64Encoded: payload),
                  let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
            return json["email"] as? String
        }
    }

    private func loadCredentials(data: Data) throws -> GeminiCredentials {
        guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw ProviderError("invalid_credentials_file", "Gemini login must be a JSON object.")
        }
        let expiryDate: Date?
        if let ed = json["expiry_date"] as? Double {
            expiryDate = Date(timeIntervalSince1970: ed / 1000)
        } else if let eds = json["expiry_date"] as? String, let ed = Double(eds) {
            expiryDate = Date(timeIntervalSince1970: ed / 1000)
        } else {
            expiryDate = nil
        }

        return GeminiCredentials(
            accessToken: json["access_token"] as? String,
            idToken: json["id_token"] as? String,
            refreshToken: json["refresh_token"] as? String,
            expiryDate: expiryDate,
            directEmail: json["email"] as? String,
            clientId: json["client_id"] as? String,
            clientSecret: json["client_secret"] as? String
        )
    }

    // MARK: - API Requests

    private struct CodeAssistInfo {
        let tierId: String?
        let projectId: String?
    }

    // These two are best-effort only — mirror JS where both failures are caught and return null
    // Use short timeout so they don't eat into the quota fetch budget
    private static let optionalRequestTimeout: Double = 5

    private func loadCodeAssist(accessToken: String) async -> CodeAssistInfo {
        guard let url = URL(string: Self.loadCodeAssistURL) else { return CodeAssistInfo(tierId: nil, projectId: nil) }
        var request = URLRequest(url: url, timeoutInterval: Self.optionalRequestTimeout)
        request.httpMethod = "POST"
        request.setValue("Bearer \(accessToken)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try? JSONSerialization.data(withJSONObject: [
            "metadata": ["ideType": "GEMINI_CLI", "pluginType": "GEMINI"]
        ])

        guard let (data, _) = try? await QuotaHTTP.data(for: request),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return CodeAssistInfo(tierId: nil, projectId: nil)
        }

        let tierId = (json["currentTier"] as? [String: Any])?["id"] as? String
        let projectId = normalizeProjectId(json["cloudaicompanionProject"])
        return CodeAssistInfo(tierId: tierId, projectId: projectId)
    }

    private func discoverProjectId(accessToken: String) async -> String? {
        guard let url = URL(string: Self.projectsURL) else { return nil }
        var request = QuotaHTTP.request(url: url, timeout: Self.optionalRequestTimeout)
        request.setValue("Bearer \(accessToken)", forHTTPHeaderField: "Authorization")

        guard let (data, _) = try? await QuotaHTTP.data(for: request),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let projects = json["projects"] as? [[String: Any]] else { return nil }

        for project in projects {
            if let id = project["projectId"] as? String {
                if id.hasPrefix("gen-lang-client") { return id }
                if let labels = project["labels"] as? [String: Any], labels["generative-language"] != nil { return id }
            }
        }
        return nil
    }

    private func retrieveQuota(accessToken: String, projectId: String?) async throws -> [String: Any] {
        guard let url = URL(string: Self.quotaURL) else {
            throw ProviderError("invalid_url", "Gemini quota URL is invalid.")
        }
        var request = URLRequest(url: url, timeoutInterval: timeoutSeconds)
        request.httpMethod = "POST"
        request.setValue("Bearer \(accessToken)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        var body: [String: Any] = [:]
        if let p = projectId { body["project"] = p }
        request.httpBody = try? JSONSerialization.data(withJSONObject: body)

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

    // MARK: - Response Parsing

    private func parseQuotaResponse(_ response: [String: Any], creds: GeminiCredentials, projectId: String?, tierId: String?, source: SourceInfo) -> ProviderUsage {
        let buckets = response["buckets"] as? [[String: Any]] ?? []

        // Group by modelId, keep lowest remainingFraction
        var byModel: [String: (remainingFraction: Double, resetTime: String?, tokenType: String?)] = [:]
        for bucket in buckets {
            guard let modelId = bucket["modelId"] as? String,
                  let remainingFraction = bucket["remainingFraction"] as? Double else { continue }
            if let existing = byModel[modelId], existing.remainingFraction <= remainingFraction { continue }
            byModel[modelId] = (
                remainingFraction: remainingFraction,
                resetTime: bucket["resetTime"] as? String,
                tokenType: bucket["tokenType"] as? String
            )
        }

        let models = byModel.map { (modelId, info) -> [String: Any] in
            let percentLeft = info.remainingFraction * 100
            return [
                "modelId": modelId,
                "tokenType": info.tokenType ?? "",
                "percentLeft": percentLeft,
                "usedPercent": max(0, 100 - percentLeft),
                "resetAt": info.resetTime ?? "",
                "resetDescription": formatGeminiReset(info.resetTime)
            ]
        }.sorted { ($0["modelId"] as? String ?? "") < ($1["modelId"] as? String ?? "") }

        // Select model groups
        let proModels    = models.filter { ($0["modelId"] as? String ?? "").lowercased().contains("pro") }
        let flashModels  = models.filter { let id = ($0["modelId"] as? String ?? "").lowercased(); return id.contains("flash") && !id.contains("flash-lite") }
        let liteModels   = models.filter { ($0["modelId"] as? String ?? "").lowercased().contains("flash-lite") }

        let lowestPercent = models.compactMap { $0["percentLeft"] as? Double }.min() ?? 100

        var usage = ProviderUsage(provider: "gemini", label: "Gemini CLI")
        usage.accountEmail = creds.accountEmail
        usage.usageAccountId = projectId ?? creds.accountEmail
        usage.accountPlan = parsePlan(tierId, email: creds.accountEmail)
        usage.primary   = selectGeminiWindow(from: proModels)
        usage.secondary = selectGeminiWindow(from: flashModels)
        usage.tertiary  = selectGeminiWindow(from: liteModels)

        usage.extra["projectId"]        = AnyCodable(projectId ?? "")
        usage.extra["lowestPercentLeft"] = AnyCodable(lowestPercent)

        usage.source = source
        return usage
    }

    private func selectGeminiWindow(from models: [[String: Any]]) -> RawQuotaWindow? {
        guard let lowest = models.min(by: { ($0["percentLeft"] as? Double ?? 100) < ($1["percentLeft"] as? Double ?? 100) }) else { return nil }
        let percentLeft = lowest["percentLeft"] as? Double ?? 100
        var window = RawQuotaWindow()
        window.remainingPercent = percentLeft
        window.usedPercent = max(0, 100 - percentLeft)
        window.resetAt = lowest["resetAt"] as? String
        window.resetDescription = lowest["resetDescription"] as? String
        return window
    }

    private func formatGeminiReset(_ resetTime: String?) -> String {
        guard let s = resetTime, let date = SharedFormatters.parseISO8601(s) else { return "Resets soon" }
        let diff = date.timeIntervalSinceNow
        if diff <= 0 { return "Resets soon" }
        let totalMin = Int(diff / 60)
        let hours = totalMin / 60
        let mins = totalMin % 60
        return hours > 0 ? "Resets in \(hours)h \(mins)m" : "Resets in \(mins)m"
    }

    private func parsePlan(_ tierId: String?, email: String?) -> String? {
        guard let tier = tierId else { return email?.hasSuffix(".edu") == true ? "Education" : nil }
        switch tier.lowercased() {
        case "free": return "Free"
        case "standard": return "Standard"
        case "enterprise": return "Enterprise"
        default: return tier.capitalized
        }
    }

    private func normalizeProjectId(_ value: Any?) -> String? {
        if let s = value as? String, !s.isEmpty { return s }
        if let d = value as? [String: Any] {
            return d["id"] as? String ?? d["projectId"] as? String
        }
        return nil
    }

}
