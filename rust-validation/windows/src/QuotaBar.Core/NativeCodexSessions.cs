using Microsoft.Win32.SafeHandles;
using System.Runtime.ExceptionServices;
using System.Runtime.InteropServices;
using System.Text;

namespace QuotaBar.Core;

internal sealed class NativeCodexSessions(string? directory, ICredentialStore? injected)
{
    internal enum Event { Invalidate, Revoke, End }
    private const string Library = "quotabar_native";
    private sealed class Handle() : SafeHandleZeroOrMinusOneIsInvalid(true)
    {
        protected override bool ReleaseHandle() { Free(handle); return true; }
    }
    private readonly Handle handle = Create(0, directory);

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int Access(IntPtr context, [MarshalAs(UnmanagedType.LPUTF8Str)] string id,
        IntPtr data, nuint count, IntPtr reader);

    [StructLayout(LayoutKind.Sequential)]
    private sealed class Callback(Access access)
    {
        public IntPtr Context = IntPtr.Zero;
        public Access AccessVault = access;
    }

    [DllImport(Library, EntryPoint = "qb_sessions_new")]
    private static extern Handle Create(int mac, [MarshalAs(UnmanagedType.LPUTF8Str)] string? directory);
    [DllImport(Library, EntryPoint = "qb_sessions_free")] private static extern void Free(IntPtr value);
    [DllImport(Library, EntryPoint = "qb_sessions_enter_windows")]
    private static extern int Enqueue(Handle h, [MarshalAs(UnmanagedType.LPUTF8Str)] string id, IntPtr waiter);
    [DllImport(Library, EntryPoint = "qb_sessions_leave_windows")]
    private static extern IntPtr Advance(Handle h, [MarshalAs(UnmanagedType.LPUTF8Str)] string id);
    [DllImport(Library, EntryPoint = "qb_sessions_begin")]
    private static extern int Start(Handle h, [MarshalAs(UnmanagedType.LPUTF8Str)] string id, byte[] identity, nuint identityLength,
        byte[]? rejected, nuint rejectedLength, Callback? store, out ulong ticket, out IntPtr output, out nuint length);
    [DllImport(Library, EntryPoint = "qb_sessions_complete")]
    private static extern int Commit(Handle h, [MarshalAs(UnmanagedType.LPUTF8Str)] string id, ulong ticket,
        byte[] replacement, nuint length, Callback? store);
    [DllImport(Library, EntryPoint = "qb_sessions_flush")]
    private static extern int FlushStore(Handle h, Callback? store);
    [DllImport(Library, EntryPoint = "qb_sessions_event")]
    private static extern ulong Signal(Handle h, [MarshalAs(UnmanagedType.LPUTF8Str)] string id, ulong ticket, Event action);
    [DllImport(Library, EntryPoint = "qb_sessions_loaded")]
    private static extern int Loaded(IntPtr reader, byte[] bytes, nuint length);

    internal Task Enter(string id)
    {
        var waiter = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var cookie = GCHandle.Alloc(waiter);
        bool queued = false;
        try
        {
            int result = Enqueue(handle, id, GCHandle.ToIntPtr(cookie));
            if (result < 0) throw new InvalidOperationException("Codex session queue unavailable.");
            queued = result == 1;
            return queued ? waiter.Task : Task.CompletedTask;
        }
        finally { if (!queued) cookie.Free(); }
    }

    internal void Leave(string id)
    {
        var next = Advance(handle, id);
        if (next == IntPtr.Zero) return;
        var cookie = GCHandle.FromIntPtr(next);
        var waiter = (TaskCompletionSource)cookie.Target!;
        cookie.Free();
        waiter.SetResult();
    }

    internal static int Check(int status)
    {
        if (status >= 0) return status;
        throw status switch
        {
            -50 => new UsageException("missing_credentials", "로그인을 다시 연결하세요."),
            -51 => new UsageException("account_mismatch", "저장된 계정이 다릅니다."),
            -52 => new UsageException("reauth_required", "앱에서 다시 로그인하세요."),
            -53 => new OperationCanceledException(),
            -54 => new IOException("인증 파일을 보호하거나 열 수 없습니다. 다시 연결하세요."),
            -55 => new IOException("인증 파일이 너무 큽니다."),
            _ => Credentials.Failure(status)
        };
    }

    private int WithStore(Func<Callback?, int> invoke)
    {
        if (injected == null) return Check(invoke(null));
        ExceptionDispatchInfo? failure = null;
        var callback = new Callback((_, uuid, bytes, count, reader) =>
        {
            try
            {
                var id = Guid.Parse(uuid);
                if (reader != IntPtr.Zero)
                {
                    var data = injected.Load(id);
                    return data == null ? 1 : Loaded(reader, data, (nuint)data.Length);
                }
                byte[] replacement = new byte[checked((int)count)];
                if (count != 0) Marshal.Copy(bytes, replacement, 0, replacement.Length);
                injected.Save(id, replacement);
                return 0;
            }
            catch (Exception error) { failure = ExceptionDispatchInfo.Capture(error); return -1; }
        });
        int result = invoke(callback);
        failure?.Throw();
        return Check(result);
    }

    internal byte[] Begin(string id, string identity, byte[]? rejected, out ulong ticket)
    {
        byte[] key = Encoding.UTF8.GetBytes(identity);
        ulong generation = 0;
        IntPtr output = IntPtr.Zero;
        nuint count = 0;
        int status = WithStore(store => Start(handle, id, key, (nuint)key.Length, rejected, (nuint)(rejected?.Length ?? 0),
            store, out generation, out output, out count));
        if (status == 2) throw new InvalidOperationException("Concurrent Windows Codex session admission.");
        ticket = generation;
        return Credentials.TakeBytes(0, output, count);
    }

    internal void Complete(string id, ulong ticket, byte[] data) =>
        WithStore(store => Commit(handle, id, ticket, data, (nuint)data.Length, store));
    internal void Flush() => WithStore(store => FlushStore(handle, store));
    internal void Notify(string id, ulong ticket, Event action) => Signal(handle, id, ticket, action);
}
