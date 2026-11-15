// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

public struct ClaudeSubscriptionProvider: CredentialAcceptingProvider {
    public let id = "claude-subscription"
    public let displayName = "Claude Subscription"
    public let description = "Official Claude Code subscription quota snapshots"
    public let supportedAuthMethods: [AuthMethod] = [.auto]
    private let store: ClaudeSubscriptionStore

    public init(store: ClaudeSubscriptionStore = .init()) { self.store = store }

    public func fetchUsage() async throws -> ProviderUsage {
        throw ProviderError("not_logged_in", "Connect an official Claude Code configuration directory.")
    }

    public func fetchUsage(with credential: AccountCredential) async throws -> ProviderUsage {
        try NativeClaude.call(.fetch(directory: credential.credential, authMethod: credential.authMethod.rawValue,
            sourceKind: credential.metadata["sourceKind"], now: Date().timeIntervalSinceReferenceDate), root: store.root.path)
    }
}
