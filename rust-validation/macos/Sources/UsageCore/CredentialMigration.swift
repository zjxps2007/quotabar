import CQuotaBar
import Foundation

public enum CredentialMigration {
    /// Production startup reads and retires legacy items entirely in the native service.
    public static func run(settings: LiteSettings, file: SettingsFile, vault: SecretVault) throws -> LiteSettings {
        try migrate(settings, file, vault, nil, nil)
    }

    private final class LegacyItems {
        let read: (UUID) -> Data?
        let delete: (UUID) -> Void
        init(_ read: @escaping (UUID) -> Data?, _ delete: @escaping (UUID) -> Void) {
            self.read = read; self.delete = delete
        }
    }

    public static func run(settings: LiteSettings, file: SettingsFile, vault: SecretVault,
                           legacyRead: (UUID) -> Data? = LegacyKeychain.read,
                           legacyDelete: (UUID) -> Void = LegacyKeychain.delete) throws -> LiteSettings {
        try withoutActuallyEscaping(legacyRead) { read in
            try withoutActuallyEscaping(legacyDelete) { delete in
                let items = LegacyItems(read, delete)
                return try withExtendedLifetime(items) {
                    try migrate(settings, file, vault, Unmanaged.passUnretained(items).toOpaque()) { context, name, receiver in
                        guard let context, let name, let id = UUID(uuidString: String(cString: name)) else { return }
                        let items = Unmanaged<LegacyItems>.fromOpaque(context).takeUnretainedValue()
                        guard let receiver else { items.delete(id); return }
                        if let data = items.read(id), data.count <= 1_048_576 {
                            _ = data.withUnsafeBytes { qb_sessions_loaded(receiver, $0.bindMemory(to: UInt8.self).baseAddress, $0.count) }
                        }
                    }
                }
            }
        }
    }

    private static func migrate(_ settings: LiteSettings, _ file: SettingsFile, _ vault: SecretVault,
                                _ context: UnsafeMutableRawPointer?, _ callback: QBLegacyItemCallback?) throws -> LiteSettings {
        let data = try JSONEncoder().encode(settings)
        var output: UnsafeMutablePointer<CChar>?
        let status = data.withUnsafeBytes {
            qb_mac_migrate(file.url.path, vault.directory.path, $0.bindMemory(to: UInt8.self).baseAddress,
                           $0.count, context, callback, &output)
        }
        guard status == 0, let output else {
            if status == -24 { throw SecretVault.failure() }
            if status == -20 { throw CocoaError(.coderReadCorrupt) }
            throw NSError(domain: NSPOSIXErrorDomain, code: status > 0 ? Int(status) : Int(POSIXErrorCode.EIO.rawValue),
                          userInfo: [NSFilePathErrorKey: file.url.path])
        }
        defer { qb_string_free(output) }
        return try JSONDecoder().decode(LiteSettings.self, from: Data(bytes: output, count: strlen(output)))
    }
}

public enum LegacyKeychain {
    public static func read(_ id: UUID) -> Data? {
        var bytes: UnsafeMutablePointer<UInt8>?
        var length = 0
        guard qb_mac_legacy_read(id.uuidString, &bytes, &length) == 0 else { return nil }
        return NativeCredentials.take(bytes, count: length)
    }
    public static func delete(_ id: UUID) { qb_mac_legacy_delete(id.uuidString) }
}
