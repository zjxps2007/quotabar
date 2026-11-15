// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

/// 官方 `claude auth status --json` 的白名单字段；输出本身不含 Token，这里也只取身份与登录类型。
public struct ClaudeAuthStatus: Decodable, Equatable, Sendable {
    public let loggedIn: Bool
    public let authMethod: String?
    public let apiProvider: String?
    public let configDirectory: String?
    public let email: String?
    public let organizationID: String?
    public let organizationName: String?
    public let subscriptionType: String?

    public init(loggedIn: Bool, authMethod: String? = nil, apiProvider: String? = nil, configDirectory: String? = nil,
                email: String? = nil, organizationID: String? = nil, organizationName: String? = nil, subscriptionType: String? = nil) {
        self.loggedIn = loggedIn
        self.authMethod = authMethod
        self.apiProvider = apiProvider
        self.configDirectory = configDirectory
        self.email = email
        self.organizationID = organizationID
        self.organizationName = organizationName
        self.subscriptionType = subscriptionType
    }

    public init(json: Data) throws {
        guard json.count < 64 * 1024 else { throw NativeClaude.failure(-56) }
        self = try NativeClaude.call(.authStatus(data: json))
    }

    /// 只有 claude.ai 订阅登录会产生 5 小时 / 每周额度；Console / API Key 登录不算。
    public var isSubscription: Bool {
        loggedIn && authMethod == "claude.ai" && (apiProvider ?? "firstParty") == "firstParty"
    }

    public var planLabel: String? { Self.planLabel(subscriptionType) }

    public static func planLabel(_ raw: String?) -> String? {
        NativeClaude.value(.plan(raw: raw))
    }
}
