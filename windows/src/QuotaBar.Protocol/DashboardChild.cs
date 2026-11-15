using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
using QuotaBar.Core;

namespace QuotaBar.Protocol;

public sealed class DashboardChild : SafeHandleZeroOrMinusOneIsInvalid
{
    private DashboardChild() : base(true) { }
    protected override bool ReleaseHandle() { Free(handle); return true; }

    private enum Operation { Ready, Close, Stop, Wait }
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_spawn")]
    private static extern DashboardChild Spawn([MarshalAs(UnmanagedType.LPUTF8Str)] string path,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? capture, out int osError);
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_free")] private static extern void Free(IntPtr child);
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_pid")] private static extern uint ProcessId(DashboardChild child);
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_control")] private static extern void Control(DashboardChild child, Operation action);
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_send")] private static extern int Write(DashboardChild child, byte[] data, nuint length);
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_read")] private static extern int Read(DashboardChild child, out IntPtr data, out nuint length);
    [DllImport("quotabar_native", EntryPoint = "qb_secret_free")] private static extern void ReleaseBytes(IntPtr data, nuint length);

    public static DashboardChild Start(string executable, string? capture = null)
    {
        var peer = Spawn(executable, capture, out int error);
        if (!peer.IsInvalid) return peer;
        peer.Dispose();
        throw new Win32Exception(error);
    }

    public int? Pid => ProcessId(this) is uint id and > 0 ? (int)id : null;
    public void Ready() => Control(this, Operation.Ready);
    public void RequestClose() => Control(this, Operation.Close);
    public void Stop() => Control(this, Operation.Stop);
    public void WaitForExit() => Control(this, Operation.Wait);
    public bool Send(Message message)
    {
        var bytes = Json.Bytes(message);
        return Write(this, bytes, (nuint)bytes.Length) == 0;
    }
    public Message? ReadNext()
    {
        int result = Read(this, out var pointer, out var length);
        if (result == 1) return null;
        if (result != 0) throw new IOException("Invalid dashboard frame or disconnected pipe");
        try
        {
            var data = new byte[checked((int)length)];
            Marshal.Copy(pointer, data, 0, data.Length);
            var message = Json.Read<Message>(data);
            if (message.Version != 1) throw new InvalidDataException("Unsupported protocol version");
            return message;
        }
        finally { ReleaseBytes(pointer, length); }
    }
}
