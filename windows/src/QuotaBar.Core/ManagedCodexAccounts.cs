using Microsoft.Win32.SafeHandles;
using System.Runtime.ExceptionServices;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;

namespace QuotaBar.Core;

public sealed class ManagedCodexAccounts(string directory, ProviderClient client)
{
    private const string Library = "quotabar_native";
    private sealed class Handle() : SafeHandleZeroOrMinusOneIsInvalid(true)
    {
        protected override bool ReleaseHandle() { Free(handle); return true; }
    }
    private readonly Handle handle = Create(directory);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int SendHttp(IntPtr context, IntPtr request, nuint size, int rotation, IntPtr body);
    [DllImport(Library, EntryPoint = "qb_managed_accounts_new")]
    private static extern Handle Create([MarshalAs(UnmanagedType.LPUTF8Str)] string directory);
    [DllImport(Library, EntryPoint = "qb_managed_accounts_free")] private static extern void Free(IntPtr handle);
    [DllImport(Library, EntryPoint = "qb_managed_accounts_invalidate")]
    private static extern void Invalidate(Handle h, [MarshalAs(UnmanagedType.LPUTF8Str)] string id);
    [DllImport(Library, EntryPoint = "qb_managed_accounts_flush")] private static extern int Flush(Handle h);
    [DllImport(Library, EntryPoint = "qb_managed_accounts_fetch")]
    private static extern int Run(Handle h, [MarshalAs(UnmanagedType.LPUTF8Str)] string id, byte[] key, nuint size,
        NativeCancellation cancel, IntPtr context, SendHttp? send, out int osError, out IntPtr output, out nuint count);
    [DllImport(Library, EntryPoint = "qb_sessions_loaded")]
    private static extern int SetBody(IntPtr receiver, byte[] body, nuint length);

    public void Invalidate(Guid id) => Invalidate(handle, id.ToString("D"));
    public void FlushPendingWrites() => NativeCodexSessions.Check(Flush(handle));

    public Task<Snapshot> Fetch(Account account, CancellationToken token) => Task.Run(() =>
    {
        using var cancel = NativeCancellation.Create();
        using var registration = token.Register(cancel.Cancel);
        ExceptionDispatchInfo? transportError = null;

        int Send(IntPtr _, IntPtr request, nuint length, int rotation, IntPtr receiver)
        {
            try
            {
                var encoded = new byte[checked((int)length)];
                NativeProviderQuery.Request description;
                try { Marshal.Copy(request, encoded, 0, encoded.Length); description = Json.Read<NativeProviderQuery.Request>(encoded); }
                finally { CryptographicOperations.ZeroMemory(encoded); }
                var (status, body) = client.Send(description, rotation != 0 ? CancellationToken.None : token).GetAwaiter().GetResult();
                try { return SetBody(receiver, body, (nuint)body.Length) == 0 ? status : -40; }
                finally { CryptographicOperations.ZeroMemory(body); }
            }
            catch (Exception error)
            {
                transportError = ExceptionDispatchInfo.Capture(error);
                return error is OperationCanceledException ? -41 : -40;
            }
        }

        var key = Encoding.UTF8.GetBytes(account.Identity);
        int status = Run(handle, account.Id.ToString("D"), key, (nuint)key.Length, cancel, IntPtr.Zero,
            client.HasCustomTransport ? Send : null, out int osError, out var output, out var size);
        transportError?.Throw();
        if (status is >= -55 and <= -50) NativeCodexSessions.Check(status);
        return Json.Read<Snapshot>(NativeProviderQuery.NativeResult(status, osError, output, size, token));
    });
}
