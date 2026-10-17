using System.Diagnostics;
using System.IO.Pipes;
using QuotaBar.Core;
using QuotaBar.Protocol;
using QuotaBar.Windows;
using Channel = QuotaBar.Protocol.Channel;
using Message = QuotaBar.Protocol.Message;

namespace QuotaBar.Monitor;

internal sealed class DashboardHost(Control dispatcher, Func<DisplayState> state, string? capture = null) : IDisposable
{
    private Process? child;
    private Channel? channel;
    private ChildLifetime? lifetime;
    private CancellationTokenSource? startup;
    public event Action<Message>? Command;
    public event Action? Exited;
    public bool IsRunning => child is { HasExited: false };
    public bool Ready { get; private set; }
    public int? Pid => IsRunning ? child!.Id : null;
    private string route = "dashboard";
    public void Show(string destination = "dashboard")
    {
        route = destination;
        if (IsRunning) { if (Ready) Send("show", route); return; }
        string exe = Path.Combine(AppContext.BaseDirectory, "Dashboard", "QuotaBar.Dashboard.exe");
        if (!File.Exists(exe)) exe = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory,
            "../../../../QuotaBar.Dashboard/bin/", AppContext.BaseDirectory.Contains("Debug") ? "Debug" : "Release", "net10.0-windows/QuotaBar.Dashboard.exe"));
        if (!File.Exists(exe)) throw new UsageException("dashboard_missing", "Dashboard 파일을 찾지 못했습니다. 패키지를 다시 설치하세요.");
        var toChild = new AnonymousPipeServerStream(PipeDirection.Out, HandleInheritability.Inheritable);
        var fromChild = new AnonymousPipeServerStream(PipeDirection.In, HandleInheritability.Inheritable);
        var info = new ProcessStartInfo(exe) { UseShellExecute = false, CreateNoWindow = true };
        foreach (var value in new[] { "--in", toChild.GetClientHandleAsString(), "--out", fromChild.GetClientHandleAsString() }) info.ArgumentList.Add(value);
        if (capture != null) { info.ArgumentList.Add("--capture"); info.ArgumentList.Add(capture); }
        var peer = new Channel(fromChild, toChild);
        var process = new Process { StartInfo = info, EnableRaisingEvents = true };
        child = process; channel = peer; Ready = false;
        process.Exited += (_, _) => Post(() =>
        {
            if (child != process) return;
            startup?.Cancel(); peer.Dispose(); lifetime?.Dispose(); lifetime = null; process.Dispose(); child = null; channel = null; Ready = false; Exited?.Invoke();
        });
        try { process.Start(); lifetime = new(process); }
        catch { try { if (!process.HasExited) process.Kill(); } catch (InvalidOperationException) { } child = null; channel = null; peer.Dispose(); process.Dispose(); throw; }
        toChild.DisposeLocalCopyOfClientHandle(); fromChild.DisposeLocalCopyOfClientHandle();
        startup = new CancellationTokenSource();
        var deadline = startup;
        _ = Task.Run(async () =>
        {
            try { await Task.Delay(TimeSpan.FromSeconds(10), deadline.Token); Post(() => { if (child == process && !Ready) Close(); }); }
            catch (OperationCanceledException) { }
        });
        _ = Task.Run(async () =>
        {
            try
            {
                await peer.ReadAsync(m =>
                {
                    Post(() =>
                    {
                        if (child != process) return;
                        if (m.Type == "ready") { Ready = true; deadline.Cancel(); Publish(); Send("show", route); }
                        else Command?.Invoke(m);
                    });
                    return Task.CompletedTask;
                });
            }
            catch (Exception e) when (e is IOException or OperationCanceledException or ObjectDisposedException or System.Text.Json.JsonException) { }
            finally { Post(() => { if (child == process) Close(); }); }
        });
    }
    public void Publish() { if (Ready) Send("state", state()); }
    public void Send<T>(string type, T value, Guid? requestId = null)
    {
        var peer = channel; if (peer == null) return;
        _ = SendQuietly(peer, Message.Create(type, value, requestId));
    }
    private static async Task SendQuietly(Channel peer, Message message)
    { try { await peer.SendAsync(message); } catch (Exception e) when (e is IOException or OperationCanceledException or ObjectDisposedException) { } }
    public void Close()
    {
        Send("close", true);
        var target = child;
        _ = Task.Run(async () => { await Task.Delay(2000); try { if (target is { HasExited: false }) target.Kill(); } catch (InvalidOperationException) { } });
    }
    private void Post(Action action) { if (!dispatcher.IsDisposed) try { dispatcher.BeginInvoke(action); } catch (InvalidOperationException) { } }
    public void Dispose()
    {
        startup?.Cancel(); channel?.Dispose();
        lifetime?.Dispose();
        try { if (IsRunning) child!.Kill(); } catch (InvalidOperationException) { }
        child?.Dispose();
    }
}
