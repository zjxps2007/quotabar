import Foundation
import Security
import LocalAuthentication

/// Runs once before monitoring. An inaccessible old login requires reconnection;
/// it must never produce a system password prompt or a new account identity.
public enum CredentialMigration {
    public static func run(settings: LiteSettings, file: SettingsFile, vault: SecretVault,
                           legacyRead: (UUID) -> Data? = LegacyKeychain.read,
                           legacyDelete: (UUID) -> Void = LegacyKeychain.delete) throws -> LiteSettings {
        guard settings.credentialStorageVersion != 1 else { return settings }
        var migrated: [UUID] = []
        for account in settings.accounts where account.provider != .claude {
            if try vault.load(account.id) == nil {
                var data = legacyRead(account.id)
                if !matches(data, account: account) { data = nil }
                if data == nil, !account.usesAppLogin, !account.sourcePath.isEmpty {
                    let url = URL(fileURLWithPath: account.sourcePath)
                    if let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize,
                       size <= 1_048_576, let source = try? Data(contentsOf: url), matches(source, account: account) {
                        data = source
                    }
                }
                if let data { try vault.save(data, id: account.id) }
            }
            if let data = try vault.load(account.id), matches(data, account: account) { migrated.append(account.id) }
        }
        var next = settings; next.credentialStorageVersion = 1
        try file.save(next)
        // Retire only entries whose replacement and migration marker are durable.
        for id in migrated { legacyDelete(id) }
        return next
    }
    private static func matches(_ data: Data?, account: MonitoredAccount) -> Bool {
        guard let data else { return false }
        return (try? ImportedIdentity.parse(data, provider: account.provider).key) == account.identity
    }
}

/// Compatibility access to this app's pre-0.2.1 items only. Never creates items.
public enum LegacyKeychain {
    private static let lock = NSLock()
    private static func query(_ id: UUID) -> [String: Any] {
        let context = LAContext(); context.interactionNotAllowed = true
        return [kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: "com.aiusage.lite.credentials",
                kSecAttrAccount as String: id.uuidString,
                kSecUseAuthenticationContext as String: context]
    }
    public static func read(_ id: UUID) -> Data? {
        withoutUI {
            var query = query(id)
            query[kSecReturnData as String] = true
            query[kSecMatchLimit as String] = kSecMatchLimitOne
            var value: CFTypeRef?
            guard SecItemCopyMatching(query as CFDictionary, &value) == errSecSuccess else { return nil }
            return value as? Data
        } ?? nil
    }
    public static func delete(_ id: UUID) {
        _ = withoutUI { SecItemDelete(query(id) as CFDictionary) }
    }
    private static func withoutUI<T>(_ operation: () -> T) -> T? {
        lock.lock(); defer { lock.unlock() }
        // The modern LAContext setting alone does not cover legacy file keychains.
        // This process-scoped switch is used only during the one-time migration.
        var previous = DarwinBoolean(false)
        guard SecKeychainGetUserInteractionAllowed(&previous) == errSecSuccess,
              SecKeychainSetUserInteractionAllowed(false) == errSecSuccess else { return nil }
        defer { SecKeychainSetUserInteractionAllowed(previous.boolValue) }
        return operation()
    }
}
