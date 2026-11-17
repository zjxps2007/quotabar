import CQuotaBar

/// The worker and the Swift task cancellation handler share one Rust atomic flag.
final class NativeCancellation: @unchecked Sendable {
    let handle = qb_cancel_new()!
    func cancel() { qb_cancel_set(handle) }
    deinit { qb_cancel_free(handle) }
}
