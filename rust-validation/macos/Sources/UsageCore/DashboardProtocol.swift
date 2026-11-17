import Foundation
import CQuotaBar

/// Only display data crosses to the disposable UI process. Credentials remain in
/// the monitor; a new browser login travels back once over its inherited pipe.
public struct DashboardState: Codable, Sendable {
    public var settings: LiteSettings
    public var snapshots: [UUID: AccountSnapshot]
    public var errors: [UUID: String]
    public var notes: [UUID: String]
    public var loading: Set<UUID>
    public var message: String?
    public var demoReport: LocalUsageReport?
    public var launchAtLogin: LaunchAtLoginState?
    public var appUpdate: AppUpdateState?

    public init(settings: LiteSettings, snapshots: [UUID: AccountSnapshot], errors: [UUID: String],
                notes: [UUID: String], loading: Set<UUID>, message: String?, demoReport: LocalUsageReport?,
                launchAtLogin: LaunchAtLoginState? = nil, appUpdate: AppUpdateState? = nil) {
        self.settings = settings; self.snapshots = snapshots; self.errors = errors
        self.notes = notes; self.loading = loading; self.message = message; self.demoReport = demoReport
        self.launchAtLogin = launchAtLogin
        self.appUpdate = appUpdate
    }
}

public enum DashboardCommand: Codable, Sendable {
    case ready, refreshAll, refreshAccounts, quit, showPopover
    case visibility(Bool)
    case route(String)
    case refresh(UUID), toggle(UUID), remove(UUID)
    case interval(Int), appearance(AppearanceMode), dockIcon(Bool), menuBarMode(MenuBarDisplayMode), scanLogs(Bool)
    case autoRefresh(Bool), launchAtLogin(Bool), refreshLaunchAtLogin, openLoginItemsSettings
    case automaticUpdates(Bool), checkAppUpdate, installAppUpdate
    case menuBarVisibility(UUID, Bool)
    case message(String?)
    case connect(id: UUID, provider: ProviderKind, path: String, alias: String)
    case codexLogin(id: UUID, data: Data, alias: String, replacing: UUID?)
}

public enum DashboardEvent: Codable, Sendable {
    case state(DashboardState)
    case show(route: String?)
    case reply(id: UUID, error: String?)
    case close
}

/// Four-byte length prefix, bounded before allocating a complete message.
/// No listening port, shared credential file, or persisted IPC transcript.
public struct PipeFrameDecoder {
    public static let maximumSize = 4 * 1024 * 1024
    private final class Storage {
        let handle: OpaquePointer
        init(copy: Storage? = nil) {
            if let copy { handle = qb_frame_decoder_copy(copy.handle)! }
            else { handle = qb_frame_decoder_new()! }
        }
        deinit { qb_frame_decoder_free(handle) }
    }
    private final class Frames { var values: [Data] = [] }
    private var storage = Storage()
    public init() {}

    public mutating func append(_ data: Data) throws -> [Data] {
        if !isKnownUniquelyReferenced(&storage) { storage = Storage(copy: storage) }
        let frames = Frames()
        let status = data.withUnsafeBytes { bytes in
            qb_frame_append(storage.handle, bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count,
                            { context, body, length in
                                guard let context, let body else { return }
                                Unmanaged<Frames>.fromOpaque(context).takeUnretainedValue()
                                    .values.append(Data(bytes: body, count: length))
                            }, Unmanaged.passUnretained(frames).toOpaque())
        }
        guard status == 0 else { throw PipeError.invalidFrame }
        return frames.values
    }

    public func finish() throws {
        guard qb_frame_finish(storage.handle) == 0 else { throw PipeError.truncatedFrame }
    }

    public static func encode(_ data: Data) throws -> Data {
        var output: UnsafeMutablePointer<UInt8>?, length = 0
        let status = data.withUnsafeBytes {
            qb_frame_encode($0.bindMemory(to: UInt8.self).baseAddress, $0.count, &output, &length)
        }
        guard status == 0, let output else { throw PipeError.invalidFrame }
        defer { qb_secret_free(output, length) }
        return Data(bytes: output, count: length)
    }
}

public enum PipeError: Error { case invalidFrame, truncatedFrame, disconnected }

/// Each end owns its two handles. Reads and writes never block the UI thread.
/// Close the writer first; EOF makes the peer close its end as well.
public final class DashboardPipe<Incoming: Decodable & Sendable, Outgoing: Encodable & Sendable>: @unchecked Sendable {
    private let input: FileHandle
    private let output: FileHandle
    private let writer = DispatchQueue(label: "com.aiusage.lite.dashboard-write")
    private let lock = NSLock()
    private var closed = false
    private var decoder = PipeFrameDecoder()
    private var onMessage: (@Sendable (Incoming) -> Void)?
    private var onClose: (@Sendable () -> Void)?

    public init(input: FileHandle, output: FileHandle) {
        self.input = input; self.output = output
    }

    public func start(onMessage: @escaping @Sendable (Incoming) -> Void, onClose: @escaping @Sendable () -> Void) {
        self.onMessage = onMessage; self.onClose = onClose
        input.readabilityHandler = { [weak self] handle in
            guard let self else { return }
            do {
                // FileHandle.read(upToCount:) can wait to fill its buffer on a
                // pipe. availableData returns the bytes that triggered readiness.
                let data = handle.availableData
                self.lock.lock()
                guard !self.closed else { self.lock.unlock(); return }
                let frames: [Data]
                do {
                    if data.isEmpty { try self.decoder.finish() }
                    frames = try self.decoder.append(data)
                    self.lock.unlock()
                } catch { self.lock.unlock(); throw error }
                if data.isEmpty { self.close(); return }
                for frame in frames {
                    self.onMessage?(try JSONDecoder().decode(Incoming.self, from: frame))
                }
            } catch { self.close() }
        }
    }

    public func send(_ message: Outgoing) {
        writer.async { [weak self] in
            guard let self else { return }
            self.lock.lock(); let closed = self.closed; self.lock.unlock()
            guard !closed else { return }
            do {
                let frame = try PipeFrameDecoder.encode(JSONEncoder().encode(message))
                try self.output.write(contentsOf: frame)
            } catch { self.close() }
        }
    }

    public func close() {
        lock.lock()
        guard !closed else { lock.unlock(); return }
        closed = true
        lock.unlock()
        input.readabilityHandler = nil
        try? output.close()
        try? input.close()
        onClose?()
    }
}
