using Microsoft.Win32;
using QuotaBar.Core;
using QuotaBar.Windows;

internal static class PreferencesTests
{
    // Runs without platform APIs as well, to validate the Windows contract on a Mac build host.
    public static void Contract()
    {
        var legacy = Json.Read<Settings>("""{"schemaVersion":1,"accounts":[],"refreshSeconds":900}"""u8);
        Suite.Check(legacy.AutoRefresh && legacy.RefreshSeconds == 900);
        var saved = Json.Read<Settings>(Json.Bytes(legacy with { AutoRefresh = false, RefreshSeconds = 3600 }));
        Suite.Check(!saved.AutoRefresh && saved.RefreshSeconds == 3600);
        var now = DateTimeOffset.Parse("2026-11-02T12:00:00Z");
        foreach (bool visible in new[] { false, true })
        foreach (int seconds in RefreshSchedule.Intervals)
        {
            Suite.Check(RefreshSchedule.Next(false, true, visible, seconds, now) == now.AddSeconds(seconds));
            Suite.Check(RefreshSchedule.Next(false, true, visible, seconds, now, autoRefresh: false) == null);
            Suite.Check(RefreshSchedule.Next(true, true, visible, seconds, now) == null);
            Suite.Check(RefreshSchedule.Next(false, false, visible, seconds, now) == null);
        }
        Suite.Check(RefreshSchedule.Next(false, true, true, 0, now) == null);
        var state = Demo.State() with { Settings = saved, LaunchAtLogin = LaunchAtLoginState.Enabled };
        var restored = Json.Read<DisplayState>(Json.Bytes(state));
        Suite.Check(!restored.Settings.AutoRefresh && restored.Settings.RefreshSeconds == 3600
            && restored.LaunchAtLogin == LaunchAtLoginState.Enabled);
    }

    public static async Task Run(Suite suite)
    {
        await suite.Run("automatic refresh settings and scheduling contract", () => { Contract(); return Task.CompletedTask; });
        await suite.Run("refresh preferences persist and old clients preserve manual mode", () =>
        {
            var paths = new AppPaths(Path.Combine(suite.Paths.Root, "refresh-preferences"));
            using (var service = new MonitorService(paths, launchAtLogin: new FakeStartup()))
            {
                service.Preferences(3600, "dark", true, true, autoRefresh: false);
                service.Preferences(3600, "light", true, true); // A client without the new field preserves it.
                Suite.Check(!service.Settings.AutoRefresh && service.Settings.RefreshSeconds == 3600);
                Suite.Throws<UsageException>(() => service.Preferences(0, "dark", true, true, autoRefresh: true));
                Suite.Check(!service.Settings.AutoRefresh && service.Settings.RefreshSeconds == 3600);
            }
            using var reopened = new MonitorService(paths, launchAtLogin: new FakeStartup());
            Suite.Check(!reopened.Settings.AutoRefresh && reopened.Settings.RefreshSeconds == 3600);
            return Task.CompletedTask;
        });
        await suite.Run("startup registration quotes paths and removes only its own isolated registry value", () =>
        {
            string keyPath = @"Software\QuotaBar.Tests\" + Guid.NewGuid();
            string folder = Path.Combine(suite.Paths.Root, "startup 한글 with spaces"); Directory.CreateDirectory(folder);
            string executable = Path.Combine(folder, "QuotaBar.Monitor.exe"); File.WriteAllText(executable, "fixture");
            try
            {
                var startup = new LaunchAtLogin(executable, keyPath);
                Suite.Check(startup.State == LaunchAtLoginState.Disabled);
                using (var absent = Registry.CurrentUser.OpenSubKey(keyPath)) Suite.Check(absent == null);
                startup.SetEnabled(true); startup.SetEnabled(true);
                Suite.Check(new LaunchAtLogin(executable, keyPath).State == LaunchAtLoginState.Enabled);
                using (var key = Registry.CurrentUser.OpenSubKey(keyPath, writable: true)!)
                {
                    Suite.Check((string?)key.GetValue(LaunchAtLogin.ValueName) == $"\"{executable}\" --background");
                    key.SetValue("unrelated", "preserved");
                }
                startup.SetEnabled(false); startup.SetEnabled(false);
                Suite.Check(startup.State == LaunchAtLoginState.Disabled);
                using var remaining = Registry.CurrentUser.OpenSubKey(keyPath)!;
                Suite.Check(remaining.GetValue(LaunchAtLogin.ValueName) == null && (string?)remaining.GetValue("unrelated") == "preserved");
                var invalid = new LaunchAtLogin(Path.Combine(folder, "missing.exe"), keyPath);
                Suite.Throws<UsageException>(() => invalid.SetEnabled(true));
                Suite.Check(invalid.State == LaunchAtLoginState.Unavailable);
            }
            finally { Registry.CurrentUser.DeleteSubKeyTree(keyPath, throwOnMissingSubKey: false); }
            return Task.CompletedTask;
        });
        await suite.Run("startup changes are explicit and registration failures keep the actual state", () =>
        {
            var startup = new FakeStartup();
            using var service = new MonitorService(new AppPaths(Path.Combine(suite.Paths.Root, "startup-service")), launchAtLogin: startup);
            Suite.Check(startup.Writes == 0 && service.State().LaunchAtLogin == LaunchAtLoginState.Disabled);
            service.SetLaunchAtLogin(true);
            Suite.Check(startup.Writes == 1 && service.State().LaunchAtLogin == LaunchAtLoginState.Enabled);
            startup.Fail = true;
            Suite.Throws<IOException>(() => service.SetLaunchAtLogin(false));
            Suite.Check(service.State().LaunchAtLogin == LaunchAtLoginState.Enabled);
            startup.Current = LaunchAtLoginState.Disabled; service.RefreshLaunchAtLogin();
            Suite.Check(service.State().LaunchAtLogin == LaunchAtLoginState.Disabled);
            return Task.CompletedTask;
        });
    }

    private sealed class FakeStartup : ILaunchAtLogin
    {
        public LaunchAtLoginState Current = LaunchAtLoginState.Disabled;
        public int Writes;
        public bool Fail;
        public LaunchAtLoginState State => Current;
        public void SetEnabled(bool enabled)
        {
            if (Fail) throw new IOException("fixture failure");
            Writes++; Current = enabled ? LaunchAtLoginState.Enabled : LaunchAtLoginState.Disabled;
        }
    }
}
