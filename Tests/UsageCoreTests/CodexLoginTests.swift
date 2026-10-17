import Foundation
import Testing
@testable import UsageCore

@MainActor
struct CodexLoginTests {
    private func fixture(wait: Bool = false, fail: Bool = false) throws -> (URL, URL) {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("lite-login-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let executable = root.appendingPathComponent("codex-fixture")
        let auth = try AccountTests().auth(account: "workspace", user: "fixture")
        var json = try JSONSerialization.jsonObject(with: auth) as! [String: Any]
        var tokens = json["tokens"] as! [String: Any]; tokens["refresh_token"] = "fixture-refresh"; json["tokens"] = tokens
        try JSONSerialization.data(withJSONObject: json).write(to: root.appendingPathComponent("fixture.json"))
        let action = wait ? "exec /bin/sleep 30" : (fail ? "exit 1" : "cp '\(root.path)/fixture.json' \"$CODEX_HOME/auth.json\"")
        let script = """
        #!/bin/sh
        test "$1" = "-c" || exit 2
        test "$2" = 'cli_auth_credentials_store="file"' || exit 2
        test "$3" = "login" || exit 2
        printf '%s' "$CODEX_HOME" > '\(root.path)/home'
        /usr/bin/stat -f '%Lp' "$CODEX_HOME" > '\(root.path)/mode'
        \(action)
        """
        try Data(script.utf8).write(to: executable)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
        return (root, executable)
    }
    private func waitFor(_ condition: () -> Bool) async throws {
        for _ in 0..<300 {
            if condition() { return }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        throw ProviderError("test_timeout", "Fixture condition did not complete")
    }
    @Test func loginUsesIsolatedPrivateHomeAndRemovesIt() async throws {
        let (root, executable) = try fixture()
        defer { try? FileManager.default.removeItem(at: root) }
        let data = try await CodexLoginSession().login(executable: executable)
        #expect(try CodexOAuthCredentials(data: data).refreshToken == "fixture-refresh")
        let loginHome = try String(contentsOf: root.appendingPathComponent("home"), encoding: .utf8)
        #expect(loginHome.contains("quotabar-login-"))
        #expect(loginHome != NSString(string: "~/.codex").expandingTildeInPath)
        #expect(try String(contentsOf: root.appendingPathComponent("mode"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines) == "700")
        try await waitFor { !FileManager.default.fileExists(atPath: loginHome) }
    }
    @Test func cancellationStopsLoginAndRemovesPrivateHome() async throws {
        let (root, executable) = try fixture(wait: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let task = Task { try await CodexLoginSession().login(executable: executable) }
        try await waitFor { FileManager.default.fileExists(atPath: root.appendingPathComponent("home").path) }
        let loginHome = try String(contentsOf: root.appendingPathComponent("home"), encoding: .utf8)
        task.cancel()
        do { _ = try await task.value; Issue.record("Cancelled login returned credentials") }
        catch { #expect(error is CancellationError) }
        try await waitFor { !FileManager.default.fileExists(atPath: loginHome) }
    }
    @Test func timeoutStopsLoginAndRemovesPrivateHome() async throws {
        let (root, executable) = try fixture(wait: true)
        defer { try? FileManager.default.removeItem(at: root) }
        // Allow fixture startup under parallel test load; it still sleeps well past this deadline.
        do { _ = try await CodexLoginSession().login(executable: executable, timeout: 3); Issue.record("Timeout ignored") }
        catch { #expect((error as? ProviderError)?.code == "codex_login_timeout") }
        let loginHome = try String(contentsOf: root.appendingPathComponent("home"), encoding: .utf8)
        try await waitFor { !FileManager.default.fileExists(atPath: loginHome) }
    }
    @Test func failedLoginNeverFallsBackToExistingCLICredentials() async throws {
        let (root, executable) = try fixture(fail: true)
        defer { try? FileManager.default.removeItem(at: root) }
        do { _ = try await CodexLoginSession().login(executable: executable); Issue.record("Failed login imported credentials") }
        catch { #expect((error as? ProviderError)?.code == "codex_login_failed") }
        let loginHome = try String(contentsOf: root.appendingPathComponent("home"), encoding: .utf8)
        try await waitFor { !FileManager.default.fileExists(atPath: loginHome) }
    }
    @Test func missingExecutableHasActionableError() async {
        do { _ = try await CodexLoginSession().login(executable: nil); Issue.record("Missing executable accepted") }
        catch { #expect((error as? ProviderError)?.code == "codex_missing") }
    }
}
