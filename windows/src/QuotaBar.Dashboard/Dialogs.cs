using System.Windows;
using System.Windows.Controls;
using Microsoft.Win32;
using QuotaBar.Core;

namespace QuotaBar.Dashboard;
internal sealed partial class DashboardWindow
{
    private Window Dialog(string title, StackPanel content)
    {
        var window = new Window { Title = title, Owner = this, Width = 590, SizeToContent = SizeToContent.Height, MaxHeight = 720,
            WindowStartupLocation = WindowStartupLocation.CenterOwner, ResizeMode = ResizeMode.NoResize, Background = backdrop, Foreground = ink, FontFamily = FontFamily, FontSize = 13 };
        window.Resources.MergedDictionaries.Add(Resources);
        content.Margin = new Thickness(28); window.Content = new ScrollViewer { Content = content, Background = backdrop, VerticalScrollBarVisibility = ScrollBarVisibility.Auto }; return window;
    }
    private void Rename(Account account)
    {
        var content = new StackPanel(); content.Children.Add(Text("계정 이름", 20, true));
        var input = new TextBox { Text = account.Name, MaxLength = 100, Margin = new Thickness(0, 18, 0, 18), Padding = new Thickness(8), Background = surface, Foreground = ink }; content.Children.Add(input);
        var dialog = Dialog("계정 이름", content); content.Children.Add(Button("저장", () => { if (string.IsNullOrWhiteSpace(input.Text)) return; Send("edit", new { id = account.Id, name = input.Text, enabled = account.Enabled, tray = account.ShowInTray }); dialog.Close(); }, true)); dialog.ShowDialog();
    }
    private void Connect() => ConnectionDialog().ShowDialog();
    private Window ConnectionDialog(Provider initialProvider = Provider.Codex)
    {
        var content = new StackPanel(); content.Children.Add(Text("계정 연결", 24, true)); content.Children.Add(Text("구독 사용량을 확인할 서비스를 선택하세요.", color: muted));
        var provider = new ComboBox { Margin = new Thickness(0, 20, 0, 20), Padding = new Thickness(8) }; foreach (string p in new[] { "Codex", "Claude", "Gemini" }) provider.Items.Add(p); provider.SelectedIndex = (int)initialProvider; content.Children.Add(provider);
        var options = new StackPanel(); content.Children.Add(options); var dialog = Dialog("QuotaBar · 계정 연결", content);
        void Options()
        {
            options.Children.Clear(); var selected = (Provider)provider.SelectedIndex;
            if (selected == Provider.Codex)
            {
                options.Children.Add(Text("브라우저 로그인은 Codex CLI가 필요합니다. 앱 계정은 CLI의 현재 로그인과 별도로 유지됩니다.", color: muted));
                var login = Button(state!.LoggingIn ? "로그인 진행 중" : "브라우저로 Codex 로그인", () => { Send("login", true); dialog.Close(); }, true); login.IsEnabled = !state.LoggingIn; login.Margin = new Thickness(0, 12, 0, 18); options.Children.Add(login);
            }
            string initial = state!.DefaultPaths?.GetValueOrDefault(selected.ToString().ToLowerInvariant()) ?? (selected == Provider.Claude ? "Claude 설정 폴더" : "로그인 JSON 파일");
            options.Children.Add(Caption(selected == Provider.Claude ? "Claude 설정 폴더" : "또는 CLI 로그인 파일 연결"));
            var path = new TextBox { Text = initial, Margin = new Thickness(0, 7, 0, 10), Padding = new Thickness(8), Background = surface, Foreground = ink }; options.Children.Add(path);
            options.Children.Add(Button("찾아보기", () =>
            {
                if (selected == Provider.Claude) { var picker = new OpenFolderDialog { Title = "Claude 설정 폴더" }; if (picker.ShowDialog(dialog) == true) path.Text = picker.FolderName; }
                else { var picker = new OpenFileDialog { Filter = "JSON 로그인 파일|*.json", CheckFileExists = true }; if (picker.ShowDialog(dialog) == true) path.Text = picker.FileName; }
            }));
            CheckBox? consent = null;
            if (selected == Provider.Claude)
            {
                var description = Text("settings.json의 status line을 연결합니다. 기존 명령은 계속 실행되며, 연결을 해제하면 복원됩니다. Claude Code를 다시 시작하고 응답을 받아야 사용량이 표시됩니다.", 12, color: muted); description.Margin = new Thickness(0, 18, 0, 10); options.Children.Add(description);
                consent = Check("기존 status line을 보존하는 연결에 동의합니다", false); options.Children.Add(consent);
            }
            else
            {
                var note = Text(selected == Provider.Gemini ? "Gemini Code Assist의 현재 지원 대상 조직 계정만 연결할 수 있습니다. 개인용 종료 및 서버 정책에 따라 조회가 거절될 수 있습니다." : "연결한 파일은 읽기만 합니다. 만료되면 CLI에서 다시 로그인하세요.", 12, color: muted); note.Margin = new Thickness(0, 18, 0, 10); options.Children.Add(note);
            }
            var connect = Button("연결", () =>
            {
                if (selected == Provider.Claude) { if (consent!.IsChecked != true) return; Send("claude", new { path = path.Text, consent = true }); }
                else Send("import", new { provider = selected.ToString(), path = path.Text }); dialog.Close();
            }, true); if (consent != null) { connect.IsEnabled = false; consent.Click += (_, _) => connect.IsEnabled = consent.IsChecked == true; }
            connect.Margin = new Thickness(0, 12, 0, 0); options.Children.Add(connect);
        }
        provider.SelectionChanged += (_, _) => Options(); Options(); return dialog;
    }
}
