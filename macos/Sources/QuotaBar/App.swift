import AppKit
import Combine
import UsageCore
import CQuotaBar

@MainActor
final class MonitorApplication {
    private var store: MonitorStore!
    private var dashboard: DashboardHost!
    private var status: OpaquePointer?
    private var observation: AnyCancellable?
    private var publishing = false

    func launch() {
        let loginLaunch = LaunchAtLoginManager.isLoginLaunch(NSAppleEventManager.shared().currentAppleEvent)
        store = MonitorStore()
        dashboard = DashboardHost(store: store)
        dashboard.onPopover = { [weak self] in qb_mac_status_show_popover(self?.status) }
        status = qb_mac_monitor_status_new(Unmanaged.passUnretained(self).toOpaque()) { context, action in
            guard let context else { return }
            MainActor.assumeIsolated { Unmanaged<MonitorApplication>.fromOpaque(context).takeUnretainedValue().action(action) }
        }
        store.applyDockIconVisibility()
        publish()
        observation = store.objectWillChange.sink { [weak self] _ in
            guard let self, !self.publishing else { return }
            self.publishing = true
            DispatchQueue.main.async { [weak self] in self?.publishing = false; self?.publish() }
        }
        if !loginLaunch && !CommandLine.arguments.contains("--menu-bar-only") { dashboard.show(route: nil) }
        record("ui-launched")
        if CommandLine.arguments.contains("--memory-probe"), CommandLine.arguments.contains("--diagnostics") { probe() }
    }

    private struct StatusAccount: Encodable {
        let provider: String
        let name: String
        let windows: [QuotaWindow]
        let visible, enabled, loading, error: Bool
        let state: String?
    }
    private struct StatusInput: Encodable {
        let appearance, mode: String
        let demo: Bool
        let accounts: [StatusAccount]
    }
    private func publish() {
        guard let status else { return }
        let accounts = store.accounts.map { account in
            let snapshot = store.snapshots[account.id]
            return StatusAccount(provider: account.provider.title, name: account.name, windows: snapshot?.windows ?? [],
                                 visible: account.isShownInMenuBar, enabled: account.enabled, loading: store.loading.contains(account.id),
                                 error: store.errors[account.id] != nil, state: snapshot?.state)
        }
        let source = StatusInput(appearance: store.settings.selectedAppearance.rawValue, mode: store.settings.selectedMenuBarDisplayMode.rawValue,
                                 demo: store.demo, accounts: accounts)
        guard let data = try? JSONEncoder().encode(source) else { return }
        _ = data.withUnsafeBytes { qb_mac_status_publish(status, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }
    }
    private func action(_ action: QBMacStatusEvent) {
        switch action {
        case QBMacStatusOpened: record("popover-opened")
        case QBMacStatusRefresh: store.refreshAccounts()
        case QBMacStatusDashboard: dashboard.show(route: "dashboard")
        case QBMacStatusSettings: dashboard.show(route: "settings")
        case QBMacStatusClosed:
            store.reclaimIdleMemory()
            record("popover-closed")
        default: break
        }
    }
    func reopen() { dashboard.show(route: nil) }
    func terminate() {
        observation?.cancel()
        dashboard?.stop()
        if let status { qb_mac_status_free(status); self.status = nil }
        store?.diagnostic("ui-terminated")
    }
    private func record(_ event: String) {
        let visible = status.map { qb_mac_status_control($0, 3) != 0 } ?? false
        store.diagnostic(event, details: ["dashboardRetained": dashboard.isRunning, "popoverRetained": visible, "popoverVisible": visible])
    }
    private func probe() {
        Task { @MainActor in
            for _ in 0..<2 {
                if !CommandLine.arguments.contains("--menu-bar-only") {
                    for route in ["dashboard", "accounts", "settings"] {
                        dashboard.show(route: route)
                        try? await Task.sleep(for: .seconds(route == "dashboard" ? 5 : 2))
                    }
                    dashboard.close()
                    try? await Task.sleep(for: .seconds(2))
                }
                qb_mac_status_show_popover(status)
                try? await Task.sleep(for: .seconds(2))
                if let status { _ = qb_mac_status_control(status, 1) }
                try? await Task.sleep(for: .seconds(1))
            }
            record("memory-probe-idle")
        }
    }
}
