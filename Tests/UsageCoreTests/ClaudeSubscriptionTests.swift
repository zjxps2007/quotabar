// Derived from sylearn/AIUsage, Apache-2.0. Selected tests retained for Lite.
import Testing
import Foundation
@testable import UsageCore

final class ClaudeSubscriptionTests{
    private func fixture() throws -> (URL, ClaudeSubscriptionStore, String) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("claude-subscription-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return (root, ClaudeSubscriptionStore(root: root.appendingPathComponent("snapshots")), root.appendingPathComponent("official profile").path)
    }

    private func input(used: Double = 23.5, reset: Date = Date().addingTimeInterval(3600), weekly: Bool = true) throws -> Data {
        var limits: [String: Any] = ["five_hour": ["used_percentage": used, "resets_at": reset.timeIntervalSince1970]]
        if weekly { limits["seven_day"] = ["used_percentage": 41.2, "resets_at": Date().addingTimeInterval(86400).timeIntervalSince1970] }
        return try JSONSerialization.data(withJSONObject: ["session_id": "ABCDEF01-0000-4000-8000-000000000001", "rate_limits": limits,
                                                           "cwd": "/private/project", "accessToken": "DO_NOT_PERSIST", "cost": ["total_cost_usd": 42]])
    }

    @Test func testInstallAndRestoreOnlyStatusLineAndKeepUserEdits() throws {
        let (root, store, path) = try fixture()
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(atPath: path, withIntermediateDirectories: true)
        let url = URL(fileURLWithPath: path).appendingPathComponent("settings.json")
        let original: [String: Any] = ["statusLine": ["type": "command", "command": "printf original", "padding": 3],
                                     "env": ["ANTHROPIC_BASE_URL": "https://proxy.invalid", "KEEP": "yes"], "hooks": ["keep": true], "effortLevel": "high"]
        try JSONSerialization.data(withJSONObject: original).write(to: url)
        let connection = ClaudeSubscriptionConnection(store: store)
        let first = try connection.install(directory: path, name: "Personal", helperPath: "/Applications/AI Usage.app/Contents/Helpers/QuotaServer")
        let second = try connection.install(directory: path, name: "Personal", helperPath: "/Applications/AIUsage.app/Contents/Helpers/QuotaServer")
        checkEqual(first.generation, second.generation)
        checkEqual(first.originalStatusLine, second.originalStatusLine)
        try connection.rollbackInstallation(second, previous: first)
        checkEqual(try store.loadProfile(id: first.id).installedCommand, first.installedCommand)
        var current = try requireValue(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        checkEqual((current["env"] as? [String: String])?["KEEP"], "yes")
        checkEqual(current["effortLevel"] as? String, "high")
        current["newPreference"] = true
        var currentStatusLine = try requireValue(current["statusLine"] as? [String: Any])
        currentStatusLine["padding"] = 5
        current["statusLine"] = currentStatusLine
        try JSONSerialization.data(withJSONObject: current).write(to: url)
        checkTrue(try connection.uninstall(profileID: first.id))
        current = try requireValue(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        checkEqual((current["statusLine"] as? [String: Any])?["command"] as? String, "printf original")
        checkEqual((current["statusLine"] as? [String: Any])?["padding"] as? Int, 5)
        checkEqual(current["newPreference"] as? Bool, true)
        _ = try connection.install(directory: path, name: "Personal", helperPath: "/tmp/helper")
        current["statusLine"] = ["type": "command", "command": "printf changed"]
        try JSONSerialization.data(withJSONObject: current).write(to: url)
        checkFalse(try connection.uninstall(profileID: first.id))
        checkEqual((try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])?["statusLine"] as? [String: String], ["type": "command", "command": "printf changed"])
    }

    @Test func testWhitespacePathProfilesAreIsolatedAndSnapshotsAreWhitelisted() throws {
        let (root, store, path) = try fixture()
        defer { try? FileManager.default.removeItem(at: root) }
        let profile = try ClaudeSubscriptionConnection(store: store).install(directory: path, name: "Personal", helperPath: "/tmp/helper")
        let other = ClaudeSubscriptionProfile(configDirectory: root.appendingPathComponent("work").path, name: "Work")
        checkNotEqual(profile.id, other.id)
        let sample = try requireValue(ClaudeSubscriptionStatusLine.snapshot(input: input(), profile: profile))
        try store.saveSnapshot(sample)
        let json = String(decoding: try JSONEncoder().encode(sample), as: UTF8.self)
        checkFalse(json.contains("DO_NOT_PERSIST"))
        checkFalse(json.contains("/private/project"))
        checkFalse(json.contains("total_cost"))
        checkNil(try store.latestSnapshot(for: other))
        let attrs = try FileManager.default.attributesOfItem(atPath: store.directory(for: profile.id).appendingPathComponent("snapshot-\(sample.sessionID).json").path)
        checkEqual((attrs[.posixPermissions] as? NSNumber)?.intValue, 0o600)
    }

    @Test func testRepeatedFeedbackDoesNotRenewEvidenceAndGenerationInvalidatesOldLogin() throws {
        let (root, store, path) = try fixture()
        defer { try? FileManager.default.removeItem(at: root) }
        var profile = try ClaudeSubscriptionConnection(store: store).install(directory: path, name: "Personal", helperPath: "/tmp/helper")
        let payload = try input()
        let past = Date().addingTimeInterval(-600)
        try store.saveSnapshot(requireValue(ClaudeSubscriptionStatusLine.snapshot(input: payload, profile: profile, now: past)))
        try store.saveSnapshot(requireValue(ClaudeSubscriptionStatusLine.snapshot(input: payload, profile: profile)))
        let sample = try requireValue(store.latestSnapshot(for: profile))
        checkEqual(sample.observedAt.timeIntervalSince1970, past.timeIntervalSince1970, accuracy: 0.01)
        checkGreater(sample.receivedAt, sample.observedAt)
        profile = try ClaudeSubscriptionConnection(store: store).install(directory: path, name: "Personal", helperPath: "/tmp/helper", renewBinding: true)
        checkNil(try store.latestSnapshot(for: profile))
    }

    @Test func testZeroAndFullUsageMissingWeeklyAndMalformedValues() throws {
        let profile = ClaudeSubscriptionProfile(configDirectory: "/tmp/official-test", name: "Test")
        checkEqual(try ClaudeSubscriptionStatusLine.snapshot(input: input(used: 0, weekly: false), profile: profile)?.fiveHour?.usedPercent, 0)
        checkNil(try ClaudeSubscriptionStatusLine.snapshot(input: input(weekly: false), profile: profile)?.sevenDay)
        checkEqual(try ClaudeSubscriptionStatusLine.snapshot(input: input(used: 100), profile: profile)?.fiveHour?.usedPercent, 100)
        checkThrows(try ClaudeSubscriptionStatusLine.snapshot(input: input(used: 101), profile: profile))
        checkThrows(try ClaudeSubscriptionStatusLine.snapshot(input: Data("{}".utf8), profile: profile))
    }

    @Test func testCLIEnvironmentDropsAPIRoutingButKeepsNetworkProxy() throws {
        checkThrows(try ClaudeSubscriptionConnection.validatedDirectory(""))
        checkThrows(try ClaudeSubscriptionConnection.validatedDirectory("/"))
        checkThrows(try ClaudeSubscriptionConnection.validatedDirectory("/Users/.."))
        checkThrows(try ClaudeSubscriptionConnection.validatedDirectory("relative"))
        let env = ClaudeSubscriptionLaunch.environment(
            ["ANTHROPIC_BASE_URL": "https://proxy.invalid", "ANTHROPIC_AUTH_TOKEN": "secret", "ANTHROPIC_MODEL": "proxy-model",
             "HTTPS_PROXY": "http://127.0.0.1:7890", "PATH": "/usr/bin"],
            configDirectory: "/tmp/it's a profile")
        checkNil(env["ANTHROPIC_BASE_URL"])
        checkNil(env["ANTHROPIC_AUTH_TOKEN"])
        checkNil(env["ANTHROPIC_MODEL"])
        checkEqual(env["HTTPS_PROXY"], "http://127.0.0.1:7890")
        checkEqual(env["PATH"], "/usr/bin")
        checkEqual(env["CLAUDE_CONFIG_DIR"], "/tmp/it's a profile")
        checkEqual(ClaudeSubscriptionLaunch.terminalCommand(configDirectory: "/tmp/it's a profile"),
                       "CLAUDE_CONFIG_DIR='/tmp/it'\\''s a profile' claude")
        // 默认目录若显式设置 CLAUDE_CONFIG_DIR，官方 CLI 会改读另一份登录；必须去掉，包括继承来的值。
        let home = FileManager.default.temporaryDirectory.appendingPathComponent("home-\(UUID().uuidString)").path
        let defaultEnv = ClaudeSubscriptionLaunch.environment(["CLAUDE_CONFIG_DIR": "\(home)/.claude", "PATH": "/usr/bin"],
                                                              configDirectory: "\(home)/.claude/", homeDirectory: home)
        checkNil(defaultEnv["CLAUDE_CONFIG_DIR"])
        checkEqual(defaultEnv["PATH"], "/usr/bin")
        checkEqual(ClaudeSubscriptionLaunch.terminalCommand(configDirectory: "\(home)/.claude", homeDirectory: home), "claude")
        checkEqual(ClaudeSubscriptionLaunch.environment([:], configDirectory: "\(home)/.claude-aiusage/accounts/a1", homeDirectory: home)["CLAUDE_CONFIG_DIR"],
                       "\(home)/.claude-aiusage/accounts/a1")
        let settings = try requireValue(JSONSerialization.jsonObject(with: Data(ClaudeSubscriptionLaunch.settingsOverride().utf8)) as? [String: Any])
        checkEqual((settings["env"] as? [String: String])?["ANTHROPIC_AUTH_TOKEN"], "")
        checkEqual(settings["apiKeyHelper"] as? String, "")
    }

    @Test func testAuthStatusAcceptsOnlySubscriptionLoginAndKeepsIdentityFields() throws {
        let pro = try ClaudeAuthStatus(json: Data(#"""
        {"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","configDirectory":"/Users/me/.claude",
         "email":"me@example.com","orgId":"org-1","orgName":"Me","subscriptionType":"pro","projectsDirectory":"/x"}
        """#.utf8))
        checkTrue(pro.isSubscription)
        checkEqual(pro.email, "me@example.com")
        checkEqual(pro.organizationName, "Me")
        checkEqual(pro.planLabel, "Pro")
        checkEqual(pro.configDirectory, "/Users/me/.claude")
        checkEqual(ClaudeAuthStatus.planLabel("max"), "Max")
        checkNil(ClaudeAuthStatus.planLabel("  "))
        checkFalse(try ClaudeAuthStatus(json: Data(#"{"loggedIn":true,"authMethod":"console"}"#.utf8)).isSubscription)
        checkFalse(try ClaudeAuthStatus(json: Data(#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"bedrock"}"#.utf8)).isSubscription)
        checkFalse(try ClaudeAuthStatus(json: Data(#"{"loggedIn":false}"#.utf8)).isSubscription)
        checkThrows(try ClaudeAuthStatus(json: Data("[]".utf8)))
    }

    @Test func testFullSettingsPreservesOwnedFeedbackButDoesNotResurrectDisconnectedBackup() throws {
        let (root, store, path) = try fixture(); defer { try? FileManager.default.removeItem(at: root) }
        let connection = ClaudeSubscriptionConnection(store: store)
        let profile = try connection.install(directory: path, name: "测试", helperPath: "/tmp/helper")
        let settingsURL = URL(fileURLWithPath: profile.configDirectory).appendingPathComponent("settings.json")
        let current = try requireValue(JSONSerialization.jsonObject(with: Data(contentsOf: settingsURL)) as? [String: Any])
        let incoming: [String: Any] = ["model": "sonnet", "env": ["KEEP": "yes"]]
        let replaced = try connection.preservingStatusLine(in: incoming, current: current, directory: profile.configDirectory)
        checkEqual((replaced["statusLine"] as? [String: Any])?["command"] as? String, profile.installedCommand)
        checkEqual(replaced["model"] as? String, "sonnet")
        checkEqual((replaced["env"] as? [String: String])?["KEEP"], "yes")
        checkTrue(try connection.uninstall(profileID: profile.id))
        let disconnected = try requireValue(JSONSerialization.jsonObject(with: Data(contentsOf: settingsURL)) as? [String: Any])
        let restored = try connection.preservingStatusLine(in: current, current: disconnected, directory: profile.configDirectory)
        checkNil(restored["statusLine"])
        var userChanged = current
        userChanged["statusLine"] = ["type": "command", "command": "printf custom"]
        let preservedUser = try connection.preservingStatusLine(in: current, current: userChanged, directory: profile.configDirectory)
        checkEqual((preservedUser["statusLine"] as? [String: String])?["command"], "printf custom")
    }

    @Test func testMissingOrCorruptProfileDoesNotWrapFeedbackOrOverwriteSettings() throws {
        for corrupt in [false, true] {
            let (root, store, path) = try fixture(); defer { try? FileManager.default.removeItem(at: root) }
            let connection = ClaudeSubscriptionConnection(store: store)
            let profile = try connection.install(directory: path, name: "测试", helperPath: "/tmp/helper")
            let settings = URL(fileURLWithPath: path).appendingPathComponent("settings.json")
            let before = try Data(contentsOf: settings)
            let profileURL = try store.directory(for: profile.id).appendingPathComponent("profile.json")
            if corrupt { try Data("invalid-json".utf8).write(to: profileURL) }
            else { try FileManager.default.removeItem(at: profileURL) }
            checkThrows(try connection.install(directory: path, name: "重连", helperPath: "/tmp/new-helper", renewBinding: true))
            checkEqual(try Data(contentsOf: settings), before)
            if corrupt { checkEqual(try Data(contentsOf: profileURL), Data("invalid-json".utf8)) }
            else { checkFalse(FileManager.default.fileExists(atPath: profileURL.path)) }
        }
    }

    @Test func testRestoredOldWrapperUsesKnownOriginalInsteadOfWrappingItAgain() throws {
        let (root, store, path) = try fixture(); defer { try? FileManager.default.removeItem(at: root) }
        let settings = URL(fileURLWithPath: path).appendingPathComponent("settings.json")
        try FileManager.default.createDirectory(atPath: path, withIntermediateDirectories: true)
        try JSONSerialization.data(withJSONObject: ["statusLine": ["type": "command", "command": "printf original"]]).write(to: settings)
        let connection = ClaudeSubscriptionConnection(store: store)
        let first = try connection.install(directory: path, name: "测试", helperPath: "/tmp/helper")
        let oldSettings = try Data(contentsOf: settings)
        _ = try connection.install(directory: path, name: "测试", helperPath: "/tmp/new-helper", renewBinding: true)
        try oldSettings.write(to: settings)
        let reconnected = try connection.install(directory: path, name: "重连", helperPath: "/tmp/new-helper", renewBinding: true)
        checkEqual(reconnected.originalStatusLine, first.originalStatusLine)
        checkNotEqual(reconnected.installedCommand, first.installedCommand)
        checkTrue(try connection.uninstall(profileID: reconnected.id))
        let restored = try requireValue(JSONSerialization.jsonObject(with: Data(contentsOf: settings)) as? [String: Any])
        checkEqual((restored["statusLine"] as? [String: String])?["command"], "printf original")
        // 已停用后手动恢复旧 wrapper，仍使用受保护的原命令备份。
        try oldSettings.write(to: settings)
        let recovered = try connection.install(directory: path, name: "重连", helperPath: "/tmp/new-helper", renewBinding: true)
        checkEqual(recovered.originalStatusLine, first.originalStatusLine)
    }

}
