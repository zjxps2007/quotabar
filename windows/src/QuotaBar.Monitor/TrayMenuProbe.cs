using System.Drawing.Imaging;
using System.Reflection;
using System.Runtime.InteropServices;
using QuotaBar.Core;

namespace QuotaBar.Monitor;

// Runs only for --probe with synthetic accounts; send callbacks to our own NotifyIcon, never to Explorer.
internal static class TrayMenuProbe
{
    internal sealed record Result(bool LeftClickIgnored, bool LeftDoubleClickIgnored, bool RightClickOpens,
        bool EscapeDismisses, bool OverflowReachable, bool LiveUpdate, bool ActionsDispatch, int Dpi, List<string> Captures);
    public static async Task<Result> Run(NotifyIcon icon, TrayMenu menu, Func<DisplayState> getState, Action<DisplayState> setState,
        Func<bool> dashboardRunning, string directory)
    {
        var original = getState(); var captures = new List<string>();
        var window = (NativeWindow)typeof(NotifyIcon).GetField("_window", BindingFlags.Instance | BindingFlags.NonPublic)!.GetValue(icon)!;
        void Mouse(int message) => SendMessage(window.Handle, 0x800, 0, message); // WinForms WM_TRAYMOUSEMESSAGE.
        void Check(bool value, string error) { if (!value) throw new InvalidOperationException("Tray menu: " + error); }
        async Task Open()
        {
            Mouse(0x204); Mouse(0x205); // WM_RBUTTONDOWN/UP uses NotifyIcon's normal ContextMenuStrip path.
            await Task.Delay(50); Check(menu.Visible, "right-click did not open the menu");
            Check(!dashboardRunning(), "opening the tray menu started Dashboard");
        }
        void Capture(string name)
        {
            menu.Update(); using var bitmap = new Bitmap(menu.Width, menu.Height);
            menu.DrawToBitmap(bitmap, new Rectangle(Point.Empty, bitmap.Size));
            bitmap.Save(Path.Combine(directory, name + ".png"), ImageFormat.Png); captures.Add(name);
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
                Check(menu.Items.Cast<ToolStripItem>().Count(i => i.Name?.StartsWith("account-") == true) == 3, "account cards missing");
                Check(Screen.FromControl(menu).WorkingArea.Contains(menu.Bounds), "menu escaped working area");
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
            menu.Items["dashboard"]!.Select();
            var updated = getState(); updated.Snapshots[accounts[0].Id] = new(DateTimeOffset.UtcNow, [new("5시간", 81)]);
            setState(updated); menu.RefreshIfVisible();
            Check(menu.Visible && menu.Items["dashboard"]!.Selected && menu.Items["account-" + accounts[0].Id]!.AccessibleName!.Contains("81%"), "live state update lost data or selection");
            menu.Close();

            var many = Enumerable.Range(0, 15).Select(i => accounts[i % accounts.Count] with { Id = Guid.NewGuid(), Name = $"스크롤 확인 계정 {i + 1}" }).ToList();
            setState(original with { Settings = original.Settings with { Accounts = many, Appearance = "dark" },
                Snapshots = many.ToDictionary(a => a.Id, _ => original.Snapshots[accounts[0].Id]) });
            await Open();
            typeof(ToolStripDropDown).GetMethod("ProcessDialogKey", BindingFlags.Instance | BindingFlags.NonPublic)!.Invoke(menu, [Keys.End]);
            await Task.Delay(50);
            Capture("tray-menu-overflow-dark");
            Check(Screen.FromControl(menu).WorkingArea.Contains(menu.Bounds), "overflow menu escaped working area");
            Check(menu.DisplayRectangle.IntersectsWith(menu.Items["quit"]!.Bounds), $"last action is unreachable with many accounts: menu={menu.Bounds}, display={menu.DisplayRectangle}, quit={menu.Items["quit"]!.Bounds}, selected={menu.Items["quit"]!.Selected}");
            typeof(ToolStripDropDown).GetMethod("ProcessDialogKey", BindingFlags.Instance | BindingFlags.NonPublic)!.Invoke(menu, [Keys.Escape]);
            await Task.Delay(50); Check(!menu.Visible, "Escape did not dismiss the menu");

            var routes = new List<string>(); int quit = 0;
            using var actions = new TrayMenu(() => original, routes.Add, () => { quit++; return Task.CompletedTask; });
            foreach (string id in new[] { "dashboard", "quit" })
            {
                actions.Show(new Point(40, 40));
                Check(actions.Items.Cast<ToolStripItem>().Where(i => i.CanSelect && i.Name?.StartsWith("account-") != true)
                    .Select(i => i.Name).SequenceEqual(new[] { "dashboard", "quit" }), "unexpected menu action remains");
                actions.Items[id]!.PerformClick(); actions.Close();
            }
            Check(routes.SequenceEqual(new[] { "dashboard" }) && quit == 1, "an action dispatched incorrectly");
            return new(true, true, true, true, true, true, true, menu.DeviceDpi, captures);
        }
        finally { menu.Close(); setState(original); }
    }
    [DllImport("user32.dll")] private static extern nint SendMessage(nint window, uint message, nint wParam, nint lParam);
}
