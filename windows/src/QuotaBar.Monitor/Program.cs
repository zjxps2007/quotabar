using System.Diagnostics;
using Microsoft.Win32;
using QuotaBar.Core;
using QuotaBar.Protocol;
using QuotaBar.Windows;
using Message = QuotaBar.Protocol.Message;

namespace QuotaBar.Monitor;
internal static class Program
{
    [STAThread] static void Main(string[] args)
    {
        ApplicationConfiguration.Initialize();
        try { Application.Run(new MonitorContext(args)); }
        catch (Exception e)
        {
            if (Arguments.Value(args, "--probe") is { } path) File.WriteAllBytes(path, Json.Bytes(new { passed = false, error = e.GetType().Name + ": " + e.Message }));
            else MessageBox.Show(MonitorService.Explain(e), "QuotaBar", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }
}
internal sealed class MonitorContext : ApplicationContext
{
    private readonly Control dispatcher = new();
    private readonly NotifyIcon tray;
    private readonly TrayMenu menu;
    private readonly DashboardHost dashboard;
    private readonly MonitorService? service;
    private readonly SingleInstance? instance;
    private readonly System.Windows.Forms.Timer timer = new();
    private readonly bool demo;
    private DisplayState sample;
    private bool visible, suspended, quitting;
    private DateTimeOffset lastRefresh = DateTimeOffset.UtcNow;
    private string route = "dashboard";
    private Icon? icon;
    private (string Text, bool Logo, bool Warning, int Size, bool Light)? iconKey;
    private string? captured;
    public MonitorContext(string[] args)
    {
        _ = dispatcher.Handle;
        string? probe = Arguments.Value(args, "--probe"); demo = args.Contains("--demo") || probe != null;
        sample = Demo.State() with { Activity = Demo.Activity() };
        if (!demo)
        {
            var paths = new AppPaths(Arguments.Value(args, "--data-dir")); instance = new(paths);
            if (!instance.IsOwner)
            {
                try { instance.Activate().GetAwaiter().GetResult(); }
                catch { MessageBox.Show("QuotaBar가 이미 실행 중입니다. 알림 영역에서 열어 주세요.", "QuotaBar"); }
                instance.Dispose(); dispatcher.Dispose(); Environment.Exit(0);
            }
            service = new(paths); service.Changed += Publish;
        }
        dashboard = new DashboardHost(dispatcher, State);
        dashboard.Command += async m => await Command(m);
        dashboard.Exited += () => { visible = false; service?.StopScan(); Schedule(); };
        menu = new TrayMenu(State, Open, Quit);
        // NotifyIcon opens ContextMenuStrip on right-click. Left-click and double-click intentionally have no action.
        tray = new NotifyIcon { Visible = true, ContextMenuStrip = menu };
        timer.Tick += async (_, _) => { timer.Stop(); await Refresh(); };
        SystemEvents.PowerModeChanged += Power;
        SystemEvents.UserPreferenceChanged += PreferencesChanged;
        SystemEvents.DisplaySettingsChanged += DisplayChanged;
        Publish();
        if (instance != null) _ = instance.Listen(() => Post(() => Open("dashboard")));
        if (probe != null) dispatcher.BeginInvoke(async () => await Probe(probe));
        else dispatcher.BeginInvoke(async () => { if (!args.Contains("--background")) Open("dashboard"); await Refresh(); });
    }
    private DisplayState State() => service?.State() ?? sample;
    private void Open(string target)
    {
        route = target;
        try { dashboard.Show(target); }
        catch (Exception e) { MessageBox.Show(MonitorService.Explain(e), "QuotaBar"); }
    }
    private async Task Command(Message m)
    {
        try
        {
            var p = m.Payload;
            if (m.Type == "visibility") { visible = p.GetBoolean(); if (!visible) service?.StopScan(); else if (route == "dashboard") await Scan(); Schedule(); return; }
            if (m.Type == "route") { route = p.GetString()!; if (route == "dashboard") await Scan(); else service?.StopScan(); return; }
            if (m.Type == "captured") { captured = p.GetString(); return; }
            if (m.Type == "quit") { await Quit(); return; }
            if (m.Type == "refresh") { await Refresh(); return; }
            if (m.Type == "refresh-account") { if (!suspended && service != null) await service.Refresh(p.GetGuid()); return; }
            if (m.Type == "scan") { await Scan(); return; }
            if (demo && m.Type == "preferences")
            {
                sample = sample with { Settings = sample.Settings with { Appearance = p.Get("theme").GetString()!, RefreshSeconds = p.Get("seconds").GetInt32(),
                    ScanLocalLogs = p.Get("scan").GetBoolean(), ShowTaskbarIcon = p.Get("taskbar").GetBoolean(),
                    TrayDisplayMode = p.Get("trayDisplay").Text() ?? sample.Settings.TrayDisplayMode } };
                Publish(); return;
            }
            if (demo) { sample = sample with { Message = "미리보기 모드입니다. 실제 계정을 연결하려면 앱을 종료하고 일반 모드로 실행하세요." }; Publish(); return; }
            switch (m.Type)
            {
                case "login": await service!.Login(); break;
                case "cancel-login": service!.CancelLogin(); break;
                case "import": await service!.Import(Enum.Parse<Provider>(p.Get("provider").GetString()!, true), p.Get("path").GetString()!); break;
                case "claude": await service!.ConnectClaude(p.Get("path").GetString()!, p.Get("consent").GetBoolean(), Helper()); break;
                case "remove": service!.Remove(p.GetGuid()); break;
                case "edit": service!.Edit(p.Get("id").GetGuid(), p.Get("name").GetString()!, p.Get("enabled").GetBoolean(), p.Get("tray").GetBoolean()); break;
                case "preferences": service!.Preferences(p.Get("seconds").GetInt32(), p.Get("theme").GetString()!, p.Get("scan").GetBoolean(), p.Get("taskbar").GetBoolean(), p.Get("trayDisplay").Text()); break;
                default: throw new UsageException("command", "지원하지 않는 요청입니다.");
            }
            dashboard.Send("result", new { success = true }, m.RequestId);
        }
        catch (Exception e) { service?.SetMessage(MonitorService.Explain(e)); dashboard.Send("result", new { success = false, error = MonitorService.Explain(e) }, m.RequestId); }
    }
    private static string Helper()
    {
        string path = Path.Combine(AppContext.BaseDirectory, "StatusLine", "QuotaBar.StatusLine.exe");
        if (!File.Exists(path)) path = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../QuotaBar.StatusLine/bin/", AppContext.BaseDirectory.Contains("Debug") ? "Debug" : "Release", "net10.0-windows/QuotaBar.StatusLine.exe"));
        if (!File.Exists(path)) throw new UsageException("helper", "StatusLine 실행 파일을 찾지 못했습니다."); return path;
    }
    private async Task Scan() { if (visible && route == "dashboard" && !suspended && service != null) await service.Scan(); }
    private async Task Refresh()
    {
        if (suspended || quitting) return;
        lastRefresh = DateTimeOffset.UtcNow; if (service != null) { await service.Refresh(); if (route == "dashboard") await Scan(); } Schedule();
    }
    private void Power(object sender, PowerModeChangedEventArgs e) => Post(async () =>
    {
        suspended = e.Mode == PowerModes.Suspend;
        if (suspended) { timer.Stop(); service?.Suspend(); }
        else if (e.Mode == PowerModes.Resume) { await Task.Delay(2000); await Refresh(); }
    });
    private void Schedule()
    {
        timer.Stop(); if (quitting || demo) return;
        var settings = State().Settings;
        var next = RefreshSchedule.Next(suspended, settings.Accounts.Any(a => a.Enabled), visible, settings.RefreshSeconds, lastRefresh);
        if (next is { } due) { timer.Interval = (int)Math.Clamp((due - DateTimeOffset.UtcNow).TotalMilliseconds, 1000, int.MaxValue); timer.Start(); }
    }
    private void Publish()
    {
        if (quitting) return;
        var summary = TraySummary.From(State()); tray.Text = summary.Tooltip;
        var key = (summary.Text, summary.UseLogo, summary.Warning, TrayIconRenderer.PixelSize(), TrayIconRenderer.LightTaskbar());
        if (iconKey != key)
        {
            Icon next = TrayIconRenderer.Create(summary, key.Item4, key.Item5);
            tray.Icon = next; icon?.Dispose(); icon = next; iconKey = key;
        }
        menu.RefreshIfVisible(); dashboard.Publish(); Schedule();
    }
    private void PreferencesChanged(object sender, UserPreferenceChangedEventArgs e) => Post(Publish);
    private void DisplayChanged(object? sender, EventArgs e) => Post(Publish);
    private async Task Probe(string path)
    {
        try
        {
            var pids = new List<int>();
            var captures = new List<string>();
            TrayIconRenderer.SaveSamples(Path.GetDirectoryName(path)!);
            var trayMenu = await TrayMenuProbe.Run(tray, menu, () => sample, value => sample = value,
                () => dashboard.IsRunning, Path.GetDirectoryName(path)!);
            captures.AddRange(trayMenu.Captures);
            async Task Capture(string target, string name, int width = 1120, int height = 810)
            {
                dashboard.Publish();
                string file = Path.Combine(Path.GetDirectoryName(path)!, name + ".png");
                captured = null; dashboard.Send("capture", new { route = target, path = file, width, height });
                await Until(() => captured == file, 10000); captures.Add(name);
            }
            for (int i = 0; i < 2; i++)
            {
                dashboard.Show(); await Until(() => dashboard.Ready, 10000); pids.Add(dashboard.Pid!.Value);
                string theme = i == 0 ? "light" : "dark";
                sample = sample with { Settings = sample.Settings with { Appearance = theme } };
                foreach (string target in new[] { "dashboard", "activity", "accounts", "settings", "connect", "connect-claude" })
                    await Capture(target, $"{target}-{theme}");
                await Capture("accounts", $"accounts-narrow-{theme}", 900, 660);
                await Capture("dashboard", $"dashboard-wide-{theme}", 1320, 900);
                if (i == 1)
                {
                    var original = sample;
                    sample = sample with { Settings = sample.Settings with { Accounts = [] }, Snapshots = [], Errors = [] };
                    await Capture("dashboard", "empty-dark");
                    sample = original with { Settings = original.Settings with { Accounts = original.Settings.Accounts.Select((a, index) => a with
                        { Name = index == 0 ? "여러 프로젝트에서 사용하는 긴 계정 이름 · 디자인 검증용 샘플 계정" : a.Name, Enabled = index != 2 }).ToList() },
                        Errors = new() { [original.Settings.Accounts[0].Id] = "서비스에 연결하지 못했습니다. 마지막으로 확인한 사용량을 표시합니다." } };
                    await Capture("dashboard", "states-dark", 900, 660);
                    sample = original;
                }
                dashboard.Close(); await Until(() => !dashboard.IsRunning, 5000);
            }
            using var current = Process.GetCurrentProcess(); current.Refresh();
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path))!);
            Publish();
            var traySummary = TraySummary.From(sample);
            if (!tray.Visible || traySummary.Text != "16" || traySummary.UseLogo || !tray.Text.Contains("16%"))
                throw new InvalidOperationException("Numeric tray did not survive Dashboard close.");
            File.WriteAllBytes(path, Json.Bytes(new { passed = true, captures, trayMenu, dashboardPids = pids, dashboardRunning = dashboard.IsRunning,
                trayText = traySummary.Text, trayVisibleAfterDashboardClose = tray.Visible, trayPixelSize = icon!.Width,
                monitorPid = current.Id, privateBytes = current.PrivateMemorySize64, workingSet = current.WorkingSet64,
                wpfLoadedInMonitor = AppDomain.CurrentDomain.GetAssemblies().Any(a => a.GetName().Name == "PresentationFramework") }));
        }
        catch (Exception e) { File.WriteAllBytes(path, Json.Bytes(new { passed = false, error = e.GetType().Name + ": " + e.Message })); }
        finally { ExitThread(); }
    }
    private static async Task Until(Func<bool> condition, int limit)
    { var deadline = Environment.TickCount64 + limit; while (!condition()) { if (Environment.TickCount64 > deadline) throw new TimeoutException(); await Task.Delay(50); } }
    private void Post(Action action) { if (!dispatcher.IsDisposed) try { dispatcher.BeginInvoke(action); } catch (InvalidOperationException) { } }
    private async Task Quit()
    {
        if (quitting) return; quitting = true; timer.Stop(); dashboard.Close();
        try { if (service != null) await service.Shutdown(); } finally { ExitThread(); }
    }
    protected override void ExitThreadCore()
    {
        quitting = true; SystemEvents.PowerModeChanged -= Power; SystemEvents.UserPreferenceChanged -= PreferencesChanged;
        SystemEvents.DisplaySettingsChanged -= DisplayChanged; timer.Dispose(); service?.Dispose(); dashboard.Dispose();
        tray.Visible = false; tray.Dispose(); menu.Dispose(); icon?.Dispose(); instance?.Dispose(); dispatcher.Dispose(); base.ExitThreadCore();
    }
}
