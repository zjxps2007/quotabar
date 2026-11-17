using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
using QuotaBar.Core;

namespace QuotaBar.Protocol;

public sealed class DashboardEndpoint : IDisposable
{
    private sealed class EndpointHandle() : SafeHandleZeroOrMinusOneIsInvalid(true)
    {
        protected override bool ReleaseHandle() { Free(handle); return true; }
    }
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_endpoint_open")]
    private static extern EndpointHandle Open(nuint input, nuint output);
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_endpoint_read")]
    private static extern int Read(EndpointHandle endpoint, out IntPtr data, out nuint length);
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_endpoint_send")]
    private static extern int Write(EndpointHandle endpoint, byte[] data, nuint length);
    [DllImport("quotabar_native", EntryPoint = "qb_dashboard_endpoint_free")]
    private static extern void Free(IntPtr handle);
    [DllImport("quotabar_native", EntryPoint = "qb_secret_free")]
    private static extern void FreeBytes(IntPtr data, nuint length);
    private readonly EndpointHandle handle;
    private readonly SemaphoreSlim writer = new(1);

    public DashboardEndpoint(string input, string output)
    {
        if (!nuint.TryParse(input, out var read) || !nuint.TryParse(output, out var write))
            throw new ArgumentException("Invalid dashboard pipe handles");
        handle = Open(read, write);
        if (handle.IsInvalid) { handle.Dispose(); throw new IOException("Dashboard pipes unavailable"); }
    }

    public async Task SendAsync(Message message)
    {
        var data = Json.Bytes(message);
        await writer.WaitAsync().ConfigureAwait(false);
        try
        {
            if (await Task.Run(() => Write(handle, data, (nuint)data.Length)).ConfigureAwait(false) != 0)
                throw new IOException("Dashboard pipe closed");
        }
        finally { writer.Release(); }
    }

    public async Task ReadAsync(Func<Message, Task> receive)
    {
        while (true)
        {
            int status = Read(handle, out var pointer, out var length);
            if (status == 1) return;
            if (status != 0) throw new IOException("Invalid dashboard frame or disconnected pipe");
            Message next;
            try
            {
                var body = new byte[checked((int)length)];
                Marshal.Copy(pointer, body, 0, body.Length);
                next = Json.Read<Message>(body);
            }
            finally { FreeBytes(pointer, length); }
            if (next.Version != 1) throw new InvalidDataException("Unsupported protocol version");
            await receive(next).ConfigureAwait(false);
        }
    }
    public void Dispose() => handle.Dispose();
}
