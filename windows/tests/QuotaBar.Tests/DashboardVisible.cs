using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Windows.Automation;
using QuotaBar.Core;
using QuotaBar.Protocol;

internal static partial class DashboardActions
{
    // Use the same Windows capture API on both live windows. The application's own
    // capture command renders an off-screen WPF Visual or reads the Rust back buffer.
    public static bool CompareVisible(string original, string candidate, string output, string captureTool)
    {
        var sample = Demo.State() with { Activity = Demo.Activity() };
        var results = new Dictionary<string, DashboardLifecycle.PixelComparison>();
        var captured = new HashSet<(string Side, string Scene)>();
        var printed = new HashSet<(string Side, string Scene)>();
        var captureErrors = new Dictionary<string, string>();
        var captureMethods = new Dictionary<string, string>();
        return DashboardLifecycle.WithParkedCursor(() =>
        {
            foreach (var (side, executable) in new[] { ("original", original), ("candidate", candidate) })
            foreach (string theme in new[] { "light", "dark" })
            {
                string folder = Path.GetFullPath(Path.Combine(output, side));
                Directory.CreateDirectory(folder);
                using var child = DashboardChild.Start(Path.GetFullPath(executable));
                using var deadline = new Timer(_ => child.Stop(), null, TimeSpan.FromSeconds(75), Timeout.InfiniteTimeSpan);
                try
                {
                    Expect(child, "ready");
                    child.Ready();
                    int pid = child.Pid ?? throw new IOException("Dashboard exited");
                    child.Send(Message.Create("state", sample with { Settings = sample.Settings with { Appearance = theme } }));
                    void Save(nint window, string page)
                    {
                        string scene = page + "-" + theme;
                        string path = Path.Combine(folder, scene + ".png");
                        try
                        {
                            CaptureClient(captureTool, window, path);
                            captured.Add((side, scene));
                        }
                        catch (Exception error) when (error is IOException or TimeoutException)
                        {
                            captureErrors[side + "/" + scene] = error.Message;
                        }
                        bool bothCaptured = captured.Contains(("original", scene)) && captured.Contains(("candidate", scene));
                        bool owned = page is not ("dashboard" or "activity" or "accounts" or "settings");
                        string PrintPath(string build) => Path.Combine(output, "printed", build, scene + ".png");
                        if (owned && !bothCaptured && PrintOwnedClient(window, pid, page == "remove", PrintPath(side)))
                            printed.Add((side, scene));
                        if (side != "candidate") return;
                        if (!bothCaptured)
                        {
                            // Use the same native rendering path for both windows or leave this pair unmeasured.
                            if (!printed.Contains(("original", scene)) || !printed.Contains(("candidate", scene))) return;
                            foreach (string build in new[] { "original", "candidate" })
                                File.Copy(PrintPath(build), Path.Combine(output, build, scene + ".png"), overwrite: true);
                        }
                        captureMethods[scene] = bothCaptured ? "Windows Graphics Capture" : "PrintWindow";
                        results[scene] = DashboardLifecycle.ComparePixels(Path.Combine(output, "original", scene + ".png"), path);
                    }
                    foreach (string page in new[] { "dashboard", "activity", "settings", "accounts" })
                    {
                        child.Send(Message.Create("show", page));
                        Expect(child, "route");
                        var window = Window(pid, "QuotaBar");
                        nint hwnd = WindowHwnd(pid, "QuotaBar");
                        // Keep the whole client on screen even on the smaller Windows test desktop.
                        if (page == "dashboard" && !SetWindowPos(hwnd, 0, 28, 24, 900, 660, 0x14))
                            throw new Win32Exception();
                        Save(hwnd, page);
                        if (page == "settings")
                        {
                            var scroll = Wait(() => window.FindFirst(TreeScope.Descendants, new AndCondition(
                                new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.ScrollBar),
                                new PropertyCondition(AutomationElement.IsRangeValuePatternAvailableProperty, true))), "Settings scrollbar");
                            var range = (RangeValuePattern)scroll.GetCurrentPattern(RangeValuePattern.Pattern);
                            range.SetValue(range.Current.Maximum);
                            Press(window, "라이선스 및 고지 보기");
                            var licenses = Window(pid, "오픈소스 라이선스");
                            var licenseHwnd = WindowHwnd(pid, "오픈소스 라이선스");
                            AutomationElement? Reader(string prefix)
                            {
                                var editor = licenses.FindFirst(TreeScope.Descendants,
                                    new PropertyCondition(AutomationElement.NameProperty, "라이선스 전문"));
                                if (editor?.TryGetCurrentPattern(ValuePattern.Pattern, out object pattern) != true) return null;
                                var value = ((ValuePattern)pattern).Current;
                                return value.IsReadOnly && value.Value.TrimStart().StartsWith(prefix, StringComparison.Ordinal) ? editor : null;
                            }
                            Wait(() => Reader("QuotaBar"), "read-only project notice");
                            Save(licenseHwnd, "licenses-notice");
                            var selector = Wait(() => licenses.FindFirst(TreeScope.Descendants,
                                new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.ComboBox)), "라이선스 문서");
                            selector.SetFocus();
                            TypeKey(selector, "{DOWN}");
                            Wait(() => Reader("Apache License"), "read-only Apache license");
                            Save(licenseHwnd, "licenses-apache");
                            Press(licenses, "닫기");
                            Wait(() => !IsWindow(licenseHwnd) ? window : null, "닫힌 라이선스 창");
                        }
                        if (page != "accounts") continue;
                        Press(window, "이름 변경");
                        _ = Window(pid, "계정 이름");
                        var renameHwnd = WindowHwnd(pid, "계정 이름");
                        Save(renameHwnd, "rename");
                        PostMessage(renameHwnd, 0x10, 0, 0);
                        Wait(() => !IsWindow(renameHwnd) ? window : null, "닫힌 이름 변경 창");
                        Press(window, "연결 해제");
                        var remove = Window(pid, "QuotaBar", "#32770");
                        var removeHwnd = WindowHwnd(pid, "QuotaBar", "#32770");
                        Save(removeHwnd, "remove");
                        DialogButton(remove, 2);
                        Wait(() => !IsWindow(removeHwnd) ? window : null, "취소한 연결 해제");
                        Press(window, "+  계정 연결");
                        var dialog = Window(pid, "QuotaBar · 계정 연결");
                        nint owned = WindowHwnd(pid, "QuotaBar · 계정 연결");
                        Save(owned, "connect");
                        var provider = Wait(() => dialog.FindFirst(TreeScope.Descendants, new PropertyCondition(
                            AutomationElement.ControlTypeProperty, ControlType.ComboBox)), "연결 서비스");
                        provider.SetFocus();
                        TypeKey(provider, "{DOWN}");
                        Wait(() => dialog.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty,
                            "기존 status line을 보존하는 연결에 동의합니다")), "Claude consent");
                        Save(owned, "connect-claude");
                        provider.SetFocus();
                        TypeKey(provider, "{DOWN}");
                        Wait(() => dialog.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty,
                            "Gemini Code Assist의 현재 지원 대상 조직 계정만 연결할 수 있습니다. 개인용 종료 및 서버 정책에 따라 조회가 거절될 수 있습니다.")), "Gemini 설명");
                        Save(owned, "connect-gemini");
                        PostMessage(owned, 0x10, 0, 0); // WM_CLOSE on this fixture's modal.
                    }
                }
                finally { child.RequestClose(); child.WaitForExit(); }
            }
            foreach (string side in new[] { "original", "candidate" })
            {
                string folder = Path.Combine(output, side);
                if (captured.Contains((side, "dashboard-light")) && captured.Contains((side, "dashboard-dark")) &&
                    DashboardLifecycle.ComparePixels(Path.Combine(folder, "dashboard-light.png"),
                    Path.Combine(folder, "dashboard-dark.png")).Identical)
                    throw new IOException($"Windows returned the same {side} surface for both themes");
            }
            if (results.Count == 22)
                File.WriteAllBytes(Path.Combine(output, "result.json"), Json.Bytes(results));
            else
                File.WriteAllBytes(Path.Combine(output, "result.json"), Json.Bytes(new
                {
                    status = "inconclusive", reason = $"Windows captured {results.Count} of 22 comparable pairs.",
                    comparisons = results, captureErrors
                }));
            File.WriteAllBytes(Path.Combine(output, "capture-methods.json"), Json.Bytes(captureMethods));
            foreach (var (scene, frame) in results) Console.WriteLine($"Window {scene}: {frame.ChangedPixels:N0} different pixels, {frame.Width}×{frame.Height}");
            return results.Count == 22 && results.Values.All(v => v.Identical);
        });
    }

    [DllImport("user32.dll", SetLastError = true)] private static extern bool SetWindowPos(nint window, nint after, int x, int y, int width, int height, uint flags);
    private static void CaptureClient(string captureTool, nint window, string destination)
    {
        Thread.Sleep(350);
        var start = new ProcessStartInfo(Path.GetFullPath(captureTool)) {
            UseShellExecute = false, CreateNoWindow = true, RedirectStandardError = true
        };
        start.ArgumentList.Add(window.ToString());
        start.ArgumentList.Add(destination);
        using var process = Process.Start(start)!;
        var detail = process.StandardError.ReadToEndAsync();
        if (!process.WaitForExit(10000)) { process.Kill(true); throw new TimeoutException("Window capture timed out"); }
        if (process.ExitCode != 0 || !File.Exists(destination))
            throw new IOException($"Windows could not capture {Path.GetFileName(destination)}. {detail.GetAwaiter().GetResult().Trim()}");
    }
}
