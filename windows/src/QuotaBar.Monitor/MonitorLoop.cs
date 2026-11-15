using System.Runtime.ExceptionServices;
using System.Runtime.InteropServices;

namespace QuotaBar.Monitor;

// Marshals async monitor continuations to the thread that owns the native tray and dashboard host.
internal sealed class MonitorLoop : SynchronizationContext, IDisposable
{
    internal enum Event { Work, Refresh, Update, Suspend, Resume, PowerStatus, Display, Dashboard }
    internal enum Timer { Refresh, Update }
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate void Callback(Event value);
    private readonly Queue<Action> pending = new();
    private readonly CancellationTokenSource stopped = new();
    private readonly int owner = Environment.CurrentManagedThreadId;
    private readonly SynchronizationContext? previous = Current;
    private readonly Callback onEvent;
    private nint handle, window;
    internal nint Handle => handle;
    private ExceptionDispatchInfo? failure;
    public event Action<Event>? Received;

    public MonitorLoop()
    {
        onEvent = Dispatch;
        handle = Create(onEvent, out window);
        if (handle == 0) throw new InvalidOperationException("모니터 이벤트 창을 만들 수 없습니다.");
        SetSynchronizationContext(this);
    }

    public bool TryPost(Action action)
    {
        lock (pending)
        {
            if (window == 0 || Wake(window) == 0) return false;
            pending.Enqueue(action);
            return true;
        }
    }
    public override void Post(SendOrPostCallback callback, object? state) => TryPost(() => callback(state));
    public override void Send(SendOrPostCallback callback, object? state)
    {
        if (Environment.CurrentManagedThreadId == owner) { callback(state); return; }
        var completion = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var token = stopped.Token;
        if (!TryPost(() =>
        {
            try { callback(state); completion.SetResult(); }
            catch (Exception e) { completion.SetException(e); }
        })) throw new ObjectDisposedException(nameof(MonitorLoop));
        completion.Task.WaitAsync(token).GetAwaiter().GetResult();
    }
    public override SynchronizationContext CreateCopy() => this;

    private void Dispatch(Event value)
    {
        try
        {
            if (value != Event.Work) { Received?.Invoke(value); return; }
            Action? action;
            lock (pending) pending.TryDequeue(out action);
            action?.Invoke();
        }
        catch (Exception e) { failure = ExceptionDispatchInfo.Capture(e); Exit(); }
    }

    public void Schedule(Timer timer, int milliseconds)
    {
        if (SetTimer(handle, (nuint)timer, checked((uint)milliseconds)) != 0)
            throw new InvalidOperationException("모니터 타이머를 설정하지 못했습니다.");
    }
    public void Run()
    {
        if (RunNative(handle) != 0) throw new InvalidOperationException("모니터 이벤트를 읽지 못했습니다.");
        failure?.Throw();
    }
    public void Exit() => Quit(handle);

    public void Dispose()
    {
        lock (pending)
        {
            if (window == 0) return;
            window = 0;
            pending.Clear();
        }
        stopped.Cancel();
        Free(handle);
        handle = 0;
        stopped.Dispose();
        SetSynchronizationContext(previous);
        GC.KeepAlive(onEvent);
    }

    [DllImport("quotabar_native", EntryPoint = "qb_loop_new")] private static extern nint Create(Callback callback, out nint window);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_wake")] private static extern int Wake(nint window);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_run")] private static extern int RunNative(nint loop);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_timer")] private static extern int SetTimer(nint loop, nuint index, uint delay);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_quit")] private static extern void Quit(nint loop);
    [DllImport("quotabar_native", EntryPoint = "qb_loop_free")] private static extern void Free(nint loop);
}
