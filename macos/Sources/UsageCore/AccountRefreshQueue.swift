import CQuotaBar
import Foundation

/// The monitor's main actor owns both the native queue and its Swift worker tasks.
@MainActor
package final class AccountRefreshQueue {
    private let handle = qb_mac_refresh_new()!
    package init() {}
    deinit { qb_mac_refresh_free(handle) }

    package var status: QBMacRefreshStatus { qb_mac_refresh_status(handle, nil, nil) }
    package var loading: Set<UUID> {
        var ids: Set<UUID> = []
        withUnsafeMutablePointer(to: &ids) { result in
            _ = qb_mac_refresh_status(handle, result) { context, id in
                context!.assumingMemoryBound(to: Set<UUID>.self).pointee.insert(UUID(uuid: id.bytes))
            }
        }
        return ids
    }

    package func enqueue(_ id: UUID) { qb_mac_refresh_enqueue(handle, QBRefreshAccount(bytes: id.uuid)) }
    package func cancel(_ id: UUID) { qb_mac_refresh_cancel(handle, QBRefreshAccount(bytes: id.uuid)) }
    package func setSleeping(_ value: Bool) { qb_mac_refresh_sleeping(handle, value ? 1 : 0) }
    package func setInstalling(_ value: Bool) { qb_mac_refresh_installing(handle, value ? 1 : 0) }

    package func next() -> QBRefreshJob? {
        let job = qb_mac_refresh_next(handle)
        return job.generation == 0 ? nil : job
    }
    package func finish(_ job: QBRefreshJob) -> Bool { qb_mac_refresh_finish(handle, job) != 0 }
}
