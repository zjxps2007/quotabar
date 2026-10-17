import Foundation

public struct TokenCounts: Codable, Hashable, Sendable {
    public var input = 0
    public var cached = 0
    public var output = 0
    public var total: Int { input + output }
    public init(input: Int = 0, cached: Int = 0, output: Int = 0) {
        self.input = input; self.cached = cached; self.output = output
    }
    func subtracting(_ other: Self) -> Self {
        Self(input: max(0, input - other.input), cached: max(0, cached - other.cached), output: max(0, output - other.output))
    }
    func adding(_ other: Self) -> Self {
        Self(input: input + other.input, cached: cached + other.cached, output: output + other.output)
    }
    func maximum(_ other: Self) -> Self {
        Self(input: max(input, other.input), cached: max(cached, other.cached), output: max(output, other.output))
    }
}

public struct DailyUsage: Identifiable, Codable, Equatable, Sendable {
    public var id: String { "\(provider.rawValue):\(day):\(model)" }
    public let day: String
    public let provider: ProviderKind
    public let model: String
    public var counts: TokenCounts
}

public struct LocalUsageReport: Codable, Equatable, Sendable {
    public var days: [DailyUsage] = []
    public var scannedFiles = 0
    public var warnings: [String] = []
    public var scannedAt: Date?
    public init() {}
    public func tokens(on day: String) -> Int { days.lazy.filter { $0.day == day }.reduce(0) { $0 + $1.counts.total } }
    public func tokens(inMonth month: String) -> Int { days.lazy.filter { $0.day.hasPrefix(month) }.reduce(0) { $0 + $1.counts.total } }
}

/// Bounded streaming projection adapted from upstream; only numeric usage and
/// session identity survive parsing. No conversation body is retained or stored.
public actor LocalUsageScanner {
    struct Event: Hashable, Sendable {
        let time: Date
        let model: String
        let counts: TokenCounts
        let cumulative: Bool
        let messageID: String?
    }
    struct ParsedFile: Sendable {
        var sessionID: String
        var parentID: String?
        var forkTime: Date?
        var events: [Event] = []
        var issue: Bool = false
    }
    struct Cached: Sendable {
        let size: Int
        let modified: Date
        let inode: UInt64
        let parsed: ParsedFile
    }
    private struct DayBucket: Hashable {
        let day: Int
        let provider: ProviderKind
        let model: String
    }
    private var cache: [String: Cached] = [:]
    public init() {}
    public func releaseCache() { cache.removeAll() }

    public func scan(codexRoots: [URL], claudeRoots: [URL], now: Date = Date(),
                     timeZone: TimeZone = .current, historyDays: Int = 90) throws -> LocalUsageReport {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let dayCount = min(366, max(1, historyDays))
        let cutoff = calendar.date(byAdding: .day, value: 1 - dayCount, to: calendar.startOfDay(for: now))!
        var report = LocalUsageReport()
        var codex: [String: ParsedFile] = [:]
        var claudeRecords: [String: Event] = [:]
        var kept: Set<String> = []
        var visited: Set<String> = []
        let maximumEvents = 250_000
        var eventCount = 0
        let roots = codexRoots.map { ($0, ProviderKind.codex) } + claudeRoots.map { ($0, ProviderKind.claude) }
        scanRoots: for (root, provider) in roots {
            guard FileManager.default.fileExists(atPath: root.path) else { continue }
            var enumerationFailed = false
            guard let files = FileManager.default.enumerator(at: root,
                includingPropertiesForKeys: [.isRegularFileKey], options: [.skipsHiddenFiles],
                errorHandler: { _, _ in enumerationFailed = true; return true }) else {
                report.warnings.append("일부 로그 폴더에 접근할 수 없습니다."); continue
            }
            for case let file as URL in files where file.pathExtension == "jsonl" {
                try Task.checkCancellation()
                if eventCount >= maximumEvents {
                    report.warnings.append("로그가 커서 일부 통계만 표시합니다 (25만 사용량 기록 한도).")
                    break scanRoots
                }
                let path = file.standardizedFileURL.resolvingSymlinksInPath().path
                guard visited.insert(path).inserted else { continue }
                do {
                    let attributes = try FileManager.default.attributesOfItem(atPath: path)
                    guard attributes[.type] as? FileAttributeType == .typeRegular else { continue }
                    let size = (attributes[.size] as? NSNumber)?.intValue ?? 0
                    let modified = attributes[.modificationDate] as? Date ?? .distantPast
                    let inode = (attributes[.systemFileNumber] as? NSNumber)?.uint64Value ?? 0
                    // Parent totals may be needed to remove inherited fork usage, so
                    // Codex metadata is scanned even for sessions older than the chart.
                    if provider == .claude && modified < cutoff { continue }
                    let parsed: ParsedFile
                    if let saved = cache[path], saved.size == size, saved.modified == modified, saved.inode == inode {
                        parsed = saved.parsed
                    } else {
                        parsed = try Self.parse(path: path, provider: provider, limit: maximumEvents - eventCount)
                        if cache.count < 128 || cache[path] != nil {
                            cache[path] = Cached(size: size, modified: modified, inode: inode, parsed: parsed)
                        }
                    }
                    eventCount += parsed.events.count
                    guard eventCount <= maximumEvents else {
                        report.warnings.append("로그가 커서 일부 통계만 표시합니다 (25만 사용량 기록 한도).")
                        break
                    }
                    kept.insert(path)
                    report.scannedFiles += 1
                    if parsed.issue { report.warnings.append("일부 로그의 손상되거나 너무 큰 사용량 기록은 제외했습니다.") }
                    if provider == .codex {
                        // A rollout can exist in both sessions and archived_sessions.
                        // Combine unique events instead of counting both files.
                        if var existing = codex[parsed.sessionID] {
                            existing.events.append(contentsOf: parsed.events)
                            existing.parentID = existing.parentID ?? parsed.parentID
                            existing.forkTime = existing.forkTime ?? parsed.forkTime
                            codex[parsed.sessionID] = existing
                        } else { codex[parsed.sessionID] = parsed }
                    } else {
                        for event in parsed.events where event.time >= cutoff && event.time <= now {
                            guard let id = event.messageID else { continue }
                            if let old = claudeRecords[id] {
                                claudeRecords[id] = Event(time: min(old.time, event.time), model: event.model,
                                    counts: old.counts.maximum(event.counts), cumulative: false, messageID: id)
                            } else { claudeRecords[id] = event }
                        }
                    }
                } catch is CancellationError { throw CancellationError() }
                catch { report.warnings.append("읽지 못한 로그 파일이 있어 통계가 불완전할 수 있습니다.") }
            }
            if enumerationFailed { report.warnings.append("일부 로그 폴더에 접근할 수 없습니다.") }
        }
        cache = cache.filter { kept.contains($0.key) }
        // Retain at most 20k small numeric events between scans.
        if cache.values.reduce(0, { $0 + $1.parsed.events.count }) > 20_000 { cache.removeAll() }
        // Calendar boundaries preserve 23/25-hour days. Format each day once,
        // instead of allocating a date string and a composite key per event.
        let dayStarts = (0..<dayCount).map {
            calendar.startOfDay(for: calendar.date(byAdding: .day, value: $0, to: cutoff)!)
        }
        let formatter = DateFormat.formatter("yyyy-MM-dd", timeZone: timeZone)
        let dayKeys = dayStarts.map { formatter.string(from: $0) }
        var days: [DayBucket: TokenCounts] = [:]
        func record(_ event: Event, counts: TokenCounts, provider: ProviderKind) {
            guard event.time >= cutoff, event.time <= now, counts.total > 0 else { return }
            var lower = 0, upper = dayStarts.count
            while lower < upper {
                let middle = (lower + upper) / 2
                if dayStarts[middle] <= event.time { lower = middle + 1 }
                else { upper = middle }
            }
            let key = DayBucket(day: lower - 1, provider: provider, model: event.model)
            days[key] = (days[key] ?? TokenCounts()).adding(counts)
        }
        for event in claudeRecords.values {
            try Task.checkCancellation()
            record(event, counts: event.counts, provider: .claude)
        }
        for file in codex.values {
            try Task.checkCancellation()
            let sorted = file.events.sorted { $0.time < $1.time }
            var inherited = TokenCounts()
            if let parent = file.parentID {
                if let original = codex[parent], let fork = file.forkTime,
                   let last = original.events.lazy.filter({ $0.cumulative && $0.time <= fork }).max(by: { $0.time < $1.time }) {
                    inherited = last.counts
                } else {
                    report.warnings.append("원본이 없는 분기 세션은 첫 누적 기록을 기준값으로 사용합니다.")
                    inherited = sorted.first(where: \.cumulative)?.counts ?? TokenCounts()
                }
            }
            var previous = inherited
            var seen: Set<Event> = []
            for event in sorted {
                try Task.checkCancellation()
                guard seen.insert(event).inserted else { continue }
                let delta: TokenCounts
                if event.cumulative {
                    delta = event.counts.subtracting(previous)
                    previous = event.counts
                    inherited = TokenCounts()
                } else {
                    delta = event.counts.subtracting(inherited)
                    inherited = inherited.subtracting(event.counts)
                    previous = previous.adding(delta)
                }
                record(event, counts: delta, provider: .codex)
            }
        }
        report.days = days.map { key, counts in
            DailyUsage(day: dayKeys[key.day], provider: key.provider, model: key.model, counts: counts)
        }.sorted { $0.id < $1.id }
        report.warnings = Array(Set(report.warnings)).sorted()
        report.scannedAt = now
        return report
    }

    static func parse(path: String, provider: ProviderKind, limit: Int) throws -> ParsedFile {
        var file = ParsedFile(sessionID: path)
        var model = "Unknown"
        let scanned = try SessionProjectionReader.scan(path: path, cursor: nil) { data in
            guard file.events.count < max(0, limit) else { file.issue = true; return }
            guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return }
            let type = object["type"] as? String
            if provider == .claude {
                guard type == "assistant",
                      let message = object["message"] as? [String: Any],
                      let id = message["id"] as? String, !id.isEmpty,
                      let name = message["model"] as? String, name != "<synthetic>",
                      let usage = message["usage"] as? [String: Any],
                      let timestamp = (object["timestamp"] as? String).flatMap(SharedFormatters.parseISO8601) else { return }
                let counts = TokenCounts(input: number(usage, "input_tokens") + number(usage, "cache_read_input_tokens") + number(usage, "cache_creation_input_tokens"),
                    cached: number(usage, "cache_read_input_tokens"), output: number(usage, "output_tokens"))
                file.events.append(Event(time: timestamp, model: name, counts: counts, cumulative: false, messageID: id))
            } else {
                let payload = object["payload"] as? [String: Any] ?? [:]
                if type == "session_meta" {
                    file.sessionID = payload["id"] as? String ?? file.sessionID
                    file.parentID = payload["forked_from_id"] as? String ?? payload["forkedFromId"] as? String
                    file.forkTime = (payload["timestamp"] as? String).flatMap(SharedFormatters.parseISO8601)
                        ?? (object["timestamp"] as? String).flatMap(SharedFormatters.parseISO8601)
                } else if type == "turn_context" { model = payload["model"] as? String ?? model }
                else if type == "event_msg", payload["type"] as? String == "token_count",
                        let info = payload["info"] as? [String: Any],
                        let usage = (info["total_token_usage"] ?? info["last_token_usage"]) as? [String: Any],
                        let timestamp = (object["timestamp"] as? String).flatMap(SharedFormatters.parseISO8601) {
                    let counts = TokenCounts(input: number(usage, "input_tokens"), cached: number(usage, "cached_input_tokens"),
                        output: number(usage, "output_tokens"))
                    model = info["model"] as? String ?? info["model_name"] as? String ?? model
                    file.events.append(Event(time: timestamp, model: model, counts: counts,
                        cumulative: info["total_token_usage"] != nil, messageID: nil))
                }
            }
        }
        if scanned.errorCode != nil { file.issue = true }
        return file
    }
    private static func number(_ values: [String: Any], _ key: String) -> Int {
        // Reject absurd/overflowing values instead of crashing the dashboard.
        min(1_000_000_000, max(0, values[key] as? Int ?? 0))
    }
}
