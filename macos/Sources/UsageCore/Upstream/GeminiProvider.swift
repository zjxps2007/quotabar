// Derived from sylearn/AIUsage (Apache-2.0), commit e3502ea77b79c2ef5b88f33e36231f156243a86b.
// Modified for QuotaBar: standalone, read-only monitoring; no proxy runtime.
import Foundation

public struct GeminiProvider: Sendable {
    let timeoutSeconds: Double
    public init(timeoutSeconds: Double = 15) { self.timeoutSeconds = timeoutSeconds }

    /// CLI-managed OAuth: no token rotation or writes to the user's login files.
    public func fetch(authData: Data) async throws -> ProviderUsage {
        try await NativeProviderQuery.usage(.gemini, auth: authData, timeout: timeoutSeconds)
    }
}
