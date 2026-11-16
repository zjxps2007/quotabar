using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using QuotaBar.Core;
using QuotaBar.Protocol;

internal static class DashboardLifecycle
{
    [StructLayout(LayoutKind.Sequential)]
    private struct CursorPoint { public int X, Y; }
    [DllImport("user32.dll", SetLastError = true)] private static extern bool GetCursorPos(out CursorPoint point);
    [DllImport("user32.dll")] private static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] private static extern int GetSystemMetrics(int index);
    public static async Task<int> Child(string input, string output, string mode)
    {
        using var peer = new DashboardEndpoint(input, output);
        if (mode == "never-ready") { await Task.Delay(60000); return 1; }
        await peer.SendAsync(Message.Create("ready", true));
        if (mode == "ignore-close") { await Task.Delay(60000); return 1; }
        try
        {
            await peer.ReadAsync(async next =>
            {
                if (next.Type == "close") throw new OperationCanceledException();
                await peer.SendAsync(next);
            });
        }
        catch (OperationCanceledException) { }
        return 0;
    }

    public static async Task RoundTrip()
    {
        using var child = DashboardChild.Start(Environment.ProcessPath!, "echo");
        await Task.Run(() =>
        {
            Suite.Check(child.ReadNext()?.Type == "ready");
            child.Ready();
            var id = Guid.NewGuid();
            var bodies = new[] { "첫 번째", new string('가', 170_000), "third" };
            foreach (var value in bodies) Suite.Check(child.Send(Message.Create("echo", value, id)));
            foreach (var expected in bodies)
            {
                var received = child.ReadNext();
                Suite.Check(received?.RequestId == id && received.Body<string>() == expected);
            }
            child.RequestClose();
            Suite.Check(child.ReadNext() == null);
            child.WaitForExit();
            Suite.Check(child.Pid == null);
        }).WaitAsync(TimeSpan.FromSeconds(15));
    }

    public static Task Deadlines() => Task.WhenAll(CheckDeadline(false), CheckDeadline(true));
    public static bool CompareScreens(string original, string candidate, string directory)
        => WithParkedCursor(() => CaptureScreens(original, candidate, directory), renderTargetsOnly: true);

    internal static bool WithParkedCursor(Func<bool> compare, bool renderTargetsOnly = false, Point? anchor = null)
    {
        if (!GetCursorPos(out var previous))
        {
            if (!renderTargetsOnly || Marshal.GetLastWin32Error() != 5)
                throw new InvalidOperationException("Cannot read cursor position");
            Console.WriteLine("Input desktop unavailable. Comparing the two application render targets.");
            return compare();
        }
        // Keep hover styling identical in both snapshots, including owned dialogs.
        int x = anchor?.X ?? GetSystemMetrics(76) + 2, y = anchor?.Y ?? GetSystemMetrics(77) + 2;
        if (!SetCursorPos(x, y)) throw new InvalidOperationException("Cannot position cursor for capture");
        try { return compare(); }
        finally
        {
            if (GetCursorPos(out var current) && current.X == x && current.Y == y)
                SetCursorPos(previous.X, previous.Y);
        }
    }
    private static bool CaptureScreens(string original, string candidate, string directory)
    {
        var sample = Demo.State() with { Activity = Demo.Activity() };
        // Both executable versions receive exactly the same IDs, timestamps and totals.
        var scenarios = new List<(DisplayState State, string Route, string Name, int Width, int Height)>();
        foreach (var theme in new[] { "light", "dark" })
        {
            var state = sample with { Settings = sample.Settings with { Appearance = theme } };
            foreach (var route in new[] { "dashboard", "activity", "accounts", "settings", "connect", "connect-claude" })
                scenarios.Add((state, route, route + "-" + theme, 1120, 810));
            scenarios.Add((state, "accounts", "accounts-narrow-" + theme, 900, 660));
            scenarios.Add((state, "dashboard", "dashboard-wide-" + theme, 1320, 900));
        }
        var dark = sample.Settings with { Appearance = "dark" };
        scenarios.Add((sample with { Settings = dark with { Accounts = [] }, Snapshots = [], Errors = [] },
            "dashboard", "empty-dark", 1120, 810));
        scenarios.Add((sample with { Settings = dark with { Accounts = dark.Accounts.Select((a, i) => a with
            { Name = i == 0 ? "여러 프로젝트에서 사용하는 긴 계정 이름 · 디자인 검증용 샘플 계정" : a.Name, Enabled = i != 2 }).ToList() },
            Errors = new() { [dark.Accounts[0].Id] = "서비스에 연결하지 못했습니다. 마지막으로 확인한 사용량을 표시합니다." } },
            "dashboard", "states-dark", 900, 660));
        var results = new Dictionary<string, PixelComparison>();
        bool clientSizesMatch = true;
        directory = Path.GetFullPath(directory);
        foreach (var (exe, folder) in new[] { (original, "original"), (candidate, "candidate") })
        {
            Directory.CreateDirectory(Path.Combine(directory, folder));
            foreach (var s in scenarios)
            {
                // A fresh process makes selection, scroll position and dialog focus reproducible.
                using var child = DashboardChild.Start(Path.GetFullPath(exe));
                using var deadline = new Timer(_ => child.Stop(), null, TimeSpan.FromSeconds(15), Timeout.InfiniteTimeSpan);
                try
                {
                    Suite.Check(child.ReadNext()?.Type == "ready");
                    child.Ready();
                    string path = Path.Combine(directory, folder, s.Name + ".png");
                    File.Delete(path);
                    child.Send(Message.Create("state", s.State));
                    child.Send(Message.Create("capture", new { route = s.Route, path, width = s.Width, height = s.Height }));
                    Message message;
                    do {
                        message = child.ReadNext() ?? throw new InvalidDataException($"{folder} exited before confirming {s.Name}.");
                    } while (message.Type != "captured" || message.Body<string>() != path);
                    if (!File.Exists(path)) throw new InvalidDataException($"{folder} did not write {s.Name}.");
                    if (!s.Route.StartsWith("connect", StringComparison.Ordinal))
                    {
                        var client = DashboardActions.DashboardClientSize(child.Pid!.Value);
                        using var image = new Bitmap(path);
                        Console.WriteLine($"{folder}/{s.Name}: client {client.Width}×{client.Height}, capture {image.Width}×{image.Height}");
                        clientSizesMatch &= image.Size == client;
                    }
                    if (folder == "original" && !HasPaintedContent(path))
                        throw new InvalidDataException($"The original dashboard produced an empty render target for {s.Name}.");
                    if (folder == "candidate")
                        results[s.Name] = ComparePixels(Path.Combine(directory, "original", s.Name + ".png"), path);
                }
                finally { child.RequestClose(); child.WaitForExit(); }
            }
        }
        File.WriteAllBytes(Path.Combine(directory, "result.json"), Json.Bytes(results));
        foreach (var (name, result) in results) Console.WriteLine($"{name}: {result.ChangedPixels:N0} different pixels, {result.Width}×{result.Height}");
        return clientSizesMatch && results.Count == scenarios.Count && results.Values.All(result => result.Identical);
    }
    internal sealed record PixelComparison(int OriginalWidth, int OriginalHeight, int Width, int Height, int ChangedPixels, double MeanChannelError) {
        public bool Identical => OriginalWidth == Width && OriginalHeight == Height && ChangedPixels == 0;
    }
    private static bool HasPaintedContent(string path)
    {
        using var image = new Bitmap(path);
        var data = image.LockBits(new Rectangle(0, 0, image.Width, image.Height), ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        try {
            var row = new int[image.Width];
            int? firstColor = null;
            for (int y = 0; y < image.Height; y++) {
                Marshal.Copy(data.Scan0 + y * data.Stride, row, 0, row.Length);
                foreach (int pixel in row) {
                    if ((pixel >> 24) == 0) continue;
                    if (firstColor.HasValue && firstColor.Value != pixel) return true;
                    firstColor = pixel;
                }
            }
            return false;
        } finally { image.UnlockBits(data); }
    }
    public static bool CompareTrayApplications(string original, string candidate, string folder)
    {
        var root = Path.GetFullPath(folder);
        // NotifyIcon uses the actual pointer position. Give it the same shell anchor as the
        // native probe so scrolling under a stationary pointer selects the same account.
        var work = System.Windows.Forms.Screen.PrimaryScreen!.WorkingArea;
        return WithParkedCursor(() =>
        {
            foreach (var (name, exe) in new[] { ("original", original), ("candidate", candidate) })
            {
                var output = Directory.CreateDirectory(Path.Combine(root, name));
                var report = Path.Combine(output.FullName, "probe.json");
                var start = new ProcessStartInfo(Path.GetFullPath(exe)) { UseShellExecute = false, CreateNoWindow = true };
                start.ArgumentList.Add("--probe");
                start.ArgumentList.Add(report);
                using var process = Process.Start(start)!;
                if (!process.WaitForExit(60000)) { process.Kill(true); throw new TimeoutException($"{name} tray probe timed out"); }
                var probe = Json.Read<System.Text.Json.JsonElement>(File.ReadAllBytes(report));
                if (process.ExitCode != 0 || probe.Get("passed").ValueKind != System.Text.Json.JsonValueKind.True ||
                    probe.Get("monitorPid").GetInt32() != process.Id)
                    throw new InvalidDataException($"{name} tray probe failed: {probe.Get("error")}");
            }
            return CompareTray(Path.Combine(root, "original"), Path.Combine(root, "candidate"), Path.Combine(root, "result.json"));
        }, renderTargetsOnly: true, anchor: new Point(work.Right - 2, work.Bottom - 2));
    }
    public static bool CompareTray(string original, string candidate, string report)
    {
        string[] scenes = ["tray-light", "tray-dark", "tray-menu-light", "tray-menu-dark",
            "tray-menu-single-light", "tray-menu-single-dark", "tray-menu-empty-light",
            "tray-menu-empty-dark", "tray-menu-states-dark", "tray-menu-overflow-dark"];
        var results = scenes.ToDictionary(name => name, name => ComparePixels(
            Path.Combine(original, name + ".png"), Path.Combine(candidate, name + ".png")));
        File.WriteAllBytes(report, Json.Bytes(results));
        foreach (var (name, result) in results)
            Console.WriteLine($"{name}: {result.ChangedPixels:N0} different pixels, {result.Width}×{result.Height}");
        return results.Values.All(result => result.Identical);
    }
    internal static PixelComparison ComparePixels(string original, string candidate)
    {
        using var a = new Bitmap(original);
        using var b = new Bitmap(candidate);
        int width = Math.Min(a.Width, b.Width), height = Math.Min(a.Height, b.Height);
        int outside = (a.Width*a.Height - width*height) + (b.Width*b.Height - width*height);
        int different = outside;
        long distance = (long)outside * 255 * 4;
        var bounds = new Rectangle(0, 0, width, height);
        var ad = a.LockBits(bounds, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        try {
            var bd = b.LockBits(bounds, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
            try {
                var left = new byte[width*4]; var right = new byte[width*4];
                for (int y = 0; y < height; y++) {
                    Marshal.Copy(ad.Scan0 + y*ad.Stride, left, 0, left.Length);
                    Marshal.Copy(bd.Scan0 + y*bd.Stride, right, 0, right.Length);
                    for (int x = 0; x < left.Length; x += 4) {
                        int error = 0;
                        for (int c = 0; c < 4; c++) error += Math.Abs(left[x+c] - right[x+c]);
                        if (error > 0) different++;
                        distance += error;
                    }
                }
            } finally { b.UnlockBits(bd); }
        } finally { a.UnlockBits(ad); }
        return new(a.Width, a.Height, b.Width, b.Height, different, (double)distance / (4L*(width*height+outside)));
    }
    private static async Task CheckDeadline(bool announceReady)
    {
        var timer = Stopwatch.StartNew();
        using var child = DashboardChild.Start(Environment.ProcessPath!, announceReady ? "ignore-close" : "never-ready");
        await Task.Run(() =>
        {
            if (announceReady)
            {
                Suite.Check(child.ReadNext()?.Type == "ready");
                child.Ready();
                timer.Restart();
                child.RequestClose();
            }
            child.WaitForExit();
            Suite.Check(child.Pid == null && timer.Elapsed >= TimeSpan.FromSeconds(announceReady ? 1.8 : 11));
        }).WaitAsync(TimeSpan.FromSeconds(20));
    }
}
