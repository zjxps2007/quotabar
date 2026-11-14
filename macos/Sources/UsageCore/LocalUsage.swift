import Foundation

public struct TokenCounts: Codable, Hashable, Sendable {
    public var input = 0
    public var cached = 0
    public var output = 0
    public var total: Int { input + output }
    public init(input: Int = 0, cached: Int = 0, output: Int = 0) {
        self.input = input; self.cached = cached; self.output = output
    }
    func adding(_ other: Self) -> Self {
        Self(input: input + other.input, cached: cached + other.cached, output: output + other.output)
    }
}

public struct DailyUsage: Identifiable, Codable, Equatable, Sendable {
    public var id: String { "\(provider.rawValue):\(day):\(model)" }
    public let day: String
    public let provider: ProviderKind
    public let model: String
    public var counts: TokenCounts
}

public struct LocalUsageReport: Codable, Equatable, Sendable {
    public var days: [DailyUsage] = []
    public var scannedFiles = 0
    public var warnings: [String] = []
    public var scannedAt: Date?
    public init() {}
    public func tokens(on day: String) -> Int { days.lazy.filter { $0.day == day }.reduce(0) { $0 + $1.counts.total } }
    public func tokens(inMonth month: String) -> Int { days.lazy.filter { $0.day.hasPrefix(month) }.reduce(0) { $0 + $1.counts.total } }
}
