import Foundation
import CoreServices
import Testing
@testable import UsageCore

struct LaunchAtLoginTests {
    private final class Service: LoginItemService {
        var state: LaunchAtLoginState = .disabled
        var calls: [Bool] = []
        var failure = false
        func register() throws {
            if failure { throw CocoaError(.fileWriteNoPermission) }
            calls.append(true); state = .enabled
        }
        func unregister() throws {
            if failure { throw CocoaError(.fileWriteNoPermission) }
            calls.append(false); state = .disabled
        }
    }

    @Test func registrationIsExplicitAndIdempotent() throws {
        let service = Service()
        let login = LaunchAtLoginManager(service: service)
        #expect(login.state == .disabled && service.calls.isEmpty)
        try login.setEnabled(true); try login.setEnabled(true)
        #expect(service.calls == [true] && login.state == .enabled)
        try login.setEnabled(false); try login.setEnabled(false)
        #expect(service.calls == [true, false] && login.state == .disabled)
    }

    @Test func systemApprovalIsNotMistakenForEnabledOrOverridden() throws {
        let service = Service(); service.state = .requiresApproval
        let login = LaunchAtLoginManager(service: service)
        try login.setEnabled(true)
        #expect(login.state == .requiresApproval && service.calls.isEmpty)
        try login.setEnabled(false)
        #expect(login.state == .disabled && service.calls == [false])
        service.state = .enabled // An external system-settings change is read directly.
        #expect(login.state == .enabled)
    }

    @Test func failureAndUnavailableServicesNeverReportSuccess() throws {
        let service = Service()
        let login = LaunchAtLoginManager(service: service)
        service.failure = true
        #expect(throws: CocoaError.self) { try login.setEnabled(true) }
        #expect(login.state == .disabled && service.calls.isEmpty)
        service.state = .enabled
        #expect(throws: CocoaError.self) { try login.setEnabled(false) }
        #expect(login.state == .enabled)
        service.state = .unavailable
        #expect(throws: ProviderError.self) { try login.setEnabled(true) }
        #expect(service.calls.isEmpty)
    }

    @Test func onlyLoginLaunchEventsSuppressTheInitialDashboard() {
        let event = NSAppleEventDescriptor(eventClass: kCoreEventClass, eventID: kAEOpenApplication,
            targetDescriptor: nil, returnID: AEReturnID(kAutoGenerateReturnID), transactionID: AETransactionID(kAnyTransactionID))
        #expect(!LaunchAtLoginManager.isLoginLaunch(nil))
        #expect(!LaunchAtLoginManager.isLoginLaunch(event))
        event.setParam(NSAppleEventDescriptor(enumCode: keyAELaunchedAsLogInItem), forKeyword: keyAEPropData)
        #expect(LaunchAtLoginManager.isLoginLaunch(event))
        event.setParam(NSAppleEventDescriptor(enumCode: keyAELaunchedAsServiceItem), forKeyword: keyAEPropData)
        #expect(!LaunchAtLoginManager.isLoginLaunch(event))
    }
}
