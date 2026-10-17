import Foundation
import CryptoKit

public enum ProviderKind: String, Codable, CaseIterable, Identifiable, Sendable {
    case codex, claude, gemini
    public var id: String { rawValue }
    public var title: String {
        switch self { case .codex: return "Codex"; case .claude: return "Claude"; case .gemini: return "Gemini" }
    }
    public var defaultPath: String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        switch self {
        case .codex: return (ProcessInfo.processInfo.environment["CODEX_HOME"] ?? home + "/.codex") + "/auth.json"
        case .claude: return ProcessInfo.processInfo.environment["CLAUDE_CONFIG_DIR"] ?? home + "/.claude"
        case .gemini: return home + "/.gemini/oauth_creds.json"
        }
    }
}

public struct MonitoredAccount: Codable, Identifiable, Equatable, Sendable {
    public let id: UUID
    public let provider: ProviderKind
    public var name: String
    public let identity: String
    public var sourcePath: String
    /// Missing in older settings: those accounts remain read-only file imports.
    public var appManagedLogin: Bool?
    public var usesAppLogin: Bool { provider == .codex && appManagedLogin == true }
    /// Older settings keep showing every monitored account.
    public var showInMenuBar: Bool?
    public var isShownInMenuBar: Bool { showInMenuBar ?? true }
    public var enabled: Bool
    public let createdAt: Date
    public init(id: UUID = UUID(), provider: ProviderKind, name: String, identity: String, sourcePath: String,
                enabled: Bool = true, createdAt: Date = Date()) {
        self.id = id; self.provider = provider; self.name = name; self.identity = identity
        self.sourcePath = sourcePath; self.enabled = enabled; self.createdAt = createdAt
    }
}

/// Identity is pinned independently of a path or display alias. CLI account switching
/// must never make an existing card silently display a different workspace.
public struct ImportedIdentity: Equatable, Sendable {
    public let key: String
    public let label: String
    public static func parse(_ data: Data, provider: ProviderKind) throws -> Self {
        guard data.count <= 1_048_576,
              let json = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw ProviderError("invalid_auth", "로그인 파일을 읽을 수 없습니다. JSON 형식을 확인하세요.")
        }
        let tokens = json["tokens"] as? [String: Any] ?? json
        guard let access = (tokens["access_token"] ?? tokens["accessToken"]) as? String, !access.isEmpty else {
            throw ProviderError("missing_token", "구독 계정의 CLI 로그인 파일이 필요합니다. API 키는 지원하지 않습니다.")
        }
        switch provider {
        case .codex:
            let identity = CodexAccountIdentity(authJSON: json)
            guard let key = identity.key else {
                throw ProviderError("missing_identity", "Codex의 워크스페이스와 사용자 정보를 확인할 수 없습니다. CLI에서 다시 로그인하세요.")
            }
            return Self(key: key, label: identity.email ?? "Codex · \(identity.accountId?.prefix(8) ?? "account")")
        case .gemini:
            let claims = jwt((json["id_token"] ?? json["idToken"]) as? String)
            let email = (claims?["email"] as? String ?? json["email"] as? String)?.lowercased()
            if let subject = claims?["sub"] as? String, !subject.isEmpty {
                return Self(key: "gemini:subject:\(subject)", label: email ?? "Gemini account")
            }
            if let email, !email.isEmpty { return Self(key: "gemini:email:\(email)", label: email) }
            guard let refresh = json["refresh_token"] as? String, !refresh.isEmpty else {
                throw ProviderError("missing_identity", "Gemini 계정 식별 정보가 없습니다. CLI에서 다시 로그인하세요.")
            }
            let digest = SHA256.hash(data: Data(refresh.utf8)).map { String(format: "%02x", $0) }.joined()
            return Self(key: "gemini:credential:\(digest)", label: "Gemini · \(digest.prefix(6))")
        case .claude:
            throw ProviderError("invalid_auth", "Claude는 CLI 설정 폴더를 연결하세요.")
        }
    }
    private static func jwt(_ token: String?) -> [String: Any]? {
        guard let pieces = token?.split(separator: "."), pieces.count >= 2 else { return nil }
        var encoded = String(pieces[1]).replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        encoded += String(repeating: "=", count: (4 - encoded.count % 4) % 4)
        guard let data = Data(base64Encoded: encoded) else { return nil }
        return try? JSONSerialization.jsonObject(with: data) as? [String: Any]
    }
}

public struct QuotaWindow: Codable, Equatable, Identifiable, Sendable {
    public var id: String { title }
    public let title: String
    public let remaining: Double
    public let resetAt: Date?
    public init(title: String, remaining: Double, resetAt: Date? = nil) {
        self.title = title; self.remaining = min(100, max(0, remaining.isFinite ? remaining : 0)); self.resetAt = resetAt
    }
}

public struct AccountSnapshot: Codable, Sendable {
    public let fetchedAt: Date
    public let observedAt: Date?
    public let plan: String?
    public let email: String?
    public let windows: [QuotaWindow]
    public let state: String?
    public var lowestRemaining: Double? { windows.map(\.remaining).min() }
    public init(usage: ProviderUsage, provider: ProviderKind, now: Date = Date()) {
        fetchedAt = now
        observedAt = SharedFormatters.parseISO8601(usage.fetchedAt)
        plan = usage.accountPlan; email = usage.accountEmail
        state = usage.extra["snapshotState"]?.value as? String
        let labels = provider == .gemini ? ["Pro", "Flash", "Flash Lite"] : ["5시간", "주간", "Code review"]
        windows = [usage.primary, usage.secondary, usage.tertiary].enumerated().compactMap { index, raw in
            guard let raw, let value = raw.remainingPercent ?? raw.usedPercent.map({ 100 - $0 }), value.isFinite else { return nil }
            return QuotaWindow(title: raw.label ?? labels[index], remaining: value,
                resetAt: raw.resetAt.flatMap(SharedFormatters.parseISO8601))
        }
    }
}

public enum AppearanceMode: String, Codable, CaseIterable, Sendable {
    case system, light, dark

    public init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(String.self)
        // A newer theme value must not prevent the saved accounts from loading.
        self = Self(rawValue: value) ?? .system
    }
}

public enum MenuBarDisplayMode: String, Codable, CaseIterable, Sendable {
    case providerAndRemaining, remainingOnly, iconOnly

    public init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer().decode(String.self)
        self = Self(rawValue: value) ?? .providerAndRemaining
    }
}

public struct LiteSettings: Codable, Sendable {
    public var accounts: [MonitoredAccount] = []
    public var refreshSeconds: Int = 300
    public var scanLocalLogs = true
    /// Missing in older settings, which follow the system appearance.
    public var appearance: AppearanceMode?
    public var selectedAppearance: AppearanceMode { appearance ?? .system }
    /// Optional storage preserves the presentation of older installations.
    public var showDockIcon: Bool?
    public var isDockIconVisible: Bool { showDockIcon ?? true }
    public var menuBarDisplayMode: MenuBarDisplayMode?
    public var selectedMenuBarDisplayMode: MenuBarDisplayMode { menuBarDisplayMode ?? .providerAndRemaining }
    /// Nil in releases that stored credentials in the macOS keychain.
    public var credentialStorageVersion: Int?
    public init() {}
}

public struct SettingsFile: Sendable {
    public let url: URL
    public init(url: URL) { self.url = url }
    public func load() throws -> LiteSettings {
        guard FileManager.default.fileExists(atPath: url.path) else { return LiteSettings() }
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode(LiteSettings.self, from: data)
    }
    public func save(_ value: LiteSettings) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700])
        try JSONEncoder().encode(value).write(to: url, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }
}
