using System.Globalization;

namespace QuotaBar.Core;

/// <summary>The tray is a single fixed-width icon. Summarize only accounts the user selected for it.</summary>
public sealed record TraySummary(string Text, bool UseLogo, bool Warning, string Tooltip)
{
    public static TraySummary From(DisplayState state)
    {
        var accounts = state.Settings.Accounts.Where(a => a.Enabled && a.ShowInTray).ToArray();
        var values = accounts.Select(a => (Account: a, Snapshot: state.Snapshots.GetValueOrDefault(a.Id)))
            .Select(p => (p.Account, p.Snapshot, Lowest: p.Snapshot?.Windows.Where(w => double.IsFinite(w.Remaining))
                .Select(w => (double?)Math.Clamp(w.Remaining, 0, 100)).Min())).ToArray();
        double? lowest = values.Select(v => v.Lowest).Min();
        bool uncertain = values.Any(v => state.Errors.ContainsKey(v.Account.Id)
            || v.Snapshot?.State is "stale" or "reset" or "disconnected" or "api-routed");
        bool missing = values.Any(v => v.Lowest == null);
        string text = lowest?.ToString("0", CultureInfo.InvariantCulture) ?? (uncertain ? "!" : "—");
        string title = "QuotaBar" + (state.Demo ? " · 미리보기" : "") + (lowest != null ? $" · 최소 잔여 {text}%" :
            accounts.Length == 0 ? " · 표시할 계정 없음" : uncertain ? " · 연결 확인 필요" : " · 사용량 조회 대기");
        if (uncertain) title += "\n연결 확인 필요 · 이전 값/갱신 대기 포함";
        else if (missing && lowest != null) title += "\n일부 계정 조회 대기";
        foreach (var value in values)
        {
            string name = Clip(value.Account.Name.Replace('\r', ' ').Replace('\n', ' '), 18);
            title += $"\n{value.Account.Provider} {name}: {value.Lowest?.ToString("0", CultureInfo.InvariantCulture) ?? "—"}%";
        }
        return new(text, state.Settings.TrayDisplayMode == "icon" || accounts.Length == 0,
            uncertain || lowest <= 20 || missing && lowest != null, Clip(title, 127));
    }

    private static string Clip(string value, int limit)
    {
        if (value.Length <= limit) return value;
        int length = limit - 1;
        if (char.IsHighSurrogate(value[length - 1])) length--;
        return value[..length] + "…";
    }
}
