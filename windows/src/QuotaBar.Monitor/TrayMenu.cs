using System.Runtime.InteropServices;
using System.Drawing;
using QuotaBar.Core;

namespace QuotaBar.Monitor;

internal sealed class TrayMenu : IDisposable
{
    [DllImport("quotabar_native", EntryPoint = "qb_monitor_message")]
    internal static extern void ShowMessage([MarshalAs(UnmanagedType.LPUTF8Str)] string message, int error = 0);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void Chosen(int action);
    private readonly Chosen callback;
    private readonly Func<DisplayState> state;
    private nint handle;
    public event Action? KeyboardClosed;
    public TrayMenu(Func<DisplayState> state, Action<string> navigate, Func<Task> quit)
    {
        this.state = state;
        callback = action =>
        {
            if (action == 0) KeyboardClosed?.Invoke();
            else if (action == 1) navigate("dashboard");
            else if (action == 2) _ = quit();
        };
        handle = Create(callback);
        if (handle == 0) throw new InvalidOperationException("트레이 메뉴를 만들 수 없습니다.");
    }
    public nint Window => WindowHandle(handle);
    public bool Visible => handle != 0 && IsWindowVisible(Window);
    public void Show(Point point) { Publish(); Open(handle, point.X, point.Y); }
    public void Close() => Hide(handle);
    public void Capture(string path)
    {
        if (SaveImage(handle, path) != 0) throw new IOException("트레이 메뉴 화면을 저장하지 못했습니다.");
    }
    public void RefreshIfVisible() { if (Visible) Publish(); }
    private void Publish()
    {
        var stateBytes = Json.Bytes(state());
        if (SetState(handle, stateBytes, (nuint)stateBytes.Length) != 0) throw new InvalidDataException("트레이 상태를 읽지 못했습니다.");
    }
    public void Dispose()
    {
        if (handle != 0) { Free(handle); handle = 0; }
        GC.KeepAlive(callback);
    }
    [DllImport("quotabar_native", EntryPoint = "qb_menu_create")] private static extern nint Create(Chosen action);
    [DllImport("quotabar_native", EntryPoint = "qb_menu_window")] private static extern nint WindowHandle(nint menu);
    [DllImport("quotabar_native", EntryPoint = "qb_menu_state")] private static extern int SetState(nint menu, byte[] data, nuint length);
    [DllImport("quotabar_native", EntryPoint = "qb_menu_show")] private static extern void Open(nint menu, int x, int y);
    [DllImport("quotabar_native", EntryPoint = "qb_menu_close")] private static extern void Hide(nint menu);
    [DllImport("quotabar_native", EntryPoint = "qb_menu_capture")] private static extern int SaveImage(nint menu, [MarshalAs(UnmanagedType.LPUTF8Str)] string path);
    [DllImport("quotabar_native", EntryPoint = "qb_menu_free")] private static extern void Free(nint menu);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(nint window);
}
