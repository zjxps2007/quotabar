using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

namespace QuotaBar.Windows;

public sealed class SingleInstance : IDisposable
{
    private sealed class InstanceHandle : SafeHandleZeroOrMinusOneIsInvalid
    {
        private InstanceHandle() : base(true) { }
        protected override bool ReleaseHandle() { Free(handle); return true; }
    }

    private readonly InstanceHandle handle;
    private int disposed;
    public bool IsOwner { get; }

    public SingleInstance(AppPaths paths)
    {
        handle = Acquire(paths.Root, out int owner, out int error);
        if (handle.IsInvalid)
        {
            handle.Dispose();
            throw new IOException("앱 데이터 폴더의 소유자와 권한을 확인하세요.", new Win32Exception(error));
        }
        IsOwner = owner != 0;
    }

    public Task Activate() => Task.Run(() =>
    {
        int error = Send(handle);
        if (error != 0) throw new IOException("실행 중인 QuotaBar에 연결하지 못했습니다.", new Win32Exception(error));
    });

    public async Task Listen(Action activate)
    {
        while (Volatile.Read(ref disposed) == 0)
        {
            int result;
            try { result = await Task.Run(() => Wait(handle)); }
            catch (ObjectDisposedException) when (Volatile.Read(ref disposed) != 0) { return; }
            if (result == 0 || Volatile.Read(ref disposed) != 0) return;
            if (result < 0) throw new IOException("앱 활성화 요청을 읽지 못했습니다.");
            activate();
        }
    }

    public void Dispose()
    {
        if (Interlocked.Exchange(ref disposed, 1) != 0) return;
        Stop(handle);
        handle.Dispose();
    }

    [DllImport("quotabar_native", EntryPoint = "qb_instance_acquire")]
    private static extern InstanceHandle Acquire([MarshalAs(UnmanagedType.LPUTF8Str)] string directory, out int owner, out int error);
    [DllImport("quotabar_native", EntryPoint = "qb_instance_activate")] private static extern int Send(InstanceHandle instance);
    [DllImport("quotabar_native", EntryPoint = "qb_instance_wait")] private static extern int Wait(InstanceHandle instance);
    [DllImport("quotabar_native", EntryPoint = "qb_instance_stop")] private static extern void Stop(InstanceHandle instance);
    [DllImport("quotabar_native", EntryPoint = "qb_instance_free")] private static extern void Free(IntPtr instance);
}
