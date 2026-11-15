import CQuotaBar
import Foundation
import UsageCore

struct UpdateInstallResult: Decodable {
    let version: String
    let success: Bool
    let message: String
}

enum MacUpdateInstaller {
    private struct Plan: Encodable {
        let staged: StagedUpdate
        let target: URL
        let currentVersion: String
    }

    static var updatesRoot: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/AIUsage Lite/Updates")
    }
    static var resultURL: URL { updatesRoot.appendingPathComponent("last-result.json") }
    static var publicKey: String? {
        guard let url = AppResources.bundle.url(forResource: "UpdatePublicKey", withExtension: "pem") else { return nil }
        return try? String(contentsOf: url, encoding: .utf8)
    }

    static func prepare(_ staged: StagedUpdate, target: URL, currentVersion: String) throws -> URL {
        let plan = try JSONEncoder().encode(Plan(staged: staged, target: target, currentVersion: currentVersion))
        let status = plan.withUnsafeBytes {
            qb_mac_update_prepare($0.bindMemory(to: UInt8.self).baseAddress, $0.count)
        }
        guard status == 0 else { throw CocoaError(.executableLoad) }
        return staged.directory.appendingPathComponent("install.json")
    }

    static func discard(_ staged: StagedUpdate) {
        staged.directory.withUnsafeFileSystemRepresentation { directory in
            Bundle.main.bundleURL.withUnsafeFileSystemRepresentation { target in
                qb_mac_update_discard(directory, target)
            }
        }
    }

    final class Helper {
        private var handle: OpaquePointer?

        init(job: URL) throws {
            guard let child = job.withUnsafeFileSystemRepresentation({ qb_mac_update_launch($0) }) else {
                throw CocoaError(.executableLoad)
            }
            handle = child
        }
        deinit { if let handle { qb_mac_update_finish(handle, 1) } }

        func isReady() throws -> Bool {
            let status = qb_mac_update_ready(handle)
            guard status >= 0 else { throw CocoaError(.executableLoad) }
            return status == 1
        }

        func finish(terminate: Bool) {
            if let handle { qb_mac_update_finish(handle, terminate ? 1 : 0); self.handle = nil }
        }
    }
}
