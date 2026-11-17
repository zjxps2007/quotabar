using System.Diagnostics;
using System.IO;
using System.Windows;
using System.Windows.Automation;
using System.Windows.Controls;
using System.Windows.Media;

namespace QuotaBar.Dashboard;

internal sealed partial class DashboardWindow
{
    private sealed record LicenseDocument(string Title, string Text);

    private StackPanel OpenSourceLicensesSettings()
    {
        var content = new StackPanel(); content.Children.Add(Text("오픈소스 라이선스", 14, true));
        var project = Text("AIUsage · sylearn 및 기여자", 12, true); project.Margin = new Thickness(0, 14, 0, 6); content.Children.Add(project);
        content.Children.Add(Caption("Apache License 2.0"));
        var description = Text("QuotaBar는 AIUsage의 사용량 조회 코드와 Claude 연결 기능 등을 재사용·수정한 독립 프로젝트입니다.");
        description.Margin = new Thickness(0, 12, 0, 0); content.Children.Add(description);
        var actions = Row(Button("라이선스 및 고지 보기", ShowOpenSourceLicenses),
            Link("원본 프로젝트", () => Process.Start(new ProcessStartInfo("https://github.com/sylearn/AIUsage") { UseShellExecute = true })));
        actions.Margin = new Thickness(0, 14, 0, 0); content.Children.Add(actions);
        return content;
    }

    private static List<LicenseDocument> LicenseDocuments()
    {
        const string unavailable = "고지 파일을 읽을 수 없습니다. 앱을 다시 설치해 주세요.";
        var documents = new List<LicenseDocument>();
        foreach (var (file, title) in new[] { ("NOTICE", "저작권 및 변경 고지"), ("LICENSE", "Apache License 2.0 전문") })
        {
            using var stream = typeof(DashboardWindow).Assembly.GetManifestResourceStream("QuotaBar.Licenses." + file);
            using var reader = stream == null ? null : new StreamReader(stream);
            documents.Add(new(title, reader?.ReadToEnd() ?? unavailable));
        }
        // The self-contained package keeps the unmodified runtime notices beside Dashboard/.
        string runtimeRoot = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "..", "licenses"));
        if (Directory.Exists(runtimeRoot))
        {
            try
            {
                foreach (string folder in Directory.EnumerateDirectories(runtimeRoot).Order(StringComparer.OrdinalIgnoreCase))
                foreach (string file in Directory.EnumerateFiles(folder).Order(StringComparer.OrdinalIgnoreCase))
                {
                    string name = Path.GetFileName(file);
                    if (!name.StartsWith("LICENSE", StringComparison.OrdinalIgnoreCase)
                        && !(name.Contains("THIRD", StringComparison.OrdinalIgnoreCase) && name.Contains("NOTICE", StringComparison.OrdinalIgnoreCase))) continue;
                    string text;
                    try { text = File.ReadAllText(file); }
                    catch (Exception error) when (error is IOException or UnauthorizedAccessException) { text = unavailable; }
                    documents.Add(new($"{Path.GetFileName(folder)} · {name}", text));
                }
            }
            catch (Exception error) when (error is IOException or UnauthorizedAccessException)
            { documents.Add(new(".NET 런타임 고지", unavailable)); }
        }
        return documents;
    }

    private void ShowOpenSourceLicenses()
    {
        var window = new Window { Title = "오픈소스 라이선스", Owner = this, Width = 760, Height = 620,
            MinWidth = 600, MinHeight = 420, WindowStartupLocation = WindowStartupLocation.CenterOwner,
            Background = backdrop, Foreground = ink, FontFamily = FontFamily, FontSize = 13 };
        window.Resources.MergedDictionaries.Add(Resources);
        var content = new Grid { Margin = new Thickness(24) };
        foreach (var height in new[] { GridLength.Auto, GridLength.Auto, new GridLength(1, GridUnitType.Star), GridLength.Auto })
            content.RowDefinitions.Add(new() { Height = height });
        content.Children.Add(Text("오픈소스 라이선스", 22, true));
        var selector = new ComboBox { ItemsSource = LicenseDocuments(), DisplayMemberPath = nameof(LicenseDocument.Title),
            Margin = new Thickness(0, 16, 0, 16), HorizontalContentAlignment = HorizontalAlignment.Left };
        AutomationProperties.SetName(selector, "라이선스 문서"); Grid.SetRow(selector, 1); content.Children.Add(selector);
        var reader = new TextBox { IsReadOnly = true, AcceptsReturn = true, TextWrapping = TextWrapping.Wrap,
            VerticalScrollBarVisibility = ScrollBarVisibility.Auto, HorizontalScrollBarVisibility = ScrollBarVisibility.Disabled,
            Padding = new Thickness(14), FontFamily = new FontFamily("Consolas"), FontSize = 12, Background = surface, Foreground = ink, BorderBrush = line };
        AutomationProperties.SetName(reader, "라이선스 전문"); Grid.SetRow(reader, 2); content.Children.Add(reader);
        selector.SelectionChanged += (_, _) =>
        {
            reader.Text = (selector.SelectedItem as LicenseDocument)?.Text ?? "";
            reader.ScrollToHome();
        };
        selector.SelectedIndex = 0;
        var close = Button("닫기", () => window.Close()); close.IsCancel = true;
        close.HorizontalAlignment = HorizontalAlignment.Right; close.Margin = new Thickness(0, 16, 0, 0);
        Grid.SetRow(close, 3); content.Children.Add(close); window.Content = content; window.ShowDialog();
    }
}
