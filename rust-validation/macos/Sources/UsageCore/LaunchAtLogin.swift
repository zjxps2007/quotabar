import Foundation
import CoreServices
import CQuotaBar

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
        switch qb_mac_login_item(0) {
        case 0: return .disabled
        case 1: return .enabled
        case 2: return .requiresApproval
        default: return .unavailable
        }
    }
    func register() throws { if qb_mac_login_item(1) != 0 { throw CocoaError(.fileWriteNoPermission) } }
    func unregister() throws { if qb_mac_login_item(2) != 0 { throw CocoaError(.fileWriteNoPermission) } }
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

    public static func openSettings() { _ = qb_mac_login_item(3) }

    public static func isLoginLaunch(_ event: NSAppleEventDescriptor?) -> Bool {
        event?.eventClass == kCoreEventClass && event?.eventID == kAEOpenApplication
            && event?.paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem
    }
}
