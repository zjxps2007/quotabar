import Foundation
import CryptoKit

/// Reproducible synthetic logs; never reads the user's sessions or credentials.
@main
struct LocalUsageBenchmark {
    static func main() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("aiusage-benchmark-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: root) }
        let codex = root.appendingPathComponent("codex")
        let claude = root.appendingPathComponent("claude")
        try FileManager.default.createDirectory(at: codex, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: claude, withIntermediateDirectories: true)
        let now = SharedFormatters.parseISO8601("2026-10-16T12:00:00Z")!
        let zone = TimeZone(secondsFromGMT: 0)!
        let body = String(repeating: "synthetic benchmark content ", count: 40)
        var bytes = 0
        for session in 0..<16 {
            var rows = ["{\"type\":\"session_meta\",\"payload\":{\"id\":\"session-\(session)\"}}"]
            for event in 0..<256 {
                let timestamp = SharedFormatters.iso8601String(from: now.addingTimeInterval(Double(-7_200 - (session % 7) * 86_400 + event * 8)))
                for _ in 0..<4 {
                    rows.append("{\"type\":\"response_item\",\"timestamp\":\"\(timestamp)\",\"payload\":{\"type\":\"message\",\"content\":\"\(body)\"}}")
                }
                rows.append("{\"type\":\"event_msg\",\"timestamp\":\"\(timestamp)\",\"payload\":{\"type\":\"token_count\",\"info\":{\"model\":\"benchmark-codex\",\"total_token_usage\":{\"input_tokens\":\((event + 1) * 100),\"cached_input_tokens\":\((event + 1) * 30),\"output_tokens\":\((event + 1) * 20)}}}}")
            }
            let data = Data((rows.joined(separator: "\n") + "\n").utf8)
            try data.write(to: codex.appendingPathComponent("session-\(session).jsonl"))
            bytes += data.count
            if session == 0 {
                try data.write(to: codex.appendingPathComponent("archived-copy.jsonl"))
                bytes += data.count
            }
        }
        for session in 0..<8 {
            var rows: [String] = []
            for event in 0..<256 {
                let timestamp = SharedFormatters.iso8601String(from: now.addingTimeInterval(Double(-7_200 - (session % 7) * 86_400 + event * 8)))
                for output in [20, 40] {
                    rows.append("{\"type\":\"assistant\",\"timestamp\":\"\(timestamp)\",\"message\":{\"id\":\"message-\(session)-\(event)\",\"model\":\"benchmark-claude\",\"content\":[{\"type\":\"tool_use\",\"id\":\"tool\",\"name\":\"synthetic\",\"input\":{\"skill\":\"benchmark\",\"text\":\"\(body)\"}}],\"usage\":{\"input_tokens\":100,\"cache_read_input_tokens\":30,\"cache_creation_input_tokens\":20,\"output_tokens\":\(output)}}}")
                }
            }
            let data = Data((rows.joined(separator: "\n") + "\n").utf8)
            try data.write(to: claude.appendingPathComponent("session-\(session).jsonl"))
            bytes += data.count
        }
        var cold: [Double] = [], warm: [Double] = []
        var checksum: String?
        var rows = 0, tokens = 0
        for _ in 0..<3 {
            let scanner = LocalUsageScanner()
            for cached in [false, true] {
                let start = ContinuousClock.now
                let report = try await scanner.scan(codexRoots: [codex], claudeRoots: [claude], now: now, timeZone: zone)
                let duration = start.duration(to: .now).components
                let milliseconds = Double(duration.seconds) * 1_000 + Double(duration.attoseconds) / 1e15
                if cached { warm.append(milliseconds) } else { cold.append(milliseconds) }
                let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys]
                let digest = SHA256.hash(data: try encoder.encode(report)).map { String(format: "%02x", $0) }.joined()
                if let checksum { precondition(digest == checksum, "Cold and cached reports differ") }
                checksum = digest
                tokens = report.days.reduce(0) { $0 + $1.counts.total }; rows = report.days.count
                precondition(tokens == 880_640 && report.scannedFiles == 25 && report.warnings.isEmpty)
            }
        }
        let result: [String: Any] = ["fixture_bytes": bytes, "files": 25, "usage_events": 8_448,
            "cold_ms": cold, "warm_ms": warm, "cold_median_ms": cold.sorted()[1], "warm_median_ms": warm.sorted()[1],
            "tokens": tokens, "daily_rows": rows, "report_sha256": checksum!]
        print(String(decoding: try JSONSerialization.data(withJSONObject: result, options: [.prettyPrinted, .sortedKeys]), as: UTF8.self))
    }
}
