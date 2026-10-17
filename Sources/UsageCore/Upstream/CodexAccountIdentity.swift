// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

/// 一个用户在一个工作区中的订阅身份。名称、套餐和导入路径都不是原生身份。
public struct CodexAccountIdentity: Equatable, Sendable {
    public let accountId: String?
    public let userId: String?
    public let email: String?

    public init(accountId: String?, userId: String?, email: String? = nil) {
        self.accountId = Self.normalized(accountId)
        self.userId = Self.normalized(userId)
        let email = Self.normalized(email)
        self.email = email?.contains("@") == true ? email : nil
    }

    public init(credential: AccountCredential) {
        self.init(
            accountId: credential.metadata["accountId"],
            userId: Self.normalized(credential.metadata["workspaceUserId"]) ?? credential.metadata["userId"],
            email: Self.normalized(credential.metadata["accountEmail"])
                ?? Self.normalized(credential.metadata["accountHandle"]) ?? credential.accountLabel
        )
    }

    /// 同时支持 Codex 原生 auth.json 和 CPA 扁平文件；先取用户原生 ID，最后才取 sub。
    public init(authJSON json: [String: Any]) {
        let tokens = json["tokens"] as? [String: Any] ?? [:]
        let claims = [
            tokens["id_token"] ?? tokens["idToken"] ?? json["id_token"] ?? json["idToken"],
            tokens["access_token"] ?? tokens["accessToken"] ?? json["access_token"] ?? json["accessToken"],
        ].compactMap { Self.jwtPayload($0 as? String) }
        let auth = claims.compactMap { $0["https://api.openai.com/auth"] as? [String: Any] }
        func first(_ values: [Any?]) -> String? {
            values.lazy.compactMap { Self.normalized($0 as? String) }.first
        }
        let accountId = first([tokens["account_id"], tokens["accountId"], json["account_id"], json["accountId"], json["chatgpt_account_id"]]
            + auth.map { $0["chatgpt_account_id"] } + claims.map { $0["chatgpt_account_id"] })
        let userId = first([json["chatgpt_user_id"]]
            + auth.map { $0["chatgpt_user_id"] } + claims.map { $0["chatgpt_user_id"] }
            + auth.map { $0["user_id"] } + claims.map { $0["user_id"] }
            + [json["user_id"]] + claims.map { $0["sub"] })
        let email = first([json["email"]] + claims.map { $0["email"] }
            + claims.map { ($0["https://api.openai.com/profile"] as? [String: Any])?["email"] })
        self.init(accountId: accountId, userId: userId, email: email)
    }

    public var nativeKey: String? {
        guard let accountId, let userId else { return nil }
        return "codex:account:\(accountId):user:\(userId)"
    }

    /// 旧数据缺少 userId 时，工作区 + 有效邮箱可用于关联；邮箱永远不能单独匹配。
    public var key: String? {
        if let nativeKey { return nativeKey }
        guard let accountId, let email else { return nil }
        return "codex:account:\(accountId):email:\(email)"
    }

    public func conflicts(with other: Self) -> Bool {
        if let accountId, let otherId = other.accountId, accountId != otherId { return true }
        if let userId, let otherUser = other.userId { return userId != otherUser }
        if let email, let otherEmail = other.email, email != otherEmail { return true }
        return false
    }

    public func matches(_ other: Self) -> Bool {
        guard !conflicts(with: other), let accountId, accountId == other.accountId else { return false }
        if let userId, let otherUser = other.userId { return userId == otherUser }
        return email != nil && email == other.email
    }

    /// 不完整身份的路径/UUID 回退也保留已知字段，防止同路径的不同账号碰撞。
    public func key(fallback: String) -> String {
        key ?? "codex:incomplete:account:\(accountId ?? "*"):user:\(userId ?? "*"):email:\(email ?? "*"):\(fallback)"
    }

    private static func normalized(_ value: String?) -> String? {
        guard let normalized = value?.trimmingCharacters(in: .whitespacesAndNewlines)
            .precomposedStringWithCanonicalMapping.lowercased(), !normalized.isEmpty else { return nil }
        return normalized
    }

    private static func jwtPayload(_ token: String?) -> [String: Any]? {
        guard let pieces = token?.split(separator: "."), pieces.count >= 2 else { return nil }
        var payload = String(pieces[1]).replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        payload += String(repeating: "=", count: (4 - payload.count % 4) % 4)
        guard let data = Data(base64Encoded: payload) else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }
}
