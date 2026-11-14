import CQuotaBar
import Foundation

/// Owns one numeric event cache for the lifetime of the dashboard.
public actor LocalUsageScanner {
    private let handle = qb_scanner_new()!

    public init() {}
    deinit { qb_scanner_free(handle) }

    public func releaseCache() { qb_scanner_release_cache(handle) }

    private final class Cancellation: @unchecked Sendable {
        // Rust's atomic cancellation flag is shared with Swift's cancellation handler.
        let handle = qb_cancel_new()!
        func cancel() { qb_cancel_set(handle) }
        deinit { qb_cancel_free(handle) }
    }

    private struct Request: Encodable {
        var codexRoots: [String]
        var claudeRoots: [String]
        var historyDays: Int
        var nowMicros: Int64
        var timeZone: String?
        var offsetSeconds: Int?
    }

    public func scan(codexRoots: [URL], claudeRoots: [URL], now: Date = Date(),
                     timeZone: TimeZone = .current, historyDays: Int = 90) async throws -> LocalUsageReport {
        let cancellation = Cancellation()
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            let fixed = timeZone.identifier == "GMT" || timeZone.identifier.hasPrefix("GMT+") || timeZone.identifier.hasPrefix("GMT-")
            let local = timeZone == .current
            let request = Request(codexRoots: codexRoots.map { $0.path }, claudeRoots: claudeRoots.map { $0.path },
                historyDays: min(366, max(1, historyDays)), nowMicros: Int64((now.timeIntervalSince1970 * 1_000_000).rounded()),
                timeZone: local || fixed ? nil : timeZone.identifier,
                offsetSeconds: !local && fixed ? timeZone.secondsFromGMT(for: now) : nil)
            let json = String(decoding: try JSONEncoder().encode(request), as: UTF8.self)
            var output: UnsafeMutablePointer<CChar>?
            let status = json.withCString { qb_scanner_scan(handle, cancellation.handle, $0, &output) }
            if status == 1 { throw CancellationError() }
            guard status == 0, let output else { throw CocoaError(.fileReadCorruptFile) }
            defer { qb_string_free(output) }
            try Task.checkCancellation()
            let decoder = JSONDecoder()
            // scannedAt is the exact Date supplied by the caller, including its original precision.
            decoder.dateDecodingStrategy = .custom { _ in now }
            return try decoder.decode(LocalUsageReport.self, from: Data(bytes: output, count: strlen(output)))
        } onCancel: { cancellation.cancel() }
    }
}
