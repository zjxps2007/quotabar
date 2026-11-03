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

public final class UpdateDownloader: @unchecked Sendable {
    private let session: URLSession
    public init() {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = nil; configuration.urlCredentialStorage = nil; configuration.urlCache = nil
        configuration.timeoutIntervalForRequest = 30; configuration.timeoutIntervalForResource = 20 * 60
        session = URLSession(configuration: configuration)
    }
    deinit { session.invalidateAndCancel() }

    public func prepare(release: AppRelease, currentVersion: String, publicKeyPEM: String, directory: URL,
                        progress: @escaping @Sendable (Int) -> Void) async throws -> StagedUpdate {
        let fm = FileManager.default
        guard !fm.fileExists(atPath: directory.path) else { throw CocoaError(.fileWriteFileExists) }
        try fm.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        var completed = false
        defer { if !completed { try? fm.removeItem(at: directory) } }
        let base = "\(ReleaseFeed.repository)/releases/download/\(release.tag)/"
        let manifestURL = directory.appendingPathComponent("update-manifest.json")
        let signatureURL = directory.appendingPathComponent("update-manifest.sig")
        try await fetch(URL(string: base + "update-manifest.json")!, to: manifestURL, limit: 128 * 1024)
        try await fetch(URL(string: base + "update-manifest.sig")!, to: signatureURL, limit: 128)
        let verified = try UpdateSignature.verify(manifest: Data(contentsOf: manifestURL), signature: Data(contentsOf: signatureURL),
            publicKeyPEM: publicKeyPEM, release: release, currentVersion: currentVersion)
        let staged = StagedUpdate(update: verified, directory: directory)
        try await fetch(verified.asset.url, to: staged.archive, limit: verified.asset.bytes, progress: progress)
        try Task.checkCancellation()
        try UpdateSignature.verifyFile(staged.archive, asset: verified.asset)
        try Task.checkCancellation()
        completed = true
        return staged
    }

    private func fetch(_ url: URL, to destination: URL, limit: Int64,
                       progress: (@Sendable (Int) -> Void)? = nil) async throws {
        var request = URLRequest(url: url)
        request.setValue("QuotaBar-Updater", forHTTPHeaderField: "User-Agent")
        let delegate = UpdateTransferDelegate(limit: limit, progress: progress)
        let (temporary, response) = try await session.download(for: request, delegate: delegate)
        defer { try? FileManager.default.removeItem(at: temporary) }
        guard let http = response as? HTTPURLResponse, http.statusCode == 200,
              response.url?.scheme == "https" else {
            throw ProviderError("update_download", "서명된 업데이트 파일을 내려받지 못했습니다. 현재 버전을 유지합니다.")
        }
        let attributes = try FileManager.default.attributesOfItem(atPath: temporary.path)
        guard let size = attributes[.size] as? NSNumber, size.int64Value <= limit else { throw URLError(.dataLengthExceedsMaximum) }
        try Task.checkCancellation()
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: temporary.path)
        try FileManager.default.moveItem(at: temporary, to: destination)
    }
}

private final class UpdateTransferDelegate: NSObject, URLSessionDownloadDelegate, @unchecked Sendable {
    let limit: Int64
    let progress: (@Sendable (Int) -> Void)?
    private var lastProgress = -1
    init(limit: Int64, progress: (@Sendable (Int) -> Void)?) { self.limit = limit; self.progress = progress }
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {}
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
                    totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64) {
        guard totalBytesWritten <= limit, totalBytesExpectedToWrite <= limit else { downloadTask.cancel(); return }
        let percent = Int(min(100, totalBytesWritten * 100 / max(1, limit)))
        if lastProgress != percent { lastProgress = percent; progress?(percent) }
    }
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        // GitHub release assets redirect to a CDN. No credentials or cookies are attached.
        completionHandler(request.url?.scheme == "https" ? request : nil)
    }
}
