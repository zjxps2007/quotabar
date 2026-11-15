using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Windows.Automation;
using QuotaBar.Core;
using QuotaBar.Protocol;

internal static partial class DashboardActions
{
    // Drive real accessible controls, then observe the commands that the monitor receives.
    // Neither process has a live monitor, credentials, or an account provider.
    public static void Compare(string original, string candidate, bool includeKeyboard)
    {
        var sample = Demo.State() with { Activity = Demo.Activity() };
        var first = sample.Settings.Accounts[0] with { Name = "테스트 ς K 𐐀" };
        sample.Settings.Accounts[0] = first;
        var periods = sample.Snapshots[first.Id].Windows;
        periods[0] = periods[0] with { Remaining = 16.5 };
        foreach (var (side, path) in new[] { ("original", original), ("candidate", candidate) })
        {
            Console.WriteLine($"Dashboard actions for {side} ({Path.GetFileName(path)})");
            using var child = DashboardChild.Start(Path.GetFullPath(path));
            using var deadline = new Timer(_ => child.Stop(), null, TimeSpan.FromSeconds(45), Timeout.InfiniteTimeSpan);
            try
            {
                Expect(child, "ready");
                child.Ready();
                child.Send(Message.Create("state", sample));
                child.Send(Message.Create("show", "dashboard"));
                Expect(child, "route");
                int pid = child.Pid ?? throw new IOException("Dashboard exited at startup");
                var window = Window(pid, "QuotaBar");
                Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(
                    AutomationElement.NameProperty, "17% 남음")), "16.5% quota rounded to 17%");
                var scroll = Wait(() => window.FindFirst(TreeScope.Descendants, new AndCondition(
                    new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.ScrollBar),
                    new PropertyCondition(AutomationElement.IsRangeValuePatternAvailableProperty, true))), "Dashboard scrollbar");
                var range = (RangeValuePattern)scroll.GetCurrentPattern(RangeValuePattern.Pattern);
                double target = Math.Min(120, range.Current.Maximum);
                Suite.Check(target > 0 && range.Current.Minimum == 0);
                range.SetValue(target);
                Wait(() => Math.Abs(range.Current.Value - target) < 1 ? scroll : null, "Dashboard scroll offset");
                range.SetValue(0);
                Wait(() => range.Current.Value < 1 ? scroll : null, "Dashboard scroll to top");
                Press(window, "↻  새로고침");
                Expect(child, "refresh");
                if (includeKeyboard)
                {
                    Toggle(window, $"{first.Provider} {first.Name} 트레이에 표시");
                    var tray = Expect(child, "edit").Payload;
                    Suite.Check(tray.GetProperty("id").GetGuid() == first.Id && tray.GetProperty("tray").GetBoolean() != first.ShowInTray);
                }
                Press(window, "Providers & Accounts");
                Suite.Check(Expect(child, "route").Body<string>() == "accounts");
                ValuePattern SearchBox() => (ValuePattern)Wait(() => window.FindFirst(TreeScope.Descendants, new AndCondition(
                    new PropertyCondition(AutomationElement.NameProperty, "계정 이름 또는 이메일 검색"),
                    new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Edit))), "계정 검색").GetCurrentPattern(ValuePattern.Pattern);
                foreach (string term in new[] { "σ", " k ", "𐐨" })
                {
                    SearchBox().SetValue(term);
                    bool match = first.Name.Contains(term, StringComparison.OrdinalIgnoreCase);
                    Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(
                        AutomationElement.NameProperty, match ? first.Name : "조건에 맞는 계정이 없습니다")), "대소문자 검색 " + term);
                }
                SearchBox().SetValue("찾을 수 없는 계정");
                Press(window, "필터 초기화");
                var claude = sample.Settings.Accounts.Single(a => a.Provider == Provider.Claude);
                var query = SearchBox();
                Suite.Check(query.Current.Value == "");
                query.SetValue("작업");
                Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "1 계정")), "검색 결과 수");
                Press(window, "새로고침");
                Suite.Check(Expect(child, "refresh-account").Payload.GetGuid() == claude.Id);
                query.SetValue("");
                Wait(() => window.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.NameProperty, "3 계정")), "전체 계정 수");
                if (!includeKeyboard)
                {
                    CheckLoginAndSettings(child, window, pid);
                    Console.WriteLine("PASS quota rounding, scrolling, route, Unicode search and reset, refresh all and one account, owned file picker, modal login and theme commands through UI Automation");
                    continue;
                }
                Toggle(window, $"{first.Provider} {first.Name} 모니터링");
                var edit = Expect(child, "edit").Payload;
                Suite.Check(edit.GetProperty("id").GetGuid() == first.Id && edit.GetProperty("enabled").GetBoolean() != first.Enabled);
                Press(window, "새로고침");
                Suite.Check(Expect(child, "refresh-account").Payload.GetGuid() == first.Id);
                Press(window, "연결 해제");
                DialogButton(Window(pid, "QuotaBar", "#32770"), 2); // IDCANCEL
                Press(window, "이름 변경");
                var rename = Window(pid, "계정 이름");
                var input = Wait(() => rename.FindFirst(TreeScope.Descendants,
                    new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Edit)), "계정 이름 입력란");
                var value = (ValuePattern)input.GetCurrentPattern(ValuePattern.Pattern);
                void ExpectName(string expected, string step)
                {
                    try { Wait(() => value.Current.Value == expected ? input : null, step); }
                    catch (TimeoutException e) { throw new InvalidOperationException($"{step}: expected '{expected}', got '{value.Current.Value}'", e); }
                }
                Suite.Check(value.Current.Value == first.Name);
                // WPF's MaxLength applies to user input. ValuePattern assigns the Text property.
                string assigned = string.Concat(Enumerable.Repeat("🐈", 55)) + "\r\n계정";
                value.SetValue(assigned);
                ExpectName(assigned, "긴 계정 이름의 접근성 값");
                string prefix = string.Concat(Enumerable.Repeat("🐈", 49)) + "한";
                value.SetValue(prefix);
                ExpectName(prefix, "입력 경계값");
                input.SetFocus();
                // Send actual keyboard input. Posting WM_KEYUP before Windows translates
                // WM_KEYDOWN can present a bare key with no character to the native window.
                void TypeKeys(string keys)
                {
                    GetWindowThreadProcessId(GetForegroundWindow(), out int focusedPid);
                    if (focusedPid != pid || !input.Current.HasKeyboardFocus) throw new InvalidOperationException("Name field lost focus");
                    System.Windows.Forms.SendKeys.SendWait(keys);
                }
                Wait(() => input.Current.HasKeyboardFocus ? input : null, "계정 이름 입력 포커스");
                TypeKeys("{END}1");
                ExpectName(prefix + "1", "UTF-16 100자 입력");
                TypeKeys("2{BACKSPACE}3");
                ExpectName(prefix + "3", "UTF-16 최대 길이");
                string imported = string.Concat(Enumerable.Repeat("🐈", 55));
                value.SetValue(imported + "끝");
                ExpectName(imported + "끝", "기존 긴 이름");
                TypeKeys("{END}{BACKSPACE}");
                ExpectName(imported, "긴 이름에서 마지막 글자 지우기");
                TypeKeys("^z");
                ExpectName(imported + "끝", "긴 이름 복원");
                const string newName = "내 테스트 계정";
                value.SetValue(newName);
                Wait(() => value.Current.Value == newName ? input : null, "변경한 계정 이름");
                Press(rename, "저장");
                var renamed = Expect(child, "edit").Payload;
                Suite.Check(renamed.GetProperty("id").GetGuid() == first.Id && renamed.GetProperty("name").GetString() == newName
                    && renamed.GetProperty("enabled").GetBoolean() == first.Enabled && renamed.GetProperty("tray").GetBoolean() == first.ShowInTray);
                Press(window, "연결 해제");
                DialogButton(Window(pid, "QuotaBar", "#32770"), 1); // IDOK
                Suite.Check(Expect(child, "remove").Payload.GetGuid() == first.Id);
                var connectButton = Wait(() => window.FindFirst(TreeScope.Descendants, new AndCondition(
                    new PropertyCondition(AutomationElement.NameProperty, "+  계정 연결"),
                    new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Button))), "계정 연결 버튼");
                connectButton.SetFocus();
                SendSpace(connectButton, () =>
                {
                    // A WPF Button stays pressed while Space is held and invokes on release.
                    var watch = Stopwatch.StartNew();
                    do
                    {
                        Suite.Check(WindowHwnd(pid, "QuotaBar · 계정 연결") == 0, "Connection dialog opened before Space was released");
                        Thread.Sleep(20);
                    } while (watch.ElapsedMilliseconds < 250);
                });
                var connect = Window(pid, "QuotaBar · 계정 연결");
                var provider = Wait(() => connect.FindFirst(TreeScope.Descendants,
                    new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.ComboBox)), "연결 서비스");
                void ExpectProviderControl(string name, string transition)
                {
                    var named = new PropertyCondition(AutomationElement.NameProperty, name);
                    try { Wait(() => connect.FindFirst(TreeScope.Descendants, named), transition); }
                    catch (TimeoutException cause)
                    {
                        var buttons = connect.FindAll(TreeScope.Descendants, new PropertyCondition(
                            AutomationElement.ControlTypeProperty, ControlType.Button)).Cast<AutomationElement>();
                        var labels = string.Join(", ", buttons.Select(b => b.Current.Name));
                        throw new TimeoutException($"{transition}. Provider focused: {provider.Current.HasKeyboardFocus}. Dialog buttons: {labels}", cause);
                    }
                }
                provider.SetFocus();
                TypeKey(provider, "{DOWN}"); // Down selects Claude with the list closed.
                ExpectProviderControl("기존 status line을 보존하는 연결에 동의합니다", "Claude consent after keyboard selection");
                TypeKey(provider, "{HOME}"); // Home returns to Codex.
                ExpectProviderControl("브라우저로 Codex 로그인", "Codex after Home");
                TypeKey(provider, "g"); // Typing G selects Gemini.
                ExpectProviderControl("Gemini Code Assist의 현재 지원 대상 조직 계정만 연결할 수 있습니다. 개인용 종료 및 서버 정책에 따라 조회가 거절될 수 있습니다.", "Gemini after typing G");
                TypeKey(provider, "{HOME}");
                TypeKey(provider, "{F4}"); // F4 opens the list; Escape discards an uncommitted selection.
                TypeKey(provider, "{DOWN}");
                TypeKey(provider, "{ESC}");
                ExpectProviderControl("브라우저로 Codex 로그인", "Codex after cancelling the provider list");
                TypeKey(provider, "{F4}");
                TypeKey(provider, "{DOWN}");
                TypeKey(provider, "{ENTER}"); // Enter commits Claude.
                Toggle(connect, "기존 status line을 보존하는 연결에 동의합니다");
                var directory = Wait(() => connect.FindFirst(TreeScope.Descendants, new PropertyCondition(
                    AutomationElement.ControlTypeProperty, ControlType.Edit)), "Claude 설정 경로");
                const string localPath = @"C:\QuotaBar fixture\클로드";
                ((ValuePattern)directory.GetCurrentPattern(ValuePattern.Pattern)).SetValue(localPath);
                Press(connect, "연결");
                var connection = Expect(child, "claude").Payload;
                Suite.Check(connection.GetProperty("path").GetString() == localPath && connection.GetProperty("consent").GetBoolean());
                CheckLoginAndSettings(child, window, pid);
                Console.WriteLine("PASS quota rounding, scrolling, navigation, Unicode search and reset, refresh all, account refresh, tray and monitoring toggles, rename with UTF-16 typing limits and UI Automation assignment, cancel and confirm removal, Space press/release, keyboard provider selection, owned file picker, modal login and theme commands");
            }
            finally { child.RequestClose(); child.WaitForExit(); }
        }
    }

    private static void CheckLoginAndSettings(DashboardChild child, AutomationElement window, int pid)
    {
        Press(window, "+  계정 연결");
        var connect = Window(pid, "QuotaBar · 계정 연결");
        Press(connect, "찾아보기");
        Window(pid, null, "#32770");
        nint pickerHwnd = WindowHwnd(pid, null, "#32770");
        nint connectionHwnd = WindowHwnd(pid, "QuotaBar · 계정 연결");
        Suite.Check(pickerHwnd != 0 && connectionHwnd != 0 && GetWindow(pickerHwnd, 4) == connectionHwnd,
            "The file picker must be owned by the connection dialog");
        // Its shell accessibility tree can contain inner controls with numeric IDs.
        if (!PostMessage(pickerHwnd, 0x111, 2, 0)) throw new IOException("Cannot cancel the file picker");
        Wait(() => !IsWindow(pickerHwnd) && IsWindowEnabled(connectionHwnd) ? AutomationElement.FromHandle(connectionHwnd) : null,
            "connection dialog after closing file picker");
        Press(AutomationElement.FromHandle(connectionHwnd), "브라우저로 Codex 로그인");
        Expect(child, "login");
        Press(window, "설정");
        Suite.Check(Expect(child, "route").Body<string>() == "settings");
        Press(window, "다크");
        Suite.Check(Expect(child, "preferences").Payload.GetProperty("theme").GetString() == "dark");
    }

    private static Message Expect(DashboardChild child, string expected)
    {
        while (child.ReadNext() is { } message)
        {
            if (message.Type == expected) return message;
            if (message.Type == "remove") throw new IOException("Unexpected removal after cancelling confirmation");
        }
        throw new IOException($"Dashboard exited before '{expected}'");
    }

    private static AutomationElement Window(int pid, string? name, string? className = null)
    {
        // UIA nests owned WPF dialogs under the parent, while AccessKit exposes
        // each viewport independently. Resolve both from their real top-level HWND.
        return Wait(() =>
        {
            nint window = WindowHwnd(pid, name, className);
            return window == 0 ? null : AutomationElement.FromHandle(window);
        }, name ?? className ?? "dashboard window");
    }

    private static nint WindowHwnd(int pid, string? name, string? className = null)
    {
        nint window = 0;
        while ((window = FindWindowEx(0, window, className, name)) != 0)
        {
            GetWindowThreadProcessId(window, out int owner);
            if (owner == pid) return window;
        }
        return 0;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern nint FindWindowEx(nint parent, nint previous, string? className, string? title);
    [DllImport("user32.dll")]
    private static extern nint GetWindow(nint window, uint command);
    [DllImport("user32.dll")]
    private static extern bool IsWindowEnabled(nint window);
    [DllImport("user32.dll")]
    private static extern bool IsWindow(nint window);
    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(nint window, out int processId);

    private static void DialogButton(AutomationElement dialog, int id)
    {
        var button = Wait(() => dialog.FindFirst(TreeScope.Descendants,
            new PropertyCondition(AutomationElement.AutomationIdProperty, id.ToString())), $"message box button {id}");
        ((InvokePattern)button.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
    }

    private static void Press(AutomationElement parent, string label)
    {
        var condition = new AndCondition(new PropertyCondition(AutomationElement.NameProperty, label),
                                         new PropertyCondition(AutomationElement.IsInvokePatternAvailableProperty, true));
        var node = Wait(() => parent.FindFirst(TreeScope.Descendants, condition), label);
        ((InvokePattern)node.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
    }

    private static void Toggle(AutomationElement parent, string label)
    {
        var condition = new AndCondition(new PropertyCondition(AutomationElement.NameProperty, label),
                                         new PropertyCondition(AutomationElement.IsTogglePatternAvailableProperty, true));
        var node = Wait(() => parent.FindFirst(TreeScope.Descendants, condition), label);
        // WPF's TogglePattern sets IsChecked without the Click event subscribed
        // by the monitor. Activate the focused control with the user's Space key.
        node.SetFocus();
        SendSpace(node);
    }

    private static nint FocusedKeyboardTarget(AutomationElement node)
    {
        Thread.Sleep(100);
        int expected = node.Current.ProcessId;
        var deadline = Stopwatch.StartNew();
        nint active;
        uint thread;
        while (true)
        {
            active = GetForegroundWindow();
            thread = GetWindowThreadProcessId(active, out int pid);
            if (thread != 0 && pid == expected) break;
            if (deadline.Elapsed >= TimeSpan.FromSeconds(2)) throw new InvalidOperationException("Dashboard test window lost focus");
            node.SetFocus();
            Thread.Sleep(75);
        }
        var info = new GuiThreadInfo { Size = Marshal.SizeOf<GuiThreadInfo>() };
        if (!GetGUIThreadInfo(thread, ref info)) throw new InvalidOperationException("Cannot resolve focused dashboard control");
        nint target = info.Focus == 0 ? active : info.Focus;
        GetWindowThreadProcessId(target, out int owner);
        if (owner != expected) throw new InvalidOperationException("Dashboard keyboard target belongs to another process");
        return target;
    }

    private static void TypeKey(AutomationElement node, string keys)
    {
        FocusedKeyboardTarget(node);
        // Like the rename field, drive the active keyboard input queue. Posting
        // WM_KEYDOWN/UP skips keyboard translation and native focus routing.
        System.Windows.Forms.SendKeys.SendWait(keys);
    }

    private static void SendSpace(AutomationElement node, Action? whileDown = null)
    {
        nint target = FocusedKeyboardTarget(node);
        const int space = 0x20, flags = 1 | 0x39 << 16;
        if (!PostMessage(target, 0x100, space, flags)) throw new InvalidOperationException("Cannot press Space");
        try { whileDown?.Invoke(); }
        finally
        {
            if (!PostMessage(target, 0x101, space, unchecked((int)0xc0000000) | flags))
                throw new InvalidOperationException("Cannot release Space");
        }
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct GuiThreadInfo
    {
        public int Size, Flags;
        public nint Active, Focus, Capture, MenuOwner, MoveSize, Caret;
        public System.Drawing.Rectangle CaretRect;
    }
    [DllImport("user32.dll")] private static extern nint GetForegroundWindow();
    [DllImport("user32.dll")] private static extern bool GetGUIThreadInfo(uint thread, ref GuiThreadInfo info);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool PostMessage(nint target, uint message, nint wParam, nint lParam);

    private static AutomationElement Wait(Func<AutomationElement?> find, string label)
    {
        var timer = Stopwatch.StartNew();
        do
        {
            try { if (find() is { } match && match.Current.IsEnabled) return match; }
            catch (ElementNotAvailableException) { }
            Thread.Sleep(80);
        } while (timer.Elapsed < TimeSpan.FromSeconds(7));
        throw new TimeoutException($"Missing accessible control '{label}'");
    }
}
