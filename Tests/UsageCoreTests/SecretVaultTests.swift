import Foundation
import Testing
@testable import UsageCore

private func inVault(_ body: (URL, SecretVault) throws -> Void) throws {
    let root = FileManager.default.temporaryDirectory.appendingPathComponent("aiusage-vault-\(UUID())")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(at: root) }
    try body(root, SecretVault(directory: root.appendingPathComponent("Credentials")))
}
private func mode(_ url: URL) throws -> Int {
    (try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as! NSNumber).intValue & 0o777
}
private func fixtureLogin() throws -> (MonitoredAccount, Data) {
    let data = try AccountTests().auth(account: "migration-workspace", user: "migration-user")
    let identity = try ImportedIdentity.parse(data, provider: .codex)
    var account = MonitoredAccount(provider: .codex, name: "Fixture", identity: identity.key, sourcePath: "")
    account.appManagedLogin = true
    return (account, data)
}

struct SecretVaultTests {
    @Test func privateFilesPersistRotateAndDeleteIndependently() throws {
        try inVault { _, vault in
            let first = UUID(), second = UUID()
            #expect(try vault.load(first) == nil)
            try vault.save(Data("first".utf8), id: first)
            try vault.save(Data("second".utf8), id: second)
            let reopened = SecretVault(directory: vault.directory)
            #expect(try reopened.load(first) == Data("first".utf8))
            try reopened.save(Data("rotated".utf8), id: first)
            #expect(try vault.load(first) == Data("rotated".utf8))
            #expect(try mode(vault.directory) == 0o700)
            #expect(try mode(vault.directory.appendingPathComponent("\(first).json")) == 0o600)
            try reopened.delete(first); try reopened.delete(first)
            #expect(try vault.load(first) == nil)
            #expect(try vault.load(second) == Data("second".utf8))
            #expect(try FileManager.default.contentsOfDirectory(atPath: vault.directory.path) == ["\(second).json"])
        }
    }

    @Test func oversizedWritesAndReadsAreRejectedWithoutLosingPreviousValue() throws {
        try inVault { _, vault in
            let id = UUID(), before = Data("preserve".utf8)
            try vault.save(before, id: id)
            #expect(throws: (any Error).self) { try vault.save(Data(repeating: 1, count: 1_048_577), id: id) }
            #expect(try vault.load(id) == before)
            try Data(repeating: 1, count: 1_048_577).write(to: vault.directory.appendingPathComponent("\(id).json"))
            #expect(throws: (any Error).self) { try vault.load(id) }
        }
    }

    @Test func symbolicAndHardLinksCannotExposeAnotherFile() throws {
        try inVault { root, vault in
            let id = UUID(), target = root.appendingPathComponent("outside")
            let link = vault.directory.appendingPathComponent("\(id).json")
            try vault.save(Data("initial".utf8), id: UUID())
            let outside = Data("do-not-touch".utf8); try outside.write(to: target)
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: target)
            #expect(throws: (any Error).self) { try vault.load(id) }
            try vault.save(Data("replacement".utf8), id: id)
            #expect(try Data(contentsOf: target) == outside)
            #expect(try vault.load(id) == Data("replacement".utf8))
            try vault.delete(id)
            try FileManager.default.linkItem(at: target, to: link)
            #expect(throws: (any Error).self) { try vault.load(id) }
            try vault.delete(id)
            #expect(try Data(contentsOf: target) == outside)
        }
    }

    @Test func linkedDirectoriesAreRejectedAndFailedPublicationCleansTemporaryFile() throws {
        try inVault { root, vault in
            let target = root.appendingPathComponent("outside-directory")
            try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o755])
            try FileManager.default.createSymbolicLink(at: vault.directory, withDestinationURL: target)
            #expect(throws: (any Error).self) { try vault.save(Data("secret".utf8), id: UUID()) }
            #expect(try mode(target) == 0o755)
            #expect(try FileManager.default.contentsOfDirectory(atPath: target.path).isEmpty)
            try FileManager.default.removeItem(at: vault.directory)
            let id = UUID(), destination = vault.directory.appendingPathComponent("\(id).json")
            try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
            #expect(throws: (any Error).self) { try vault.save(Data("secret".utf8), id: id) }
            #expect(try FileManager.default.contentsOfDirectory(atPath: vault.directory.path) == ["\(id).json"])
        }
    }

    @Test func migrationCommitsBeforeDeletingLegacyAndNeverRunsTwice() throws {
        try inVault { root, vault in
            let (account, data) = try fixtureLogin()
            var settings = LiteSettings(); settings.accounts = [account]
            let file = SettingsFile(url: root.appendingPathComponent("settings.json"))
            var reads = 0, deletes = 0
            let migrated = try CredentialMigration.run(settings: settings, file: file, vault: vault,
                legacyRead: { id in reads += 1; #expect(id == account.id); return data },
                legacyDelete: { id in
                    deletes += 1
                    #expect((try? vault.load(id)) == data)
                    #expect((try? file.load().credentialStorageVersion) == 1)
                })
            #expect(reads == 1 && deletes == 1)
            #expect(migrated.accounts == settings.accounts)
            #expect(try file.load().credentialStorageVersion == 1)
            #expect(!String(decoding: try Data(contentsOf: file.url), as: UTF8.self).contains("access_token"))
            try vault.delete(account.id)
            _ = try CredentialMigration.run(settings: file.load(), file: file, vault: vault,
                legacyRead: { _ in Issue.record("Migration repeated"); return data },
                legacyDelete: { _ in Issue.record("Deletion repeated") })
            #expect(try vault.load(account.id) == nil)
        }
    }

    @Test func inaccessibleLegacyPreservesCardsWithoutRetryOrAppLoginCLIFallback() throws {
        try inVault { root, vault in
            var (account, data) = try fixtureLogin()
            let source = root.appendingPathComponent("cli.json"); try data.write(to: source)
            account.sourcePath = source.path; account.enabled = false
            var settings = LiteSettings(); settings.accounts = [account]
            let file = SettingsFile(url: root.appendingPathComponent("settings.json"))
            let migrated = try CredentialMigration.run(settings: settings, file: file, vault: vault,
                legacyRead: { _ in nil }, legacyDelete: { _ in Issue.record("Uncopied credentials deleted") })
            #expect(try vault.load(account.id) == nil)
            #expect(migrated.accounts == [account])
            #expect(migrated.credentialStorageVersion == 1)
        }
    }

    @Test func onlyMatchingFileImportCanRecoverUnavailableLegacy() throws {
        try inVault { root, vault in
            var (account, data) = try fixtureLogin()
            let wrong = try AccountTests().auth(account: "other-workspace", user: "other-user")
            let source = root.appendingPathComponent("cli.json"); try data.write(to: source)
            account.appManagedLogin = false; account.sourcePath = source.path
            var settings = LiteSettings(); settings.accounts = [account]
            let file = SettingsFile(url: root.appendingPathComponent("settings.json"))
            _ = try CredentialMigration.run(settings: settings, file: file, vault: vault,
                legacyRead: { _ in wrong }, legacyDelete: { _ in })
            #expect(try vault.load(account.id) == data)
            try vault.delete(account.id); try wrong.write(to: source)
            _ = try CredentialMigration.run(settings: settings, file: file, vault: vault,
                legacyRead: { _ in wrong }, legacyDelete: { _ in Issue.record("Mismatched login deleted") })
            #expect(try vault.load(account.id) == nil)
        }
    }

    @Test func failedMigrationCommitRetainsLegacyAndRetryKeepsNewerLocalCredentials() throws {
        try inVault { root, vault in
            let (account, data) = try fixtureLogin()
            var settings = LiteSettings(); settings.accounts = [account]
            let obstruction = root.appendingPathComponent("not-a-folder"); try Data().write(to: obstruction)
            let blocked = SettingsFile(url: obstruction.appendingPathComponent("settings.json"))
            #expect(throws: (any Error).self) {
                try CredentialMigration.run(settings: settings, file: blocked, vault: vault,
                    legacyRead: { _ in data }, legacyDelete: { _ in Issue.record("Deleted before commit") })
            }
            #expect(try vault.load(account.id) == data)
            var json = try JSONSerialization.jsonObject(with: data) as! [String: Any]
            json["last_refresh"] = "newer-local-value"
            let newer = try JSONSerialization.data(withJSONObject: json)
            try vault.save(newer, id: account.id)
            let file = SettingsFile(url: root.appendingPathComponent("settings.json"))
            _ = try CredentialMigration.run(settings: settings, file: file, vault: vault,
                legacyRead: { _ in Issue.record("Read stale login over existing file"); return data }, legacyDelete: { _ in })
            #expect(try vault.load(account.id) == newer)
        }
    }
}
