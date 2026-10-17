using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;
using QuotaBar.Core;

namespace QuotaBar.Dashboard;

internal sealed partial class DashboardWindow
{
    private void AccountsPage()
    {
        var filters = new Grid { Margin = new Thickness(0, 0, 0, 18) };
        filters.ColumnDefinitions.Add(new()); filters.ColumnDefinitions.Add(new() { Width = new GridLength(140) });
        filters.ColumnDefinitions.Add(new() { Width = GridLength.Auto }); filters.ColumnDefinitions.Add(new() { Width = GridLength.Auto });
        var search = new TextBox { Text = accountQuery, MaxLength = 100, HorizontalAlignment = HorizontalAlignment.Stretch,
            VerticalAlignment = VerticalAlignment.Center, VerticalContentAlignment = VerticalAlignment.Center, ToolTip = "계정 이름 또는 이메일 검색" };
        AutomationProperties.SetName(search, "계정 이름 또는 이메일 검색"); searchBox = search;
        var searchFrame = new Grid { Width = 280, HorizontalAlignment = HorizontalAlignment.Left, Margin = new Thickness(0, 0, 12, 0) }; searchFrame.Children.Add(search);
        var hint = Caption("계정 이름 또는 이메일 검색"); hint.Margin = new Thickness(11, 0, 23, 0);
        hint.IsHitTestVisible = false; hint.Visibility = accountQuery.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
        searchFrame.Children.Add(hint); filters.Children.Add(searchFrame);
        var provider = new ComboBox { Margin = new Thickness(0, 0, 12, 0), VerticalAlignment = VerticalAlignment.Center };
        foreach (string item in new[] { "전체 Provider", "Codex", "Claude", "Gemini" }) provider.Items.Add(item);
        string[] providerKeys = ["all", "codex", "claude", "gemini"];
        provider.SelectedIndex = Math.Max(0, Array.IndexOf(providerKeys, filter)); AutomationProperties.SetName(provider, "Provider 필터");
        Grid.SetColumn(provider, 1); filters.Children.Add(provider);
        var attention = Check("연결 확인 필요", attentionOnly); attention.Margin = new Thickness(0, 0, 12, 0); attention.VerticalAlignment = VerticalAlignment.Center;
        Grid.SetColumn(attention, 2); filters.Children.Add(attention);
        var count = Caption(""); Grid.SetColumn(count, 3); filters.Children.Add(count);
        body.Children.Add(filters);
        var list = new StackPanel(); body.Children.Add(list);
        void Update()
        {
            list.Children.Clear();
            var matches = state!.Settings.Accounts.Where(a => (filter == "all" || a.Provider.ToString().Equals(filter, StringComparison.OrdinalIgnoreCase))
                && (accountQuery.Length == 0 || $"{a.Name} {a.Provider} {state.Snapshots.GetValueOrDefault(a.Id)?.Email}".Contains(accountQuery, StringComparison.OrdinalIgnoreCase))
                && (!attentionOnly || ConnectionAttention(a))).ToArray();
            count.Text = $"{matches.Length} 계정"; hint.Visibility = accountQuery.Length == 0 ? Visibility.Visible : Visibility.Collapsed;
            foreach (var account in matches)
            {
                var card = ConnectionCard(account); card.Margin = new Thickness(0, 0, 0, 14); list.Children.Add(card);
            }
            if (matches.Length != 0) return;
            var empty = new StackPanel { Margin = new Thickness(0, 60, 0, 60), HorizontalAlignment = HorizontalAlignment.Center };
            empty.Children.Add(Text(state.Settings.Accounts.Count == 0 ? "연결된 계정이 없습니다" : "조건에 맞는 계정이 없습니다", 18, true));
            var note = Caption(state.Settings.Accounts.Count == 0 ? "오른쪽 위의 계정 연결에서 Codex, Claude, Gemini를 추가하세요." : "다른 검색어나 Provider를 선택하세요.");
            note.Margin = new Thickness(0, 10, 0, 16); empty.Children.Add(note);
            if (state.Settings.Accounts.Count > 0)
                empty.Children.Add(Button("필터 초기화", () => { accountQuery = ""; filter = "all"; attentionOnly = false; Render(); }));
            list.Children.Add(empty);
        }
        search.TextChanged += (_, _) => { accountQuery = search.Text; Update(); };
        provider.SelectionChanged += (_, _) => { filter = providerKeys[Math.Max(0, provider.SelectedIndex)]; Update(); };
        attention.Click += (_, _) => { attentionOnly = attention.IsChecked == true; Update(); };
        Update();
    }

    private FrameworkElement ConnectionCard(Account account)
    {
        var snapshot = state!.Snapshots.GetValueOrDefault(account.Id);
        var tint = ProviderColor(account.Provider);
        var stack = new StackPanel(); var top = new DockPanel();
        string status = !account.Enabled ? "일시 정지" : state.Loading.Contains(account.Id) ? "확인 중"
            : ConnectionAttention(account) ? "확인 필요" : snapshot == null ? "조회 대기" : "연결됨";
        var statusText = Text("●  " + status, 11, color: ConnectionAttention(account) && account.Enabled ? warning : !account.Enabled || snapshot == null ? muted : tint);
        DockPanel.SetDock(statusText, Dock.Right); top.Children.Add(statusText);
        var logo = ProviderLogo(account.Provider); logo.Margin = new Thickness(0, 0, 12, 0); DockPanel.SetDock(logo, Dock.Left); top.Children.Add(logo);
        var labels = new StackPanel(); labels.Children.Add(Text(account.Provider.ToString(), 13, true));
        var alias = Text(account.Name); alias.Margin = new Thickness(0, 4, 16, 0); alias.TextWrapping = TextWrapping.NoWrap;
        alias.TextTrimming = TextTrimming.CharacterEllipsis; alias.ToolTip = account.Name; labels.Children.Add(alias);
        if (snapshot?.Email is { } email && !email.Equals(account.Name, StringComparison.OrdinalIgnoreCase)) labels.Children.Add(Caption(email));
        top.Children.Add(labels); stack.Children.Add(top);
        string? error = state.Errors.GetValueOrDefault(account.Id);
        if (error != null || snapshot?.State is "stale" or "waiting" or "disconnected" or "api-routed")
        {
            var note = Text(error ?? SnapshotMessage(snapshot), 11, color: warning); note.Margin = new Thickness(0, 16, 0, 0); stack.Children.Add(note);
        }
        var footer = new DockPanel { Margin = new Thickness(0, 18, 0, 0) };
        var enabled = Switch("모니터링", account.Enabled, value => Edit(account, enabled: value), tint, $"{account.Provider} {account.Name} 모니터링");
        DockPanel.SetDock(enabled, Dock.Right); footer.Children.Add(enabled);
        footer.Children.Add(Caption(account.AppManagedLogin ? "앱 로그인 · 자동 갱신" : account.Provider == Provider.Claude ? "Claude CLI 연결" : "로그인 파일 연결")); stack.Children.Add(footer);
        if (!string.IsNullOrEmpty(account.SourcePath))
        {
            var path = new StackPanel(); path.Children.Add(Text(account.SourcePath, 11, color: muted));
            path.Children.Add(Caption(account.Provider == Provider.Claude ? "Claude에서 메시지를 보내면 잔여량이 갱신됩니다. 연결 해제 시 기존 status line을 복구합니다."
                : "CLI가 갱신한 로그인 파일을 읽습니다. 로그인 만료 시 CLI에서 다시 로그인하세요."));
            stack.Children.Add(new Expander { Header = account.Provider == Provider.Claude ? "연결한 설정 폴더" : "연결한 로그인 파일",
                Content = path, Foreground = muted, FontSize = 11, Margin = new Thickness(0, 14, 0, 0) });
        }
        var actions = new WrapPanel { Margin = new Thickness(0, 16, 0, 0) };
        var refresh = Button("새로고침", () => Send("refresh-account", account.Id)); refresh.IsEnabled = account.Enabled && !state.Loading.Contains(account.Id);
        var rename = Button("이름 변경", () => Rename(account));
        var remove = Button("연결 해제", () =>
        {
            if (MessageBox.Show(this, $"{account.Name} 연결을 해제할까요?" + (account.Provider == Provider.Claude ? "\nQuotaBar가 설치한 status line은 기존 설정으로 복원합니다." : ""),
                "QuotaBar", MessageBoxButton.OKCancel) == MessageBoxResult.OK) Send("remove", account.Id);
        });
        foreach (var button in new[] { refresh, rename, remove }) { button.Margin = new Thickness(0, 0, 8, 0); actions.Children.Add(button); }
        stack.Children.Add(actions); return Card(stack);
    }

    private void Preferences(Settings value)
    {
        state = state! with { Settings = value }; Render();
        Send("preferences", new { seconds = value.RefreshSeconds, theme = value.Appearance, scan = value.ScanLocalLogs, taskbar = value.ShowTaskbarIcon, trayDisplay = value.TrayDisplayMode });
    }
    private void SettingsPage()
    {
        var appearance = new StackPanel(); appearance.Children.Add(Text("화면 모드", 14, true));
        var segments = new UniformGrid { Columns = 3, MaxWidth = 420, HorizontalAlignment = HorizontalAlignment.Left };
        foreach (var (key, label) in new[] { ("system", "시스템 설정 따르기"), ("light", "라이트"), ("dark", "다크") })
        {
            var choice = Button(label, () => Preferences(state!.Settings with { Appearance = key }));
            choice.Background = state!.Settings.Appearance == key ? surface : Brushes.Transparent;
            choice.BorderBrush = state.Settings.Appearance == key ? line : Brushes.Transparent; choice.Padding = new Thickness(14, 7, 14, 7);
            segments.Children.Add(choice);
        }
        appearance.Children.Add(new Border { Background = track, CornerRadius = new CornerRadius(9), Padding = new Thickness(3),
            HorizontalAlignment = HorizontalAlignment.Left, Child = segments, Margin = new Thickness(0, 14, 0, 14) });
        appearance.Children.Add(Caption("앱 창에 바로 적용되며, 다음 실행에도 선택한 모드를 유지합니다."));
        var appearanceCard = Card(appearance); appearanceCard.Margin = new Thickness(0, 0, 0, 20); body.Children.Add(appearanceCard);

        var display = new StackPanel(); display.Children.Add(Text("작업 표시줄 및 트레이", 14, true));
        var taskbar = Switch("작업 표시줄에 앱 아이콘 표시", state!.Settings.ShowTaskbarIcon,
            value => Preferences(state!.Settings with { ShowTaskbarIcon = value }));
        taskbar.Margin = new Thickness(0, 18, 0, 6); display.Children.Add(taskbar);
        display.Children.Add(Caption("끄면 트레이에서 Dashboard와 설정을 열 수 있습니다.")); display.Children.Add(Divider());
        display.Children.Add(Text("트레이 표시", 12));
        var modes = new UniformGrid { Columns = 2, HorizontalAlignment = HorizontalAlignment.Left };
        foreach (var (key, label) in new[] { ("remaining", "남은 비율 숫자"), ("icon", "로고만") })
        {
            var choice = Button(label, () => Preferences(state!.Settings with { TrayDisplayMode = key }));
            choice.Background = state.Settings.TrayDisplayMode == key ? surface : Brushes.Transparent;
            choice.BorderBrush = state.Settings.TrayDisplayMode == key ? line : Brushes.Transparent;
            modes.Children.Add(choice);
        }
        display.Children.Add(new Border { Child = modes, Background = track, Padding = new Thickness(3), CornerRadius = new CornerRadius(9),
            HorizontalAlignment = HorizontalAlignment.Left, Margin = new Thickness(0, 12, 0, 12) });
        var summary = TraySummary.From(state);
        var preview = Row(summary.UseLogo ? AppLogo(22) : Text(summary.Text, 22, true, summary.Warning ? warning : ink),
            Caption(summary.UseLogo ? "QuotaBar" : summary.Text is "!" or "—" ? "조회 대기 · 연결 상태 확인" : $"최소 잔여 {summary.Text}%"));
        display.Children.Add(new Border { Background = backdrop, CornerRadius = new CornerRadius(7), Padding = new Thickness(12, 8, 12, 8),
            HorizontalAlignment = HorizontalAlignment.Left, Child = preview, Margin = new Thickness(0, 12, 0, 12) });
        display.Children.Add(Caption("숫자는 표시 대상 계정·기간 중 가장 낮은 잔여 비율(%)입니다. 작은 아이콘에서는 % 기호를 생략합니다."));
        var trayNote = Caption("Dashboard에서 표시할 계정을 선택하세요. 마우스를 올리거나 클릭하면 계정별 내역을 확인할 수 있습니다.");
        trayNote.Margin = new Thickness(0, 6, 0, 0); display.Children.Add(trayNote);
        var displayCard = Card(display); displayCard.Margin = new Thickness(0, 0, 0, 20); body.Children.Add(displayCard);

        var monitoring = new StackPanel(); monitoring.Children.Add(Text("모니터링", 14, true));
        var intervalRow = new DockPanel { Margin = new Thickness(0, 18, 0, 0) };
        var interval = new ComboBox { Width = 130, VerticalAlignment = VerticalAlignment.Center };
        foreach (string label in new[] { "1분", "5분", "15분", "30분" }) interval.Items.Add(label);
        int[] intervals = [60, 300, 900, 1800]; interval.SelectedIndex = Math.Max(0, Array.IndexOf(intervals, state.Settings.RefreshSeconds));
        AutomationProperties.SetName(interval, "자동 새로고침 간격");
        interval.SelectionChanged += (_, _) => Preferences(state!.Settings with { RefreshSeconds = intervals[Math.Max(0, interval.SelectedIndex)] });
        DockPanel.SetDock(interval, Dock.Right); intervalRow.Children.Add(interval);
        var intervalLabel = new StackPanel(); intervalLabel.Children.Add(Text("자동 새로고침")); intervalLabel.Children.Add(Caption("창을 닫으면 최소 15분 간격으로 조회합니다."));
        intervalRow.Children.Add(intervalLabel); monitoring.Children.Add(intervalRow); monitoring.Children.Add(Divider());
        monitoring.Children.Add(Switch("로컬 토큰 통계", state.Settings.ScanLocalLogs, value => Preferences(state!.Settings with { ScanLocalLogs = value })));
        var scanNote = Caption("Dashboard가 열려 있을 때만 로그를 확인합니다. 대화 본문은 저장하지 않습니다."); scanNote.Margin = new Thickness(0, 6, 0, 0); monitoring.Children.Add(scanNote);
        var monitoringCard = Card(monitoring); monitoringCard.Margin = new Thickness(0, 0, 0, 20); body.Children.Add(monitoringCard);

        var about = new StackPanel(); about.Children.Add(Text("QuotaBar", 14, true));
        about.Children.Add(Caption($"Windows · 버전 {Brand.Version}")); about.Children.Add(Divider());
        about.Children.Add(Caption("연결 정보는 현재 Windows 사용자만 열 수 있도록 보호됩니다."));
        if (state.DataDirectory != null) { var data = Caption("데이터 위치: " + state.DataDirectory); data.Margin = new Thickness(0, 6, 0, 0); about.Children.Add(data); }
        body.Children.Add(Card(about));
    }
}
