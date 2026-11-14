import CQuotaBar
import Foundation

enum NativeProviderRequest {
    enum Endpoint: String, Encodable { case codex, refresh, geminiAssist, geminiProjects, geminiQuota }
    struct Credentials: Decodable {
        let accessToken: String
        let accountId: String?
        let email: String?
        let plan: String?
        let userId: String?
    }
    struct Discovery: Decodable {
        let project: String?
        let tier: String?
    }
    private struct Parameters: Encodable {
        let endpoint: Endpoint
        let token: String
        let account: String?
        let project: String?
        let timeout: TimeInterval
    }
    private struct Request: Decodable {
        let url: URL
        let method: String
        let headers: [String: String]
        let body: [String: AnyCodable]?
        let timeout: TimeInterval
    }
    private static func decode<T: Decodable>(_ type: T.Type, _ bytes: Data, _ mode: Int32) throws -> T {
        var output: UnsafeMutablePointer<UInt8>?
        var count = 0
        let status = bytes.withUnsafeBytes { buffer in
            qb_provider_request(mode, buffer.bindMemory(to: UInt8.self).baseAddress, buffer.count, &output, &count)
        }
        guard status == 0, let output else { throw ProviderError("parse_failed", "Invalid provider request.") }
        defer { qb_secret_free(output, count) }
        return try JSONDecoder().decode(T.self, from: Data(bytesNoCopy: output, count: count, deallocator: .none))
    }
    static func make(_ endpoint: Endpoint, token: String, account: String? = nil,
                     project: String? = nil, timeout: TimeInterval) throws -> URLRequest {
        let bytes = try JSONEncoder().encode(Parameters(endpoint: endpoint, token: token, account: account, project: project, timeout: timeout))
        let result = try decode(Request.self, bytes, 0)
        var request = URLRequest(url: result.url, timeoutInterval: result.timeout)
        if result.headers["Cache-Control"] != nil { request.cachePolicy = .reloadIgnoringLocalCacheData }
        request.httpMethod = result.method
        for (key, value) in result.headers { request.setValue(value, forHTTPHeaderField: key) }
        if let body = result.body { request.httpBody = try JSONEncoder().encode(body) }
        return request
    }
    static func discovery(_ data: Data, list: Bool) throws -> Discovery {
        try decode(Discovery.self, data, list ? 2 : 1)
    }
}
