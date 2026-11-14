import CQuotaBar
import Foundation

enum NativeQuota {
    static func parse(provider: String, response: [String: Any], source: SourceInfo, email: String?,
                      accountID: String? = nil, jwtPlan: String? = nil, userID: String? = nil,
                      project: String? = nil, tier: String? = nil) throws -> ProviderUsage {
        var request: [String: Any] = ["provider": provider, "response": response,
            "source": try JSONSerialization.jsonObject(with: JSONEncoder().encode(source))]
        request["email"] = email
        request["accountId"] = accountID
        request["userId"] = userID
        request["jwtPlan"] = jwtPlan
        request["project"] = project
        request["tier"] = tier
        let json = try JSONSerialization.data(withJSONObject: request)
        let result = String(decoding: json, as: UTF8.self).withCString { qb_parse_quota($0) }
        guard let result else { throw ProviderError("parse_failed", "Quota response could not be parsed.") }
        defer { qb_string_free(result) }
        return try JSONDecoder().decode(ProviderUsage.self, from: Data(bytes: result, count: strlen(result)))
    }
}
