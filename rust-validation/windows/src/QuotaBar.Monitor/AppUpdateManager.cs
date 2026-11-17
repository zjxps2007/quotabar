using System.Runtime.InteropServices;
using System.Text.Json;
using Microsoft.Win32.SafeHandles;
using QuotaBar.Core;
using QuotaBar.Windows;

namespace QuotaBar.Monitor;

// The native controller and the dashboard share the Win32 owner thread. The
// account service acknowledges a drain before the controller can release the app.
internal sealed class AppUpdateManager : IDisposable
{
    private const string Library = "quotabar_native";
    private sealed class Owner() : SafeHandleZeroOrMinusOneIsInvalid(true)
    {
        protected override bool ReleaseHandle() { Free(handle); return true; }
    }
    private enum Event { None, State, PrepareExit, Resume, Exit }
    private readonly Owner owner;
    private readonly MonitorLoop dispatcher;
    private readonly Action changed, resume;
    private readonly Func<Task> prepareExit, exit;
    private bool delivering, disposed;
    public AppUpdateState State { get; private set; }

    public AppUpdateManager(MonitorLoop dispatcher, AppPaths paths, Action changed, Func<Task> prepareExit, Action resume, Func<Task> exit)
    {
        this.dispatcher = dispatcher;
        this.changed = changed;
        this.prepareExit = prepareExit;
        this.resume = resume;
        this.exit = exit;
        owner = Create(dispatcher.Handle, paths.Root, AppContext.BaseDirectory, Brand.Version);
        if (owner.IsInvalid) throw new IOException("Could not start the Windows update controller.");
        Next(owner, 1, out var initial);
        State = ReadState(initial);
        dispatcher.Received += OnSystemEvent;
    }

    public void Configure(bool enabled, bool canInstall, bool suspended)
    {
        if (disposed) return;
        ConfigureNative(owner, enabled, canInstall, suspended);
        Deliver();
    }
    public void Check() => BeginCheck(true, false);
    public void InstallNow() => BeginCheck(true, true);
    private void BeginCheck(bool manual, bool install)
    {
        if (disposed) return;
        CheckNative(owner, manual, install);
        Deliver();
    }
    private void OnSystemEvent(MonitorLoop.Event kind)
    {
        if (kind == MonitorLoop.Event.Update) BeginCheck(false, false);
        if (kind == MonitorLoop.Event.UpdateWork && !disposed) { Dispatch(owner); Deliver(); }
    }

    private void Deliver()
    {
        if (delivering || disposed) return;
        delivering = true;
        try
        {
            while (!disposed)
            {
                switch (Next(owner, 0, out var json))
                {
                    case Event.State: State = ReadState(json); changed(); break;
                    case Event.PrepareExit: _ = Drain(); break;
                    case Event.Resume: resume(); break;
                    case Event.Exit: _ = Exit(); break;
                    default: dispatcher.Schedule(MonitorLoop.Timer.Update, checked((int)Delay(owner))); return;
                }
            }
        }
        finally { delivering = false; }
    }
    private async Task Drain()
    {
        string? error = null;
        try { await prepareExit(); }
        catch (Exception e) { error = e is UsageException ? e.Message : "자동 업데이트를 완료하지 못했습니다. 현재 앱을 유지합니다."; }
        if (!disposed) { Drained(owner, error); Deliver(); }
    }
    private async Task Exit()
    {
        bool success = false;
        try { await exit(); success = true; }
        finally { if (!disposed) { Exited(owner, success); Deliver(); } }
    }
    private static AppUpdateState ReadState(nint json)
    {
        try { return JsonSerializer.Deserialize<AppUpdateState>(Marshal.PtrToStringUTF8(json)!, Json.Options)!; }
        finally { FreeString(json); }
    }
    public void Dispose()
    {
        if (disposed) return;
        disposed = true;
        dispatcher.Received -= OnSystemEvent;
        dispatcher.Schedule(MonitorLoop.Timer.Update, 0);
        owner.Dispose();
    }

    [DllImport(Library, EntryPoint = "qb_windows_updater_new")]
    private static extern Owner Create(nint dispatcher, [MarshalAs(UnmanagedType.LPUTF8Str)] string root,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string target, [MarshalAs(UnmanagedType.LPUTF8Str)] string version);
    [DllImport(Library, EntryPoint = "qb_windows_updater_free")] private static extern void Free(nint owner);
    [DllImport(Library, EntryPoint = "qb_windows_updater_configure")] private static extern void ConfigureNative(Owner owner, bool enabled, bool idle, bool suspended);
    [DllImport(Library, EntryPoint = "qb_windows_updater_check")] private static extern void CheckNative(Owner owner, bool manual, bool install);
    [DllImport(Library, EntryPoint = "qb_windows_updater_dispatch")] private static extern void Dispatch(Owner owner);
    [DllImport(Library, EntryPoint = "qb_windows_updater_delay")] private static extern uint Delay(Owner owner);
    [DllImport(Library, EntryPoint = "qb_windows_updater_event")] private static extern Event Next(Owner owner, int initial, out nint json);
    [DllImport(Library, EntryPoint = "qb_string_free")] private static extern void FreeString(nint json);
    [DllImport(Library, EntryPoint = "qb_windows_updater_drained")] private static extern void Drained(Owner owner, [MarshalAs(UnmanagedType.LPUTF8Str)] string? failure);
    [DllImport(Library, EntryPoint = "qb_windows_updater_exited")] private static extern void Exited(Owner owner, bool success);
    [DllImport(Library, EntryPoint = "qb_windows_updater_probe")] internal static extern int Probe(
        [MarshalAs(UnmanagedType.LPUTF8Str)] string path, [MarshalAs(UnmanagedType.LPUTF8Str)] string version);
}
