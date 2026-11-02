import Testing
import Foundation
@testable import UsageCore

struct DateFormattingTests {
    @Test func testISO8601ParsingPreservesLegacyPrecisionAndFallbacks() {
        let standard = ISO8601DateFormatter()
        standard.formatOptions = [.withInternetDateTime]
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        for value in [
            "2026-10-16T12:00:00Z", "2026-10-16T12:00:00.123Z",
            "2026-10-16T12:00:00.123456789Z", "2026-10-16T12:00:00+09:00",
            "2026-10-16T12:00:00.123-07:00", "2026-10-16T12:00:00+0545",
            "2024-02-29T23:59:59Z", "2026-03-08T01:59:59-08:00",
            "2026-11-01T01:30:00-07:00", "2026-11-01T01:30:00-08:00",
            "1970-01-01T00:00:00Z", "1969-12-31T23:59:59.999Z",
            "", "invalid", "2026-10-16", "2026-10-16T12:00:00.invalidZ"
        ] {
            let expected = fractional.date(from: value) ?? standard.date(from: value)
            #expect(SharedFormatters.parseISO8601(value) == expected, "Timestamp: \(value)")
        }
    }
}
