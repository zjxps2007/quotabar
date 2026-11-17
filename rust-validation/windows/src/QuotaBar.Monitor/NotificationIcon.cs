using System.Runtime.InteropServices;
using System.Drawing;
using QuotaBar.Core;

namespace QuotaBar.Monitor;

internal sealed class NotificationIcon : IDisposable
{
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void OpenMenu(int x, int y);
    private readonly OpenMenu callback;
    private readonly TrayMenu menu;
    private IntPtr handle;
    public NotificationIcon(TrayMenu menu)
    {
        this.menu = menu;
        callback = (x, y) => menu.Show(new Point(x, y));
        handle = Create(callback);
        if (handle == IntPtr.Zero) throw new InvalidOperationException("트레이 알림 창을 만들 수 없습니다.");
        menu.KeyboardClosed += Closed;
    }
    public string Text { get; private set; } = "";
    public int PixelSize { get; private set; }
    public bool Visible => handle != 0 && IsVisible(handle) != 0;
    public IntPtr Window => WindowHandle(handle);
    public void Publish(TraySummary summary)
    {
        int size = Update(handle, summary.Text, summary.UseLogo ? 1 : 0, summary.Warning ? 1 : 0, summary.Tooltip);
        if (size < 0) throw new InvalidOperationException("트레이 아이콘을 그릴 수 없습니다.");
        Text = summary.Tooltip;
        PixelSize = size;
    }
    private void Closed()
    {
        if (handle != 0) FocusIcon(handle);
    }
    public void Dispose()
    {
        if (handle == 0) return;
        menu.KeyboardClosed -= Closed;
        Free(handle);
        handle = 0;
        GC.KeepAlive(callback);
    }
    [DllImport("quotabar_native", EntryPoint = "qb_tray_create")]
    private static extern IntPtr Create(OpenMenu callback);
    [DllImport("quotabar_native", EntryPoint = "qb_tray_update")]
    private static extern int Update(IntPtr tray, [MarshalAs(UnmanagedType.LPUTF8Str)] string label, int logo, int warning,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string tooltip);
    [DllImport("quotabar_native", EntryPoint = "qb_tray_window")]
    private static extern IntPtr WindowHandle(IntPtr tray);
    [DllImport("quotabar_native", EntryPoint = "qb_tray_visible")]
    private static extern int IsVisible(IntPtr tray);
    [DllImport("quotabar_native", EntryPoint = "qb_tray_focus")]
    private static extern void FocusIcon(IntPtr tray);
    [DllImport("quotabar_native", EntryPoint = "qb_tray_free")]
    private static extern void Free(IntPtr tray);
}
