using System.Text.Json;
using System.Text.Json.Serialization;
using System.Runtime.InteropServices;

namespace QuotaBar.Core;

public static class Json
{
    public static readonly JsonSerializerOptions Options = new(JsonSerializerDefaults.Web)
    { WriteIndented = false, Converters = { new JsonStringEnumConverter(JsonNamingPolicy.CamelCase) } };
    public static byte[] Bytes<T>(T value) => JsonSerializer.SerializeToUtf8Bytes(value, Options);
    public static T Read<T>(ReadOnlySpan<byte> data) => JsonSerializer.Deserialize<T>(data, Options)
        ?? throw new UsageException("invalid_json", "데이터 형식을 확인하세요.");
    public static JsonElement Element<T>(T value) => JsonSerializer.SerializeToElement(value, Options);
    public static JsonElement Get(this JsonElement e, string key) => e.ValueKind == JsonValueKind.Object && e.TryGetProperty(key, out var v) ? v : default;
    public static string? Text(this JsonElement e) => e.ValueKind == JsonValueKind.String ? e.GetString() : null;
    public static double? Number(this JsonElement e) => e.ValueKind == JsonValueKind.Number && e.TryGetDouble(out var v) && double.IsFinite(v) ? v : null;
}

public sealed class UsageException(string code, string message) : Exception(message)
{ public string Code { get; } = code; }
public enum Provider { Codex, Claude, Gemini }
public sealed record Account(Guid Id, Provider Provider, string Name, string Identity, string SourcePath,
    bool Enabled = true, bool ShowInTray = true, bool AppManagedLogin = false);
public sealed record QuotaWindow(string Title, double Remaining, DateTimeOffset? ResetAt = null);
public sealed record Snapshot(DateTimeOffset FetchedAt, List<QuotaWindow> Windows, string? Plan = null,
    string? Email = null, DateTimeOffset? ObservedAt = null, string? State = null)
{ public double? Lowest => Windows.Count == 0 ? null : Windows.Min(w => w.Remaining); }
public sealed record Settings
{
    public int SchemaVersion { get; init; } = 1;
    public List<Account> Accounts { get; init; } = [];
    public int RefreshSeconds { get; init; } = 300;
    public bool AutoRefresh { get; init; } = true;
    public bool AutomaticUpdates { get; init; } = true;
    public bool ScanLocalLogs { get; init; } = true;
    public string Appearance { get; init; } = "system";
    public bool ShowTaskbarIcon { get; init; } = true;
    public string TrayDisplayMode { get; init; } = "remaining";
}
public enum LaunchAtLoginState { Disabled, Enabled, Unavailable }
public sealed record DisplayState(Settings Settings, Dictionary<Guid, Snapshot> Snapshots,
    Dictionary<Guid, string> Errors, HashSet<Guid> Loading, string? Message = null, bool Demo = false,
    LocalUsageReport? Activity = null, bool Scanning = false, bool LoggingIn = false, string? DataDirectory = null,
    Dictionary<string, string>? DefaultPaths = null, LaunchAtLoginState LaunchAtLogin = LaunchAtLoginState.Disabled,
    AppUpdateState? AppUpdate = null);
public interface ICredentialStore
{
    byte[]? Load(Guid id);
    void Save(Guid id, byte[] data);
    void Delete(Guid id);
}
public static class RefreshSchedule
{
    [StructLayout(LayoutKind.Sequential)]
    private struct Delays { public int Accounts, Local; }
    [DllImport("quotabar_native", EntryPoint = "qb_refresh_delays")]
    private static extern Delays Calculate(int suspended, int automatic, int accounts, int dashboard, int logs, int scanning, int interval);
    [DllImport("quotabar_native", EntryPoint = "qb_refresh_interval")]
    private static extern int Interval(nuint index);
    public static readonly IReadOnlyList<int> Intervals = Array.AsReadOnly(ReadIntervals());
    private static int[] ReadIntervals()
    {
        var values = new List<int>();
        for (int value; (value = Interval((nuint)values.Count)) > 0;) values.Add(value);
        return values.ToArray();
    }
    public static DateTimeOffset? Next(bool suspended, bool hasAccounts, bool visible, int seconds, DateTimeOffset last, bool autoRefresh = true)
    {
        int delay = Calculate(suspended ? 1 : 0, autoRefresh ? 1 : 0, hasAccounts ? 1 : 0, visible ? 1 : 0, 0, 0, seconds).Accounts;
        return delay < 0 ? null : last.AddSeconds(delay);
    }
}
