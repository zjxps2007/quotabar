import Foundation

/// Stable releases use three numeric components. Compare numbers, never tag text.
public struct ReleaseVersion: Equatable, Comparable, Sendable {
    public let major: Int32
    public let minor: Int32
    public let patch: Int32
    public var description: String { "\(major).\(minor).\(patch)" }

    public init?(_ value: String) {
        guard let numbers = NativeRelease.version(value) else { return nil }
        major = numbers[0]; minor = numbers[1]; patch = numbers[2]
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        if lhs.major != rhs.major { return lhs.major < rhs.major }
        if lhs.minor != rhs.minor { return lhs.minor < rhs.minor }
        return lhs.patch < rhs.patch
    }
}

public enum ReleasePlatform: String, Codable, Sendable {
    case macOSArm64 = "macOS-arm64.zip"
    case macOSX64 = "macOS-x86_64.zip"
    case windowsInstaller = "Windows-x64-Setup.exe"
    case windowsPortable = "Windows-x64.zip"
}

public struct ReleaseAsset: Codable, Equatable, Sendable {
    public let name: String
    public let url: URL
    public let bytes: Int64
    public let sha256: String
}

public struct AppRelease: Codable, Equatable, Sendable {
    public let tag: String
    public let version: String
    public let page: URL
    public let asset: ReleaseAsset?

    public func isNewer(than current: String) -> Bool {
        guard let installed = ReleaseVersion(current), let offered = ReleaseVersion(version) else { return false }
        return offered > installed
    }
}

public enum ReleaseFeed {
    public static let repository = "https://github.com/zjxps2007/quotabar"
    public static let endpoint = URL(string: "https://api.github.com/repos/zjxps2007/quotabar/releases/latest")!
    public static let maximumResponseBytes = 512 * 1024
    public static let maximumAssetBytes: Int64 = 512 * 1024 * 1024

    public static func parse(_ data: Data, platform: ReleasePlatform) throws -> AppRelease? {
        try NativeRelease.feed(data, platform: platform)
    }
}

public struct VerifiedUpdate: Codable, Equatable, Sendable {
    public let release: AppRelease
    public let build: Int
    public let asset: ReleaseAsset
}

public enum UpdatePhase: String, Codable, Sendable {
    case idle, checking, current, available, downloading, ready, installing, failed
}

public struct AppUpdateState: Codable, Equatable, Sendable {
    public var phase: UpdatePhase
    public var version: String?
    public var progress: Int?
    public var message: String?
    public var checkedAt: Date?
    public init(phase: UpdatePhase = .idle, version: String? = nil, progress: Int? = nil,
                message: String? = nil, checkedAt: Date? = nil) {
        self.phase = phase; self.version = version; self.progress = progress
        self.message = message; self.checkedAt = checkedAt
    }
}

public enum UpdateSignature {
    public static func verify(manifest: Data, signature: Data, publicKeyPEM: String,
                              release: AppRelease, currentVersion: String) throws -> VerifiedUpdate {
        try NativeRelease.verify(manifest, signature: signature, key: publicKeyPEM, release: release, current: currentVersion)
    }

    public static func verifyFile(_ url: URL, asset: ReleaseAsset) throws {
        try NativeRelease.verifyFile(url, asset: asset)
    }
}

/// A separate, unauthenticated session never sends provider credentials or cookies.
public final class ReleaseClient: @unchecked Sendable {
    private let session: URLSession
    private let redirects = ReleaseRedirectPolicy()

    public init() {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = nil
        configuration.urlCredentialStorage = nil
        configuration.urlCache = nil
        configuration.timeoutIntervalForRequest = 20
        configuration.timeoutIntervalForResource = 30
        session = URLSession(configuration: configuration)
    }

    deinit { session.invalidateAndCancel() }

    public func latest(platform: ReleasePlatform, currentVersion: String) async throws -> AppRelease? {
        guard ReleaseVersion(currentVersion) != nil else {
            throw ProviderError("development_build", "배포 앱에서 업데이트를 확인하세요.")
        }
        var request = URLRequest(url: ReleaseFeed.endpoint)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("QuotaBar/\(currentVersion)", forHTTPHeaderField: "User-Agent")
        let (bytes, response) = try await session.bytes(for: request, delegate: redirects)
        guard let response = response as? HTTPURLResponse else { throw URLError(.badServerResponse) }
        if response.statusCode == 404 { return nil }
        if response.statusCode == 403 || response.statusCode == 429 {
            throw ProviderError("update_rate_limit", "업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요.")
        }
        guard response.statusCode == 200, response.expectedContentLength <= ReleaseFeed.maximumResponseBytes else {
            throw URLError(.badServerResponse)
        }
        var data = Data()
        for try await byte in bytes {
            guard data.count < ReleaseFeed.maximumResponseBytes else { throw URLError(.dataLengthExceedsMaximum) }
            data.append(byte)
        }
        try Task.checkCancellation()
        return try ReleaseFeed.parse(data, platform: platform)
    }
}

private final class ReleaseRedirectPolicy: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}
