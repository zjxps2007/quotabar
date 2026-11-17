import CQuotaBar
import Foundation

enum NativeRelease {
    static func version(_ value: String) -> [Int32]? {
        let data = Data(value.utf8.prefix(34))
        var components = [Int32](repeating: 0, count: 3)
        let code = data.withUnsafeBytes { bytes in
            components.withUnsafeMutableBufferPointer {
                qb_release_version(bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, $0.baseAddress)
            }
        }
        return code == 0 ? components : nil
    }

    private static func platformIndex(_ platform: ReleasePlatform) -> Int32 {
        switch platform {
        case .macOSArm64: return 0
        case .macOSX64: return 1
        case .windowsInstaller: return 2
        case .windowsPortable: return 3
        }
    }

    static func feed(_ data: Data, platform: ReleasePlatform) throws -> AppRelease? {
        let result = data.withUnsafeBytes {
            qb_release_feed(platformIndex(platform), $0.bindMemory(to: UInt8.self).baseAddress, $0.count)
        }
        guard let result else { throw ProviderError("invalid_release", "업데이트 정보를 확인할 수 없습니다. 잠시 후 다시 확인하세요.") }
        defer { qb_string_free(result) }
        return try JSONDecoder().decode(AppRelease?.self, from: Data(bytes: result, count: strlen(result)))
    }

    private struct Metadata: Encodable {
        let release: AppRelease
        let currentVersion: String
        let publicKey: String
    }

    private static func transfer<T: Decodable & Sendable>(
        _ body: @escaping @Sendable (OpaquePointer, UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>) -> Int32
    ) async throws -> T {
        let cancellation = NativeCancellation()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                DispatchQueue.global(qos: .utility).async {
                    var output: UnsafeMutablePointer<CChar>?
                    let status = body(cancellation.handle, &output)
                    defer { if let output { qb_string_free(output) } }
                    continuation.resume(with: Result {
                        switch status {
                        case 0:
                            guard let output else { throw URLError(.cannotDecodeContentData) }
                            return try JSONDecoder().decode(T.self, from: Data(bytes: output, count: strlen(output)))
                        case 1: throw ProviderError("development_build", "배포 앱에서 업데이트를 확인하세요.")
                        case 2: throw ProviderError("update_rate_limit", "업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요.")
                        case 3: throw ProviderError("invalid_release", "업데이트 정보를 확인할 수 없습니다. 잠시 후 다시 확인하세요.")
                        case 4: throw ProviderError("update_download", "서명된 업데이트 파일을 내려받지 못했습니다. 현재 버전을 유지합니다.")
                        case 5: throw invalidSignature()
                        case 6: throw CancellationError()
                        case 7: throw CocoaError(.fileWriteUnknown)
                        case 1000 + EEXIST: throw CocoaError(.fileWriteFileExists)
                        case 1000...: throw POSIXError(POSIXErrorCode(rawValue: status - 1000) ?? .EIO)
                        default: throw NSError(domain: NSURLErrorDomain, code: Int(status))
                        }
                    })
                }
            }
        } onCancel: { cancellation.cancel() }
    }

    static func latest(_ platform: ReleasePlatform, current: String) async throws -> AppRelease? {
        try await transfer { cancel, out in
            qb_mac_release_latest(platformIndex(platform), current, cancel, out)
        }
    }

    private final class ProgressCallback: Sendable {
        let notify: @Sendable (Int) -> Void
        init(_ notify: @escaping @Sendable (Int) -> Void) { self.notify = notify }
    }

    static func stage(_ release: AppRelease, current: String, key: String, directory: URL,
                      progress: @escaping @Sendable (Int) -> Void) async throws -> StagedUpdate {
        let metadata = try JSONEncoder().encode(Metadata(release: release, currentVersion: current, publicKey: key))
        let callback = ProgressCallback(progress)
        let verified: VerifiedUpdate = try await transfer { cancel, out in
            withExtendedLifetime(callback) {
                metadata.withUnsafeBytes { bytes in
                    directory.withUnsafeFileSystemRepresentation { path in
                        qb_mac_release_stage(bytes.bindMemory(to: UInt8.self).baseAddress, bytes.count, path, cancel,
                            Unmanaged.passUnretained(callback).toOpaque(), { context, percent in
                                guard let context else { return }
                                Unmanaged<ProgressCallback>.fromOpaque(context).takeUnretainedValue().notify(Int(percent))
                            }, out)
                    }
                }
            }
        }
        return StagedUpdate(update: verified, directory: directory)
    }
    static func verify(_ manifest: Data, signature: Data, key: String, release: AppRelease, current: String) throws -> VerifiedUpdate {
        let meta = try JSONEncoder().encode(Metadata(release: release, currentVersion: current, publicKey: key))
        let build = meta.withUnsafeBytes { m in
            manifest.withUnsafeBytes { body in
                signature.withUnsafeBytes { sig in
                    qb_release_verify(m.bindMemory(to: UInt8.self).baseAddress, m.count, body.bindMemory(to: UInt8.self).baseAddress,
                                      body.count, sig.bindMemory(to: UInt8.self).baseAddress, sig.count)
                }
            }
        }
        guard build > 0, let asset = release.asset else { throw invalidSignature() }
        return VerifiedUpdate(release: release, build: Int(build), asset: asset)
    }

    static func verifyFile(_ url: URL, asset: ReleaseAsset) throws {
        let data = try JSONEncoder().encode(asset)
        let status = url.withUnsafeFileSystemRepresentation { path in
            data.withUnsafeBytes { qb_release_file(path, $0.bindMemory(to: UInt8.self).baseAddress, $0.count, nil) }
        }
        if status > 1 { throw POSIXError(POSIXErrorCode(rawValue: status - 1) ?? .EIO) }
        guard status == 0 else { throw invalidSignature() }
    }
    private static func invalidSignature() -> ProviderError {
        ProviderError("update_signature", "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다.")
    }
}
