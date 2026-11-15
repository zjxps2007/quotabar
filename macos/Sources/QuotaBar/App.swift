import AppKit
import CQuotaBar
import UsageCore

/// Adapts the account/settings service to the resident Rust presentation controller.
@MainActor
final class MonitorDataSource {
    private var store: MonitorStore?

    func event(_ handle: OpaquePointer, _ event: QBMacStoreEvent, _ value: UInt32,
               _ bytes: UnsafePointer<UInt8>?, _ length: Int) -> Int32 {
        if event == QBStoreLaunch {
            let loginLaunch = LaunchAtLoginManager.isLoginLaunch(NSAppleEventManager.shared().currentAppleEvent)
            let service = MonitorStore()
            store = service
            service.onChange = { qb_mac_presentation_changed(handle) }
            return loginLaunch ? 1 : 0
        }
        guard let store else { return -1 }

        func send(_ event: DashboardEvent, to targets: UInt32) {
            guard let data = try? JSONEncoder().encode(event) else {
                qb_mac_presentation_send(handle, targets, nil, 0)
                return
            }
            data.withUnsafeBytes {
                qb_mac_presentation_send(handle, targets, $0.bindMemory(to: UInt8.self).baseAddress, $0.count)
            }
        }

        switch event {
        case QBStoreApplyDock: store.applyDockIconVisibility()
        case QBStoreTerminate:
            store.onChange = nil
            store.diagnostic("ui-terminated")
            self.store = nil
        case QBStoreRefresh: store.refreshAccounts()
        case QBStoreReclaim: store.reclaimIdleMemory()
        case QBStoreDiagnostic:
            guard let bytes else { return -1 }
            let name = String(decoding: UnsafeBufferPointer(start: bytes, count: length), as: UTF8.self)
            let shown = value & QBPopoverVisible.rawValue != 0
            store.diagnostic(name, details: ["dashboardRetained": value & QBDashboardRetained.rawValue != 0,
                                             "popoverRetained": shown, "popoverVisible": shown])
        case QBStoreDashboardStarted:
            store.setDashboardRunning(true)
            store.diagnostic("dashboard-process-started", details: ["pid": value])
        case QBStoreDashboardExited:
            if value != 0 { store.message = "Dashboard가 응답하지 않습니다. 다시 열어 주세요." }
            store.setVisible(false)
            store.setDashboardRunning(false)
            store.diagnostic("dashboard-process-exited")
        case QBStoreMissingHelper:
            store.message = "Dashboard 실행 파일을 찾지 못했습니다. 앱을 다시 설치해 주세요."
        case QBStoreLaunchFailed:
            store.setDashboardRunning(false)
            store.message = "Dashboard를 열지 못했습니다. 앱을 다시 실행해 주세요."
        case QBStoreSnapshot:
            var snapshot = store.dashboardState
            if value & QBDashboardSnapshot.rawValue == 0 { snapshot.demoReport = nil }
            send(.state(snapshot), to: value)
        case QBStoreCommand:
            guard let bytes, let command = try? JSONDecoder().decode(DashboardCommand.self, from: Data(bytes: bytes, count: length)) else { return -1 }
            return store.receive(command) { send($0, to: QBDashboardSnapshot.rawValue) }
        default: return -1
        }
        return 0
    }
}
