import AppKit
import CQuotaBar
import Foundation

public enum CodexExecutable {
    public static func find(environment: [String: String] = ProcessInfo.processInfo.environment) -> URL? {
        let result = (environment["PATH"] ?? "").withCString { search in
            FileManager.default.homeDirectoryForCurrentUser.path.withCString { qb_mac_find_codex(search, $0) }
        }
        guard let result else { return nil }
        defer { qb_string_free(result) }
        return URL(fileURLWithPath: String(cString: result))
    }
}

@MainActor
public final class CodexLoginSession {
    private var attempt: OpaquePointer?
    public init() {}

    public func login(executable: URL? = CodexExecutable.find(), timeout: TimeInterval = 300,
                      onAuthorizationURL: @escaping @MainActor (URL) -> Void = { _ in }) async throws -> Data {
        guard attempt == nil else { throw ProviderError("login_running", "이미 로그인이 진행 중입니다.") }
        guard let executable else {
            throw ProviderError("codex_missing", "Codex 실행 파일을 찾지 못했습니다. Codex CLI 또는 ChatGPT 데스크톱 앱을 설치한 뒤 다시 시도하세요.")
        }
        let handle = executable.path.withCString { path in
            FileManager.default.temporaryDirectory.path.withCString { qb_mac_login_start(path, $0, timeout) }
        }
        guard let handle else {
            throw ProviderError("codex_launch", "Codex 로그인을 시작하지 못했습니다. 설치된 Codex를 확인하세요.")
        }
        attempt = handle
        let observer = NotificationCenter.default.addObserver(forName: NSApplication.willTerminateNotification,
            object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.close(immediately: true) }
            }
        defer {
            NotificationCenter.default.removeObserver(observer)
            if attempt == handle { close(immediately: false) }
        }
        while attempt == handle {
            try Task.checkCancellation()
            var output: UnsafeMutablePointer<UInt8>?
            var count = 0
            let status = qb_mac_login_poll(handle, &output, &count)
            if let output {
                defer { qb_secret_free(output, count) }
                try Task.checkCancellation()
                if status == 2 { return Data(bytes: output, count: count) }
                if status == 1, let url = URL(string: String(decoding: UnsafeBufferPointer(start: output, count: count), as: UTF8.self)) {
                    onAuthorizationURL(url)
                }
            }
            if status == -2 { throw ProviderError("codex_login_timeout", "로그인 대기 시간이 지났습니다. 다시 로그인하세요.") }
            if status < 0 { throw ProviderError("codex_login_failed", "로그인이 완료되지 않았습니다. 다른 Codex 로그인 창을 닫고 다시 시도하세요.") }
            try await Task.sleep(nanoseconds: 200_000_000)
        }
        throw CancellationError()
    }
    private func close(immediately: Bool) {
        guard let handle = attempt else { return }
        attempt = nil
        qb_mac_login_close(handle, immediately ? 1 : 0)
    }
    static func loginURL(in text: String) -> URL? {
        guard let result = text.withCString({ qb_mac_login_url($0) }) else { return nil }
        defer { qb_string_free(result) }
        return URL(string: String(cString: result))
    }
}
