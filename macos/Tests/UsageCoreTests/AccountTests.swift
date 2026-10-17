import Testing
import Foundation
@testable import UsageCore

final class AccountTests{
    func auth(account: String, user: String, email: String = "same@example.com") throws -> Data {
        let claims: [String: Any] = ["email": email, "https://api.openai.com/auth": ["chatgpt_account_id": account, "chatgpt_user_id": user]]
        let encoded = try JSONSerialization.data(withJSONObject: claims).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
        return try JSONSerialization.data(withJSONObject: ["tokens": ["access_token": "test.\(encoded).signature", "account_id": account]])
    }
    @Test func testSameEmailDifferentWorkspaceAndUserStaySeparate() throws {
        let a = try ImportedIdentity.parse(auth(account: "personal", user: "u1"), provider: .codex)
        let b = try ImportedIdentity.parse(auth(account: "business", user: "u1"), provider: .codex)
        let c = try ImportedIdentity.parse(auth(account: "personal", user: "u2"), provider: .codex)
        checkNotEqual(a.key, b.key); checkNotEqual(a.key, c.key)
        checkEqual(a.label, b.label)
    }
    @Test func testCodexIdentitySurvivesTokenRotation() throws {
        let a = try auth(account: "personal", user: "u1")
        var json = try requireValue(JSONSerialization.jsonObject(with: a) as? [String: Any])
        json["last_refresh"] = "changed"
        checkEqual(try ImportedIdentity.parse(a, provider: .codex),
                       try ImportedIdentity.parse(JSONSerialization.data(withJSONObject: json), provider: .codex))
    }
    @Test func testAPIKeyAndIncompleteIdentityRejected() {
        checkThrows(try ImportedIdentity.parse(Data(#"{"OPENAI_API_KEY":"not-a-login"}"#.utf8), provider: .codex))
        checkThrows(try ImportedIdentity.parse(Data(#"{"access_token":"test","email":"a@b.com"}"#.utf8), provider: .codex))
    }
    @Test func testGeminiIdentityDoesNotUseAccessToken() throws {
        let a = Data(#"{"access_token":"one","refresh_token":"stable"}"#.utf8)
        let b = Data(#"{"access_token":"two","refresh_token":"stable"}"#.utf8)
        let c = Data(#"{"access_token":"one","refresh_token":"another"}"#.utf8)
        checkEqual(try ImportedIdentity.parse(a, provider: .gemini), try ImportedIdentity.parse(b, provider: .gemini))
        checkNotEqual(try ImportedIdentity.parse(a, provider: .gemini), try ImportedIdentity.parse(c, provider: .gemini))
    }
    @Test func testMalformedAndOversizedLoginRejected() {
        checkThrows(try ImportedIdentity.parse(Data("not JSON".utf8), provider: .codex))
        checkThrows(try ImportedIdentity.parse(Data(repeating: 32, count: 1_048_577), provider: .gemini))
    }
    @Test func testCodexDurationLabelsFollowAPIAndKeepWorkspaceAnchor() {
        let provider = CodexProvider()
        let response: [String: Any] = ["account_id": "old-user", "plan_type": "plus", "rate_limit": ["primary_window": ["used_percent": 25.0, "limit_window_seconds": 604800, "reset_at": 1900000000.0]]]
        let raw = provider.parseResponse(response, accountId: "workspace", source: SourceInfo(mode: "test", type: "test"), fallbackEmail: nil)
        let summary = AccountSnapshot(usage: raw, provider: .codex)
        checkEqual(raw.usageAccountId, "workspace")
        checkEqual(summary.windows.first?.title, "Weekly Window")
        checkEqual(summary.windows.first?.remaining, 75)
    }
    @Test func testUnknownQuotaIsNotReportedAsFull() {
        let snapshot = AccountSnapshot(usage: ProviderUsage(provider: "codex", label: "Codex"), provider: .codex)
        checkTrue(snapshot.windows.isEmpty); checkNil(snapshot.lowestRemaining)
    }
    @Test func testInvalidPercentIsOmittedAndValidPercentClamped() {
        var usage = ProviderUsage(provider: "codex", label: "Codex")
        var first = RawQuotaWindow(); first.remainingPercent = .nan
        var second = RawQuotaWindow(); second.remainingPercent = 120
        usage.primary = first; usage.secondary = second
        let result = AccountSnapshot(usage: usage, provider: .codex)
        checkEqual(result.windows.count, 1); checkEqual(result.lowestRemaining, 100)
    }
    @Test func testSettingsRoundTripAndCorruptionDoesNotOverwrite() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let file = SettingsFile(url: root.appendingPathComponent("settings.json"))
        var settings = LiteSettings()
        settings.accounts.append(MonitoredAccount(provider: .codex, name: "Personal", identity: "p:u", sourcePath: "/tmp/auth.json"))
        settings.appearance = .dark
        settings.showDockIcon = false
        settings.menuBarDisplayMode = .iconOnly
        try file.save(settings)
        checkEqual(try file.load().accounts, settings.accounts)
        checkEqual(try file.load().selectedAppearance, .dark)
        checkFalse(try file.load().isDockIconVisible)
        checkEqual(try file.load().selectedMenuBarDisplayMode, .iconOnly)
        let data = Data("corrupt".utf8); try data.write(to: file.url)
        checkThrows(try file.load()); checkEqual(try Data(contentsOf: file.url), data)
    }
    @Test func testLegacyAndUnknownPresentationKeepSavedAccounts() throws {
        let legacy = Data(#"{"accounts":[{"id":"492C628F-EC48-4B07-BF81-A2A5CB8C7254","provider":"codex","name":"Personal","identity":"p:u","sourcePath":"/tmp/auth.json","enabled":true,"createdAt":0}],"refreshSeconds":900,"scanLocalLogs":false}"#.utf8)
        for appearance in [nil, "future-theme"] as [String?] {
            var json = try requireValue(JSONSerialization.jsonObject(with: legacy) as? [String: Any])
            if let appearance {
                json["appearance"] = appearance
                json["menuBarDisplayMode"] = "future-mode"
            }
            let settings = try JSONDecoder().decode(LiteSettings.self, from: JSONSerialization.data(withJSONObject: json))
            checkEqual(settings.selectedAppearance, .system)
            checkTrue(settings.isDockIconVisible)
            checkEqual(settings.selectedMenuBarDisplayMode, .providerAndRemaining)
            checkEqual(settings.accounts.count, 1)
            checkEqual(settings.accounts.first?.identity, "p:u")
            checkEqual(settings.refreshSeconds, 900)
            checkFalse(settings.scanLocalLogs)
        }
    }
    @Test func testCacheIsDisabledBetweenAccountRequests() {
        checkNil(QuotaHTTP.session.configuration.urlCache)
        checkEqual(QuotaHTTP.session.configuration.requestCachePolicy, .reloadIgnoringLocalCacheData)
        checkEqual(QuotaHTTP.request(url: URL(string: "https://example.invalid")!, timeout: 1).value(forHTTPHeaderField: "Cache-Control"), "no-cache, no-store, max-age=0")
    }
}
