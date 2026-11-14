// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation
#if canImport(Darwin)
import Darwin
#endif

public enum ClaudeSubscriptionStatusLine {
    private struct Input: Decodable {
        struct Limits: Decodable {
            struct Window: Decodable {
                let used_percentage: Double
                let resets_at: Double
            }
            let five_hour: Window?
            let seven_day: Window?
        }
        let session_id: String
        let rate_limits: Limits?
    }

    public static func snapshot(input: Data, profile: ClaudeSubscriptionProfile, now: Date = Date()) throws -> ClaudeSubscriptionSnapshot? {
        guard input.count <= 2 * 1024 * 1024 else { throw ProviderError("invalid_snapshot", "Status line input is too large.") }
        let payload = try JSONDecoder().decode(Input.self, from: input)
        guard UUID(uuidString: payload.session_id) != nil, let limits = payload.rate_limits else { return nil }
        func window(_ raw: Input.Limits.Window?) throws -> ClaudeSubscriptionSnapshot.Window? {
            guard let raw else { return nil }
            guard raw.used_percentage.isFinite, (0...100).contains(raw.used_percentage),
                  raw.resets_at.isFinite, raw.resets_at > 0 else {
                throw ProviderError("invalid_snapshot", "Claude returned an invalid quota window.")
            }
            return .init(usedPercent: raw.used_percentage, resetAt: Date(timeIntervalSince1970: raw.resets_at))
        }
        return try .init(schemaVersion: 1, profileID: profile.id, generation: profile.generation,
                         sessionID: payload.session_id, receivedAt: now, observedAt: now,
                         fiveHour: window(limits.five_hour), sevenDay: window(limits.seven_day))
    }

}

public enum ClaudeSubscriptionLaunch {
    /// 网络代理（HTTPS_PROXY 等）不在此列表中，保留用户的出网通道。
    public static let authEnvironmentKeys = [
        "ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_PROFILE", "ANTHROPIC_FEDERATION_RULE_ID", "ANTHROPIC_ORGANIZATION_ID",
        "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY",
        "CLAUDE_CODE_USE_ANTHROPIC_FOUNDRY", "CLAUDE_CODE_GATEWAY_URL"
    ]
    public static let modelEnvironmentKeys = [
        "ANTHROPIC_MODEL", "ANTHROPIC_SMALL_FAST_MODEL", "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_CUSTOM_MODEL_OPTION", "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY"
    ]

    public static func quote(_ value: String) -> String { "'" + value.replacingOccurrences(of: "'", with: "'\\''") + "'" }

    /// 会话级覆盖，不写代理配置，不修改官方凭证，也不停止已运行的进程。
    public static func settingsOverride() throws -> String {
        let env = Dictionary(uniqueKeysWithValues: (authEnvironmentKeys + modelEnvironmentKeys).map { ($0, "") })
        let data = try JSONSerialization.data(withJSONObject: ["env": env, "apiKeyHelper": "", "model": "sonnet"], options: [.sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }

    /// AIUsage 在后台运行官方 CLI（登录 / 状态检查）时的环境：去掉 API 与代理模型变量，保留出网代理。
    public static func environment(_ base: [String: String], configDirectory: String,
                                   homeDirectory: String = NSHomeDirectory()) -> [String: String] {
        var env = base
        for key in authEnvironmentKeys + modelEnvironmentKeys { env.removeValue(forKey: key) }
        // 显式设置 CLAUDE_CONFIG_DIR 会让官方 CLI 改用另一份登录凭证，即使路径就是默认的 ~/.claude。
        // 默认配置必须不设置，才能读到用户平时 `claude` 的登录状态。
        if isDefaultConfigDirectory(configDirectory, homeDirectory: homeDirectory) {
            env.removeValue(forKey: "CLAUDE_CONFIG_DIR")
        } else {
            env["CLAUDE_CONFIG_DIR"] = configDirectory
        }
        return env
    }

    public static func isDefaultConfigDirectory(_ directory: String, homeDirectory: String = NSHomeDirectory()) -> Bool {
        let defaultPath = URL(fileURLWithPath: homeDirectory).appendingPathComponent(".claude").path
        return ClaudeSubscriptionProfile(configDirectory: directory, name: "").configDirectory
            == ClaudeSubscriptionProfile(configDirectory: defaultPath, name: "").configDirectory
    }

    /// 独立账号由用户在自己的终端里使用；AIUsage 不代为打开任何终端。
    public static func terminalCommand(configDirectory: String, homeDirectory: String = NSHomeDirectory()) -> String {
        isDefaultConfigDirectory(configDirectory, homeDirectory: homeDirectory) ? "claude" : "CLAUDE_CONFIG_DIR=\(quote(configDirectory)) claude"
    }
}
