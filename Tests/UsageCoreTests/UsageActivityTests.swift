import Foundation
import Testing
@testable import UsageCore

struct UsageActivityTests {
    private let utc = TimeZone(secondsFromGMT: 0)!
    private func date(_ value: String) -> Date { SharedFormatters.parseISO8601(value)! }

    @Test func weeksStartOnMondayAndKeepTodayAndFutureSeparate() {
        let monday = UsageActivity(report: LocalUsageReport(), provider: .codex,
            now: date("2026-10-19T12:00:00Z"), timeZone: utc)
        #expect(monday.days.count == 182)
        #expect(monday.days[175].key == "2026-10-19")
        #expect(monday.days.filter(\.isToday).count == 1)
        #expect(monday.days.filter(\.isFuture).count == 6)
        let sunday = UsageActivity(report: LocalUsageReport(), provider: .codex,
            now: date("2026-10-18T12:00:00Z"), timeZone: utc)
        #expect(sunday.days.first?.key == "2026-04-20")
        #expect(sunday.days.last?.isToday == true)
        #expect(sunday.days.allSatisfy { !$0.isFuture })
    }

    @Test func footerAndTooltipUseOnlyTheVisibleProviderAndPeriod() {
        var report = LocalUsageReport()
        report.days = [
            DailyUsage(day: "2026-10-17", provider: .codex, model: "a", counts: TokenCounts(input: 80, cached: 50, output: 20)),
            DailyUsage(day: "2026-10-17", provider: .codex, model: "a", counts: TokenCounts(input: 40, output: 10)),
            DailyUsage(day: "2026-10-17", provider: .codex, model: "b", counts: TokenCounts(input: 50, output: 50)),
            DailyUsage(day: "2026-10-16", provider: .codex, model: "a", counts: TokenCounts(input: 200)),
            DailyUsage(day: "2026-10-17", provider: .claude, model: "other", counts: TokenCounts(input: 999)),
            DailyUsage(day: "2026-10-18", provider: .codex, model: "future", counts: TokenCounts(input: 999)),
            DailyUsage(day: "2026-04-19", provider: .codex, model: "old", counts: TokenCounts(input: 999))
        ]
        let result = UsageActivity(report: report, provider: .codex, now: date("2026-10-17T12:00:00Z"), timeZone: utc)
        #expect(result.totalTokens == 450)
        #expect(result.activeDays == 2)
        #expect(result.peak?.key == "2026-10-17")
        #expect(result.peak?.tokens == 250)
        #expect(result.peak?.models.map(\.model) == ["a", "b"])
        #expect(result.peak?.models.first?.counts.cached == 50)
        #expect(result.days.last?.tokens == 0)
    }

    @Test func quartilesKeepModerateActivityVisibleWithAnOutlier() {
        var report = LocalUsageReport()
        report.days = [10, 20, 30, 1_000_000].enumerated().map { index, count in
            DailyUsage(day: "2026-10-\(10 + index)", provider: .codex, model: "a", counts: TokenCounts(input: count))
        }
        let result = UsageActivity(report: report, provider: .codex, now: date("2026-10-17T12:00:00Z"), timeZone: utc)
        #expect(result.thresholds == [10, 20, 30])
        #expect([0, 10, 20, 30, 1_000_000].map(result.intensity) == [0, 1, 2, 3, 4])
        let empty = UsageActivity(report: LocalUsageReport(), provider: .claude)
        #expect(empty.totalTokens == 0 && empty.activeDays == 0 && empty.peak == nil)
        #expect(empty.intensity(0) == 0)
    }

    @Test func calendarCellsStayDistinctAcrossDSTAndYearBoundary() {
        for (zone, now) in [("America/Havana", "2026-06-05T12:00:00Z"),
                            ("America/New_York", "2026-11-02T12:00:00Z"),
                            ("Asia/Kathmandu", "2027-01-01T20:00:00Z")] {
            let timeZone = TimeZone(identifier: zone)!
            let result = UsageActivity(report: LocalUsageReport(), provider: .codex, now: date(now), timeZone: timeZone)
            var calendar = Calendar(identifier: .gregorian); calendar.timeZone = timeZone
            #expect(Set(result.days.map(\.key)).count == 182)
            #expect(calendar.component(.weekday, from: result.days[0].date) == 2)
            #expect(result.days.filter(\.isToday).count == 1)
            #expect(result.days.last!.date > result.days.first!.date)
        }
    }
}
