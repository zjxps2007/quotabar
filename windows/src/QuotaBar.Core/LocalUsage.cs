using System.Text.Json;

namespace QuotaBar.Core;
public sealed record TokenCounts(long Input = 0, long Cached = 0, long Output = 0)
{
    public long Total => Input + Output;
    public TokenCounts Add(TokenCounts b) => new(Input + b.Input, Cached + b.Cached, Output + b.Output);
    public TokenCounts Subtract(TokenCounts b) => new(Math.Max(0, Input - b.Input), Math.Max(0, Cached - b.Cached), Math.Max(0, Output - b.Output));
    public TokenCounts Max(TokenCounts b) => new(Math.Max(Input, b.Input), Math.Max(Cached, b.Cached), Math.Max(Output, b.Output));
}
public sealed record DailyUsage(string Day, Provider Provider, string Model, TokenCounts Counts);
public sealed record LocalUsageReport(List<DailyUsage> Days, int ScannedFiles, List<string> Warnings, DateTimeOffset ScannedAt);
public sealed class LocalUsageScanner
{
    private sealed record Event(DateTimeOffset Time, string Model, TokenCounts Counts, bool Cumulative, string? Id = null);
    private sealed class Parsed(string id)
    { public string Id = id; public string? Parent; public DateTimeOffset? Fork; public List<Event> Events = []; public bool Issue; }
    private sealed record Cached(long Size, long Modified, string Identity, Parsed Parsed);
    private readonly Dictionary<string, Cached> cache = new(StringComparer.OrdinalIgnoreCase);
    public void ReleaseCache() => cache.Clear();
    public static TokenCounts Counts(JsonElement value, bool claude = false)
    {
        long N(string key) => (long)Math.Clamp(value.Get(key).Number() ?? 0, 0, 1_000_000_000);
        return claude ? new(N("input_tokens") + N("cache_read_input_tokens") + N("cache_creation_input_tokens"), N("cache_read_input_tokens"), N("output_tokens"))
            : new(N("input_tokens"), N("cached_input_tokens"), N("output_tokens"));
    }
    private static Parsed Parse(Stream file, string path, Provider provider, int limit, CancellationToken token)
    {
        using var bounded = new PrefixStream(file, file.Length);
        using var reader = new StreamReader(bounded, leaveOpen: true); var parsed = new Parsed(path); string model = "Unknown";
        foreach (var row in new SessionProjection(reader, token).Rows())
        {
            if (row.Invalid) { parsed.Issue = true; continue; }
            if (row.Value == null) continue;
            var root = Json.Element(row.Value); var payload = root.Get("payload"); string? type = root.Get("type").Text();
            if (provider == Provider.Codex)
            {
                if (type == "session_meta")
                {
                    parsed.Id = payload.Get("id").Text() ?? parsed.Id;
                    parsed.Parent = payload.Get("forked_from_id").Text() ?? payload.Get("forkedFromId").Text();
                    if (DateTimeOffset.TryParse(payload.Get("timestamp").Text() ?? root.Get("timestamp").Text(), out var fork)) parsed.Fork = fork;
                }
                if (type == "turn_context") model = payload.Get("model").Text() ?? model;
                if (type != "event_msg" || payload.Get("type").Text() != "token_count") continue;
                var info = payload.Get("info"); var total = info.Get("total_token_usage"); bool cumulative = total.ValueKind == JsonValueKind.Object;
                var count = cumulative ? total : info.Get("last_token_usage");
                if (count.ValueKind != JsonValueKind.Object || !DateTimeOffset.TryParse(root.Get("timestamp").Text(), out var time)) continue;
                model = info.Get("model").Text() ?? info.Get("model_name").Text() ?? model;
                parsed.Events.Add(new(time, model, Counts(count), cumulative));
            }
            else
            {
                var msg = root.Get("message"); var usage = msg.Get("usage"); string? id = msg.Get("id").Text(), m = msg.Get("model").Text();
                if (type != "assistant" || id == null || m == null || m == "<synthetic>" || usage.ValueKind != JsonValueKind.Object || !DateTimeOffset.TryParse(root.Get("timestamp").Text(), out var time)) continue;
                parsed.Events.Add(new(time, m, Counts(usage, true), false, id));
            }
            if (parsed.Events.Count >= limit) { parsed.Issue = true; break; }
        }
        return parsed;
    }
    // Called by one background worker. Cache ownership stays with that worker.
    public LocalUsageReport Scan(IEnumerable<string> codexRoots, IEnumerable<string> claudeRoots, CancellationToken token,
        DateTimeOffset? now = null, TimeZoneInfo? zone = null, int historyDays = 182, Func<FileStream, string>? fileIdentity = null)
    {
        var clock = now ?? DateTimeOffset.UtcNow; zone ??= TimeZoneInfo.Local;
        var today = DateOnly.FromDateTime(TimeZoneInfo.ConvertTime(clock, zone).DateTime); var cutoff = today.AddDays(1 - Math.Clamp(historyDays, 1, 366));
        var warnings = new HashSet<string>(); var sessions = new Dictionary<string, Parsed>(); var messages = new Dictionary<string, Event>();
        var visited = new HashSet<string>(StringComparer.OrdinalIgnoreCase); int scanned = 0, count = 0;
        foreach (var (root, provider) in codexRoots.Select(p => (p, Provider.Codex)).Concat(claudeRoots.Select(p => (p, Provider.Claude))))
        {
            token.ThrowIfCancellationRequested(); if (!Directory.Exists(root)) continue;
            try
            {
                if ((File.GetAttributes(root) & FileAttributes.ReparsePoint) != 0) { warnings.Add("링크된 로그 폴더는 제외했습니다."); continue; }
                foreach (var path in Directory.EnumerateFiles(root, "*.jsonl", new EnumerationOptions { RecurseSubdirectories = true, IgnoreInaccessible = false, AttributesToSkip = FileAttributes.ReparsePoint }))
                {
                    token.ThrowIfCancellationRequested(); if (!visited.Add(Path.GetFullPath(path))) continue;
                    if (count >= 250_000) { warnings.Add("25만 사용량 기록 한도로 일부 통계만 표시합니다."); break; }
                    try
                    {
                        using var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete, 32768, FileOptions.SequentialScan);
                        long size = file.Length, modified = File.GetLastWriteTimeUtc(path).Ticks;
                        string identity = fileIdentity?.Invoke(file) ?? File.GetCreationTimeUtc(path).Ticks.ToString();
                        Parsed parsed;
                        if (cache.TryGetValue(path, out var saved) && saved.Size == size && saved.Modified == modified && saved.Identity == identity) parsed = saved.Parsed;
                        else
                        {
                            parsed = Parse(file, path, provider, 250_000 - count, token);
                            if ((cache.Count < 128 || cache.ContainsKey(path)) && file.Length == size && File.GetLastWriteTimeUtc(path).Ticks == modified)
                                cache[path] = new(size, modified, identity, parsed);
                        }
                        count += parsed.Events.Count; scanned++;
                        if (parsed.Issue) warnings.Add("손상되거나 제한을 초과한 사용량 기록은 제외했습니다.");
                        if (provider == Provider.Codex)
                        {
                            if (!sessions.TryGetValue(parsed.Id, out var combined)) sessions[parsed.Id] = combined = new(parsed.Id) { Parent = parsed.Parent, Fork = parsed.Fork };
                            combined.Events.AddRange(parsed.Events); combined.Parent ??= parsed.Parent; combined.Fork ??= parsed.Fork;
                        }
                        else foreach (var item in parsed.Events)
                        {
                            if (item.Time > clock) continue;
                            if (messages.TryGetValue(item.Id!, out var old)) messages[item.Id!] = item with { Time = old.Time < item.Time ? old.Time : item.Time, Counts = item.Counts.Max(old.Counts) };
                            else messages[item.Id!] = item;
                        }
                    }
                    catch (Exception e) when (e is IOException or UnauthorizedAccessException or JsonException) { warnings.Add("읽지 못한 로그 파일이 있어 통계가 불완전할 수 있습니다."); }
                }
            }
            catch (Exception e) when (e is IOException or UnauthorizedAccessException) { warnings.Add("일부 로그 폴더에 접근할 수 없습니다."); }
        }
        foreach (var key in cache.Keys.Where(k => !visited.Contains(k)).ToArray()) cache.Remove(key);
        if (cache.Values.Sum(c => c.Parsed.Events.Count) > 20_000) cache.Clear();
        var days = new Dictionary<(string Day, Provider Provider, string Model), TokenCounts>();
        void Record(Event item, TokenCounts counts, Provider provider)
        {
            var day = DateOnly.FromDateTime(TimeZoneInfo.ConvertTime(item.Time, zone).DateTime);
            if (day < cutoff || item.Time > clock || counts.Total <= 0) return;
            var key = (day.ToString("yyyy-MM-dd"), provider, item.Model); days[key] = days.GetValueOrDefault(key, new()).Add(counts);
        }
        foreach (var session in sessions.Values)
        {
            token.ThrowIfCancellationRequested(); var inherited = new TokenCounts(); var ordered = session.Events.Distinct().OrderBy(e => e.Time).ToArray();
            if (session.Parent != null)
            {
                var parent = sessions.GetValueOrDefault(session.Parent)?.Events.Where(e => e.Cumulative && e.Time <= session.Fork).OrderBy(e => e.Time).LastOrDefault();
                if (parent != null) inherited = parent.Counts;
                else { inherited = ordered.FirstOrDefault(e => e.Cumulative)?.Counts ?? new(); warnings.Add("부모가 없는 분기 세션은 첫 누적 기록을 기준값으로 사용합니다."); }
            }
            var baseline = inherited;
            foreach (var item in ordered)
            {
                TokenCounts delta;
                if (item.Cumulative) { delta = item.Counts.Subtract(baseline); baseline = item.Counts; inherited = new(); }
                else { delta = item.Counts.Subtract(inherited); inherited = inherited.Subtract(item.Counts); baseline = baseline.Add(delta); }
                Record(item, delta, Provider.Codex);
            }
        }
        foreach (var item in messages.Values) Record(item, item.Counts, Provider.Claude);
        return new(days.Select(p => new DailyUsage(p.Key.Day, p.Key.Provider, p.Key.Model, p.Value)).OrderBy(d => d.Day).ToList(), scanned, warnings.ToList(), clock);
    }
}

// Read the file length observed at scan start, even while the CLI appends to it.
internal sealed class PrefixStream(Stream source, long remaining) : Stream
{
    public override bool CanRead => true;
    public override bool CanSeek => false;
    public override bool CanWrite => false;
    public override long Length => throw new NotSupportedException();
    public override long Position { get => throw new NotSupportedException(); set => throw new NotSupportedException(); }
    public override int Read(byte[] buffer, int offset, int count) => Read(buffer.AsSpan(offset, count));
    public override int Read(Span<byte> buffer) { int count = source.Read(buffer[..(int)Math.Min(buffer.Length, remaining)]); remaining -= count; return count; }
    public override void Flush() { }
    public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
    public override void SetLength(long value) => throw new NotSupportedException();
    public override void Write(byte[] buffer, int offset, int count) => throw new NotSupportedException();
}
