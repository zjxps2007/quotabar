import Foundation
import CQuotaBar

/// Deadlines, rather than a polling tick, keep a menu-bar-only app asleep
/// between quota requests. No deadline means no monitoring timer is needed.
public struct RefreshSchedule: Equatable, Sendable {
    public static let intervals: [Int] = {
        var values = [Int]()
        while case let value = qb_refresh_interval(values.count), value > 0 { values.append(Int(value)) }
        return values
    }()
    public static let localInterval = TimeInterval(qb_local_interval())
    public let accounts: Date?
    public let localLogs: Date?
    public var next: Date? {
        switch (accounts, localLogs) {
        case let (a?, b?): return min(a, b)
        case let (a?, nil): return a
        case let (nil, b?): return b
        case (nil, nil): return nil
        }
    }

    public init(suspended: Bool, hasAccounts: Bool, windowVisible: Bool,
                dashboardSelected: Bool, scanLocalLogs: Bool, scanning: Bool,
                refreshSeconds: Int, lastAccountRefresh: Date, lastLocalAttempt: Date?, autoRefresh: Bool = true) {
        let delays = qb_refresh_delays(suspended ? 1 : 0, autoRefresh ? 1 : 0, hasAccounts ? 1 : 0,
                                      windowVisible && dashboardSelected ? 1 : 0, scanLocalLogs ? 1 : 0,
                                      scanning ? 1 : 0, Int32(clamping: refreshSeconds))
        accounts = delays.accounts < 0 ? nil : lastAccountRefresh.addingTimeInterval(Double(refreshSeconds))
        localLogs = delays.local < 0 ? nil : (lastLocalAttempt?.addingTimeInterval(Double(delays.local)) ?? .distantPast)
    }
}
