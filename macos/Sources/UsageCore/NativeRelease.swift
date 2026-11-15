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

    static func feed(_ data: Data, platform: ReleasePlatform) throws -> AppRelease? {
        let index: Int32
        switch platform {
        case .macOSArm64: index = 0
        case .macOSX64: index = 1
        case .windowsInstaller: index = 2
        case .windowsPortable: index = 3
        }
        let result = data.withUnsafeBytes {
            qb_release_feed(index, $0.bindMemory(to: UInt8.self).baseAddress, $0.count)
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
