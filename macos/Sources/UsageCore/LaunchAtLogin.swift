import Foundation
import CoreServices
import ServiceManagement

public enum LaunchAtLoginState: String, Codable, Sendable {
    case disabled, enabled, requiresApproval, unavailable
    public var isRegistered: Bool { self == .enabled || self == .requiresApproval }
}

protocol LoginItemService {
    var state: LaunchAtLoginState { get }
    func register() throws
    func unregister() throws
}

private struct SystemLoginItem: LoginItemService {
    var state: LaunchAtLoginState {
        // Only the packaged monitor may register itself, never the Dashboard,
        // command-line build or test runner.
        guard Bundle.main.bundleURL.pathExtension == "app",
              Bundle.main.bundleIdentifier == "com.aiusage.lite" else { return .unavailable }
        switch SMAppService.mainApp.status {
        case .notRegistered: return .disabled
        case .enabled: return .enabled
        case .requiresApproval: return .requiresApproval
        case .notFound: return .unavailable
        @unknown default: return .unavailable
        }
    }
    func register() throws { try SMAppService.mainApp.register() }
    func unregister() throws { try SMAppService.mainApp.unregister() }
}

/// The OS owns persistence and approval; merely launching the app never registers it.
public final class LaunchAtLoginManager {
    private let service: any LoginItemService
    public convenience init() { self.init(service: SystemLoginItem()) }
    init(service: any LoginItemService) { self.service = service }
    public var state: LaunchAtLoginState { service.state }

    public func setEnabled(_ enabled: Bool) throws {
        let current = state
        guard current != .unavailable else {
            throw ProviderError("login_item_unavailable", "빌드된 QuotaBar.app에서 자동 실행을 설정하세요.")
        }
        if enabled && !current.isRegistered { try service.register() }
        else if !enabled && current.isRegistered { try service.unregister() }
    }

    public static func openSettings() { SMAppService.openSystemSettingsLoginItems() }

    public static func isLoginLaunch(_ event: NSAppleEventDescriptor?) -> Bool {
        event?.eventClass == kCoreEventClass && event?.eventID == kAEOpenApplication
            && event?.paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem
    }
}
