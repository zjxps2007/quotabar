using System.Drawing;
using System.Text;
using System.Runtime.InteropServices;
using QuotaBar.Core;

namespace QuotaBar.Monitor;

// Runs only for --probe with synthetic accounts and sends shell-format events to the app's own window.
internal static class TrayMenuProbe
{
    private enum Keys { Escape = 0x1b, End = 0x23, Home = 0x24, Up = 0x26 }
    [DllImport("quotabar_native", EntryPoint = "qb_tray_samples")]
    private static extern int RenderSamples([MarshalAs(UnmanagedType.LPUTF8Str)] string directory);
    public static void SaveSamples(string directory)
    {
        if (RenderSamples(directory) != 0) throw new IOException("트레이 아이콘 화면을 저장하지 못했습니다.");
    }
    internal sealed record Result(bool LeftClickIgnored, bool LeftDoubleClickIgnored, bool RightClickOpens,
        bool EscapeDismisses, bool OverflowReachable, bool LiveUpdate, bool ActionsDispatch, int Dpi, List<string> Captures);
    public static async Task<Result> Run(NotificationIcon icon, TrayMenu menu, Func<DisplayState> getState, Action<DisplayState> setState,
        Func<bool> dashboardRunning, string directory)
    {
        var original = getState(); var captures = new List<string>();
        // This probe sends shell-format notifications to its own monitor. Anchor them
        // near the taskbar even when the interactive desktop has no accessible cursor.
        var area = WorkArea(icon.Window);
        int anchor = ((area.Right - 2) & 0xffff) | (((area.Bottom - 2) & 0xffff) << 16);
        void Mouse(int message)
        {
            SendMessage(icon.Window, 0x8018, anchor, 0x10000 | message);
        }
        void Check(bool value, string error) { if (!value) throw new InvalidOperationException("Tray menu: " + error); }
        async Task Open()
        {
            Mouse(0x204); Mouse(0x205); Mouse(0x7b); // Right click and the shell v4 WM_CONTEXTMENU notification.
            await Task.Delay(50); Check(menu.Visible, "right-click did not open the menu");
            Check(!dashboardRunning(), "opening the tray menu started Dashboard");
        }
        void Capture(string name)
        {
            menu.Capture(Path.Combine(directory, name + ".png")); captures.Add(name);
        }
        try
        {
            Mouse(0x201); Mouse(0x202); await Task.Delay(50);
            Check(!menu.Visible && !dashboardRunning(), "left-click opened a window");
            Mouse(0x203); Mouse(0x202); await Task.Delay(50);
            Check(!menu.Visible && !dashboardRunning(), "left double-click opened a window");
            foreach (string theme in new[] { "light", "dark" })
            {
                setState(original with { Settings = original.Settings with { Appearance = theme } });
                await Open();
                Check(Children(menu.Window).Count(i => i.Label.Contains("Dashboard에서 보기")) == 3, "account cards missing");
                Check(WorkArea(menu.Window).Contains(Bounds(menu.Window)), "menu escaped working area");
                Capture("tray-menu-" + theme); menu.Close();
                var singlePeriods = original.Snapshots.ToDictionary(pair => pair.Key, pair =>
                    pair.Key == original.Settings.Accounts[0].Id ? pair.Value : pair.Value with { Windows = [pair.Value.Windows[^1]] });
                setState(original with { Settings = original.Settings with { Appearance = theme }, Snapshots = singlePeriods });
                await Open(); Capture("tray-menu-single-" + theme); menu.Close();
                setState(original with { Settings = original.Settings with { Accounts = [], Appearance = theme }, Snapshots = [], Errors = [] });
                await Open(); Capture("tray-menu-empty-" + theme); menu.Close();
            }
            var accounts = original.Settings.Accounts;
            var snapshots = new Dictionary<Guid, Snapshot>(original.Snapshots);
            snapshots.Remove(accounts[1].Id);
            setState(original with { Settings = original.Settings with { Appearance = "dark", Accounts = accounts.Select((a, index) => a with
                { Name = index == 0 ? "여러 프로젝트에서 사용하는 아주 긴 계정 이름 · 한글과 English 표시 확인" : a.Name, Enabled = index != 2 }).ToList() },
                Snapshots = snapshots, Errors = new() { [accounts[0].Id] = "offline" }, Loading = [accounts[1].Id] });
            await Open(); Capture("tray-menu-states-dark");
            SendMessage(menu.Window, 0x100, (int)Keys.End, 0);
            SendMessage(menu.Window, 0x100, (int)Keys.Up, 0);
            var updated = getState(); updated.Snapshots[accounts[0].Id] = new(DateTimeOffset.UtcNow, [new("5시간", 80.5)]);
            setState(updated); menu.RefreshIfVisible();
            Check(menu.Visible && Children(menu.Window).Any(i => i.Label.Contains("81%")), "live state update lost account data");
            menu.Close();

            var many = Enumerable.Range(0, 15).Select(i => accounts[i % accounts.Count] with { Id = Guid.NewGuid(), Name = $"스크롤 확인 계정 {i + 1}" }).ToList();
            setState(original with { Settings = original.Settings with { Accounts = many, Appearance = "dark" },
                Snapshots = many.ToDictionary(a => a.Id, _ => original.Snapshots[accounts[0].Id]) });
            await Open();
            var scrollButtons = Children(menu.Window);
            var up = scrollButtons.Single(i => i.Label == "위로 스크롤").Handle;
            var down = scrollButtons.Single(i => i.Label == "아래로 스크롤").Handle;
            Check(IsWindowVisible(up) && !IsWindowEnabled(up) && IsWindowEnabled(down), "initial overflow arrows are incorrect");
            var first = scrollButtons.First(i => i.Label.Contains("스크롤 확인 계정 1.")).Handle;
            int start = Bounds(first).Top;
            SendMessage(down, 0xF5, 0, 0);
            Check(Bounds(first).Top < start && IsWindowEnabled(up), "down arrow did not scroll");
            SendMessage(up, 0xF5, 0, 0);
            Check(Bounds(first).Top == start && !IsWindowEnabled(up), "up arrow did not return to the first row");
            SendMessage(menu.Window, 0x100, (int)Keys.End, 0);
            await Task.Delay(50);
            Capture("tray-menu-overflow-dark");
            Check(WorkArea(menu.Window).Contains(Bounds(menu.Window)), "overflow menu escaped working area");
            Check(Bounds(menu.Window).Contains(Bounds(Children(menu.Window).Single(i => i.Label == "종료").Handle)), "last action is unreachable with many accounts");
            Check(!IsWindowEnabled(down) && IsWindowEnabled(up), "last row did not disable the down arrow");
            SendMessage(menu.Window, 0x100, (int)Keys.Home, 0);
            Check(IsWindowVisible(first) && Bounds(menu.Window).Contains(Bounds(first)), "Home did not reveal the first account");
            SendMessage(menu.Window, 0x100, (int)Keys.Escape, 0);
            await Task.Delay(50); Check(!menu.Visible, "Escape did not dismiss the menu");

            var routes = new List<string>(); int quit = 0;
            using var actions = new TrayMenu(() => original, routes.Add, () => { quit++; return Task.CompletedTask; });
            foreach (string label in new[] { "Dashboard 열기", "종료" })
            {
                actions.Show(new Point(40, 40));
                var items = Children(actions.Window);
                Check(items.Where(i => IsWindowVisible(i.Handle) && !i.Label.Contains("Dashboard에서 보기")).Select(i => i.Label).Order().SequenceEqual(new[] { "Dashboard 열기", "종료" }.Order()), "unexpected menu action remains");
                SendMessage(items.Single(i => i.Label == label).Handle, 0xF5, 0, 0); // BM_CLICK
                actions.Close();
            }
            Check(routes.SequenceEqual(new[] { "dashboard" }) && quit == 1, "an action dispatched incorrectly");
            return new(true, true, true, true, true, true, true, GetDpiForWindow(menu.Window), captures);
        }
        finally { menu.Close(); setState(original); }
    }
    [DllImport("user32.dll")] private static extern nint SendMessage(nint window, uint message, nint wParam, nint lParam);
    [StructLayout(LayoutKind.Sequential)] private struct Rect { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] private struct MonitorInfo { public int Size; public Rect Monitor, Work; public uint Flags; }
    [DllImport("user32.dll")] private static extern nint MonitorFromWindow(nint window, uint flags);
    [DllImport("user32.dll", EntryPoint = "GetMonitorInfoW")] private static extern bool GetMonitorInfo(nint monitor, ref MonitorInfo info);
    private static Rectangle WorkArea(nint window)
    {
        var info = new MonitorInfo { Size = Marshal.SizeOf<MonitorInfo>() };
        if (!GetMonitorInfo(MonitorFromWindow(window, 2), ref info)) throw new InvalidOperationException("No monitor work area.");
        return Rectangle.FromLTRB(info.Work.Left, info.Work.Top, info.Work.Right, info.Work.Bottom);
    }
    [DllImport("user32.dll")] private static extern bool GetWindowRect(nint window, out Rect rect);
    [DllImport("user32.dll")] private static extern int GetDpiForWindow(nint window);
    [DllImport("user32.dll")] private static extern bool IsWindowVisible(nint window);
    [DllImport("user32.dll")] private static extern bool IsWindowEnabled(nint window);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(nint window, StringBuilder name, int size);
    private delegate bool Enumerate(nint window, nint parameter);
    [DllImport("user32.dll")] private static extern bool EnumChildWindows(nint window, Enumerate visitor, nint parameter);
    private static Rectangle Bounds(nint window)
    { GetWindowRect(window, out var r); return Rectangle.FromLTRB(r.Left, r.Top, r.Right, r.Bottom); }
    private static List<(nint Handle, string Label)> Children(nint parent)
    {
        var items = new List<(nint, string)>();
        EnumChildWindows(parent, (h, _) => { var name = new StringBuilder(2048); GetWindowText(h, name, name.Capacity); items.Add((h, name.ToString())); return true; }, 0);
        return items;
    }
}
