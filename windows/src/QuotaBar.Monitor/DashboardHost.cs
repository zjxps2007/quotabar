using System.ComponentModel;
using System.Runtime.ExceptionServices;
using System.Runtime.InteropServices;
using System.Text.Json;
using QuotaBar.Core;
using QuotaBar.Protocol;

namespace QuotaBar.Monitor;

// The Rust monitor loop owns the dashboard process, reader and ready/show handshake.
// Store commands and snapshots are delivered on that loop's owning thread.
internal sealed class DashboardHost : IDisposable
{
    private enum Event { Command, Publish, Exited }
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int Callback(Event kind, nint bytes, nuint length);
    private readonly MonitorLoop dispatcher;
    private readonly Func<DisplayState> state;
    private readonly string? capture;
    private readonly Callback callback;
    private readonly string executable = Path.Combine(AppContext.BaseDirectory, "Dashboard", "QuotaBar.Dashboard.exe");
    private ExceptionDispatchInfo? failure;

    public DashboardHost(MonitorLoop dispatcher, Func<DisplayState> state, string? capture = null)
    {
        this.dispatcher = dispatcher;
        this.state = state;
        this.capture = capture;
        callback = Receive;
        dispatcher.Received += Dispatch;
    }
    public event Action<Message>? Command;
    public event Action? Exited;
    public bool Ready { get { Status(dispatcher.Handle, out int ready); return ready != 0; } }
    public int? Pid => Status(dispatcher.Handle, out _) is uint id and > 0 ? (int)id : null;
    public bool IsRunning => Pid != null;

    public void Show(string destination = "dashboard")
    {
        int error = ShowNative(dispatcher.Handle, executable, capture, destination);
        if (error == -1) throw new UsageException("dashboard_missing", "Dashboard 파일을 찾지 못했습니다. 패키지를 다시 설치하세요.");
        if (error != 0) throw new Win32Exception(error);
    }

    private void Dispatch(MonitorLoop.Event kind)
    {
        if (kind != MonitorLoop.Event.Dashboard) return;
        DispatchNative(dispatcher.Handle, callback);
        failure?.Throw();
    }

    private int Receive(Event kind, nint bytes, nuint length)
    {
        try
        {
            switch (kind)
            {
                case Event.Publish: Publish(); break;
                case Event.Exited: Exited?.Invoke(); break;
                case Event.Command:
                    var data = new byte[checked((int)length)];
                    Marshal.Copy(bytes, data, 0, data.Length);
                    Message message;
                    try { message = Json.Read<Message>(data); }
                    catch (Exception e) when (e is JsonException or UsageException) { return -1; }
                    if (message.Version != 1) return -1;
                    if (message.Type == "ready") return 1;
                    Command?.Invoke(message);
                    break;
            }
            return 0;
        }
        catch (Exception e)
        {
            // Report application exceptions after returning from the unmanaged callback.
            failure = ExceptionDispatchInfo.Capture(e);
            return -1;
        }
    }
    public void Publish() { if (Ready) Send("state", state()); }
    public void Send<T>(string type, T value, Guid? requestId = null)
    {
        if (!IsRunning) return;
        var bytes = Json.Bytes(Message.Create(type, value, requestId));
        SendNative(dispatcher.Handle, bytes, (nuint)bytes.Length);
    }
    public void Close() => CloseNative(dispatcher.Handle, 0);
    public void Dispose()
    {
        dispatcher.Received -= Dispatch;
        CloseNative(dispatcher.Handle, 1);
    }

    [DllImport("quotabar_native", EntryPoint = "qb_loop_dashboard_show")]
    private static extern int ShowNative(nint loop, [MarshalAs(UnmanagedType.LPUTF8Str)] string exe,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? capture, [MarshalAs(UnmanagedType.LPUTF8Str)] string destination);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_dashboard_status")] private static extern uint Status(nint loop, out int ready);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_dashboard_dispatch")] private static extern void DispatchNative(nint loop, Callback callback);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_dashboard_send")] private static extern void SendNative(nint loop, byte[] bytes, nuint length);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_dashboard_close")] private static extern void CloseNative(nint loop, int force);
}
