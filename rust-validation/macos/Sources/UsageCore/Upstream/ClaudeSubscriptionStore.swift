// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import CQuotaBar
import Foundation

/// 这里只保存官方配置目录引用和额度快照，不保存登录凭证。
public struct ClaudeSubscriptionProfile: Codable, Sendable, Identifiable, Equatable {
    public let id: String
    public let configDirectory: String
    public var name: String
    public var generation: String
    public var originalStatusLine: Data?
    public var installedCommand: String?
    /// 连接时由官方 `auth status` 返回的身份，仅用于展示；旧 profile 解码为空。
    public var email: String?
    public var organizationName: String?
    public var subscriptionType: String?

    public init(configDirectory: String, name: String) {
        self = NativeClaude.value(.reference(directory: configDirectory, name: name))
    }
}

public struct ClaudeSubscriptionSnapshot: Codable, Sendable, Equatable {
    public struct Window: Codable, Sendable, Equatable {
        public let usedPercent: Double
        public let resetAt: Date
        public init(usedPercent: Double, resetAt: Date) {
            self.usedPercent = usedPercent
            self.resetAt = resetAt
        }
    }
    static func decodeNative(_ result: UnsafeMutablePointer<CChar>?) throws -> Self? {
        guard let result else { throw ProviderError("invalid_snapshot", "Could not read Claude quota data.") }
        defer { qb_string_free(result) }
        return try JSONDecoder().decode(Self?.self, from: Data(bytes: result, count: strlen(result)))
    }

    public let schemaVersion: Int
    public let profileID: String
    public let generation: String
    public let sessionID: String
    public var receivedAt: Date
    /// 仅表示这些额度字段第一次出现/变化的时间，不冒充官方测量时间。
    public var observedAt: Date
    public let fiveHour: Window?
    public let sevenDay: Window?
}

public struct ClaudeSubscriptionStore: Sendable {
    public let root: URL
    public init(root: URL = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent("Library/Application Support/AIUsage Lite/ClaudeProfiles", isDirectory: true)) {
        self.root = root
    }

    public func directory(for id: String) throws -> URL {
        let path: String = try NativeClaude.call(.folder(id: id), root: root.path)
        return URL(fileURLWithPath: path, isDirectory: true)
    }

    public func loadProfile(id: String) throws -> ClaudeSubscriptionProfile {
        try NativeClaude.call(.load(id: id), root: root.path)
    }

    public func saveProfile(_ profile: ClaudeSubscriptionProfile) throws {
        let _: Bool = try NativeClaude.call(.save(profile: profile), root: root.path)
    }

    func connectedProfiles() -> [ClaudeSubscriptionProfile] {
        (try? NativeClaude.call(.connected, root: root.path)) ?? []
    }

    public func latestSnapshot(for profile: ClaudeSubscriptionProfile) throws -> ClaudeSubscriptionSnapshot? {
        guard let folder = try? directory(for: profile.id) else { return nil }
        return try folder.withUnsafeFileSystemRepresentation { path in
            try ClaudeSubscriptionSnapshot.decodeNative(qb_mac_snapshot_latest(path, profile.id, profile.generation,
                                                                                Date().timeIntervalSinceReferenceDate))
        }
    }

    public func saveSnapshot(_ snapshot: ClaudeSubscriptionSnapshot) throws {
        let folder = try directory(for: snapshot.profileID)
        let data = try JSONEncoder().encode(snapshot)
        let status = folder.withUnsafeFileSystemRepresentation { path in
            data.withUnsafeBytes { qb_mac_snapshot_save(path, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }
        }
        if status > 0 { throw POSIXError(POSIXErrorCode(rawValue: status) ?? .EIO) }
        if status < 0 { throw ProviderError("invalid_snapshot", "Claude session ID is not valid.") }
    }

    public func write(_ data: Data, to url: URL) throws {
        try writePrivateFile(data, to: url)
    }
}
