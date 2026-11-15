import Foundation
import UsageCore
import CQuotaBar

/// The native updater calls back on the AppKit thread to publish state and to
/// drain the account service before the installer takes ownership of the app.
@MainActor
final class AppUpdateManager {
    private let onState: (AppUpdateState) -> Void
    private let prepareExit: () async throws -> Void
    private let resume: () -> Void
    private let exit: () -> Void
    private var handle: OpaquePointer!

    init(onState: @escaping (AppUpdateState) -> Void, prepareExit: @escaping () async throws -> Void,
         resume: @escaping () -> Void, exit: @escaping () -> Void) {
        self.onState = onState
        self.prepareExit = prepareExit
        self.resume = resume
        self.exit = exit
        handle = qb_mac_updater_new(Unmanaged.passUnretained(self).toOpaque()) { context, event, bytes, length in
            guard let context else { return }
            MainActor.assumeIsolated {
                let manager = Unmanaged<AppUpdateManager>.fromOpaque(context).takeUnretainedValue()
                switch event {
                case QBMacUpdateState:
                    if let bytes, let state = try? JSONDecoder().decode(AppUpdateState.self, from: Data(bytes: bytes, count: length)) {
                        manager.onState(state)
                    }
                case QBMacUpdatePrepareExit:
                    Task { [weak manager] in
                        guard let manager else { return }
                        do { try await manager.prepareExit(); qb_mac_updater_drained(manager.handle, 1) }
                        catch { qb_mac_updater_drained(manager.handle, 0) }
                    }
                case QBMacUpdateResume: manager.resume()
                case QBMacUpdateExit: manager.exit()
                default: break
                }
            }
        }
    }

    deinit { qb_mac_updater_free(handle) }

    func configure(enabled: Bool, canInstall: Bool, suspended: Bool) {
        qb_mac_updater_configure(handle, enabled ? 1 : 0, canInstall ? 1 : 0, suspended ? 1 : 0)
    }

    func check() { qb_mac_updater_check(handle, 0) }
    func installNow() { qb_mac_updater_check(handle, 1) }
}
