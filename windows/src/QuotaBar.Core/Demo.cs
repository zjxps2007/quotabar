namespace QuotaBar.Core;
public static class Demo
{
    public static LocalUsageReport Activity()
    {
        var days = new List<DailyUsage>(); var now = DateTimeOffset.Now;
        for (int i = 0; i < 182; i++)
        {
            if (i % 7 == 3 || i % 11 == 0) continue;
            foreach (var p in new[] { Provider.Codex, Provider.Claude })
                days.Add(new(now.AddDays(-i).ToString("yyyy-MM-dd"), p, p == Provider.Codex ? "gpt-5.4" : "claude-sonnet-4-6", new(10000 + (i * 7927 % 320000), i * 200, 3000 + i * 70)));
        }
        return new(days, 247, [], now);
    }
    public static DisplayState State()
    {
        var accounts = new[] { new Account(Guid.NewGuid(), Provider.Codex, "개인 계정 · 샘플", "demo-1", ""),
            new Account(Guid.NewGuid(), Provider.Claude, "작업 계정 · 샘플", "demo-2", ""), new Account(Guid.NewGuid(), Provider.Gemini, "Code Assist · 샘플", "demo-3", "") };
        return new(new Settings { Accounts = [.. accounts] }, accounts.Select((a,i) => (a,i)).ToDictionary(x => x.a.Id,
            x => new Snapshot(DateTimeOffset.UtcNow, [new("5시간", 38 + x.i * 18, DateTimeOffset.UtcNow.AddHours(3)), new("주간", 16 + x.i * 23, DateTimeOffset.UtcNow.AddDays(4))], "샘플")), [], [], null, true);
    }
}
