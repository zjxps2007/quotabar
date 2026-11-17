import Foundation
import Testing
@testable import UsageCore

struct RefreshScheduleTests {
    private let anchor = Date(timeIntervalSince1970: 1_700_000_000)

    @Test @MainActor func monitorWorkersKeepAccountIdentityUntilTheyFinish() throws {
        let queue = AccountRefreshQueue()
        let ids = (0..<5).map { _ in UUID() }
        for id in ids { queue.enqueue(id) }
        queue.enqueue(ids[0])
        let first = try #require(queue.next())
        #expect(UUID(uuid: first.account.bytes) == ids[0])
        _ = try #require(queue.next())
        _ = try #require(queue.next())
        #expect(queue.next() == nil)
        #expect(queue.status.queued == 2 && queue.status.running == 3)
        #expect(queue.loading == Set(ids))
        queue.cancel(ids[0])
        queue.enqueue(ids[0])
        let next = try #require(queue.next())
        #expect(UUID(uuid: next.account.bytes) == ids[3])
        #expect(!queue.finish(first))
        queue.setInstalling(true)
        #expect(queue.loading == Set([ids[1], ids[2], ids[3]]))
        #expect(queue.finish(next))
        queue.setSleeping(true)
        queue.setInstalling(false)
        queue.enqueue(ids[0])
        #expect(queue.loading.isEmpty)
        queue.setSleeping(false)
        queue.enqueue(ids[0])
        let replacement = try #require(queue.next())
        #expect(UUID(uuid: replacement.account.bytes) == ids[0] && replacement.generation != first.generation)
        #expect(!queue.finish(first))
        #expect(queue.finish(replacement) && queue.loading.isEmpty)
    }

    private func schedule(suspended: Bool = false, accounts: Bool = true, visible: Bool = true,
                          dashboard: Bool = true, logs: Bool = true, scanning: Bool = false,
                          interval: Int = 300, localAttempt: Date? = nil, automatic: Bool = true) -> RefreshSchedule {
        RefreshSchedule(suspended: suspended, hasAccounts: accounts, windowVisible: visible,
            dashboardSelected: dashboard, scanLocalLogs: logs, scanning: scanning,
            refreshSeconds: interval, lastAccountRefresh: anchor, lastLocalAttempt: localAttempt, autoRefresh: automatic)
    }

    @Test func menuBarOnlyHonorsSelectedIntervalWithoutScanning() {
        for interval in RefreshSchedule.intervals {
            let result = schedule(visible: false, interval: interval)
            #expect(result.accounts == anchor.addingTimeInterval(Double(interval)))
            #expect(result.localLogs == nil)
            #expect(result.next == result.accounts)
        }
        #expect(schedule(visible: false, interval: 1800).next == anchor.addingTimeInterval(1800))
    }

    @Test func reopeningAndIntervalChangesRecomputeFromLastRefresh() {
        #expect(schedule(visible: false, interval: 60).next == anchor.addingTimeInterval(60))
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

    @Test func automaticRefreshCanBeDisabledAndReenabled() {
        for visible in [true, false] {
            let stopped = schedule(visible: visible, automatic: false)
            #expect(stopped.accounts == nil && stopped.localLogs == nil && stopped.next == nil)
            #expect(schedule(visible: visible, logs: false, interval: 3600).next == anchor.addingTimeInterval(3600))
        }
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
