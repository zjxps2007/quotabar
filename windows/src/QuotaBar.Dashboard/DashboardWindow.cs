using System.IO;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using System.Xml.Linq;
using Microsoft.Win32;
using QuotaBar.Core;
using QuotaBar.Protocol;
using Channel = QuotaBar.Protocol.Channel;
using ShapePath = System.Windows.Shapes.Path;

namespace QuotaBar.Dashboard;

internal sealed partial class DashboardWindow : Window
{
    private readonly Channel channel;
    private DisplayState? state;
    private string route = "dashboard", filter = "all", accountQuery = "";
    private bool dark, attentionOnly;
    private Brush ink = Brushes.Black, muted = Brushes.Gray, surface = Brushes.White, line = Brushes.LightGray,
        accent = Brushes.Black, onAccent = Brushes.White, backdrop = Brushes.WhiteSmoke, sidebar = Brushes.White,
        track = Brushes.WhiteSmoke, warning = Brushes.DarkOrange;
    private StackPanel body = new();
    private ScrollViewer? scroll;
    private FrameworkElement? activitySection;
    private TextBox? searchBox;
    private static readonly Dictionary<Provider, ImageSource> logos = [];

    public DashboardWindow(Channel channel)
    {
        this.channel = channel;
        Title = "QuotaBar"; Width = 1120; Height = 810; MinWidth = 900; MinHeight = 660;
        Icon = BitmapFrame.Create(new Uri("pack://application:,,,/Assets/QuotaBar.ico"));
        FontFamily = new FontFamily("Segoe UI, Malgun Gothic"); FontSize = 12;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        Resources.MergedDictionaries.Add(new ResourceDictionary { Source = new Uri("pack://application:,,,/Controls.xaml") });
        Loaded += (_, _) => Send("visibility", true);
        StateChanged += (_, _) => Send("visibility", WindowState != WindowState.Minimized);
        Closed += (_, _) => Send("visibility", false);
        PreviewKeyDown += (_, e) =>
        {
            if (e.Key == Key.R && Keyboard.Modifiers == ModifierKeys.Control && route == "dashboard")
            { Send("refresh", true); e.Handled = true; }
        };
    }

    public async void Receive(Message message)
    {
        switch (message.Type)
        {
            case "state": state = message.Body<DisplayState>(); Render(); break;
            case "show":
                Navigate(message.Body<string>());
                Show(); if (WindowState == WindowState.Minimized) WindowState = WindowState.Normal;
                Activate(); break;
            case "close": Close(); break;
            case "capture":
                if (message.Payload.Get("width").Number() is { } width) Width = width;
                if (message.Payload.Get("height").Number() is { } height) Height = height;
                string target = message.Payload.Get("route").GetString()!;
                Navigate(target.StartsWith("connect") ? "accounts" : target, notify: false); Show();
                Window? dialog = null;
                if (target.StartsWith("connect")) { dialog = ConnectionDialog(target == "connect-claude" ? Provider.Claude : Provider.Codex); dialog.Show(); }
                await Task.Delay(150); UpdateLayout();
                string path = message.Payload.Get("path").GetString()!;
                var visual = (FrameworkElement)(dialog?.Content ?? Content); var dpi = VisualTreeHelper.GetDpi(visual);
                var bmp = new RenderTargetBitmap((int)Math.Ceiling(visual.ActualWidth * dpi.DpiScaleX),
                    (int)Math.Ceiling(visual.ActualHeight * dpi.DpiScaleY), dpi.PixelsPerInchX, dpi.PixelsPerInchY, PixelFormats.Pbgra32);
                bmp.Render(visual);
                var png = new PngBitmapEncoder(); png.Frames.Add(BitmapFrame.Create(bmp));
                using (var file = File.Create(path)) png.Save(file);
                dialog?.Close();
                Send("captured", path); break;
        }
    }

    private async void Send<T>(string type, T value)
    {
        try { await channel.SendAsync(Message.Create(type, value, Guid.NewGuid())); }
        catch (Exception e) when (e is IOException or OperationCanceledException or ObjectDisposedException) { }
    }

    private static Brush Color(string color) => new SolidColorBrush((System.Windows.Media.Color)ColorConverter.ConvertFromString(color));
    private static Brush Alpha(Brush brush, double opacity) { var copy = brush.Clone(); copy.Opacity = opacity; return copy; }
    private void Theme()
    {
        bool systemDark = false;
        try { systemDark = Registry.GetValue(@"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme", 1) is int n && n == 0; } catch { }
        dark = state!.Settings.Appearance == "dark" || state.Settings.Appearance == "system" && systemDark;
        // Match macOS AppTheme: neutral surfaces and monochrome app controls; color belongs to providers.
        ink = Color(dark ? "#F2F2F2" : "#171717"); muted = Color(dark ? "#A3A3A3" : "#757575");
        surface = Color(dark ? "#1C1C1C" : "#FFFFFF"); backdrop = Color(dark ? "#0E0E0E" : "#F5F5F5");
        sidebar = Color(dark ? "#131313" : "#FFFFFF"); line = Color(dark ? "#2D2D2D" : "#EBEBEB");
        track = Color(dark ? "#2A2A2A" : "#F0F0F0"); accent = dark ? Brushes.White : Brushes.Black;
        onAccent = dark ? Brushes.Black : Brushes.White; warning = Color(dark ? "#F5A34F" : "#B86708");
        foreach (var (key, value) in new[] { ("Ink", ink), ("Muted", muted), ("Surface", surface), ("Line", line),
            ("Track", track), ("Accent", accent), ("OnAccent", onAccent) }) Resources[key] = value;
        Background = backdrop; Foreground = ink; ShowInTaskbar = state.Settings.ShowTaskbarIcon;
        Resources[SystemColors.WindowBrushKey] = surface; Resources[SystemColors.WindowTextBrushKey] = ink;
        Resources[SystemColors.ControlBrushKey] = surface; Resources[SystemColors.ControlTextBrushKey] = ink;
    }

    private TextBlock Text(string? text, double size = 12, bool bold = false, Brush? color = null) => new()
    {
        Text = text ?? "", FontSize = size, FontWeight = bold ? FontWeights.SemiBold : FontWeights.Normal,
        Foreground = color ?? ink, TextWrapping = TextWrapping.Wrap, VerticalAlignment = VerticalAlignment.Center
    };
    private TextBlock Caption(string text) => Text(text, 11, color: muted);
    private Button Button(string label, Action click, bool primary = false)
    {
        var button = new Button
        {
            Content = label, Padding = new Thickness(12, 7, 12, 7), Background = primary ? accent : surface,
            Foreground = primary ? onAccent : ink, BorderBrush = primary ? accent : line, BorderThickness = new Thickness(1),
            Cursor = Cursors.Hand, HorizontalContentAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center
        };
        AutomationProperties.SetName(button, label); button.Click += (_, _) => click(); return button;
    }
    private Button Link(string label, Action click)
    {
        var button = Button(label, click); button.Background = Brushes.Transparent; button.BorderThickness = new Thickness(0);
        button.Padding = new Thickness(0, 4, 0, 4); return button;
    }
    private Border Card(UIElement content, Thickness? padding = null) => new()
    {
        Child = content, Background = surface, BorderBrush = line, BorderThickness = new Thickness(1),
        CornerRadius = new CornerRadius(16), Padding = padding ?? new Thickness(20)
    };
    private static StackPanel Row(params UIElement[] items)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal };
        foreach (var item in items) { if (item is FrameworkElement f) f.Margin = new Thickness(0, 0, 10, 0); row.Children.Add(item); }
        return row;
    }
    private Border Divider() => new() { Height = 1, Background = line, Margin = new Thickness(0, 16, 0, 16) };
    private CheckBox Check(string label, bool value) => new()
    { Content = label, IsChecked = value, Foreground = ink, Margin = new Thickness(0, 5, 0, 10), VerticalContentAlignment = VerticalAlignment.Center };
    private CheckBox Switch(string label, bool value, Action<bool> changed, Brush? tint = null, string? accessibleName = null)
    {
        var toggle = new CheckBox { Content = Text(label, 11), IsChecked = value, Foreground = tint ?? accent, Style = (Style)FindResource("Switch") };
        AutomationProperties.SetName(toggle, accessibleName ?? label);
        toggle.Click += (_, _) => changed(toggle.IsChecked == true); return toggle;
    }
    private Brush ProviderColor(Provider provider) => Color(provider switch
    {
        Provider.Codex => dark ? "#8995FF" : "#4F5CC8",
        Provider.Claude => dark ? "#E6824A" : "#A84F26",
        _ => dark ? "#8AB4F8" : "#1A73E8"
    });
    private static string Number(long value) => value >= 1_000_000_000 ? $"{value / 1_000_000_000d:0.0}B"
        : value >= 1_000_000 ? $"{value / 1_000_000d:0.0}M" : value >= 1000 ? $"{value / 1000d:0.0}K" : value.ToString("N0");
    private static string Reset(DateTimeOffset? at) => at == null ? "리셋 시간 미제공" : at <= DateTimeOffset.UtcNow
        ? "리셋 시간 경과 · 새로고침 필요" : $"리셋 {at.Value.ToLocalTime():M월 d일 HH:mm}";

    private FrameworkElement AppLogo(double size = 38)
    {
        var mark = new Canvas { Width = 266, Height = 282 };
        foreach (var bar in Brand.Bars)
        {
            var shape = new System.Windows.Shapes.Rectangle { Width = bar.Width, Height = bar.Height, RadiusX = bar.Radius, RadiusY = bar.Radius, Fill = onAccent };
            Canvas.SetLeft(shape, bar.X); Canvas.SetTop(shape, bar.Y); mark.Children.Add(shape);
        }
        return new Border
        {
            Width = size, Height = size, CornerRadius = new CornerRadius(size * 108 / 456), Background = accent,
            Child = new Viewbox { Child = mark, Height = size * 282 / 456, HorizontalAlignment = HorizontalAlignment.Center, VerticalAlignment = VerticalAlignment.Center }
        };
    }
    private FrameworkElement ProviderLogo(Provider provider, double size = 36)
    {
        if (!logos.TryGetValue(provider, out var source))
        {
            if (provider == Provider.Claude)
            {
                using var stream = Application.GetResourceStream(new Uri("pack://application:,,,/Assets/ClaudeLogo.svg")).Stream;
                var path = XDocument.Load(stream).Descendants().Single(e => e.Name.LocalName == "path");
                var group = new DrawingGroup();
                group.Children.Add(new GeometryDrawing(Brushes.Transparent, null, new RectangleGeometry(new Rect(0, 0, 24, 24))));
                group.Children.Add(new GeometryDrawing(Color(path.Attribute("fill")!.Value), null, Geometry.Parse(path.Attribute("d")!.Value)));
                source = new DrawingImage(group);
            }
            else source = BitmapFrame.Create(new Uri($"pack://application:,,,/Assets/{provider}Logo.png"));
            source.Freeze(); logos[provider] = source;
        }
        double scale = provider == Provider.Claude ? .75 : provider == Provider.Gemini ? .82 : 1;
        var image = new Image { Source = source, Width = size * scale, Height = size * scale };
        RenderOptions.SetBitmapScalingMode(image, BitmapScalingMode.HighQuality);
        var frame = new Grid { Width = size, Height = size }; frame.Children.Add(image);
        AutomationProperties.SetName(frame, provider.ToString()); return frame;
    }
    private FrameworkElement Glyph(string name, Brush color, double size = 16)
    {
        // Simple vector counterparts to the macOS sidebar symbols; no font-dependent fallback glyphs.
        string data = name switch
        {
            "dashboard" => "M2,2 H8 V8 H2 Z M12,2 H18 V8 H12 Z M2,12 H8 V18 H2 Z M12,12 H18 V18 H12 Z",
            "accounts" => "M10,7 A3,3 0 1 0 4,7 A3,3 0 1 0 10,7 M1,18 V16 C1,10 13,10 13,16 V18 M14,4 A3,3 0 0 1 14,10 M15,13 C19,13 19,15 19,18",
            "settings" => "M2,5 H7 M11,5 H18 M2,10 H12 M16,10 H18 M2,15 H4 M8,15 H18 M7,3 H11 V7 H7 Z M12,8 H16 V12 H12 Z M4,13 H8 V17 H4 Z",
            "attention" => "M10,2 A8,8 0 1 1 9.99,2 M10,6 V11 M10,14 V15",
            "bolt" => "M11,1 L4,11 H9 L8,19 L16,8 H11 Z",
            _ => "M3,18 V11 H6 V18 M9,18 V6 H12 V18 M15,18 V2 H18 V18"
        };
        return new Viewbox { Width = size, Height = size, Child = new ShapePath { Data = Geometry.Parse(data), Stroke = color, StrokeThickness = 1.3, StrokeLineJoin = PenLineJoin.Round, Width = 20, Height = 20 } };
    }

    private void Navigate(string page, bool notify = true)
    {
        bool activity = page == "activity"; route = page is "accounts" or "settings" ? page : "dashboard";
        Render(preserveScroll: false);
        if (activity) Dispatcher.BeginInvoke(DispatcherPriority.Loaded, () =>
        {
            UpdateLayout();
            if (activitySection != null)
                scroll?.ScrollToVerticalOffset(activitySection.TransformToAncestor(body).Transform(new Point()).Y);
        });
        if (notify) Send("route", route);
    }
    private void Render(bool preserveScroll = true)
    {
        if (state == null) return;
        double offset = preserveScroll ? scroll?.VerticalOffset ?? 0 : 0;
        bool restoreSearch = searchBox?.IsKeyboardFocused == true;
        int caret = searchBox?.CaretIndex ?? 0; searchBox = null; activitySection = null; Theme();
        var layout = new Grid { Background = backdrop };
        layout.ColumnDefinitions.Add(new() { Width = new GridLength(210) }); layout.ColumnDefinitions.Add(new());
        var rail = new StackPanel { Margin = new Thickness(18, 34, 18, 20) };
        rail.Children.Add(Row(AppLogo(), Text("QuotaBar", 18, true)));
        var workspace = Text("WORKSPACE", 9, true, muted); workspace.Margin = new Thickness(12, 30, 0, 12); rail.Children.Add(workspace);
        foreach (var (key, label) in new[] { ("dashboard", "Dashboard"), ("accounts", "Providers & Accounts"), ("settings", "설정") })
        {
            bool selected = route == key; var color = selected ? onAccent : ink;
            var nav = Button(label, () => Navigate(key), selected);
            nav.Content = Row(Glyph(key, color), Text(label, 12, selected, color));
            nav.HorizontalContentAlignment = HorizontalAlignment.Left; nav.Padding = new Thickness(12, 11, 12, 11);
            nav.Margin = new Thickness(0, 0, 0, 5); nav.BorderThickness = new Thickness(0);
            nav.Background = selected ? accent : Brushes.Transparent;
            rail.Children.Add(nav);
        }
        layout.Children.Add(new Border { Background = sidebar, BorderBrush = line, BorderThickness = new Thickness(0, 0, 1, 0), Child = rail });
        var main = new DockPanel(); Grid.SetColumn(main, 1); layout.Children.Add(main); Content = layout;
        var header = new DockPanel { Margin = new Thickness(28, 32, 28, 22) };
        if (route != "settings")
        {
            var action = Button(route == "accounts" ? "+  계정 연결" : "↻  새로고침", () =>
            { if (route == "accounts") Connect(); else Send("refresh", true); }, route == "accounts");
            action.ToolTip = route == "accounts" ? "계정 연결" : "새로고침 (Ctrl+R)";
            DockPanel.SetDock(action, Dock.Right); header.Children.Add(action);
        }
        var titles = new StackPanel();
        titles.Children.Add(Text(route == "accounts" ? "Providers & Accounts" : route == "settings" ? "설정" : "Dashboard", 25, true));
        var subtitle = Caption(route == "accounts" ? "계정 연결과 로그인, 모니터링 상태를 관리하세요."
            : route == "settings" ? "화면 모드, 작업 표시줄과 트레이 표시, 조회 방식을 설정하세요." : "남은 사용량과 활동을 확인하세요.");
        subtitle.Margin = new Thickness(0, 5, 16, 0); titles.Children.Add(subtitle); header.Children.Add(titles);
        DockPanel.SetDock(header, Dock.Top); main.Children.Add(header);
        if (state.Demo)
        {
            var preview = new Border { Background = track, Padding = new Thickness(28, 9, 28, 9), Margin = new Thickness(0, 0, 0, 18),
                Child = Caption("미리보기 · 샘플 데이터입니다. 실제 계정이나 로그인 파일에 접근하지 않습니다.") };
            DockPanel.SetDock(preview, Dock.Top); main.Children.Add(preview);
        }
        if (state.Message != null || state.LoggingIn)
        {
            var note = new DockPanel();
            if (state.LoggingIn)
            {
                var cancel = Button("로그인 취소", () => Send("cancel-login", true));
                DockPanel.SetDock(cancel, Dock.Right); note.Children.Add(cancel);
            }
            note.Children.Add(Text(state.Message ?? "브라우저에서 로그인을 완료하세요.", color: warning));
            var notice = new Border { Child = note, Background = Alpha(warning, .08), Padding = new Thickness(14), Margin = new Thickness(28, 0, 28, 18) };
            DockPanel.SetDock(notice, Dock.Top); main.Children.Add(notice);
        }
        body = new StackPanel { Margin = new Thickness(28, 0, 28, 28) };
        scroll = new ScrollViewer { Content = body, VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled };
        main.Children.Add(scroll);
        if (route == "accounts") AccountsPage(); else if (route == "settings") SettingsPage(); else Dashboard();
        scroll.ScrollToVerticalOffset(offset);
        if (restoreSearch && searchBox != null) { searchBox.Focus(); searchBox.CaretIndex = Math.Min(caret, searchBox.Text.Length); }
    }
}

/// <summary>SwiftUI's adaptive card grid, using the available client width rather than a fixed column count.</summary>
internal sealed class AdaptivePanel(double minimum = 290, double gap = 14) : Panel
{
    protected override Size MeasureOverride(Size available)
    {
        double width = double.IsInfinity(available.Width) ? minimum : available.Width;
        int columns = Math.Max(1, (int)((width + gap) / (minimum + gap)));
        double cellWidth = Math.Max(0, (width - gap * (columns - 1)) / columns), height = 0;
        for (int start = 0; start < InternalChildren.Count; start += columns)
        {
            double rowHeight = 0;
            for (int i = start; i < Math.Min(start + columns, InternalChildren.Count); i++)
            { InternalChildren[i].Measure(new Size(cellWidth, double.PositiveInfinity)); rowHeight = Math.Max(rowHeight, InternalChildren[i].DesiredSize.Height); }
            height += rowHeight + (start == 0 ? 0 : gap);
        }
        return new Size(width, height);
    }
    protected override Size ArrangeOverride(Size final)
    {
        int columns = Math.Max(1, (int)((final.Width + gap) / (minimum + gap)));
        double cellWidth = Math.Max(0, (final.Width - gap * (columns - 1)) / columns), y = 0;
        for (int start = 0; start < InternalChildren.Count; start += columns)
        {
            double height = Enumerable.Range(start, Math.Min(columns, InternalChildren.Count - start)).Max(i => InternalChildren[i].DesiredSize.Height);
            for (int i = start; i < Math.Min(start + columns, InternalChildren.Count); i++)
                InternalChildren[i].Arrange(new Rect((i - start) * (cellWidth + gap), y, cellWidth, height));
            y += height + gap;
        }
        return final;
    }
}
