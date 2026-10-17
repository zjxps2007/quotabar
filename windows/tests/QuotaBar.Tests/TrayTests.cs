using QuotaBar.Core;
using QuotaBar.Windows;

internal static class TrayTests
{
    public static async Task Run(Suite suite)
    {
        await suite.Run("tray summarizes only visible enabled accounts and all quota windows", () =>
        {
            var state = Demo.State();
            Suite.Check(TraySummary.From(state).Text == "16" && TraySummary.From(state).Warning);
            var accounts = state.Settings.Accounts;
            state = state with { Settings = state.Settings with { Accounts =
                [accounts[0] with { ShowInTray = false }, accounts[1] with { Enabled = false }, accounts[2]] } };
            var summary = TraySummary.From(state);
            Suite.Check(summary.Text == "62" && !summary.Warning && summary.Tooltip.Contains("62%"));
            Suite.Check(!summary.Tooltip.Contains("Codex") && !summary.Tooltip.Contains("Claude"));
            state = state with { Settings = state.Settings with { TrayDisplayMode = "icon" } };
            Suite.Check(TraySummary.From(state).UseLogo && TraySummary.From(state).Text == "62");
            return Task.CompletedTask;
        });
        await suite.Run("tray distinguishes no data errors stale values and zero", () =>
        {
            var state = Demo.State(); var account = state.Settings.Accounts[0];
            state = state with { Settings = state.Settings with { Accounts = [account] }, Snapshots = [] };
            Suite.Check(TraySummary.From(state).Text == "—" && !TraySummary.From(state).UseLogo);
            state.Errors[account.Id] = "offline";
            Suite.Check(TraySummary.From(state).Text == "!" && TraySummary.From(state).Warning);
            state.Snapshots[account.Id] = new(DateTimeOffset.UtcNow, [new("5h", 38.4)]);
            var stale = TraySummary.From(state);
            Suite.Check(stale.Text == "38" && stale.Warning && stale.Tooltip.Contains("이전 값"));
            state.Errors.Clear();
            foreach (var (remaining, expected, warning) in new[] { (0d, "0", true), (100d, "100", false), (38.6, "39", false) })
            {
                state.Snapshots[account.Id] = new(DateTimeOffset.UtcNow, [new("5h", remaining)]);
                var summary = TraySummary.From(state);
                Suite.Check(summary.Text == expected && summary.Warning == warning);
            }
            state.Snapshots[account.Id] = new(DateTimeOffset.UtcNow, [new("5h", 75)], State: "stale");
            Suite.Check(TraySummary.From(state).Warning);
            state = state with { Settings = state.Settings with { Accounts = [] } };
            Suite.Check(TraySummary.From(state).UseLogo && TraySummary.From(state).Tooltip.Contains("표시할 계정 없음"));
            return Task.CompletedTask;
        });
        await suite.Run("tray preference upgrades old settings and rejects invalid changes", () =>
        {
            var paths = new AppPaths(Path.Combine(suite.Paths.Root, "tray-preferences"));
            PrivateFiles.EnsureDirectory(paths.Root);
            File.WriteAllText(Path.Combine(paths.Root, "settings.json"), """{"schemaVersion":1,"accounts":[]}""");
            using (var service = new MonitorService(paths))
            {
                Suite.Check(service.Settings.TrayDisplayMode == "remaining");
                service.Preferences(300, "system", true, true, "icon");
                service.Preferences(900, "dark", true, true); // Old clients must preserve the selected tray mode.
                Suite.Throws<UsageException>(() => service.Preferences(300, "system", true, true, "invalid"));
                Suite.Check(service.Settings.TrayDisplayMode == "icon" && service.Settings.RefreshSeconds == 900);
            }
            using var reopened = new MonitorService(paths);
            Suite.Check(reopened.Settings.TrayDisplayMode == "icon" && reopened.Settings.Appearance == "dark");
            return Task.CompletedTask;
        });
    }
}
