import AppKit
import Combine
import UsageCore

/// The persistent monitor owns every account, credential and quota request.
/// Closing the Dashboard exits its UI process and releases SwiftUI's caches.
@MainActor
final class DashboardHost {
    private let store: MonitorStore
    private var process: Process?
    private var channel: DashboardPipe<DashboardCommand, DashboardEvent>?
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
            if ready { channel?.send(.show(route: requestedRoute)) }
            return
        }
        let helperURL = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/QuotaBar Dashboard.app")
        guard let executable = Bundle(url: helperURL)?.executableURL else {
            store.message = "Dashboard 실행 파일을 찾지 못했습니다. 앱을 다시 설치해 주세요."
            return
        }
        let child = Process(), toChild = Pipe(), fromChild = Pipe()
        child.executableURL = executable
        child.arguments = ["--dashboard-helper"] + (store.demo ? ["--demo"] : [])
        child.standardInput = toChild
        child.standardOutput = fromChild
        child.standardError = FileHandle.nullDevice
        let pipe = DashboardPipe<DashboardCommand, DashboardEvent>(input: fromChild.fileHandleForReading,
                                                                   output: toChild.fileHandleForWriting)
        let id = UUID(); generation = id; ready = false
        child.terminationHandler = { [weak self] _ in
            Task { @MainActor in self?.didExit(generation: id) }
        }
        process = child; channel = pipe
        do { try child.run() }
        catch {
            process = nil; channel = nil; pipe.close()
            store.message = "Dashboard를 열지 못했습니다. 앱을 다시 실행해 주세요."
            return
        }
        // The parent must not keep the child's pipe ends open, or EOF never arrives.
        try? toChild.fileHandleForReading.close()
        try? fromChild.fileHandleForWriting.close()
        pipe.start(onMessage: { [weak self] command in
            DispatchQueue.main.async {
                guard let self, self.generation == id else { return }
                self.receive(command)
            }
        }, onClose: { [weak self] in
            DispatchQueue.main.async {
                guard let self, self.generation == id else { return }
                self.close()
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
        channel?.send(.close)
        // A normal termination runs CodexLoginSession's cleanup before exit.
        // If the UI cannot respond, ensure it cannot remain as an idle helper.
        guard let process else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { [weak process] in
            if process?.isRunning == true { process?.terminate() }
        }
    }

    func stop() {
        startupDeadline?.cancel(); startupDeadline = nil
        observation = nil
        channel?.close(); channel = nil
    }

    private func didExit(generation id: UUID) {
        guard generation == id else { return }
        generation = UUID()
        startupDeadline?.cancel(); startupDeadline = nil
        observation = nil; channel?.close(); channel = nil; process = nil
        ready = false; updatePending = false
        store.setVisible(false)
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

    private func sendState() { channel?.send(.state(store.dashboardState)) }

    private func receive(_ command: DashboardCommand) {
        func account(_ id: UUID) -> MonitoredAccount? { store.accounts.first { $0.id == id } }
        func reply(_ id: UUID, operation: () throws -> Void) {
            do {
                try operation()
                sendState()
                channel?.send(.reply(id: id, error: nil))
            } catch {
                channel?.send(.reply(id: id, error: (error as? ProviderError)?.message ?? error.localizedDescription))
            }
        }
        switch command {
        case .ready:
            startupDeadline?.cancel(); startupDeadline = nil
            ready = true; sendState(); channel?.send(.show(route: requestedRoute))
        case .visibility(let visible): store.setVisible(visible)
        case .route(let route):
            if ["dashboard", "accounts", "settings"].contains(route) { store.route = route }
        case .refreshAll: store.refreshAll()
        case .refreshAccounts: store.refreshAccounts()
        case .refresh(let id): if let value = account(id) { store.refresh(value) }
        case .toggle(let id): if let value = account(id) { store.toggle(value) }
        case .remove(let id): if let value = account(id) { store.remove(value) }
        case .interval(let value): if [60, 300, 900, 1800].contains(value) { store.setInterval(value) }
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

@MainActor
final class DashboardClient {
    private let channel = DashboardPipe<DashboardEvent, DashboardCommand>(input: .standardInput, output: .standardOutput)
    private var pending: [UUID: CheckedContinuation<Void, Error>] = [:]
    private var timeouts: [UUID: Task<Void, Never>] = [:]
    private var connected = true
    var onState: ((DashboardState) -> Void)?
    var onShow: ((String?) -> Void)?

    func start() {
        channel.start(onMessage: { [weak self] event in
            // Preserve wire order: settings must arrive before a window opens.
            DispatchQueue.main.async { self?.receive(event) }
        }, onClose: { [weak self] in
            DispatchQueue.main.async {
                self?.disconnect()
                NSApp.terminate(nil)
            }
        })
        send(.ready)
    }

    func send(_ command: DashboardCommand) { if connected { channel.send(command) } }

    func request(_ command: (UUID) -> DashboardCommand) async throws {
        try Task.checkCancellation()
        guard connected else { throw PipeError.disconnected }
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                pending[id] = continuation
                timeouts[id] = Task { [weak self] in
                    do { try await Task.sleep(for: .seconds(30)) } catch { return }
                    self?.complete(id, error: ProviderError("dashboard_timeout", "연결 결과를 받지 못했습니다. 계정 목록을 확인한 뒤 다시 시도하세요."))
                }
                send(command(id))
            }
        } onCancel: {
            Task { @MainActor [weak self] in self?.complete(id, error: CancellationError()) }
        }
    }

    func disconnect() {
        connected = false
        for id in Array(pending.keys) { complete(id, error: PipeError.disconnected) }
        channel.close()
    }

    private func complete(_ id: UUID, error: Error?) {
        timeouts.removeValue(forKey: id)?.cancel()
        guard let continuation = pending.removeValue(forKey: id) else { return }
        if let error { continuation.resume(throwing: error) } else { continuation.resume() }
    }

    private func receive(_ event: DashboardEvent) {
        switch event {
        case .state(let state): onState?(state)
        case .show(let route): onShow?(route)
        case .reply(let id, let error):
            complete(id, error: error.map { ProviderError("connection", $0) })
        case .close: NSApp.terminate(nil)
        }
    }
}
