import Foundation
import AppKit
import Darwin

public enum CodexExecutable {
    public static func find(environment: [String: String] = ProcessInfo.processInfo.environment) -> URL? {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let paths = (environment["PATH"] ?? "").split(separator: ":").map { String($0) + "/codex" }
            + ["/Applications/ChatGPT.app/Contents/Resources/codex-cli/CodexCLI.app/Contents/MacOS/codex",
               "/Applications/Codex.app/Contents/Resources/codex",
               "/opt/homebrew/bin/codex", "/usr/local/bin/codex", home + "/.local/bin/codex"]
        return paths.first(where: { $0.hasPrefix("/") && FileManager.default.isExecutableFile(atPath: $0) }).map(URL.init(fileURLWithPath:))
    }
}

/// Runs the official browser login with a private, disposable CODEX_HOME.
/// No fallback to the user's CLI credentials: only this attempt can supply the result.
@MainActor
public final class CodexLoginSession {
    private var output = ""
    private var authorizationURL: URL?
    public init() {}

    public func login(executable: URL? = CodexExecutable.find(), timeout: TimeInterval = 300,
                      onAuthorizationURL: @escaping @MainActor (URL) -> Void = { _ in }) async throws -> Data {
        guard activeAttempt == nil else { throw ProviderError("login_running", "이미 로그인이 진행 중입니다.") }
        guard let executable else {
            throw ProviderError("codex_missing", "Codex 실행 파일을 찾지 못했습니다. Codex CLI 또는 ChatGPT 데스크톱 앱을 설치한 뒤 다시 시도하세요.")
        }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("quotabar-login-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        let process = Process()
        let pipe = Pipe()
        let attempt = UUID()
        activeAttempt = attempt; output = ""; authorizationURL = nil
        process.executableURL = executable
        process.arguments = ["-c", "cli_auth_credentials_store=\"file\"", "login"]
        process.currentDirectoryURL = directory
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = pipe; process.standardError = pipe
        var environment = ProcessInfo.processInfo.environment
        environment["CODEX_HOME"] = directory.path
        environment["PATH"] = [environment["PATH"] ?? "", "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"].joined(separator: ":")
        for key in ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN", "OPENAI_BASE_URL"] { environment.removeValue(forKey: key) }
        process.environment = environment
        let terminationObserver = NotificationCenter.default.addObserver(forName: NSApplication.willTerminateNotification,
            object: nil, queue: .main) { _ in
                if process.isRunning {
                    kill(process.processIdentifier, SIGKILL)
                    process.waitUntilExit()
                }
                try? FileManager.default.removeItem(at: directory)
            }
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let chunk = handle.availableData
            guard !chunk.isEmpty else { return }
            Task { @MainActor in
                guard let self, self.activeAttempt == attempt else { return }
                self.output = String((self.output + String(decoding: chunk, as: UTF8.self)).suffix(16_384))
                if self.authorizationURL == nil, let url = Self.loginURL(in: self.output) {
                    self.authorizationURL = url
                    onAuthorizationURL(url)
                }
            }
        }
        defer {
            NotificationCenter.default.removeObserver(terminationObserver)
            if activeAttempt == attempt { activeAttempt = nil; output = ""; authorizationURL = nil }
            pipe.fileHandleForReading.readabilityHandler = nil
            if process.isRunning {
                process.terminate()
                DispatchQueue.global().asyncAfter(deadline: .now() + 2) {
                    if process.isRunning { kill(process.processIdentifier, SIGKILL) }
                }
                Task.detached {
                    process.waitUntilExit()
                    try? FileManager.default.removeItem(at: directory)
                }
            } else { try? FileManager.default.removeItem(at: directory) }
        }
        do { try process.run() }
        catch { throw ProviderError("codex_launch", "Codex 로그인을 시작하지 못했습니다. 설치된 Codex를 확인하세요.") }
        let deadline = Date().addingTimeInterval(timeout)
        let authFile = directory.appendingPathComponent("auth.json")
        while Date() < deadline {
            try Task.checkCancellation()
            if let size = try? authFile.resourceValues(forKeys: [.fileSizeKey]).fileSize,
               size <= 1_048_576, let data = try? Data(contentsOf: authFile),
               let credentials = try? CodexOAuthCredentials(data: data), credentials.refreshToken != nil {
                try Task.checkCancellation()
                return data
            }
            guard process.isRunning else {
                throw ProviderError("codex_login_failed", "로그인이 완료되지 않았습니다. 다른 Codex 로그인 창을 닫고 다시 시도하세요.")
            }
            try await Task.sleep(nanoseconds: 200_000_000)
        }
        throw ProviderError("codex_login_timeout", "로그인 대기 시간이 지났습니다. 다시 로그인하세요.")
    }
    private var activeAttempt: UUID?

    static func loginURL(in text: String) -> URL? {
        guard let expression = try? NSRegularExpression(pattern: #"https://[^\s\x1B<>\"]+"#) else { return nil }
        for match in expression.matches(in: text, range: NSRange(text.startIndex..., in: text)) {
            guard let range = Range(match.range, in: text), let url = URL(string: String(text[range])),
                  url.scheme == "https", url.host == "auth.openai.com", url.user == nil, url.password == nil,
                  url.port == nil || url.port == 443,
                  ["/oauth/authorize", "/api/accounts/authorize"].contains(url.path) else { continue }
            return url
        }
        return nil
    }
}
