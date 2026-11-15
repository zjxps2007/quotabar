import AppKit
import Combine
import UsageCore

/// The persistent monitor owns every account, credential and quota request.
/// Closing the Rust dashboard releases its rendering and local scan caches.
@MainActor
final class DashboardHost {
    private let store: MonitorStore
    private var process: NativeDashboardChild?
    private var observation: AnyCancellable?
    private var generation = UUID()
    private var ready = false
    private var updatePending = false
    private var requestedRoute: String?
    private var startupDeadline: Task<Void, Never>?
    var onPopover: (() -> Void)?
    var isRunning: Bool { process?.isRunning == true }

    init(store: MonitorStore) { self.store = store }

    func show(route: String?) {
        requestedRoute = route ?? store.route
        if isRunning {
            if ready { process?.send(.show(route: requestedRoute)) }
            return
        }
        let helperURL = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/QuotaBar Dashboard.app")
        guard let executable = Bundle(url: helperURL)?.executableURL else {
            store.message = "Dashboard 실행 파일을 찾지 못했습니다. 앱을 다시 설치해 주세요."
            return
        }
        let child: NativeDashboardChild
        do { child = try NativeDashboardChild(executable: executable, demo: store.demo) }
        catch {
            store.setDashboardRunning(false)
            store.message = "Dashboard를 열지 못했습니다. 앱을 다시 실행해 주세요."
            return
        }
        let id = UUID(); generation = id; ready = false
        process = child
        store.setDashboardRunning(true)
        child.start(onMessage: { [weak self] command in
            DispatchQueue.main.async {
                guard let self, self.generation == id else { return }
                self.receive(command)
            }
        }, onExit: { [weak self] timedOut in
            DispatchQueue.main.async {
                guard let self, self.generation == id else { return }
                if timedOut { self.store.message = "Dashboard가 응답하지 않습니다. 다시 열어 주세요." }
                self.didExit(generation: id)
            }
        })
        observation = store.objectWillChange.sink { [weak self] _ in self?.scheduleState() }
        startupDeadline = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(10)) } catch { return }
            guard let self, self.generation == id, !self.ready else { return }
            self.store.message = "Dashboard가 응답하지 않습니다. 다시 열어 주세요."
            self.close()
        }
        store.diagnostic("dashboard-process-started", details: ["pid": child.processIdentifier])
    }

    func close() {
        process?.close()
    }

    func stop() {
        startupDeadline?.cancel(); startupDeadline = nil
        observation = nil
        process?.close()
    }

    private func didExit(generation id: UUID) {
        guard generation == id else { return }
        generation = UUID()
        startupDeadline?.cancel(); startupDeadline = nil
        observation = nil; process = nil
        ready = false; updatePending = false
        store.setVisible(false)
        store.setDashboardRunning(false)
        store.diagnostic("dashboard-process-exited")
    }

    private func scheduleState() {
        guard ready, !updatePending else { return }
        updatePending = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.updatePending = false
            if self.ready { self.sendState() }
        }
    }

    private func sendState() { process?.send(.state(store.dashboardState)) }

    private func receive(_ command: DashboardCommand) {
        func account(_ id: UUID) -> MonitoredAccount? { store.accounts.first { $0.id == id } }
        func reply(_ id: UUID, operation: () throws -> Void) {
            do {
                try operation()
                sendState()
                process?.send(.reply(id: id, error: nil))
            } catch {
                process?.send(.reply(id: id, error: (error as? ProviderError)?.message ?? error.localizedDescription))
            }
        }
        switch command {
        case .ready:
            startupDeadline?.cancel(); startupDeadline = nil
            process?.ready()
            ready = true; sendState(); process?.send(.show(route: requestedRoute))
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
    }
}
