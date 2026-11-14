import CQuotaBar
import Foundation

enum NativeCredentials {
    static func decode<T: Decodable>(_ type: T.Type, data: Data, provider: ProviderKind, mode: Int32) throws -> T {
        var output: UnsafeMutablePointer<UInt8>?
        var count = 0
        let providerID: Int32
        switch provider { case .codex: providerID = 0; case .claude: providerID = 1; case .gemini: providerID = 2 }
        let status = data.withUnsafeBytes { raw in
            qb_credentials(raw.bindMemory(to: UInt8.self).baseAddress, raw.count, providerID, mode, &output, &count)
        }
        guard status == 0, let output else { throw failure(status, provider: provider) }
        defer { qb_secret_free(output, count) }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        return try decoder.decode(T.self, from: Data(bytesNoCopy: output, count: count, deallocator: .none))
    }

    static func rotate(_ current: Data, response: Data, now: Date) throws -> Data {
        var output: UnsafeMutablePointer<UInt8>?
        var count = 0
        let status = current.withUnsafeBytes { source in
            response.withUnsafeBytes { reply in
                qb_rotate_credentials(source.bindMemory(to: UInt8.self).baseAddress, source.count,
                    reply.bindMemory(to: UInt8.self).baseAddress, reply.count,
                    Int64((now.timeIntervalSince1970 * 1_000_000).rounded()), &output, &count)
            }
        }
        guard status == 0, let output else { throw failure(status, provider: .codex) }
        defer { qb_secret_free(output, count) }
        return Data(bytes: output, count: count)
    }

    private static func failure(_ status: Int32, provider: ProviderKind) -> ProviderError {
        switch status {
        case -4: return ProviderError("missing_token", "구독 계정의 CLI 로그인 파일이 필요합니다. API 키는 지원하지 않습니다.")
        case -5: return ProviderError("missing_identity", provider == .codex
            ? "Codex의 워크스페이스와 사용자 정보를 확인할 수 없습니다. CLI에서 다시 로그인하세요."
            : "Gemini 계정 식별 정보가 없습니다. CLI에서 다시 로그인하세요.")
        case -6: return ProviderError("invalid_auth", "Claude는 CLI 설정 폴더를 연결하세요.")
        case -7: return ProviderError("invalid_auth", "ChatGPT 구독 계정으로 로그인하세요.")
        case -8: return ProviderError("oauth_response", "로그인 갱신 응답에 필요한 인증 정보가 없습니다.")
        case -9: return ProviderError("account_mismatch", "갱신된 인증의 계정이 달라 연결을 중단했습니다. 다시 로그인하세요.")
        case -10: return ProviderError("account_mismatch", "갱신된 로그인 정보를 기존 계정에 연결할 수 없습니다.")
        default: return ProviderError("invalid_auth", "로그인 파일을 읽을 수 없습니다. JSON 형식을 확인하세요.")
        }
    }
}
