import AppKit
import Combine
import UsageCore
import CQuotaBar

/// Provides the monitor store's typed commands and snapshots to the Rust dashboard host.
@MainActor
final class DashboardHost {
    private let store: MonitorStore
    private var handle: OpaquePointer?
    private var observation: AnyCancellable?
    var onPopover: (() -> Void)?
    var isRunning: Bool { qb_mac_dashboard_host_running(handle) != 0 }

    init(store: MonitorStore) {
        self.store = store
        handle = qb_mac_dashboard_host_new(Unmanaged.passUnretained(self).toOpaque(), { context, event, value, bytes, length in
            guard let context else { return -1 }
            return MainActor.assumeIsolated {
                Unmanaged<DashboardHost>.fromOpaque(context).takeUnretainedValue().event(event, value, bytes, length)
            }
        }, store.demo ? 1 : 0)
    }
    deinit { qb_mac_dashboard_host_free(handle) }

    func show(route: String?) {
        (route ?? store.route).withCString { qb_mac_dashboard_host_show(handle, $0) }
    }
    func close() { qb_mac_dashboard_host_close(handle) }
    func stop() {
        observation = nil
        qb_mac_dashboard_host_free(handle)
        handle = nil
    }

    private func event(_ event: QBMacHostEvent, _ value: UInt32, _ bytes: UnsafePointer<UInt8>?, _ length: Int) -> Int32 {
        switch event {
        case QBHostStarted:
            store.setDashboardRunning(true)
            observation = store.objectWillChange.sink { [weak self] _ in qb_mac_dashboard_host_changed(self?.handle) }
            store.diagnostic("dashboard-process-started", details: ["pid": value])
        case QBHostExited:
            observation = nil
            if value != 0 { store.message = "Dashboard가 응답하지 않습니다. 다시 열어 주세요." }
            store.setVisible(false)
            store.setDashboardRunning(false)
            store.diagnostic("dashboard-process-exited")
        case QBHostMissingHelper:
            store.message = "Dashboard 실행 파일을 찾지 못했습니다. 앱을 다시 설치해 주세요."
        case QBHostLaunchFailed:
            store.setDashboardRunning(false)
            store.message = "Dashboard를 열지 못했습니다. 앱을 다시 실행해 주세요."
        case QBHostPublishState:
            send(.state(store.dashboardState))
        case QBHostCommand:
            guard let bytes, let command = try? JSONDecoder().decode(DashboardCommand.self, from: Data(bytes: bytes, count: length)) else { return -1 }
            return receive(command)
        default: return -1
        }
        return 0
    }

    private func send(_ event: DashboardEvent) {
        guard let data = try? JSONEncoder().encode(event) else { close(); return }
        data.withUnsafeBytes { qb_mac_dashboard_host_send(handle, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }
    }

    private func receive(_ command: DashboardCommand) -> Int32 {
        func account(_ id: UUID) -> MonitoredAccount? { store.accounts.first { $0.id == id } }
        func reply(_ id: UUID, operation: () throws -> Void) {
            do {
                try operation()
                send(.state(store.dashboardState))
                send(.reply(id: id, error: nil))
            } catch {
                send(.reply(id: id, error: (error as? ProviderError)?.message ?? error.localizedDescription))
            }
        }
        switch command {
        case .ready: return 1
        case .visibility(let visible): store.setVisible(visible)
        case .route(let route):
            if ["dashboard", "accounts", "settings"].contains(route) { store.route = route }
        case .refreshAll, .refreshAccounts: store.refreshAccounts()
        case .refresh(let id): if let value = account(id) { store.refresh(value) }
        case .toggle(let id): if let value = account(id) { store.toggle(value) }
        case .remove(let id): if let value = account(id) { store.remove(value) }
        case .interval(let value): store.setInterval(value)
        case .autoRefresh(let value): store.setAutoRefresh(value)
        case .automaticUpdates(let value): store.setAutomaticUpdates(value)
        case .checkAppUpdate: store.checkAppUpdate()
        case .installAppUpdate: store.installAppUpdate()
        case .launchAtLogin(let value): store.setLaunchAtLogin(value)
        case .refreshLaunchAtLogin: store.refreshLaunchAtLogin()
        case .openLoginItemsSettings: store.openLoginItemsSettings()
        case .appearance(let value): store.setAppearance(value)
        case .dockIcon(let value): store.setDockIconVisible(value)
        case .menuBarMode(let value): store.setMenuBarDisplayMode(value)
        case .scanLogs(let value): store.setScanLogs(value)
        case .menuBarVisibility(let id, let visible):
            if let value = account(id) { store.setMenuBarVisibility(visible, for: value) }
        case .message(let value): store.message = value
        case .connect(let id, let provider, let path, let alias):
            reply(id) { try store.connect(provider: provider, path: path, alias: alias) }
        case .codexLogin(let id, let data, let alias, let replacing):
            reply(id) {
                if let replacing, account(replacing) == nil {
                    throw ProviderError("account_removed", "삭제된 계정입니다. 새 계정으로 연결하세요.")
                }
                guard data.count <= 1_048_576 else { throw ProviderError("large_auth", "로그인 정보가 너무 큽니다.") }
                try store.connectCodexLogin(data, alias: alias, replacing: replacing.flatMap(account))
            }
        case .quit: NSApp.terminate(nil)
        case .showPopover: onPopover?()
        }
        return 0
    }
}
