import Foundation
import Darwin

/// Incoming and backup directories live beside the app so renames stay on one volume.
public struct UpdateReplacement: Sendable {
    public let target: URL
    public let incoming: URL
    public let backup: URL
    public init(target: URL, transaction: UUID) throws {
        let target = target.standardizedFileURL
        guard target.isFileURL, target.pathExtension == "app", target.path != "/",
              target.resolvingSymlinksInPath().path == target.path else { throw CocoaError(.fileWriteInvalidFileName) }
        self.target = target
        let parent = target.deletingLastPathComponent()
        let name = target.deletingPathExtension().lastPathComponent
        incoming = parent.appendingPathComponent(".\(name).update-\(transaction.uuidString).app")
        backup = parent.appendingPathComponent(".\(name).backup-\(transaction.uuidString).app")
    }

    public func commit(afterSwap: () throws -> Void = {}) throws {
        let fm = FileManager.default
        guard fm.fileExists(atPath: target.path), fm.fileExists(atPath: incoming.path),
              !fm.fileExists(atPath: backup.path) else { throw CocoaError(.fileWriteFileExists) }
        try swap(target, incoming)
        do {
            try afterSwap()
            try fm.moveItem(at: incoming, to: backup)
        } catch {
            // The app path remains present even across an interrupted transaction.
            try swap(target, incoming)
            throw error
        }
    }

    public func rollback() throws {
        let fm = FileManager.default
        guard fm.fileExists(atPath: backup.path) else { throw CocoaError(.fileNoSuchFile) }
        if fm.fileExists(atPath: target.path) {
            try swap(target, backup)
            try? fm.removeItem(at: backup)
        } else { try fm.moveItem(at: backup, to: target) }
    }

    private func swap(_ lhs: URL, _ rhs: URL) throws {
        let result = lhs.path.withCString { a in
            rhs.path.withCString { b in renameatx_np(AT_FDCWD, a, AT_FDCWD, b, UInt32(RENAME_SWAP)) }
        }
        guard result == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    }
}
