import Testing
import Foundation
@testable import UsageCore

struct SessionProjectionTests {
    private func scan(_ lines: [String]) throws -> (rows: [Data], error: String?) {
        let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".jsonl")
        defer { try? FileManager.default.removeItem(at: file) }
        try Data((lines.joined(separator: "\n") + "\n").utf8).write(to: file)
        var rows: [Data] = []
        let result = try SessionProjectionReader.scan(path: file.path, cursor: nil) { rows.append($0) }
        return (rows, result.errorCode)
    }

    @Test func testNestedProjectionPreservesEscapedKeysAndScalarValues() throws {
        let input = #"{"ty\u0070e":"event_msg","timestamp":"2026-10-16T12:00:00Z","payload":{"ignored":[{"private":"secret"}],"type":"token_count","info":{"model":"quoted\"model\\name","total_token_usage":{"input_tokens":1e2,"cached_input_tokens":null,"output_tokens":20}}},"isSidechain":false}"#
        let result = try scan([input])
        #expect(result.error == nil)
        let row = try #require(result.rows.first)
        let object = try #require(JSONSerialization.jsonObject(with: row) as? [String: Any])
        #expect(object["type"] as? String == "event_msg")
        #expect(object["isSidechain"] as? Bool == false)
        let payload = try #require(object["payload"] as? [String: Any])
        #expect(payload["ignored"] == nil)
        let info = try #require(payload["info"] as? [String: Any])
        #expect(info["model"] as? String == "quoted\"model\\name")
        let counts = try #require(info["total_token_usage"] as? [String: Any])
        #expect(counts["input_tokens"] as? Int == 100)
        #expect(counts["cached_input_tokens"] is NSNull)
        #expect(counts["output_tokens"] as? Int == 20)
        #expect(!String(decoding: row, as: UTF8.self).contains("secret"))
    }

    @Test func testOversizedAndMalformedRowsDoNotLeakIntoNextProjection() throws {
        let valid = #"{"type":"assistant","message":{"id":"ok","model":"test","usage":{"input_tokens":3,"output_tokens":2}}}"#
        for invalid in [
            "{\"type\":\"" + String(repeating: "x", count: 16 * 1024) + "\"}",
            "{\"payload\":{\"info\":{\"total_token_usage\":{\"input_tokens\":" + String(repeating: "9", count: 129) + "}}}}",
            // Individually bounded strings can still exceed the per-row cap.
            "{" + Array(repeating: "\"type\":\"" + String(repeating: "x", count: 16_000) + "\"", count: 66).joined(separator: ",") + "}",
            #"{"type":"assistant","message":{"usage":{"input_tokens":}}}"#
        ] {
            let result = try scan([invalid, valid])
            #expect(result.error != nil)
            #expect(result.rows == [Data(valid.utf8)])
        }
    }

    @Test func testUnexpectedContainersRemainNullInScalarFields() throws {
        let result = try scan([#"{"type":["ignored"],"payload":{"id":{"private":true},"info":false},"message":[]}"#])
        #expect(result.error == nil)
        #expect(result.rows == [Data(#"{"type":null,"payload":{"id":null,"info":null},"message":null}"#.utf8)])
    }
}
