import Foundation
import Darwin
import UsageCore

struct UpdateInstallResult: Codable {
    let version: String
    let success: Bool
    let message: String
}

struct MacUpdatePlan: Codable {
    let staged: StagedUpdate
    let target: URL
    let currentVersion: String
    var transaction: UUID? { UUID(uuidString: staged.directory.lastPathComponent) }
}

enum MacUpdateInstaller {
    static var dataRoot: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/AIUsage Lite")
    }
    static var updatesRoot: URL { dataRoot.appendingPathComponent("Updates") }
    static var resultURL: URL { updatesRoot.appendingPathComponent("last-result.json") }
    static var publicKey: String? {
        guard let url = AppResources.bundle.url(forResource: "UpdatePublicKey", withExtension: "pem") else { return nil }
        return try? String(contentsOf: url, encoding: .utf8)
    }

    static func prepare(_ staged: StagedUpdate, target: URL, currentVersion: String) throws -> URL {
        let fm = FileManager.default
        let plan = MacUpdatePlan(staged: staged, target: target, currentVersion: currentVersion)
        guard let transaction = plan.transaction, let key = publicKey,
              staged.directory.deletingLastPathComponent().standardizedFileURL == updatesRoot.standardizedFileURL,
              fm.isWritableFile(atPath: target.deletingLastPathComponent().path) else { throw CocoaError(.fileWriteNoPermission) }
        try validateSignature(plan, key: key)
        let replacement = try UpdateReplacement(target: target, transaction: transaction)
        guard !fm.fileExists(atPath: replacement.incoming.path), !fm.fileExists(atPath: replacement.backup.path) else { throw CocoaError(.fileWriteFileExists) }
        let unpacked = staged.directory.appendingPathComponent("unpacked")
        try fm.createDirectory(at: unpacked, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        try run("/usr/bin/ditto", ["-x", "-k", staged.archive.path, unpacked.path])
        let incoming = unpacked.appendingPathComponent("QuotaBar.app")
        try validateBundle(incoming, update: staged.update, key: key)
        do {
            try run("/usr/bin/ditto", [incoming.path, replacement.incoming.path])
            try validateBundle(replacement.incoming, update: staged.update, key: key)
            // The helper runs from a private copy after the old app has exited.
            try run("/usr/bin/ditto", [target.path, staged.directory.appendingPathComponent("Installer.app").path])
            let job = staged.directory.appendingPathComponent("install.json")
            try write(JSONEncoder().encode(plan), to: job)
            return job
        } catch { try? fm.removeItem(at: replacement.incoming); throw error }
    }

    static func launchHelper(job: URL) throws -> Process {
        try? FileManager.default.removeItem(at: job.deletingLastPathComponent().appendingPathComponent("installer-ready.json"))
        let process = Process()
        process.executableURL = job.deletingLastPathComponent().appendingPathComponent("Installer.app/Contents/MacOS/QuotaBar")
        process.arguments = ["--apply-update", job.path]
        process.standardInput = FileHandle.nullDevice; process.standardOutput = FileHandle.nullDevice; process.standardError = FileHandle.nullDevice
        try process.run()
        return process
    }

    static func acknowledgeIfRequested() {
        let args = CommandLine.arguments
        guard let index = args.firstIndex(of: "--update-ack"), args.indices.contains(index + 1),
              let transaction = UUID(uuidString: args[index + 1]),
              let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String else { return }
        let directory = updatesRoot.appendingPathComponent(transaction.uuidString)
        guard let data = try? Data(contentsOf: directory.appendingPathComponent("install.json")), data.count < 64 * 1024,
              let plan = try? JSONDecoder().decode(MacUpdatePlan.self, from: data),
              plan.target.standardizedFileURL == Bundle.main.bundleURL.standardizedFileURL,
              plan.staged.update.release.version == version else { return }
        let result = UpdateInstallResult(version: version, success: true, message: "업데이트를 설치했습니다.")
        try? write(JSONEncoder().encode(result), to: directory.appendingPathComponent("started.json"))
    }

    private static func validateSignature(_ plan: MacUpdatePlan, key: String) throws {
        let staged = plan.staged
        let verified = try UpdateSignature.verify(manifest: Data(contentsOf: staged.directory.appendingPathComponent("update-manifest.json")),
            signature: Data(contentsOf: staged.directory.appendingPathComponent("update-manifest.sig")), publicKeyPEM: key,
            release: staged.update.release, currentVersion: plan.currentVersion)
        guard verified == staged.update else { throw CocoaError(.fileReadCorruptFile) }
        try UpdateSignature.verifyFile(staged.archive, asset: verified.asset)
    }
    private static func validateBundle(_ url: URL, update: VerifiedUpdate, key: String) throws {
        guard let bundle = Bundle(url: url), bundle.bundleIdentifier == "com.aiusage.lite",
              bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String == update.release.version,
              bundle.object(forInfoDictionaryKey: "CFBundleVersion") as? String == String(update.build),
              let resources = Bundle(url: url.appendingPathComponent("Contents/Resources/QuotaBar_QuotaBar.bundle")),
              let keyURL = resources.url(forResource: "UpdatePublicKey", withExtension: "pem"),
              try String(contentsOf: keyURL, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines) == key.trimmingCharacters(in: .whitespacesAndNewlines)
        else { throw CocoaError(.fileReadCorruptFile) }
        try run("/usr/bin/codesign", ["--verify", "--strict", "--deep", url.path])
        #if arch(arm64)
        let architecture = "arm64"
        #else
        let architecture = "x86_64"
        #endif
        try run("/usr/bin/lipo", [url.appendingPathComponent("Contents/MacOS/QuotaBar").path, "-verify_arch", architecture])
    }
    private static func run(_ executable: String, _ arguments: [String]) throws {
        let process = Process(); process.executableURL = URL(fileURLWithPath: executable); process.arguments = arguments
        process.standardInput = FileHandle.nullDevice; process.standardOutput = FileHandle.nullDevice; process.standardError = FileHandle.nullDevice
        try process.run(); process.waitUntilExit()
        guard process.terminationStatus == 0 else { throw CocoaError(.executableLoad) }
    }
    private static func write(_ data: Data, to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try data.write(to: url, options: .atomic)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: url.path)
    }
}
