import Foundation

/// Calendar-aligned data for the upstream-style, Monday-first activity cards.
/// Built once per report; hovering does not parse dates or aggregate logs again.
public struct UsageActivity: Equatable, Sendable {
    public static let weeks = 26
    public static let historyDays = weeks * 7

    public struct Day: Identifiable, Equatable, Sendable {
        public var id: String { key }
        public let key: String
        public let date: Date
        public let month: Int
        public let tokens: Int
        public let models: [DailyUsage]
        public let isFuture: Bool
        public let isToday: Bool
    }

    public let provider: ProviderKind
    public let days: [Day]
    public let thresholds: [Int]
    public let totalTokens: Int
    public let activeDays: Int
    public let peak: Day?

    public init(report: LocalUsageReport, provider: ProviderKind, now: Date = Date(), timeZone: TimeZone = .current) {
        self.provider = provider
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let today = calendar.startOfDay(for: now)
        let fromMonday = (calendar.component(.weekday, from: today) + 5) % 7
        let start = calendar.date(byAdding: .day, value: -fromMonday - (Self.weeks - 1) * 7, to: today)!
        let formatter = DateFormat.formatter("yyyy-MM-dd", timeZone: timeZone)
        let grouped = Dictionary(grouping: report.days.filter { $0.provider == provider }, by: \.day)
        days = (0..<Self.historyDays).map { offset in
            let date = calendar.startOfDay(for: calendar.date(byAdding: .day, value: offset, to: start)!)
            let key = formatter.string(from: date)
            var counts: [String: TokenCounts] = [:]
            if date <= today {
                for row in grouped[key] ?? [] where row.counts.total > 0 {
                    counts[row.model] = (counts[row.model] ?? TokenCounts()).adding(row.counts)
                }
            }
            let models = counts.map { DailyUsage(day: key, provider: provider, model: $0.key, counts: $0.value) }
                .sorted { $0.counts.total == $1.counts.total ? $0.model < $1.model : $0.counts.total > $1.counts.total }
            return Day(key: key, date: date, month: calendar.component(.month, from: date),
                tokens: models.reduce(0) { $0 + $1.counts.total }, models: models,
                isFuture: date > today, isToday: date == today)
        }
        let values = days.lazy.map(\.tokens).filter { $0 > 0 }.sorted()
        thresholds = values.isEmpty ? [] : [0.25, 0.50, 0.75].map { values[Int(Double(values.count - 1) * $0)] }
        totalTokens = values.reduce(0, +)
        activeDays = values.count
        peak = days.filter { $0.tokens > 0 }.max { a, b in
            a.tokens == b.tokens ? a.date > b.date : a.tokens < b.tokens
        }
    }

    public func intensity(_ tokens: Int) -> Int {
        guard tokens > 0 else { return 0 }
        guard thresholds.count == 3 else { return 1 }
        for (index, threshold) in thresholds.enumerated() where tokens <= threshold { return index + 1 }
        return 4
    }
}
