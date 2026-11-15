import Foundation
import UsageCore

extension MonitorStore {
    func receive(_ command: DashboardCommand, send: (DashboardEvent) -> Void) -> Int32 {
        func account(_ id: UUID) -> MonitoredAccount? { self.accounts.first { $0.id == id } }
        func reply(_ id: UUID, operation: () throws -> Void) {
            do {
                try operation()
                send(.state(self.dashboardState))
                send(.reply(id: id, error: nil))
            } catch {
                send(.reply(id: id, error: (error as? ProviderError)?.message ?? error.localizedDescription))
            }
        }
        switch command {
        case .ready, .route, .quit, .showPopover: return -1 // Handled by the Rust presentation controller.
        case .visibility(let visible): self.setVisible(visible)
        case .refreshAll, .refreshAccounts: self.refreshAccounts()
        case .refresh(let id): if let value = account(id) { self.refresh(value) }
        case .toggle(let id): if let value = account(id) { self.toggle(value) }
        case .remove(let id): if let value = account(id) { self.remove(value) }
        case .interval(let value): self.setInterval(value)
        case .autoRefresh(let value): self.setAutoRefresh(value)
        case .automaticUpdates(let value): self.setAutomaticUpdates(value)
        case .checkAppUpdate: self.checkAppUpdate()
        case .installAppUpdate: self.installAppUpdate()
        case .launchAtLogin(let value): self.setLaunchAtLogin(value)
        case .refreshLaunchAtLogin: self.refreshLaunchAtLogin()
        case .openLoginItemsSettings: self.openLoginItemsSettings()
        case .appearance(let value): self.setAppearance(value)
        case .dockIcon(let value): self.setDockIconVisible(value)
        case .menuBarMode(let value): self.setMenuBarDisplayMode(value)
        case .scanLogs(let value): self.setScanLogs(value)
        case .menuBarVisibility(let id, let visible):
            if let value = account(id) { self.setMenuBarVisibility(visible, for: value) }
        case .message(let value): self.message = value
        case .connect(let id, let provider, let path, let alias):
            reply(id) { try self.connect(provider: provider, path: path, alias: alias) }
        case .codexLogin(let id, let data, let alias, let replacing):
            reply(id) {
                if let replacing, account(replacing) == nil {
                    throw ProviderError("account_removed", "삭제된 계정입니다. 새 계정으로 연결하세요.")
                }
                guard data.count <= 1_048_576 else { throw ProviderError("large_auth", "로그인 정보가 너무 큽니다.") }
                try self.connectCodexLogin(data, alias: alias, replacing: replacing.flatMap(account))
            }
        }
        return 0
    }
}
