using System.Diagnostics;
using QuotaBar.Core;
using QuotaBar.Protocol;
using QuotaBar.Windows;
using Message = QuotaBar.Protocol.Message;

namespace QuotaBar.Monitor;
internal static class Program
{
    [STAThread] static void Main(string[] args)
    {
        try
        {
            using var dispatcher = new MonitorLoop();
            if (Arguments.Value(args, "--update-manager-probe") is { } updateProbe)
            { UpdateManagerProbe.Start(dispatcher, updateProbe); dispatcher.Run(); return; }
            string? probe = Arguments.Value(args, "--probe");
            var paths = probe != null || args.Contains("--demo") ? null : new AppPaths(Arguments.Value(args, "--data-dir"));
            using var instance = paths == null ? null : new SingleInstance(paths);
            bool background = args.Contains("--background");
            if (instance is { IsOwner: false })
            {
                try { if (!background) instance.Activate().GetAwaiter().GetResult(); }
                catch { TrayMenu.ShowMessage("QuotaBar가 이미 실행 중입니다. 알림 영역에서 열어 주세요."); }
                return;
            }
            using var monitor = new MonitorContext(dispatcher, paths, instance, probe, args, background);
            dispatcher.Run();
        }
        catch (Exception e)
        {
            if (Arguments.Value(args, "--probe") is { } path) File.WriteAllBytes(path, Json.Bytes(new { passed = false, error = e.GetType().Name + ": " + e.Message }));
            else TrayMenu.ShowMessage(MonitorService.Explain(e), 1);
        }
    }
}
internal sealed class MonitorContext : IDisposable
{
    private readonly MonitorLoop dispatcher;
    private readonly NotificationIcon tray;
    private readonly TrayMenu menu;
    private readonly DashboardHost dashboard;
    private readonly MonitorService? service;
    private readonly AppUpdateManager? updater;
    private readonly bool demo;
    private DisplayState sample;
    private bool visible, suspended, quitting;
    private DateTimeOffset lastRefresh = DateTimeOffset.UtcNow;
    private string route = "dashboard";
    private string? captured;
    public MonitorContext(MonitorLoop dispatcher, AppPaths? paths, SingleInstance? instance, string? probe, string[] args, bool background)
    {
        this.dispatcher = dispatcher;
        demo = paths == null;
        sample = Demo.State(probe == null ? null : new DateTimeOffset(2099, 1, 15, 12, 0, 0, TimeSpan.Zero)) with { Activity = Demo.Activity() };
        if (paths != null)
        {
            service = new(paths); service.Changed += Publish;
        }
        dashboard = new DashboardHost(dispatcher, State);
        dashboard.Command += async m => await Command(m);
        dashboard.Exited += () => { visible = false; service?.StopScan(); Schedule(); ConfigureUpdates(); };
        menu = new TrayMenu(State, Open, Quit);
        tray = new NotificationIcon(menu);
        if (service != null) updater = new(dispatcher, service.Paths, Publish, service.PrepareUpdateExit,
            () => { service.ResumeAfterUpdate(); Schedule(); }, Quit);
        dispatcher.Received += OnSystemEvent;
        Publish();
        if (instance != null) _ = instance.Listen(() => Post(() => Open("dashboard")));
        if (probe != null) Post(async () => await Probe(probe));
        else Post(async () => {
            if (!background) Open("dashboard");
            if (service != null) UpdateInstaller.Acknowledge(args, service.Paths);
            await Refresh();
        });
    }
    private DisplayState State() => service is { } monitor ? monitor.State() with { AppUpdate = updater?.State } : sample;
    private void ConfigureUpdates()
    {
        if (service == null || quitting) return;
        var state = service.State();
        updater?.Configure(state.Settings.AutomaticUpdates, !visible && !dashboard.IsRunning, suspended || state.LoggingIn);
    }
    private void Open(string target)
    {
        route = target;
        try { visible = true; ConfigureUpdates(); dashboard.Show(target); }
        catch (Exception e) { visible = dashboard.IsRunning; ConfigureUpdates(); TrayMenu.ShowMessage(MonitorService.Explain(e)); }
    }
    private async Task Command(Message m)
    {
        try
        {
            var p = m.Payload;
            if (m.Type == "visibility") { visible = p.GetBoolean(); ConfigureUpdates(); if (!visible) service?.StopScan(); else if (route == "dashboard") await Scan(); Schedule(); return; }
            if (m.Type == "route") { route = p.GetString()!; if (route == "dashboard") await Scan(); else service?.StopScan(); return; }
            if (m.Type == "captured") { captured = p.GetString(); return; }
            if (m.Type == "quit") { await Quit(); return; }
            if (m.Type == "refresh") { await Refresh(); return; }
            if (m.Type == "refresh-account") { if (!suspended && service != null) await service.Refresh(p.GetGuid()); return; }
            if (m.Type == "scan") { await Scan(); return; }
            if (m.Type == "startup-status") { service?.RefreshLaunchAtLogin(); return; }
            if (m.Type == "startup-settings")
            {
                if (!demo) Process.Start(new ProcessStartInfo("ms-settings:startupapps") { UseShellExecute = true });
                return;
            }
            if (m.Type == "startup")
            {
                if (demo) { sample = sample with { LaunchAtLogin = p.GetBoolean() ? LaunchAtLoginState.Enabled : LaunchAtLoginState.Disabled }; Publish(); }
                else service!.SetLaunchAtLogin(p.GetBoolean());
                return;
            }
            if (demo && m.Type == "preferences")
            {
                sample = sample with { Settings = sample.Settings with { Appearance = p.Get("theme").GetString()!, RefreshSeconds = p.Get("seconds").GetInt32(),
                    ScanLocalLogs = p.Get("scan").GetBoolean(), ShowTaskbarIcon = p.Get("taskbar").GetBoolean(),
                    TrayDisplayMode = p.Get("trayDisplay").Text() ?? sample.Settings.TrayDisplayMode,
                    AutoRefresh = p.Get("autoRefresh").ValueKind == System.Text.Json.JsonValueKind.Undefined ? sample.Settings.AutoRefresh : p.Get("autoRefresh").GetBoolean(),
                    AutomaticUpdates = p.Get("automaticUpdates").ValueKind == System.Text.Json.JsonValueKind.Undefined ? sample.Settings.AutomaticUpdates : p.Get("automaticUpdates").GetBoolean() } };
                Publish(); return;
            }
            if (demo) { sample = sample with { Message = "미리보기 모드입니다. 실제 계정을 연결하려면 앱을 종료하고 일반 모드로 실행하세요." }; Publish(); return; }
            switch (m.Type)
            {
                case "check-app-update": updater!.Check(); break;
                case "install-app-update": updater!.InstallNow(); break;
                case "login": await service!.Login(); break;
                case "cancel-login": service!.CancelLogin(); break;
                case "import": await service!.Import(Enum.Parse<Provider>(p.Get("provider").GetString()!, true), p.Get("path").GetString()!); break;
                case "claude": await service!.ConnectClaude(p.Get("path").GetString()!, p.Get("consent").GetBoolean(), Helper()); break;
                case "remove": service!.Remove(p.GetGuid()); break;
                case "edit": service!.Edit(p.Get("id").GetGuid(), p.Get("name").GetString()!, p.Get("enabled").GetBoolean(), p.Get("tray").GetBoolean()); break;
                case "preferences": service!.Preferences(p.Get("seconds").GetInt32(), p.Get("theme").GetString()!, p.Get("scan").GetBoolean(), p.Get("taskbar").GetBoolean(), p.Get("trayDisplay").Text(),
                    p.Get("autoRefresh").ValueKind == System.Text.Json.JsonValueKind.Undefined ? null : p.Get("autoRefresh").GetBoolean(),
                    p.Get("automaticUpdates").ValueKind == System.Text.Json.JsonValueKind.Undefined ? null : p.Get("automaticUpdates").GetBoolean()); break;
                default: throw new UsageException("command", "지원하지 않는 요청입니다.");
            }
            dashboard.Send("result", new { success = true }, m.RequestId);
        }
        catch (Exception e) { service?.SetMessage(MonitorService.Explain(e)); dashboard.Send("result", new { success = false, error = MonitorService.Explain(e) }, m.RequestId); }
    }
    private static string Helper()
    {
        string path = Path.Combine(AppContext.BaseDirectory, "StatusLine", "QuotaBar.StatusLine.exe");
        if (!File.Exists(path)) throw new UsageException("helper", "StatusLine 실행 파일을 찾지 못했습니다."); return path;
    }
    private async Task Scan() { if (visible && route == "dashboard" && !suspended && service != null) await service.Scan(); }
    private async Task Refresh(bool automatic = false)
    {
        if (suspended || quitting || (automatic && !State().Settings.AutoRefresh)) return;
        lastRefresh = DateTimeOffset.UtcNow; if (service != null) { await service.Refresh(); if (route == "dashboard") await Scan(); } Schedule();
    }
    private async void OnSystemEvent(MonitorLoop.Event value)
    {
        switch (value)
        {
            case MonitorLoop.Event.Refresh: await Refresh(automatic: true); break;
            case MonitorLoop.Event.Display: Post(Publish); break;
            case MonitorLoop.Event.Suspend:
            case MonitorLoop.Event.Resume:
            case MonitorLoop.Event.PowerStatus: Post(async () => await Power(value)); break;
        }
    }
    private async Task Power(MonitorLoop.Event value)
    {
        suspended = value == MonitorLoop.Event.Suspend;
        ConfigureUpdates();
        if (suspended) { dispatcher.Schedule(MonitorLoop.Timer.Refresh, 0); service?.Suspend(); }
        else if (value == MonitorLoop.Event.Resume) { await Task.Delay(2000); await Refresh(automatic: true); }
    }
    private void Schedule()
    {
        if (quitting || demo) return;
        var settings = State().Settings;
        var next = RefreshSchedule.Next(suspended, settings.Accounts.Any(a => a.Enabled), visible, settings.RefreshSeconds, lastRefresh, settings.AutoRefresh);
        dispatcher.Schedule(MonitorLoop.Timer.Refresh, next is { } due ? (int)Math.Clamp((due - DateTimeOffset.UtcNow).TotalMilliseconds, 1000, int.MaxValue) : 0);
    }
    private void Publish()
    {
        if (quitting) return;
        tray.Publish(TraySummary.From(State()));
        menu.RefreshIfVisible(); dashboard.Publish(); Schedule(); ConfigureUpdates();
    }
    private async Task Probe(string path)
    {
        try
        {
            var pids = new List<int>();
            var captures = new List<string>();
            TrayMenuProbe.SaveSamples(Path.GetDirectoryName(path)!);
            var trayMenu = await TrayMenuProbe.Run(tray, menu, () => sample, value => sample = value,
                () => dashboard.IsRunning, Path.GetDirectoryName(path)!);
            captures.AddRange(trayMenu.Captures);
            async Task Capture(string target, string name, int width = 1120, int height = 810)
            {
                dashboard.Publish();
                string file = Path.Combine(Path.GetDirectoryName(path)!, name + ".png");
                captured = null; dashboard.Send("capture", new { route = target, path = file, width, height });
                await Until(() => captured == file, 10000, $"capturing {name}"); captures.Add(name);
            }
            for (int i = 0; i < 2; i++)
            {
                dashboard.Show(); await Until(() => dashboard.Ready, 10000, "starting the Dashboard helper"); pids.Add(dashboard.Pid!.Value);
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
                dashboard.Close(); await Until(() => !dashboard.IsRunning, 5000, "closing the Dashboard helper");
            }
            using var current = Process.GetCurrentProcess(); current.Refresh();
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(path))!);
            Publish();
            var traySummary = TraySummary.From(sample);
            if (!tray.Visible || traySummary.Text != "16" || traySummary.UseLogo || !tray.Text.Contains("16%"))
                throw new InvalidOperationException("Numeric tray did not survive Dashboard close.");
            File.WriteAllBytes(path, Json.Bytes(new { passed = true, captures, trayMenu, dashboardPids = pids, dashboardRunning = dashboard.IsRunning,
                trayText = traySummary.Text, trayVisibleAfterDashboardClose = tray.Visible, trayPixelSize = tray.PixelSize,
                monitorPid = current.Id, privateBytes = current.PrivateMemorySize64, workingSet = current.WorkingSet64,
                wpfLoadedInMonitor = AppDomain.CurrentDomain.GetAssemblies().Any(a => a.GetName().Name == "PresentationFramework"),
                windowsFormsLoadedInMonitor = AppDomain.CurrentDomain.GetAssemblies().Any(a => a.GetName().Name == "System.Windows.Forms") }));
        }
        catch (Exception e) { File.WriteAllBytes(path, Json.Bytes(new { passed = false, error = e.GetType().Name + ": " + e.Message })); }
        finally { dispatcher.Exit(); }
    }
    private async Task Until(Func<bool> condition, int limit, string operation)
    {
        var deadline = Environment.TickCount64 + limit;
        while (!condition())
        {
            if (Environment.TickCount64 > deadline)
                throw new TimeoutException($"Timed out {operation} (helper running: {dashboard.IsRunning}, ready: {dashboard.Ready}).");
            await Task.Delay(50);
        }
    }
    private void Post(Action action) => dispatcher.TryPost(action);
    private async Task Quit()
    {
        if (quitting) return; quitting = true; dispatcher.Schedule(MonitorLoop.Timer.Refresh, 0); dashboard.Close();
        try { if (service != null) await service.Shutdown(); } finally { dispatcher.Exit(); }
    }
    public void Dispose()
    {
        quitting = true; dispatcher.Received -= OnSystemEvent;
        updater?.Dispose(); service?.Dispose(); dashboard.Dispose(); tray.Dispose(); menu.Dispose();
    }
}
