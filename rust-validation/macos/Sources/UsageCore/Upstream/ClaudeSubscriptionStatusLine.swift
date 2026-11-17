// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import CQuotaBar
import Foundation
#if canImport(Darwin)
import Darwin
#endif

public enum ClaudeSubscriptionStatusLine {
    public static func snapshot(input: Data, profile: ClaudeSubscriptionProfile, now: Date = Date()) throws -> ClaudeSubscriptionSnapshot? {
        var output: UnsafeMutablePointer<CChar>?
        let status = input.withUnsafeBytes {
            qb_mac_snapshot_parse(profile.id, profile.generation, now.timeIntervalSinceReferenceDate,
                                  $0.bindMemory(to: UInt8.self).baseAddress, $0.count, &output)
        }
        switch status {
        case 0: return try ClaudeSubscriptionSnapshot.decodeNative(output)
        case -2: throw ProviderError("invalid_snapshot", "Status line input is too large.")
        case -3: throw DecodingError.dataCorrupted(.init(codingPath: [], debugDescription: "Invalid Claude status line input."))
        default: throw ProviderError("invalid_snapshot", "Claude returned an invalid quota window.")
        }
    }
}

public enum ClaudeSubscriptionLaunch {
    /// 网络代理（HTTPS_PROXY 等）不在此列表中，保留用户的出网通道。
    public static let authEnvironmentKeys: [String] = NativeClaude.value(.keys(model: false))
    public static let modelEnvironmentKeys: [String] = NativeClaude.value(.keys(model: true))

    public static func quote(_ value: String) -> String { NativeClaude.value(.quote(value: value)) }

    public static func settingsOverride() throws -> String { try NativeClaude.call(.settingsOverride) }

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
        NativeClaude.value(.defaultDirectory(directory: directory, home: homeDirectory))
    }

    /// 独立账号由用户在自己的终端里使用；AIUsage 不代为打开任何终端。
    public static func terminalCommand(configDirectory: String, homeDirectory: String = NSHomeDirectory()) -> String {
        NativeClaude.value(.terminal(directory: configDirectory, home: homeDirectory))
    }
}
