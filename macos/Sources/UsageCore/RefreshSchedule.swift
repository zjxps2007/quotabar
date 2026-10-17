import Foundation

/// Deadlines, rather than a polling tick, keep a menu-bar-only app asleep
/// between quota requests. No deadline means no monitoring timer is needed.
public struct RefreshSchedule: Equatable, Sendable {
    public static let localInterval: TimeInterval = 300
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
                refreshSeconds: Int, lastAccountRefresh: Date, lastLocalAttempt: Date?) {
        accounts = !suspended && hasAccounts
            ? lastAccountRefresh.addingTimeInterval(Double(windowVisible ? refreshSeconds : max(900, refreshSeconds)))
            : nil
        localLogs = !suspended && windowVisible && dashboardSelected && scanLocalLogs && !scanning
            ? (lastLocalAttempt?.addingTimeInterval(Self.localInterval) ?? .distantPast)
            : nil
    }
}
