import Foundation
import Testing
@testable import UsageCore

struct NativeSettingsTests {
    @Test func importedCredentialsKeepTheSavedAccountUntilRefreshCommits() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = dir.appendingPathComponent("로그인.json")
        let vault = SecretVault(directory: dir.appendingPathComponent("Credentials"))
        let account = MonitoredAccount(provider: .gemini, name: "저장된 계정", identity: "gemini:email:me@example.test", sourcePath: file.path)
        let saved = Data(#"{"access_token":"old","email":"me@example.test"}"#.utf8)
        let updated = Data(#"{"access_token":"new","email":"me@example.test"}"#.utf8)
        try updated.write(to: file)
        checkThrows(try vault.imported(account))
        try vault.save(saved, id: account.id)
        let match = try vault.imported(account)
        checkTrue(match.updated); checkNil(match.note); checkEqual(match.data, updated)
        checkEqual(try vault.load(account.id), saved)
        try Data(#"{"access_token":"other","email":"other@example.test"}"#.utf8).write(to: file)
        let switched = try vault.imported(account)
        checkFalse(switched.updated); checkEqual(switched.data, saved)
        checkEqual(switched.note, "CLI가 다른 계정으로 전환되어 보관된 로그인을 사용합니다.")
    }

    @Test func diskFormatMatchesFoundationInBothDirections() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = SettingsFile(url: dir.appendingPathComponent("settings.json"))
        let row = #"{"id":"492c628f-ec48-4b07-bf81-a2a5cb8c7254","name":"별칭 🍏","provider":"codex","identity":"p:u","sourcePath":"/tmp/{,}","createdAt":-12345.123456789,"enabled":true}"#
        let base = #"{"accounts":[ROW],"refreshSeconds":300,"scanLocalLogs":true}"#.replacingOccurrences(of: "ROW", with: row)
        func compare(_ data: Data, _ name: String) throws {
            try data.write(to: file.url)
            let reference = try? JSONDecoder().decode(LiteSettings.self, from: data)
            let native = try? file.load()
            checkEqual(reference, native, file: name)
            checkEqual(try Data(contentsOf: file.url), data, file: name)
            if let reference {
                try file.save(reference)
                let saved = try Data(contentsOf: file.url)
                checkEqual(try JSONDecoder().decode(LiteSettings.self, from: saved), reference, file: name)
                let original = try JSONEncoder().encode(reference)
                let originalObject = try JSONSerialization.jsonObject(with: original) as? NSDictionary
                let nativeObject = try JSONSerialization.jsonObject(with: saved) as? NSDictionary
                checkEqual(nativeObject, originalObject, file: name)
                try original.write(to: file.url)
                checkEqual(try file.load(), reference, file: name)
            }
        }
        let replacements = [
            ("300", "3e2"), ("300", "300.0"), ("300","300.25"), ("300","-1"),
            ("300","9223372036854775807"), ("300","-9223372036854775808"),
            ("300","9223372036854775807.0"), ("300","9.007199254740993e15"),
            ("300","-9223372036854775808.0"), ("300","-9223372036854775807.0"),
            ("300","9007199254740993.5"), ("300","9007199254740993.00001"),
            ("300","9223372036854775808"), ("300","\"300\""), ("300","false"), ("300","null"),
            ("300", "300,\"refreshSeconds\":null"), ("300", "null,\"refreshSeconds\":300"),
            (#""name":"별칭 🍏""#, #""name":"먼저","name":false"#),
            ("true}", "true,}"), (row + "]", row + ",]"), (row + "]", row + ",,]"),
            (#""enabled":true"#, #""enabled":true,"showInMenuBar":null,"appManagedLogin":false"#),
            (#""enabled":true"#, #""enabled":"true""#), (#""enabled":true"#, #""enabled":1"#),
            (#""codex""#, #""Codex""#), (#""codex""#, #"{"codex":null}"#),
            ("492c628f-ec48-4b07-bf81-a2a5cb8c7254","492c628fec484b07bf81a2a5cb8c7254"),
            ("-12345.123456789", "1.1754943508222875e-38"), ("-12345.123456789","\"0\""),
            (#""scanLocalLogs":true"#, #""scanLocalLogs":true,"appearance":"future","menuBarDisplayMode":"future""#),
            (#""scanLocalLogs":true"#, #""scanLocalLogs":true,"appearance":{}"#),
            (#""scanLocalLogs":true"#, #""scanLocalLogs":true,"menuBarDisplayMode":null,"credentialStorageVersion":1e0"#),
            (#""scanLocalLogs":true"#, #""scanLocalLogs":true,"future":{"keep":[1,2,3]}"#),
        ]
        for (index, pair) in replacements.enumerated() {
            try compare(Data(base.replacingOccurrences(of: pair.0, with: pair.1).utf8), "settings-case-\(index)")
        }
        for encoding: String.Encoding in [.utf8, .utf16LittleEndian, .utf16BigEndian, .utf32LittleEndian, .utf32BigEndian] {
            let bytes = try requireValue(base.data(using: encoding))
            try compare(bytes, "encoding-\(encoding.rawValue)")
            let mark: [UInt8]
            switch encoding {
            case .utf8: mark = [0xef, 0xbb, 0xbf]
            case .utf16LittleEndian: mark = [0xff, 0xfe]
            case .utf16BigEndian: mark = [0xfe, 0xff]
            case .utf32LittleEndian: mark = [0xff, 0xfe, 0, 0]
            default: mark = [0, 0, 0xfe, 0xff]
            }
            try compare(Data(mark) + bytes, "bom-\(encoding.rawValue)")
            if encoding == .utf32LittleEndian {
                try compare(Data([0xfe, 0xff, 0, 0]) + bytes, "foundation-utf32-bom")
            }
        }
        for invalid in ["{}", "[]", "null", #"{"accounts":[,],"refreshSeconds":300,"scanLocalLogs":true}"#] {
            try compare(Data(invalid.utf8), invalid)
        }
    }

    @Test func nativeAccountEditsPreserveIdentityOrderingAndRollback() throws {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let file = SettingsFile(url: dir.appendingPathComponent("settings.json"))
        let vault = SecretVault(directory: dir.appendingPathComponent("Credentials"))
        let login = Data(#"{"access_token":"old","refresh_token":"stay","account_id":"a","user_id":"u"}"#.utf8)
        let rotation = Data(#"{"access_token":"new","refresh_token":"stay","account_id":"a","user_id":"u"}"#.utf8)
        let source = dir.appendingPathComponent("auth.json")
        try login.write(to: source)
        var state = LiteSettings()
        var original = MonitoredAccount(provider: .codex, name: "별칭", identity: "codex:account:a:user:u", sourcePath: "old",
                                        enabled: false, createdAt: Date(timeIntervalSinceReferenceDate: -12345.123456789))
        original.showInMenuBar = false
        let other = MonitoredAccount(provider: .claude, name: "Claude", identity: "é", sourcePath: "/old")
        state.accounts = [other, original]
        func apply(_ edit: SettingsEdit, bytes: Data = Data()) throws -> SettingsChange {
            let result = try file.apply(edit, to: state, storage: .disk, vault: vault, credential: bytes)
            state = result.settings
            return result
        }
        checkFalse(try apply(.autoRefresh(enabled: true)).changed)
        checkFalse(FileManager.default.fileExists(atPath: file.url.path))
        let missing = dir.appendingPathComponent("missing-login.json")
        func failure(_ op: () throws -> Void) -> NSError? {
            do { try op(); return nil } catch { return error as NSError }
        }
        let previousError = try requireValue(failure { _ = try FileManager.default.attributesOfItem(atPath: missing.path) })
        let nativeError = try requireValue(failure { _ = try apply(.importLogin(provider: .codex, path: missing.path, alias: "")) })
        checkEqual(nativeError.domain, previousError.domain)
        checkEqual(nativeError.code, previousError.code)
        checkEqual(nativeError.userInfo[NSFilePathErrorKey] as? String, previousError.userInfo[NSFilePathErrorKey] as? String)
        checkFalse(FileManager.default.fileExists(atPath: file.url.path))
        let imported = try requireValue(try apply(.importLogin(provider: .codex, path: source.path, alias: "")).account)
        checkEqual(imported.id, original.id)
        checkEqual(imported.name, original.name)
        checkEqual(imported.createdAt, original.createdAt)
        checkEqual(imported.showInMenuBar, false)
        checkEqual(imported.appManagedLogin, false)
        checkTrue(imported.enabled)
        checkEqual(try vault.load(original.id), login)
        let broken = SettingsFile(url: dir)
        checkThrows(try broken.apply(.managedLogin(alias:"새 이름", target:imported), to:state, storage:.disk,
                                     vault:vault, credential:rotation))
        checkEqual(try vault.load(original.id), login)
        checkThrows(try apply(.managedLogin(alias: "", target: other), bytes: rotation))
        let managed = try requireValue(try apply(.managedLogin(alias: "새 이름", target: imported), bytes: rotation).account)
        checkEqual(managed.id, original.id)
        checkEqual(managed.name, "새 이름")
        checkEqual(managed.sourcePath, "")
        checkEqual(managed.appManagedLogin, true)
        checkEqual(try vault.load(original.id), rotation)
        let claude = try requireValue(try apply(.claude(identity: "e\u{301}", sourcePath: "/new", label: "Claude · new", alias: "")).account)
        checkEqual(claude, other)
        checkEqual(state.accounts.map(\.id), [other.id, original.id])
        checkThrows(try broken.apply(.remove(id: original.id, deleteCredential:true), to:state, storage:.disk, vault:vault))
        checkEqual(try vault.load(original.id), rotation)
        _ = try apply(.remove(id: original.id, deleteCredential: true))
        checkEqual(try file.load().accounts, [other])
        checkNil(try vault.load(original.id))
    }

    @Test func claudeAccountFileAndOfficialCommandRecoverTogether() throws {
        let fm = FileManager.default
        let dir = fm.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? fm.removeItem(at: dir) }
        let cli = dir.appendingPathComponent("Profile's 설정")
        try fm.createDirectory(at: cli, withIntermediateDirectories: true)
        let config = cli.appendingPathComponent("settings.json")
        try Data(#"{"keep":31,"statusLine":{"type":"command","command":"my original","padding":3}}"#.utf8).write(to: config)
        let store = ClaudeSubscriptionStore(root: dir.appendingPathComponent("ClaudeProfiles"))
        let file = SettingsFile(url: dir.appendingPathComponent("settings.json"))
        let blocked = SettingsFile(url: dir.appendingPathComponent("cannot-replace-directory"))
        try fm.createDirectory(at: blocked.url, withIntermediateDirectories: true)
        let bind = SettingsEdit.bindClaude(directory: cli.path, helper: "/Applications/QuotaBar.app/Contents/MacOS/QuotaBar", alias: "")
        let result = try file.apply(bind, to: LiteSettings(), storage: .disk)
        let account = try requireValue(result.account)
        let owned = try store.loadProfile(id: account.identity)
        checkEqual(account.name, "Claude · Profile's 설정")
        checkEqual(account.sourcePath, cli.path)
        let remove = SettingsEdit.removeClaude(id: account.id, identity: account.identity)
        checkThrows(try blocked.apply(.bindClaude(directory: cli.path, helper: "/moved", alias: "new"),
                                      to: result.settings, storage: .disk))
        checkEqual(try store.loadProfile(id: account.identity), owned)
        checkThrows(try blocked.apply(remove, to: result.settings, storage: .disk))
        checkEqual(try store.loadProfile(id: account.identity), owned)
        let bound = try requireValue(JSONSerialization.jsonObject(with: Data(contentsOf: config)) as? [String: Any])
        checkEqual((bound["statusLine"] as? [String: Any])?["command"] as? String, owned.installedCommand)
        _ = try file.apply(remove, to: result.settings, storage: .disk)
        checkTrue(try file.load().accounts.isEmpty)
        checkFalse(fm.fileExists(atPath: try store.directory(for: account.identity).path))
        let restored = try requireValue(JSONSerialization.jsonObject(with: Data(contentsOf: config)) as? [String: Any])
        checkEqual((restored["statusLine"] as? [String: Any])?["command"] as? String, "my original")
        checkEqual(restored["keep"] as? Int, 31)
    }
}
