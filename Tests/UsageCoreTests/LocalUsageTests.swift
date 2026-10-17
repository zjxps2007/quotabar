import Testing
import Foundation
@testable import UsageCore

final class LocalUsageTests{
    let now = ISO8601DateFormatter().date(from: "2026-10-16T12:00:00Z")!
    let zone = TimeZone(secondsFromGMT: 0)!
    func directory() throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }
    func write(_ rows: [[String: Any]], to url: URL) throws {
        let lines = try rows.map { String(decoding: try JSONSerialization.data(withJSONObject: $0), as: UTF8.self) }
        try (lines.joined(separator: "\n") + "\n").write(to: url, atomically: true, encoding: .utf8)
    }
    func metadata(_ id: String, parent: String? = nil, time: String = "2026-10-16T00:00:00Z") -> [String: Any] {
        var payload: [String: Any] = ["id": id, "timestamp": time]
        payload["forked_from_id"] = parent
        return ["type": "session_meta", "timestamp": time, "payload": payload]
    }
    func count(_ input: Int, output: Int = 10, time: String = "2026-10-16T01:00:00Z", cumulative: Bool = true) -> [String: Any] {
        ["type": "event_msg", "timestamp": time, "payload": ["type": "token_count", "info": ["model": "test-model",
            cumulative ? "total_token_usage" : "last_token_usage": ["input_tokens": input, "cached_input_tokens": input / 2, "output_tokens": output]]]]
    }
    func claude(_ id: String, input: Int, output: Int, content: String = "private text") -> [String: Any] {
        ["type": "assistant", "timestamp": "2026-10-16T02:00:00Z", "message": ["id": id, "model": "claude-test", "content": content,
          "usage": ["input_tokens": input, "output_tokens": output, "cache_read_input_tokens": 30, "cache_creation_input_tokens": 20]]]
    }
    @Test func testCumulativeCodexAndCopiedFilesAreNotDoubleCounted() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        let rows = [metadata("one"), count(100), count(100), count(150, output: 30, time: "2026-10-16T02:00:00Z")]
        try write(rows, to: root.appendingPathComponent("one.jsonl"))
        try write(rows, to: root.appendingPathComponent("copy.jsonl"))
        let report = try await LocalUsageScanner().scan(codexRoots: [root, root], claudeRoots: [], now: now, timeZone: zone)
        checkEqual(report.tokens(on: "2026-10-16"), 180)
        checkEqual(report.days.first?.counts.cached, 75)
    }
    @Test func testForkSubtractsParentBaseline() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        try write([metadata("parent"), count(100)], to: root.appendingPathComponent("parent.jsonl"))
        try write([metadata("child", parent: "parent", time: "2026-10-16T02:00:00Z"), count(140, output: 20, time: "2026-10-16T03:00:00Z")], to: root.appendingPathComponent("child.jsonl"))
        let result = try await LocalUsageScanner().scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        checkEqual(result.tokens(on: "2026-10-16"), 160)
    }
    @Test func testOrphanForkHasExplicitPartialWarning() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        try write([metadata("child", parent: "missing"), count(140), count(160, output: 20, time: "2026-10-16T03:00:00Z")], to: root.appendingPathComponent("child.jsonl"))
        let result = try await LocalUsageScanner().scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        checkEqual(result.tokens(on: "2026-10-16"), 30); checkFalse(result.warnings.isEmpty)
    }
    @Test func testClaudeStreamingRecordsMergeAndCacheIsIncludedOnce() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        try write([claude("response", input: 100, output: 10), claude("response", input: 100, output: 30)], to: root.appendingPathComponent("one.jsonl"))
        try write([claude("response", input: 100, output: 20)], to: root.appendingPathComponent("copy.jsonl"))
        let result = try await LocalUsageScanner().scan(codexRoots: [], claudeRoots: [root], now: now, timeZone: zone)
        checkEqual(result.tokens(on: "2026-10-16"), 180)
    }
    @Test func testLargeConversationBodyIsSkipped() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        try write([claude("response", input: 100, output: 30, content: String(repeating: "SENSITIVE", count: 400_000))], to: root.appendingPathComponent("one.jsonl"))
        let result = try await LocalUsageScanner().scan(codexRoots: [], claudeRoots: [root], now: now, timeZone: zone)
        checkEqual(result.tokens(on: "2026-10-16"), 180)
        checkFalse(String(decoding: try JSONEncoder().encode(result), as: UTF8.self).contains("SENSITIVE"))
    }
    @Test func testCacheInvalidatesOnAppendAndRemoval() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("one.jsonl"), scanner = LocalUsageScanner()
        try write([metadata("one"), count(100)], to: file)
        let first = try await scanner.scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        checkEqual(first.tokens(on: "2026-10-16"), 110)
        try write([metadata("one"), count(100), count(200, output: 20, time: "2026-10-16T02:00:00Z")], to: file)
        let next = try await scanner.scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        checkEqual(next.tokens(on: "2026-10-16"), 220)
        try FileManager.default.removeItem(at: file)
        let removed = try await scanner.scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        checkTrue(removed.days.isEmpty)
    }
    @Test func testDateBoundaryAndOldBaseline() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        try write([metadata("one"), count(100, time: "2026-10-15T23:30:00Z"), count(200, output: 20, time: "2026-10-16T01:30:00Z")], to: root.appendingPathComponent("one.jsonl"))
        let result = try await LocalUsageScanner().scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: TimeZone(secondsFromGMT: 9 * 3600)!)
        checkEqual(result.tokens(on: "2026-10-16"), 220)
        checkEqual(result.tokens(on: "2026-10-15"), 0)
    }
    @Test func testMalformedCompleteRowIsReported() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        try Data("{broken}\n".utf8).write(to: root.appendingPathComponent("one.jsonl"))
        let result = try await LocalUsageScanner().scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        checkFalse(result.warnings.isEmpty); checkTrue(result.days.isEmpty)
    }

    @Test func testToolContentIsSkippedWithoutDiscardingUsage() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        let file = root.appendingPathComponent("tools.jsonl")
        var row = claude("response", input: 100, output: 30)
        var message = row["message"] as! [String: Any]
        message["content"] = [["type": "tool_use", "id": "tool", "name": String(repeating: "PRIVATE", count: 4_000),
            "input": ["skill": "PRIVATE", "arguments": ["quoted": "\\\"nested\\\"", "data": [1, 2, 3]]]]] as [[String: Any]]
        row["message"] = message
        try write([row], to: file)
        var projected: [Data] = []
        _ = try SessionProjectionReader.scan(path: file.path, cursor: nil) { projected.append($0) }
        checkEqual(projected.count, 1)
        checkFalse(String(decoding: projected[0], as: UTF8.self).contains("PRIVATE"))
        let result = try await LocalUsageScanner().scan(codexRoots: [], claudeRoots: [root], now: now, timeZone: zone)
        checkEqual(result.tokens(on: "2026-10-16"), 180)
        checkTrue(result.warnings.isEmpty)
    }

    @Test func testDeduplicationPreservesDifferentCountsAndModelsAtSameTime() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        let first = count(100, output: 10, cumulative: false)
        let second = count(100, output: 20, cumulative: false)
        var anotherModel = first
        var payload = anotherModel["payload"] as! [String: Any]
        var info = payload["info"] as! [String: Any]; info["model"] = "another-model"
        payload["info"] = info; anotherModel["payload"] = payload
        let rows = [metadata("same-time"), first, second, first, anotherModel]
        try write(rows, to: root.appendingPathComponent("one.jsonl"))
        try write(rows, to: root.appendingPathComponent("copy.jsonl"))
        let result = try await LocalUsageScanner().scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        checkEqual(result.tokens(on: "2026-10-16"), 340)
        checkEqual(result.days.count, 2)
        checkEqual(result.days.reduce(0) { $0 + $1.counts.cached }, 150)
    }

    @Test func testCalendarBucketsHandleDSTAndNonWholeHourOffsets() async throws {
        let fixtures: [(String, String, [String], [String: Int])] = [
            ("America/Los_Angeles", "2026-03-10T12:00:00Z",
             ["2026-03-08T07:59:59Z", "2026-03-08T08:00:00Z", "2026-03-08T09:30:00Z", "2026-03-08T10:30:00Z", "2026-03-09T07:00:00Z"],
             ["2026-03-07": 11, "2026-03-08": 33, "2026-03-09": 11]),
            ("America/Los_Angeles", "2026-11-03T12:00:00Z",
             ["2026-11-01T06:59:59Z", "2026-11-01T08:30:00Z", "2026-11-01T09:30:00Z", "2026-11-02T08:00:00Z"],
             ["2026-10-31": 11, "2026-11-01": 22, "2026-11-02": 11]),
            ("Asia/Kathmandu", "2026-10-17T12:00:00Z",
             ["2026-10-15T18:14:59Z", "2026-10-15T18:15:00Z"],
             ["2026-10-15": 11, "2026-10-16": 11]),
            // Havana's spring transition skips midnight on the first day in the range.
            ("America/Havana", "2026-06-05T12:00:00Z",
             ["2026-03-09T03:59:59Z", "2026-03-09T04:00:00Z", "2026-03-10T04:00:00Z"],
             ["2026-03-08": 11, "2026-03-09": 11, "2026-03-10": 11])
        ]
        for (identifier, end, timestamps, expected) in fixtures {
            let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
            try write([metadata("calendar")] + timestamps.map { count(10, output: 1, time: $0, cumulative: false) },
                      to: root.appendingPathComponent("calendar.jsonl"))
            let result = try await LocalUsageScanner().scan(codexRoots: [root], claudeRoots: [],
                now: SharedFormatters.parseISO8601(end)!, timeZone: TimeZone(identifier: identifier)!)
            checkEqual(Dictionary(uniqueKeysWithValues: result.days.map { ($0.day, $0.counts.total) }), expected)
        }
    }

    @Test func testSixMonthActivityIncludesOlderLogsAndPreservesCachedResults() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        try write([metadata("six-months"), count(100, time: "2026-04-17T23:59:59Z"),
                   count(200, output: 20, time: "2026-04-18T00:00:00Z"),
                   count(300, output: 30, time: "2026-07-01T00:00:00Z"),
                   count(400, output: 40, time: "2026-10-16T12:00:00Z")],
                  to: root.appendingPathComponent("activity.jsonl"))
        let scanner = LocalUsageScanner()
        let report = try await scanner.scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone,
            historyDays: UsageActivity.historyDays)
        let cached = try await scanner.scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone,
            historyDays: UsageActivity.historyDays)
        checkEqual(report, cached)
        checkEqual(report.tokens(on: "2026-04-17"), 0)
        checkEqual(report.tokens(on: "2026-04-18"), 110)
        checkEqual(report.tokens(on: "2026-07-01"), 110)
        checkEqual(report.tokens(on: "2026-10-16"), 110)
    }

    @Test func testWindowCutoffKeepsOldBaselineAndExcludesFutureEvents() async throws {
        let root = try directory(); defer { try? FileManager.default.removeItem(at: root) }
        try write([metadata("boundary"), count(100, time: "2026-07-18T23:59:59Z"),
                   count(200, output: 20, time: "2026-07-19T00:00:00Z"),
                   count(400, output: 40, time: "2026-10-16T12:00:00Z"),
                   count(800, output: 80, time: "2026-10-16T12:00:01Z")],
                  to: root.appendingPathComponent("boundary.jsonl"))
        let scanner = LocalUsageScanner()
        let first = try await scanner.scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        let cached = try await scanner.scan(codexRoots: [root], claudeRoots: [], now: now, timeZone: zone)
        checkEqual(first, cached)
        checkEqual(first.tokens(on: "2026-07-19"), 110)
        checkEqual(first.tokens(on: "2026-10-16"), 220)
        checkEqual(first.days.count, 2)
    }
}
