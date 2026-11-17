import Testing
import Foundation
@testable import UsageCore

/// Both native implementations read the same synthetic log fixtures.
struct SharedContractTests {
    @Test func testSharedLocalUsageContract() async throws {
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("shared/fixtures/local-usage")
        let expected = try JSONSerialization.jsonObject(with: Data(contentsOf: root.appendingPathComponent("expected.json"))) as! [String: Any]
        let now = ISO8601DateFormatter().date(from: expected["now"] as! String)!
        let report = try await LocalUsageScanner().scan(codexRoots: [root.appendingPathComponent("codex")],
            claudeRoots: [root.appendingPathComponent("claude")], now: now, timeZone: TimeZone(secondsFromGMT: 0)!)
        for provider in [ProviderKind.codex, ProviderKind.claude] {
            let actual = report.days.filter { $0.provider == provider }.reduce(TokenCounts()) { $0.adding($1.counts) }
            let counts = expected[provider.rawValue] as! [String: Int]
            checkEqual(actual.input, counts["input"]!)
            checkEqual(actual.cached, counts["cached"]!)
            checkEqual(actual.output, counts["output"]!)
        }
        checkEqual(report.warnings.count, expected["warningCount"] as! Int)
    }
}
