import Foundation
import Testing
@testable import UsageCore

struct RefreshScheduleTests {
    private let anchor = Date(timeIntervalSince1970: 1_700_000_000)

    private func schedule(suspended: Bool = false, accounts: Bool = true, visible: Bool = true,
                          dashboard: Bool = true, logs: Bool = true, scanning: Bool = false,
                          interval: Int = 300, localAttempt: Date? = nil) -> RefreshSchedule {
        RefreshSchedule(suspended: suspended, hasAccounts: accounts, windowVisible: visible,
            dashboardSelected: dashboard, scanLocalLogs: logs, scanning: scanning,
            refreshSeconds: interval, lastAccountRefresh: anchor, lastLocalAttempt: localAttempt)
    }

    @Test func menuBarOnlyKeepsBackgroundQuotaIntervalWithoutScanning() {
        for interval in [60, 300, 900] {
            let result = schedule(visible: false, interval: interval)
            #expect(result.accounts == anchor.addingTimeInterval(900))
            #expect(result.localLogs == nil)
            #expect(result.next == result.accounts)
        }
        #expect(schedule(visible: false, interval: 1800).next == anchor.addingTimeInterval(1800))
    }

    @Test func reopeningAndIntervalChangesRecomputeFromLastRefresh() {
        #expect(schedule(visible: false, interval: 60).next == anchor.addingTimeInterval(900))
        #expect(schedule(logs: false, interval: 60).next == anchor.addingTimeInterval(60))
        #expect(schedule(logs: false, interval: 1800).next == anchor.addingTimeInterval(1800))
    }

    @Test func dashboardScanAndQuotaUseIndependentDeadlines() {
        let result = schedule(interval: 900, localAttempt: anchor.addingTimeInterval(100))
        #expect(result.accounts == anchor.addingTimeInterval(900))
        #expect(result.localLogs == anchor.addingTimeInterval(400))
        #expect(result.next == result.localLogs)
        // Returning from another tab preserves the last scan's deadline.
        #expect(schedule(dashboard: false, interval: 900, localAttempt: anchor).localLogs == nil)
        #expect(schedule(interval: 900, localAttempt: anchor).next == anchor.addingTimeInterval(300))
    }

    @Test func nothingToRefreshMeansNoTimer() {
        #expect(schedule(accounts: false, visible: false).next == nil)
        #expect(schedule(accounts: false, dashboard: false).next == nil)
        #expect(schedule(accounts: false, logs: false).next == nil)
        let sleeping = schedule(suspended: true)
        #expect(sleeping.accounts == nil)
        #expect(sleeping.localLogs == nil)
        #expect(sleeping.next == nil)
    }

    @Test func localOnlyDashboardStillRefreshes() {
        #expect(schedule(accounts: false).next == .distantPast)
        #expect(schedule(accounts: false, localAttempt: anchor).next == anchor.addingTimeInterval(300))
    }

    @Test func runningAndFailedScansCannotCauseImmediateRetryLoop() {
        #expect(schedule(accounts: false, scanning: true).next == nil)
        #expect(schedule(scanning: true).next == anchor.addingTimeInterval(300))
        // A failed attempt is an anchor even when no successful report exists.
        let failedAt = anchor.addingTimeInterval(700)
        #expect(schedule(accounts: false, localAttempt: failedAt).next == failedAt.addingTimeInterval(300))
    }
}
