import CQuotaBar
import Foundation

public struct MenuBarSummary: Decodable, Sendable {
    public let title: String
    public let remaining: String
    public let help: String
    public let emptyMessage: String

    private struct Entry: Encodable, Equatable {
        let provider: String
        let name: String
        let visible: Bool
        let enabled: Bool
        let remaining: [Double]
        let error: Bool
        let state: String?
    }
    private struct Request: Encodable { let accounts: [Entry] }

    public final class Cache {
        private var previous: [Entry]?
        private var value: MenuBarSummary?
        public init() {}

        public func summary(accounts: [MonitoredAccount], snapshots: [UUID: AccountSnapshot],
                            errors: [UUID: String]) -> MenuBarSummary {
            let entries = accounts.map {
                let snapshot = snapshots[$0.id]
                return Entry(provider: $0.provider.title, name: $0.name, visible: $0.isShownInMenuBar, enabled: $0.enabled,
                    remaining: snapshot?.windows.map(\.remaining).filter(\.isFinite) ?? [], error: errors[$0.id] != nil, state: snapshot?.state)
            }
            if previous == entries, let value { return value }
            do {
                let bytes = try JSONEncoder().encode(Request(accounts: entries))
                let name = String(decoding: bytes, as: UTF8.self)
                guard let out = name.withCString({ qb_tray_summary($0) }) else { throw CocoaError(.coderInvalidValue) }
                defer { qb_string_free(out) }
                let next = try JSONDecoder().decode(MenuBarSummary.self, from: Data(bytes: out, count: strlen(out)))
                previous = entries
                value = next
                return next
            } catch {
                return value ?? MenuBarSummary(title: "QuotaBar", remaining: "—", help: "QuotaBar", emptyMessage: "")
            }
        }
    }
}
