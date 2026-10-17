import Foundation
import Testing
@testable import UsageCore

private func jwt(account: String = "workspace", user: String = "user", expiry: Double) throws -> String {
    let claims: [String: Any] = ["exp": expiry, "email": "fixture@example.invalid",
        "https://api.openai.com/auth": ["chatgpt_account_id": account, "chatgpt_user_id": user]]
    let body = try JSONSerialization.data(withJSONObject: claims).base64EncodedString()
        .replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    return "fixture.\(body).signature"
}
private func loginData(expired: Bool = true, refresh: String = "old-refresh") throws -> Data {
    try JSONSerialization.data(withJSONObject: ["tokens": ["account_id": "workspace", "refresh_token": refresh,
        "access_token": jwt(expiry: Date().addingTimeInterval(expired ? -60 : 3600).timeIntervalSince1970)]])
}
private func tokenResponse(account: String = "workspace", user: String = "user") throws -> Data {
    try JSONSerialization.data(withJSONObject: ["access_token": jwt(account: account, user: user,
        expiry: Date().addingTimeInterval(3600).timeIntervalSince1970), "refresh_token": "rotated-refresh", "expires_in": 3600])
}
private actor Wire {
    var requests: [URLRequest] = []
    let body: Data
    let status: Int
    let delay: UInt64
    init(body: Data, status: Int = 200, delay: UInt64 = 0) { self.body = body; self.status = status; self.delay = delay }
    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        requests.append(request)
        if delay > 0 { try await Task.sleep(nanoseconds: delay) }
        return (body, HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!)
    }
}
private actor GateWire {
    var continuation: CheckedContinuation<(Data, HTTPURLResponse), Never>?
    func send(_ request: URLRequest) async -> (Data, HTTPURLResponse) {
        await withCheckedContinuation { continuation = $0 }
    }
    var started: Bool { continuation != nil }
    func release(_ body: Data) {
        continuation?.resume(returning: (body, HTTPURLResponse(url: URL(string: "https://auth.openai.com/oauth/token")!,
            statusCode: 200, httpVersion: nil, headerFields: nil)!))
        continuation = nil
    }
}
@MainActor private final class MemoryVault {
    var value: Data?
    var locked = false
    var writes = 0
    init(_ value: Data) { self.value = value }
    func save(_ value: Data) throws {
        if locked { throw ProviderError("credential_storage", "fixture storage unavailable") }
        self.value = value; writes += 1
    }
    func manager(_ client: CodexOAuthClient) -> CodexSessionManager {
        CodexSessionManager(client: client, read: { _ in self.value }, write: { _, data in try self.save(data) })
    }
}

@MainActor
struct CodexOAuthTests {
    @Test func legacySettingsRemainFileImports() throws {
        let account = MonitoredAccount(provider: .codex, name: "Fixture", identity: "key", sourcePath: "/fixture/auth.json")
        var json = try JSONSerialization.jsonObject(with: JSONEncoder().encode(account)) as! [String: Any]
        json.removeValue(forKey: "appManagedLogin")
        var decoded = try JSONDecoder().decode(MonitoredAccount.self, from: JSONSerialization.data(withJSONObject: json))
        #expect(!decoded.usesAppLogin)
        decoded.appManagedLogin = true
        #expect(try JSONDecoder().decode(MonitoredAccount.self, from: JSONEncoder().encode(decoded)).usesAppLogin)
    }
    @Test func expiryControlsProactiveRefresh() throws {
        #expect(try CodexOAuthCredentials(data: loginData()).needsRefresh())
        #expect(try !CodexOAuthCredentials(data: loginData(expired: false)).needsRefresh())
    }
    @Test func refreshUsesOfficialTokenEndpointAndPersistsRotation() async throws {
        let wire = Wire(body: try tokenResponse())
        let client = CodexOAuthClient(transport: { try await wire.send($0) })
        let updated = try CodexOAuthCredentials(data: await client.refresh(loginData()))
        #expect(updated.refreshToken == "rotated-refresh")
        #expect(!updated.needsRefresh())
        #expect(updated.identity.key == (try CodexOAuthCredentials(data: loginData()).identity.key))
        let request = try requireValue(await wire.requests.first)
        #expect(request.url?.absoluteString == "https://auth.openai.com/oauth/token")
        #expect(request.httpMethod == "POST")
        #expect(request.value(forHTTPHeaderField: "Cache-Control")?.contains("no-store") == true)
        let body = try JSONSerialization.jsonObject(with: requireValue(request.httpBody)) as! [String: String]
        #expect(body["grant_type"] == "refresh_token")
        #expect(body["refresh_token"] == "old-refresh")
        #expect(body["client_id"] == CodexOAuthClient.clientID)
    }
    @Test func omittedRefreshTokenKeepsPreviousValue() async throws {
        let body = try JSONSerialization.data(withJSONObject: ["access_token": jwt(expiry: Date().timeIntervalSince1970 + 3600)])
        let wire = Wire(body: body)
        let result = try await CodexOAuthClient(transport: { try await wire.send($0) }).refresh(loginData())
        #expect(try CodexOAuthCredentials(data: result).refreshToken == "old-refresh")
    }
    @Test func changedWorkspaceOrUserIsRejected() async throws {
        for body in [try tokenResponse(account: "other"), try tokenResponse(user: "other")] {
            let wire = Wire(body: body)
            do { _ = try await CodexOAuthClient(transport: { try await wire.send($0) }).refresh(loginData()); Issue.record("Identity change accepted") }
            catch { #expect((error as? ProviderError)?.code == "account_mismatch") }
        }
    }
    @Test func errorsDoNotEchoTokenResponse() async throws {
        for status in [400, 401, 403, 429, 500] {
            let wire = Wire(body: Data("secret-sentinel".utf8), status: status)
            do { _ = try await CodexOAuthClient(transport: { try await wire.send($0) }).refresh(loginData()); Issue.record("Failure accepted") }
            catch {
                #expect(!error.localizedDescription.contains("secret-sentinel"))
                #expect((error as? ProviderError)?.code == (status < 429 ? "reauth_required" : "oauth_unavailable"))
            }
        }
    }
    @Test func malformedSuccessDoesNotReplaceCredentials() async throws {
        let box = MemoryVault(try loginData())
        let before = box.value
        let wire = Wire(body: Data(#"{"refresh_token":"replacement-but-no-access"}"#.utf8))
        let manager = box.manager(CodexOAuthClient(transport: { try await wire.send($0) }))
        do { _ = try await manager.credential(id: UUID(), identity: CodexOAuthCredentials(data: before!).identity.key); Issue.record("Malformed success accepted") }
        catch { #expect((error as? ProviderError)?.code == "oauth_response") }
        #expect(box.value == before); #expect(box.writes == 0)
    }
    @Test func concurrentRequestsShareOneRotation() async throws {
        let box = MemoryVault(try loginData()), id = UUID()
        let identity = try CodexOAuthCredentials(data: box.value!).identity.key
        let wire = Wire(body: try tokenResponse(), delay: 30_000_000)
        let manager = box.manager(CodexOAuthClient(transport: { try await wire.send($0) }))
        async let a = manager.credential(id: id, identity: identity)
        async let b = manager.credential(id: id, identity: identity)
        let (left, right) = try await (a, b)
        #expect(left == right); #expect(box.value == left); #expect(box.writes == 1)
        #expect(await wire.requests.count == 1)
    }
    @Test func staleUnauthorizedResponseDoesNotRotateAgain() async throws {
        let original = try loginData()
        let box = MemoryVault(original), id = UUID()
        let identity = try CodexOAuthCredentials(data: original).identity.key
        let wire = Wire(body: try tokenResponse())
        let manager = box.manager(CodexOAuthClient(transport: { try await wire.send($0) }))
        let first = try await manager.credential(id: id, identity: identity)
        let second = try await manager.credential(id: id, identity: identity, rejected: original)
        #expect(first == second); #expect(await wire.requests.count == 1)
    }
    @Test func unauthorizedCurrentTokenForcesRefreshBeforeExpiry() async throws {
        let original = try loginData(expired: false)
        let box = MemoryVault(original), id = UUID()
        let identity = try CodexOAuthCredentials(data: original).identity.key
        let wire = Wire(body: try tokenResponse())
        let manager = box.manager(CodexOAuthClient(transport: { try await wire.send($0) }))
        _ = try await manager.credential(id: id, identity: identity)
        #expect(await wire.requests.isEmpty)
        _ = try await manager.credential(id: id, identity: identity, rejected: original)
        #expect(await wire.requests.count == 1)
    }
    @Test func rotatedLoginSurvivesFileStoreReopenWithoutAnotherRefresh() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("aiusage-oauth-store-\(UUID())")
        defer { try? FileManager.default.removeItem(at: directory) }
        let vault = SecretVault(directory: directory), id = UUID(), original = try loginData()
        try vault.save(original, id: id)
        let identity = try CodexOAuthCredentials(data: original).identity.key
        let wire = Wire(body: try tokenResponse())
        let client = CodexOAuthClient(transport: { try await wire.send($0) })
        let manager = CodexSessionManager(client: client, read: { try vault.load($0) }, write: { try vault.save($1, id: $0) })
        let rotated = try await manager.credential(id: id, identity: identity)
        let reopened = SecretVault(directory: directory)
        let restarted = CodexSessionManager(client: client, read: { try reopened.load($0) }, write: { try reopened.save($1, id: $0) })
        #expect(try await restarted.credential(id: id, identity: identity) == rotated)
        #expect(try CodexOAuthCredentials(data: requireValue(reopened.load(id))).refreshToken == "rotated-refresh")
        #expect(await wire.requests.count == 1)
    }
    @Test func deletionDuringRefreshCannotResurrectSecret() async throws {
        let box = MemoryVault(try loginData()), id = UUID(), gate = GateWire()
        let identity = try CodexOAuthCredentials(data: box.value!).identity.key
        let manager = box.manager(CodexOAuthClient(transport: { await gate.send($0) }))
        let pending = Task { try await manager.credential(id: id, identity: identity) }
        while !(await gate.started) { await Task.yield() }
        manager.invalidate(id); box.value = nil
        await gate.release(try tokenResponse())
        do { _ = try await pending.value; Issue.record("Deleted credentials returned") } catch {}
        #expect(box.value == nil); #expect(box.writes == 0)
    }
    @Test func reconnectionWinsOverInFlightRotation() async throws {
        let box = MemoryVault(try loginData()), id = UUID(), gate = GateWire()
        let identity = try CodexOAuthCredentials(data: box.value!).identity.key
        let manager = box.manager(CodexOAuthClient(transport: { await gate.send($0) }))
        let pending = Task { try await manager.credential(id: id, identity: identity) }
        while !(await gate.started) { await Task.yield() }
        let newer = try loginData(expired: false, refresh: "new-login")
        box.value = newer // Deliberately omit invalidate to exercise compare-and-swap too.
        await gate.release(try tokenResponse())
        do { _ = try await pending.value; Issue.record("Old generation returned") } catch {}
        #expect(box.value == newer); #expect(box.writes == 0)
    }
    @Test func storageFailureRetainsReplacementWithoutRotatingAgain() async throws {
        let box = MemoryVault(try loginData()), id = UUID()
        let identity = try CodexOAuthCredentials(data: box.value!).identity.key
        let wire = Wire(body: try tokenResponse())
        let manager = box.manager(CodexOAuthClient(transport: { try await wire.send($0) }))
        box.locked = true
        do { _ = try await manager.credential(id: id, identity: identity); Issue.record("Write should fail") } catch {}
        box.locked = false
        let recovered = try await manager.credential(id: id, identity: identity)
        #expect(box.value == recovered)
        #expect(try CodexOAuthCredentials(data: recovered).refreshToken == "rotated-refresh")
        #expect(await wire.requests.count == 1)
    }
    @Test func revokedSessionStopsAutomaticRetriesUntilReconnection() async throws {
        let box = MemoryVault(try loginData()), id = UUID()
        let identity = try CodexOAuthCredentials(data: box.value!).identity.key
        let wire = Wire(body: Data(), status: 401)
        let manager = box.manager(CodexOAuthClient(transport: { try await wire.send($0) }))
        for _ in 0..<2 { do { _ = try await manager.credential(id: id, identity: identity) } catch {} }
        #expect(await wire.requests.count == 1)
        manager.invalidate(id)
        do { _ = try await manager.credential(id: id, identity: identity) } catch {}
        #expect(await wire.requests.count == 2)
    }
    @Test func loginLinksOnlyAcceptExactOfficialOrigin() {
        for value in ["http://auth.openai.com/oauth/authorize", "https://auth.openai.com.evil.invalid/oauth/authorize",
                      "https://auth.openai.com@evil.invalid/oauth/authorize", "https://auth.openai.com:8443/oauth/authorize",
                      "https://auth.openai.com/oauth/token"] {
            #expect(CodexLoginSession.loginURL(in: value) == nil)
        }
        #expect(CodexLoginSession.loginURL(in: "Open https://auth.openai.com/oauth/authorize?state=fixture\n")?.host == "auth.openai.com")
    }
}
