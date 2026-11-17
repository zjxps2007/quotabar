using System.Globalization;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Media;
using QuotaBar.Core;

namespace QuotaBar.Dashboard;

internal sealed partial class DashboardWindow
{
    private bool ConnectionAttention(Account account) => state!.Errors.ContainsKey(account.Id)
        || state.Snapshots.GetValueOrDefault(account.Id)?.State is "stale" or "disconnected" or "api-routed";
    private void Edit(Account account, bool? enabled = null, bool? tray = null) =>
        Send("edit", new { id = account.Id, name = account.Name, enabled = enabled ?? account.Enabled, tray = tray ?? account.ShowInTray });

    private FrameworkElement Section(string title, string subtitle, UIElement? action = null)
    {
        var row = new DockPanel { Margin = new Thickness(0, 24, 0, 16) };
        if (action != null) { DockPanel.SetDock(action, Dock.Right); row.Children.Add(action); }
        var titles = new StackPanel(); titles.Children.Add(Text(title, 15, true));
        var caption = Caption(subtitle); caption.Margin = new Thickness(0, 4, 12, 0); titles.Children.Add(caption);
        row.Children.Add(titles); return row;
    }
    private void Dashboard()
    {
        var accounts = state!.Settings.Accounts;
        var enabled = accounts.Where(a => a.Enabled).ToArray();
        int attention = enabled.Count(a => ConnectionAttention(a) || state.Snapshots.GetValueOrDefault(a.Id)?.Lowest <= 20);
        var days = state.Activity?.Days;
        string today = DateTime.Today.ToString("yyyy-MM-dd"), month = DateTime.Today.ToString("yyyy-MM");
        string Tokens(bool monthly) => !state.Settings.ScanLocalLogs || days == null ? "—" :
            Number(days.Where(d => monthly ? d.Day.StartsWith(month, StringComparison.Ordinal) : d.Day == today).Sum(d => d.Counts.Total));
        var stats = new UniformGrid { Columns = 4, Margin = new Thickness(-6, 0, -6, 0) };
        var tiles = new[]
        {
            ("모니터링 중", enabled.Length.ToString(), $"{enabled.Select(a => a.Provider).Distinct().Count()}개 Provider", "accounts", accent),
            ("확인 필요", attention.ToString(), "잔여량 20% 이하 · 연결 상태", "attention", attention > 0 ? warning : accent),
            ("오늘의 토큰", Tokens(false), "이 PC의 로컬 기록", "bolt", accent),
            ("이번 달 토큰", Tokens(true), $"{DateTime.Today.Month}월", "chart", accent)
        };
        foreach (var (title, value, subtitle, symbol, tint) in tiles)
        {
            var tile = new StackPanel(); var label = new DockPanel();
            var icon = Glyph(symbol, tint, 14); DockPanel.SetDock(icon, Dock.Right); label.Children.Add(icon);
            label.Children.Add(Caption(title)); tile.Children.Add(label);
            var number = Text(value, 28, true); number.Margin = new Thickness(0, 12, 0, 12); tile.Children.Add(number);
            var foot = Text(subtitle, 10, color: muted); foot.TextWrapping = TextWrapping.NoWrap;
            foot.TextTrimming = TextTrimming.CharacterEllipsis; foot.ToolTip = subtitle; tile.Children.Add(foot);
            var card = Card(tile); card.Margin = new Thickness(6, 0, 6, 0); stats.Children.Add(card);
        }
        body.Children.Add(stats);
        if (accounts.Count == 0)
        {
            var empty = new StackPanel { HorizontalAlignment = HorizontalAlignment.Center, Margin = new Thickness(0, 28, 0, 28) };
            var logos = Row(ProviderLogo(Provider.Codex, 44), ProviderLogo(Provider.Claude, 44), ProviderLogo(Provider.Gemini, 44));
            logos.HorizontalAlignment = HorizontalAlignment.Center; empty.Children.Add(logos);
            var title = Text("첫 번째 계정을 연결하세요", 18, true); title.Margin = new Thickness(0, 15, 0, 10); empty.Children.Add(title);
            empty.Children.Add(Caption("Codex, Claude, Gemini의 잔여량을 나란히 확인할 수 있습니다."));
            var manage = Button("계정 관리로 이동", () => Navigate("accounts"), true);
            manage.Margin = new Thickness(0, 16, 0, 0); manage.HorizontalAlignment = HorizontalAlignment.Center; empty.Children.Add(manage);
            var card = Card(empty); card.Margin = new Thickness(0, 24, 0, 0); body.Children.Add(card);
        }
        else
        {
            body.Children.Add(Section("구독 현황", "계정별 잔여량 · 트레이에 표시할 계정을 선택하세요", Link("계정 관리 →", () => Navigate("accounts"))));
            var grid = new AdaptivePanel();
            foreach (var account in accounts) grid.Children.Add(UsageCard(account));
            body.Children.Add(grid);
        }
        Activity();
    }

    private FrameworkElement UsageCard(Account account)
    {
        var snapshot = state!.Snapshots.GetValueOrDefault(account.Id); var tint = ProviderColor(account.Provider);
        var stack = new StackPanel();
        var top = new DockPanel();
        var logo = ProviderLogo(account.Provider); logo.Margin = new Thickness(0, 0, 10, 0); DockPanel.SetDock(logo, Dock.Left); top.Children.Add(logo);
        var label = new StackPanel();
        var title = Row(Text(account.Provider.ToString(), 13, true));
        if (snapshot?.Plan is { } plan)
            title.Children.Add(new Border { Child = Text(plan, 9), Background = track, CornerRadius = new CornerRadius(8), Padding = new Thickness(6, 3, 6, 3) });
        label.Children.Add(title);
        var alias = Text(account.Name, 11, color: muted); alias.Margin = new Thickness(0, 3, 0, 0);
        alias.TextWrapping = TextWrapping.NoWrap; alias.TextTrimming = TextTrimming.CharacterEllipsis; alias.ToolTip = account.Name;
        label.Children.Add(alias); top.Children.Add(label); stack.Children.Add(top);
        if (!account.Enabled)
        {
            var paused = Caption("모니터링 일시 정지"); paused.Margin = new Thickness(0, 24, 0, 24); stack.Children.Add(paused);
        }
        else if (snapshot?.Windows.Count > 0)
        {
            foreach (var window in snapshot.Windows)
            {
                var quota = new StackPanel { Margin = new Thickness(0, 17, 0, 0) };
                var row = new DockPanel();
                var value = Text($"{window.Remaining:0}% 남음", 11, true, window.Remaining <= 20 ? warning : ink);
                DockPanel.SetDock(value, Dock.Right); row.Children.Add(value); row.Children.Add(Caption(window.Title)); quota.Children.Add(row);
                var bar = new Grid { Height = 5, Margin = new Thickness(0, 7, 0, 6) };
                double remaining = Math.Clamp(window.Remaining, 0, 100);
                bar.ColumnDefinitions.Add(new() { Width = new GridLength(remaining, GridUnitType.Star) });
                bar.ColumnDefinitions.Add(new() { Width = new GridLength(100 - remaining, GridUnitType.Star) });
                var fill = new Border { CornerRadius = new CornerRadius(2.5), Background = window.Remaining <= 20 ? warning : tint,
                    Opacity = state.Errors.ContainsKey(account.Id) || snapshot.State == "stale" ? .45 : 1 };
                bar.Children.Add(fill);
                var trackBar = new Border { Background = track, CornerRadius = new CornerRadius(2.5), Child = bar, Height = 5, Margin = new Thickness(0, 7, 0, 6) };
                bar.Margin = new Thickness(0); AutomationProperties.SetName(trackBar, $"{window.Title} 잔여량 {window.Remaining:0} 퍼센트");
                quota.Children.Add(trackBar); quota.Children.Add(Text(Reset(window.ResetAt), 9, color: muted)); stack.Children.Add(quota);
            }
        }
        else
        {
            var pending = Caption(SnapshotMessage(snapshot)); pending.Margin = new Thickness(0, 24, 0, 20); stack.Children.Add(pending);
        }
        if (state.Errors.ContainsKey(account.Id))
        {
            var error = Text("최신 사용량을 확인하지 못했습니다.", 11, color: warning); error.Margin = new Thickness(0, 14, 0, 0);
            stack.Children.Add(error); stack.Children.Add(Link("계정 관리에서 확인 →", () => Navigate("accounts")));
        }
        else if (snapshot?.State is "stale" or "reset")
        {
            var note = Text(snapshot.State == "stale" ? "이전 스냅샷 · Claude에서 메시지를 보내면 갱신됩니다." : "리셋 이후 새 응답을 기다리고 있습니다.", 11, color: warning);
            note.Margin = new Thickness(0, 14, 0, 0); stack.Children.Add(note);
        }
        if (snapshot != null)
        {
            var fetched = Text(state.Loading.Contains(account.Id) ? "업데이트 중…" : $"●  마지막 데이터 {(snapshot.ObservedAt ?? snapshot.FetchedAt).ToLocalTime():HH:mm}"
                + (state.Errors.ContainsKey(account.Id) ? " · 이전 값" : ""), 10, color: muted);
            fetched.Margin = new Thickness(0, 17, 0, 0); stack.Children.Add(fetched);
        }
        stack.Children.Add(Divider());
        stack.Children.Add(Switch("트레이에 표시", account.ShowInTray, value => Edit(account, tray: value), tint, $"{account.Provider} {account.Name} 트레이에 표시"));
        var card = Card(stack); card.Opacity = account.Enabled ? 1 : .65; return card;
    }

    private static string SnapshotMessage(Snapshot? snapshot) => snapshot?.State switch
    {
        "waiting" => "Claude에서 첫 메시지를 보내면 잔여량이 표시됩니다.",
        "api-routed" => "API 연결 세션은 구독 사용량을 제공하지 않습니다.",
        "disconnected" => "Claude 연결이 해제되었습니다. 다시 연결하세요.",
        "stale" => "새 데이터가 없습니다. Claude에서 메시지를 보내 연결을 확인하세요.",
        _ => "아직 조회된 잔여량이 없습니다."
    };

    private void Activity()
    {
        var providers = new[] { Provider.Claude, Provider.Codex }.Where(p => state!.Settings.Accounts.Any(a => a.Provider == p)).ToArray();
        if (providers.Length == 0) return;
        activitySection = Section("사용 활동", $"최근 26주 · 이 PC의 {string.Join(" / ", providers)} 로그",
            state!.Scanning ? Caption("업데이트 중…") : null);
        body.Children.Add(activitySection);
        if (!state.Settings.ScanLocalLogs) { body.Children.Add(Card(Caption("로컬 통계가 꺼져 있습니다. 설정에서 켤 수 있습니다."))); return; }
        if (state.Activity == null) { body.Children.Add(Card(Caption(state.Scanning ? "로컬 기록을 읽고 있습니다…" : "로컬 사용 기록을 확인하세요."))); return; }
        var grid = new AdaptivePanel(300, 16);
        foreach (var provider in providers) grid.Children.Add(ActivityCard(provider, state.Activity));
        body.Children.Add(grid);
        foreach (var message in state.Activity.Warnings)
        { var text = Text(message, 11, color: warning); text.Margin = new Thickness(0, 12, 0, 0); body.Children.Add(text); }
        var note = Caption("토큰은 계정별 할당이 아닌 기기 전체 기록입니다. 구독 요금이나 API 청구 금액으로 환산하지 않습니다.");
        note.Margin = new Thickness(0, 16, 0, 6); body.Children.Add(note);
        body.Children.Add(Text($"{state.Activity.ScannedFiles:N0}개 파일 · {state.Activity.ScannedAt.ToLocalTime():MM/dd HH:mm} 집계 · 총 토큰 = 입력 + 출력 (캐시 입력 포함)", 10, color: muted));
    }

    private FrameworkElement ActivityCard(Provider provider, LocalUsageReport report)
    {
        var tint = ProviderColor(provider); var card = new StackPanel();
        card.Children.Add(Row(ProviderLogo(provider, 26), Text(provider.ToString(), 13, true, tint)));
        // Same Monday-first, 26-week window and quartile intensity as macOS UsageActivity.
        var today = report.ScannedAt.LocalDateTime.Date;
        var start = today.AddDays(-(((int)today.DayOfWeek + 6) % 7) - 25 * 7);
        var days = report.Days.Where(d => d.Provider == provider
            && string.CompareOrdinal(d.Day, start.ToString("yyyy-MM-dd")) >= 0
            && string.CompareOrdinal(d.Day, today.ToString("yyyy-MM-dd")) <= 0).ToArray();
        var groups = days.GroupBy(d => d.Day).ToDictionary(g => g.Key, g => g.ToArray());
        var totals = groups.ToDictionary(g => g.Key, g => g.Value.Sum(d => d.Counts.Total));
        long[] values = totals.Values.Where(v => v > 0).Order().ToArray();
        if (values.Length == 0)
        {
            var empty = new StackPanel { Margin = new Thickness(0, 24, 0, 24) };
            empty.Children.Add(Text("아직 사용 기록이 없습니다", 13, true)); empty.Children.Add(Caption("로컬 로그가 확인되면 일별 토큰이 표시됩니다."));
            card.Children.Add(empty); return Card(card, new Thickness(16));
        }
        long[] thresholds = new[] { .25, .50, .75 }.Select(q => values[(int)((values.Length - 1) * q)]).ToArray();
        var calendar = new Grid { Height = 118, Margin = new Thickness(0, 8, 0, 0) };
        calendar.ColumnDefinitions.Add(new() { Width = new GridLength(28) });
        for (int i = 0; i < 26; i++) calendar.ColumnDefinitions.Add(new());
        calendar.RowDefinitions.Add(new() { Height = new GridLength(18) });
        for (int i = 0; i < 7; i++) calendar.RowDefinitions.Add(new());
        calendar.SizeChanged += (_, _) =>
        {
            double side = Math.Clamp((calendar.ActualWidth - 28) / 26 - 3, 7, 16);
            double height = 18 + (side + 3) * 7; if (Math.Abs(calendar.Height - height) > .1) calendar.Height = height;
        };
        int previousMonth = -1;
        for (int column = 0; column < 26; column++)
        {
            var monday = start.AddDays(column * 7);
            if (monday.Month != previousMonth)
            {
                var label = Text($"{monday.Month}월", 9, color: muted); Grid.SetColumn(label, column + 1); Grid.SetColumnSpan(label, Math.Min(3, 26 - column));
                calendar.Children.Add(label); previousMonth = monday.Month;
            }
            for (int row = 0; row < 7; row++)
            {
                var date = monday.AddDays(row); string key = date.ToString("yyyy-MM-dd"); long total = totals.GetValueOrDefault(key);
                int level = total == 0 ? 0 : total <= thresholds[0] ? 1 : total <= thresholds[1] ? 2 : total <= thresholds[2] ? 3 : 4;
                double opacity = dark ? new[] { 0, .44, .62, .80, .98 }[level] : new[] { 0, .38, .56, .74, .92 }[level];
                var tooltip = new ToolTip { Background = surface, Foreground = ink, BorderBrush = line, Padding = new Thickness(12), MaxWidth = 340 };
                var details = new StackPanel(); details.Children.Add(Text($"{provider} · {date:yyyy.MM.dd}", 12, true));
                details.Children.Add(Text($"{total:N0} tokens", 18, true, tint));
                foreach (var model in groups.GetValueOrDefault(key, []).GroupBy(d => d.Model).OrderByDescending(g => g.Sum(d => d.Counts.Total)))
                {
                    var counts = model.Select(d => d.Counts).Aggregate(new TokenCounts(), (a, b) => a.Add(b));
                    var text = Text($"{model.Key} · {Number(counts.Total)}\n입력 {Number(counts.Input)} · 캐시 {Number(counts.Cached)} · 출력 {Number(counts.Output)}", 10, color: muted);
                    text.Margin = new Thickness(0, 7, 0, 0); details.Children.Add(text);
                }
                tooltip.Content = details;
                var fill = new Border { CornerRadius = new CornerRadius(2), Background = Alpha(tint, opacity) };
                var cell = new Border { CornerRadius = new CornerRadius(2), Background = date > today ? Alpha(track, .4) : track,
                    Child = fill, BorderThickness = new Thickness(date == today ? 1 : 0), BorderBrush = tint, ToolTip = tooltip, Margin = new Thickness(1.5), Focusable = date <= today };
                AutomationProperties.SetName(cell, $"{provider} {key}, {total:N0} tokens{(date == today ? ", 오늘" : "")}");
                ToolTipService.SetInitialShowDelay(cell, 80); ToolTipService.SetShowDuration(cell, 60000);
                cell.MouseLeftButtonUp += (_, _) => { tooltip.PlacementTarget = cell; tooltip.IsOpen = true; };
                Grid.SetRow(cell, row + 1); Grid.SetColumn(cell, column + 1); calendar.Children.Add(cell);
            }
        }
        foreach (var (row, label) in new[] { (0, "월"), (2, "수"), (4, "금") })
        { var day = Text(label, 9, color: muted); Grid.SetRow(day, row + 1); calendar.Children.Add(day); }
        card.Children.Add(calendar);
        var peak = totals.OrderByDescending(p => p.Value).ThenBy(p => p.Key, StringComparer.Ordinal).First();
        var footer = new StackPanel { Margin = new Thickness(0, 10, 0, 0) };
        footer.Children.Add(Text($"총 {Number(values.Sum())} tokens", 11, true));
        footer.Children.Add(Text($"활동 {values.Length}일 · 최고 {Number(peak.Value)} ({DateTime.ParseExact(peak.Key, "yyyy-MM-dd", CultureInfo.InvariantCulture):M월 d일})", 10, color: muted));
        card.Children.Add(footer);
        var models = new StackPanel();
        foreach (var group in days.GroupBy(d => d.Model).OrderByDescending(g => g.Sum(d => d.Counts.Total)).Take(12))
        {
            var counts = group.Select(d => d.Counts).Aggregate(new TokenCounts(), (a, b) => a.Add(b));
            var row = new DockPanel { Margin = new Thickness(0, 8, 0, 4) };
            var total = Text(Number(counts.Total), 11, true); DockPanel.SetDock(total, Dock.Right); row.Children.Add(total);
            row.Children.Add(Text(group.Key, 11, color: tint)); models.Children.Add(row);
            models.Children.Add(Text($"입력 {Number(counts.Input)} · 캐시 입력 {Number(counts.Cached)} · 출력 {Number(counts.Output)}", 10, color: muted));
        }
        card.Children.Add(new Expander { Header = "모델별 사용량", Content = models, Foreground = muted, FontSize = 10, Margin = new Thickness(0, 12, 0, 0) });
        return Card(card, new Thickness(16));
    }
}
