import Foundation

public struct StagedUpdate: Codable, Sendable {
    public let update: VerifiedUpdate
    public let directory: URL
    public var archive: URL { directory.appendingPathComponent(update.asset.name) }

    init(update: VerifiedUpdate, directory: URL) {
        self.update = update
        // The installer compares this decoded URL with install.json's parent.
        // Preserve the directory hint so the same path survives that comparison.
        self.directory = URL(fileURLWithPath: directory.path, isDirectory: true)
    }
}

public final class UpdateDownloader: Sendable {
    public init() {}
    public func prepare(release: AppRelease, currentVersion: String, publicKeyPEM: String, directory: URL,
                        progress: @escaping @Sendable (Int) -> Void) async throws -> StagedUpdate {
        try await NativeRelease.stage(release, current: currentVersion, key: publicKeyPEM,
                                      directory: directory, progress: progress)
    }
}
