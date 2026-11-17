// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

/// 可测试的字段级安装/恢复，不接触官方登录和其他 settings 字段。
public struct ClaudeSubscriptionConnection: Sendable {
    public let store: ClaudeSubscriptionStore
    public init(store: ClaudeSubscriptionStore = .init()) { self.store = store }

    public static func validatedDirectory(_ directory: String) throws -> String {
        try NativeClaude.call(.validate(directory: directory))
    }

    public func install(directory: String, name: String, helperPath: String, renewBinding: Bool = false) throws -> ClaudeSubscriptionProfile {
        try NativeClaude.call(.install(directory: directory, name: name, helper: helperPath, renew: renewBinding), root: store.root.path)
    }

    public static func routesThroughAPI(directory: String) -> Bool {
        NativeClaude.value(.routes(directory: directory))
    }

    public func preservingStatusLine(in incoming: [String: Any], current: [String: Any], directory: String) throws -> [String: Any] {
        let useCurrent: Bool = try NativeClaude.call(.preserve(directory: directory,
            activeObject: current["statusLine"] is [String: Any],
            incomingCommand: (incoming["statusLine"] as? [String: Any])?["command"] as? String), root: store.root.path)
        var result = incoming
        if useCurrent { result["statusLine"] = current["statusLine"] }
        return result
    }

    public func rollbackInstallation(_ installed: ClaudeSubscriptionProfile, previous: ClaudeSubscriptionProfile?) throws {
        let _: Bool = try NativeClaude.call(.rollback(installed: installed, previous: previous), root: store.root.path)
    }

    @discardableResult
    public func uninstall(profileID: String, forAccountRemoval: Bool = false) throws -> Bool {
        try NativeClaude.call(.uninstall(id: profileID, forRemoval: forAccountRemoval), root: store.root.path)
    }
}
