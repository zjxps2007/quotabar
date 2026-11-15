import Foundation
import CQuotaBar

/// Owns the helper and its private pipes until the read loop observes shutdown.
public final class NativeDashboardChild: @unchecked Sendable {
    private let handle: OpaquePointer
    private let writer = DispatchQueue(label: "com.aiusage.lite.dashboard-events")

    public init(executable: URL, demo: Bool) throws {
        guard let child = executable.withUnsafeFileSystemRepresentation({
            qb_mac_dashboard_spawn($0, demo ? 1 : 0)
        }) else { throw PipeError.disconnected }
        handle = child
    }
    deinit { qb_dashboard_free(handle) }
    public var processIdentifier: Int32 { Int32(bitPattern: qb_dashboard_pid(handle)) }
    public var isRunning: Bool { processIdentifier != 0 }
    public func ready() { qb_dashboard_control(handle, 0) }
    public func close() { qb_dashboard_control(handle, 1) }

    public func send(_ event: DashboardEvent) {
        writer.async { [self] in
            do {
                let data = try JSONEncoder().encode(event)
                let status = data.withUnsafeBytes {
                    qb_dashboard_send(handle, $0.bindMemory(to: UInt8.self).baseAddress, $0.count)
                }
                if status != 0 { close() }
            } catch { close() }
        }
    }

    private func read() throws -> DashboardCommand? {
        var output: UnsafeMutablePointer<UInt8>?, length = 0
        let status = qb_dashboard_read(handle, &output, &length)
        if status == 1 { return nil }
        guard status == 0, let output else { throw PipeError.disconnected }
        defer { qb_secret_free(output, length) }
        return try JSONDecoder().decode(DashboardCommand.self, from: Data(bytes: output, count: length))
    }

    public func start(onMessage: @escaping @Sendable (DashboardCommand) -> Void,
                      onExit: @escaping @Sendable (_ startupTimedOut: Bool) -> Void) {
        DispatchQueue.global(qos: .userInitiated).async { [self] in
            do { while let command = try read() { onMessage(command) } }
            catch { /* A malformed or broken pipe ends this helper session. */ }
            close()
            qb_dashboard_control(handle, 3)
            onExit(qb_mac_dashboard_timed_out(handle) != 0)
        }
    }
}
