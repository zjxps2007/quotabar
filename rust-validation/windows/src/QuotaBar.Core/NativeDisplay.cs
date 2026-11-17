using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace QuotaBar.Core;

internal static class NativeDisplay
{
    [DllImport("quotabar_native", EntryPoint = "qb_parse_quota")]
    private static extern IntPtr ParseNative([MarshalAs(UnmanagedType.LPUTF8Str)] string request);
    [DllImport("quotabar_native", EntryPoint = "qb_tray_summary")]
    private static extern IntPtr TrayNative([MarshalAs(UnmanagedType.LPUTF8Str)] string request);
    [DllImport("quotabar_native", EntryPoint = "qb_string_free")] private static extern void Release(IntPtr response);

    public static Snapshot Quota(string provider, JsonElement response, string? email = null)
        => Decode<Snapshot>(ParseNative(Encoding.UTF8.GetString(Json.Bytes(new { provider, response, email }))));

    public static TraySummary Tray(DisplayState state)
    {
        var accounts = state.Settings.Accounts.Select(account => {
            var snapshot = state.Snapshots.GetValueOrDefault(account.Id);
            return new { provider = account.Provider.ToString(), account.Name, account.Enabled, visible = account.ShowInTray,
                remaining = snapshot?.Windows.Select(w => w.Remaining).Where(double.IsFinite) ?? [],
                error = state.Errors.ContainsKey(account.Id), state = snapshot?.State };
        });
        return Decode<TraySummary>(TrayNative(Encoding.UTF8.GetString(Json.Bytes(new { accounts, state.Demo, icon = state.Settings.TrayDisplayMode == "icon" }))));
    }
    private static T Decode<T>(IntPtr pointer)
    {
        if (pointer == IntPtr.Zero) throw new UsageException("invalid_json", "데이터 형식을 확인하세요.");
        try { return JsonSerializer.Deserialize<T>(Marshal.PtrToStringUTF8(pointer)!, Json.Options)!; }
        finally { Release(pointer); }
    }
}
