using System.Text.Json;
using QuotaBar.Core;
using QuotaBar.Protocol;
using Message = QuotaBar.Protocol.Message;

namespace QuotaBar.Monitor;

internal sealed class DashboardHost(MonitorLoop dispatcher, Func<DisplayState> state, string? capture = null) : IDisposable
{
    private DashboardChild? child;
    private string route = "dashboard";
    public event Action<Message>? Command;
    public event Action? Exited;
    public bool Ready { get; private set; }
    public int? Pid => child?.Pid;
    public bool IsRunning => Pid != null;

    public void Show(string destination = "dashboard")
    {
        route = destination;
        if (IsRunning) { if (Ready) Send("show", route); return; }
        string exe = Path.Combine(AppContext.BaseDirectory, "Dashboard", "QuotaBar.Dashboard.exe");
        if (!File.Exists(exe)) throw new UsageException("dashboard_missing", "Dashboard 파일을 찾지 못했습니다. 패키지를 다시 설치하세요.");
        var peer = DashboardChild.Start(exe, capture);
        child = peer;
        Ready = false;
        _ = Task.Run(() => Receive(peer));
    }

    private void Receive(DashboardChild peer)
    {
        try
        {
            while (peer.ReadNext() is { } message)
            {
                Post(() =>
                {
                    if (child != peer) return;
                    if (message.Type == "ready")
                    {
                        Ready = true;
                        peer.Ready();
                        Publish();
                        Send("show", route);
                    }
                    else Command?.Invoke(message);
                });
            }
        }
        catch (Exception e) when (e is JsonException or IOException or InvalidDataException or UsageException) { }
        finally
        {
            peer.RequestClose();
            peer.WaitForExit();
            if (!Post(() =>
            {
                if (child == peer) { child = null; Ready = false; Exited?.Invoke(); }
                peer.Dispose();
            })) peer.Dispose();
        }
    }
    public void Publish() { if (Ready) Send("state", state()); }
    public void Send<T>(string type, T value, Guid? requestId = null)
    {
        if (child is not { IsClosed: false } peer) return;
        peer.Send(Message.Create(type, value, requestId));
    }
    public void Close()
    {
        if (child is { IsClosed: false } peer) peer.RequestClose();
    }
    private bool Post(Action action) => dispatcher.TryPost(action);
    public void Dispose()
    {
        if (child is { IsClosed: false } peer) peer.Stop();
        child = null;
        Ready = false;
    }
}
