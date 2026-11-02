import Foundation
import CryptoKit

/// Stable releases use three numeric components. Compare numbers, never tag text.
public struct ReleaseVersion: Equatable, Comparable, Sendable {
    public let major: Int32
    public let minor: Int32
    public let patch: Int32
    public var description: String { "\(major).\(minor).\(patch)" }

    public init?(_ value: String) {
        let text = value.hasPrefix("v") ? String(value.dropFirst()) : value
        let parts = text.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count == 3 else { return nil }
        var numbers: [Int32] = []
        for part in parts {
            guard !part.isEmpty, part.count <= 10, (part.count == 1 || part.first != "0"),
                  part.utf8.allSatisfy({ $0 >= 48 && $0 <= 57 }), let number = Int32(part) else { return nil }
            numbers.append(number)
        }
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

    private struct Response: Decodable {
        let tag_name: String
        let html_url: URL
        let draft: Bool
        let prerelease: Bool
        let assets: [Asset]
        struct Asset: Decodable {
            let name: String
            let browser_download_url: URL
            let state: String
            let size: Int64
            let digest: String?
        }
    }

    /// URLs are restricted to this repository and the selected release/architecture.
    public static func parse(_ data: Data, platform: ReleasePlatform) throws -> AppRelease? {
        guard data.count <= maximumResponseBytes else { throw invalid() }
        let response = try JSONDecoder().decode(Response.self, from: data)
        guard !response.draft, !response.prerelease else { return nil }
        guard let version = ReleaseVersion(response.tag_name),
              response.html_url.absoluteString == "\(repository)/releases/tag/\(response.tag_name)" else { throw invalid() }
        let name = "QuotaBar-\(version.description)-\(platform.rawValue)"
        let matches = response.assets.filter { $0.name == name }
        guard matches.count <= 1 else { throw invalid() }
        var asset: ReleaseAsset?
        if let candidate = matches.first {
            guard candidate.state == "uploaded", candidate.size > 0, candidate.size <= maximumAssetBytes,
                  candidate.browser_download_url.absoluteString == "\(repository)/releases/download/\(response.tag_name)/\(name)",
                  let digest = candidate.digest, digest.hasPrefix("sha256:") else { throw invalid() }
            let hash = String(digest.dropFirst(7)).lowercased()
            guard hash.utf8.count == 64, hash.utf8.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else { throw invalid() }
            asset = ReleaseAsset(name: name, url: candidate.browser_download_url, bytes: candidate.size, sha256: hash)
        }
        return AppRelease(tag: response.tag_name, version: version.description, page: response.html_url, asset: asset)
    }

    private static func invalid() -> ProviderError {
        ProviderError("invalid_release", "업데이트 정보를 확인할 수 없습니다. 잠시 후 다시 확인하세요.")
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
    private struct Manifest: Decodable {
        let schemaVersion: Int
        let version: String
        let build: Int
        let assets: [Asset]
        struct Asset: Decodable { let name: String; let bytes: Int64; let sha256: String }
    }

    /// Validate the exact manifest bytes before trusting any version or file hash.
    public static func verify(manifest: Data, signature: Data, publicKeyPEM: String,
                              release: AppRelease, currentVersion: String) throws -> VerifiedUpdate {
        guard manifest.count <= 128 * 1024, (8...128).contains(signature.count),
              release.isNewer(than: currentVersion), let asset = release.asset else { throw invalid() }
        do {
            let key = try P256.Signing.PublicKey(pemRepresentation: publicKeyPEM)
            let signed = try P256.Signing.ECDSASignature(derRepresentation: signature)
            guard key.isValidSignature(signed, for: manifest) else { throw invalid() }
            let value = try JSONDecoder().decode(Manifest.self, from: manifest)
            let matches = value.assets.filter { $0.name == asset.name }
            guard value.schemaVersion == 1, value.version == release.version, value.build > 0,
                  matches.count == 1, let signedAsset = matches.first,
                  signedAsset.bytes == asset.bytes, signedAsset.sha256 == asset.sha256 else { throw invalid() }
            return VerifiedUpdate(release: release, build: value.build, asset: asset)
        } catch { throw invalid() }
    }

    public static func verifyFile(_ url: URL, asset: ReleaseAsset) throws {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hash = SHA256(), total: Int64 = 0
        while let data = try handle.read(upToCount: 1024 * 1024), !data.isEmpty {
            total += Int64(data.count)
            guard total <= asset.bytes else { throw invalid() }
            hash.update(data: data)
        }
        let digest = hash.finalize().map { String(format: "%02x", $0) }.joined()
        guard total == asset.bytes, digest == asset.sha256 else { throw invalid() }
    }

    private static func invalid() -> ProviderError {
        ProviderError("update_signature", "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다.")
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
