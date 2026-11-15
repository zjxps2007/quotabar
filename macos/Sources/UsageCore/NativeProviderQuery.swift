import CQuotaBar
import Foundation
#if canImport(FoundationNetworking)
import FoundationNetworking
#endif

enum NativeProviderQuery {
    enum Operation: Int32 { case codex, gemini, refresh }
    typealias Transport = @Sendable (URLRequest) async throws -> (Data, URLResponse)

    private struct Failure: Decodable { let code: String; let message: String }
    private struct Request: Decodable {
        let url: URL
        let method: String
        let headers: [String: String]
        let body: [String: AnyCodable]?
        let timeout: TimeInterval

        func httpRequest() throws -> URLRequest {
            var request = URLRequest(url: url, timeoutInterval: timeout)
            if headers["Cache-Control"] != nil { request.cachePolicy = .reloadIgnoringLocalCacheData }
            request.httpMethod = method
            for (name, value) in headers { request.setValue(value, forHTTPHeaderField: name) }
            if let body { request.httpBody = try JSONEncoder().encode(body) }
            return request
        }
    }

    private static func checked(_ state: Int32, data: Data, operation: Operation) throws -> Data {
        if state >= 0 { return data }
        if state == -30 {
            let error = try JSONDecoder().decode(Failure.self, from: data)
            throw ProviderError(error.code, error.message)
        }
        throw NativeCredentials.failure(state, provider: operation == .gemini ? .gemini : .codex)
    }

    static func native(_ operation: Operation, auth: Data, timeout: TimeInterval, now: Date? = nil) async throws -> Data {
        let cancellation = NativeCancellation()
        let micros = now.map { Int64(($0.timeIntervalSince1970 * 1_000_000).rounded()) } ?? Int64.min
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                DispatchQueue.global(qos: .utility).async {
                    var output: UnsafeMutablePointer<UInt8>?
                    var count = 0
                    var osError: Int32 = 0
                    let state = auth.withUnsafeBytes { raw in
                        qb_provider_query_run(operation.rawValue, raw.bindMemory(to: UInt8.self).baseAddress, raw.count,
                                              timeout, micros, cancellation.handle, &osError, &output, &count)
                    }
                    let bytes = NativeCredentials.take(output, count: count)
                    continuation.resume(with: Result {
                        switch state {
                        case -40: throw NSError(domain: NSURLErrorDomain, code: Int(osError))
                        case -41: throw URLError(.cancelled)
                        default: return try checked(state, data: bytes, operation: operation)
                        }
                    })
                }
            }
        } onCancel: { cancellation.cancel() }
    }

    static func run(_ operation: Operation, auth: Data, timeout: TimeInterval, now: Date? = nil,
                    transport: Transport) async throws -> Data {
        var query: OpaquePointer?
        var output: UnsafeMutablePointer<UInt8>?
        var count = 0
        var state = auth.withUnsafeBytes { raw in
            qb_provider_query_start(operation.rawValue, 1, raw.bindMemory(to: UInt8.self).baseAddress,
                                    raw.count, timeout, &query, &output, &count)
        }
        defer { qb_provider_query_free(query) }
        while true {
            let data = try checked(state, data: NativeCredentials.take(output, count: count), operation: operation)
            if state == 0 { return data }
            let request = try JSONDecoder().decode(Request.self, from: data).httpRequest()
            let status: Int32
            let body: Data
            do {
                let (bytes, response) = try await transport(request)
                status = Int32((response as? HTTPURLResponse)?.statusCode ?? 200)
                body = bytes
            } catch {
                guard state == 1 else { throw error }
                status = -1
                body = Data()
            }
            let micros = Int64(((now ?? Date()).timeIntervalSince1970 * 1_000_000).rounded())
            state = body.withUnsafeBytes { raw in
                qb_provider_query_step(query, status, raw.bindMemory(to: UInt8.self).baseAddress, raw.count,
                                       micros, &output, &count)
            }
        }
    }

    static func usage(_ operation: Operation, auth: Data, timeout: TimeInterval) async throws -> ProviderUsage {
        let bytes = try await native(operation, auth: auth, timeout: timeout)
        return try JSONDecoder().decode(ProviderUsage.self, from: bytes)
    }
}
