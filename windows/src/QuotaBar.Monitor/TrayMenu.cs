using System.ComponentModel;
using System.Drawing.Drawing2D;
using System.Drawing.Text;
using Microsoft.Win32;
using QuotaBar.Core;

namespace QuotaBar.Monitor;

// Keep native menu dismissal, keyboard navigation and screen-edge scrolling, while matching the Mac popover.
internal sealed class TrayMenu : ContextMenuStrip
{
    private readonly Func<DisplayState> state;
    private readonly Action<string> navigate;
    private readonly Func<Task> quit;
    private readonly Dictionary<Provider, Bitmap> logos = [];
    private MenuPalette colors = MenuPalette.For("system");
    private const int ContentWidth = 336;

    public TrayMenu(Func<DisplayState> state, Action<string> navigate, Func<Task> quit)
    {
        this.state = state; this.navigate = navigate; this.quit = quit;
        ShowImageMargin = false; ShowCheckMargin = false; ShowItemToolTips = true;
        DropShadowEnabled = true; AccessibleName = "QuotaBar 사용량 메뉴";
        Renderer = new MenuRenderer(() => colors);
        foreach (var provider in Enum.GetValues<Provider>())
        {
            using var stream = typeof(TrayMenu).Assembly.GetManifestResourceStream($"QuotaBar.{provider}Logo.png")!;
            using var image = Image.FromStream(stream); logos[provider] = new Bitmap(image);
        }
    }

    protected override void OnOpening(CancelEventArgs e) { Rebuild(); e.Cancel = Items.Count == 0; base.OnOpening(e); }
    protected override void OnOpened(EventArgs e)
    {
        base.OnOpened(e);
        var work = Screen.FromControl(this).WorkingArea;
        Location = new(Math.Clamp(Left, work.Left, Math.Max(work.Left, work.Right - Width)),
            Math.Clamp(Top, work.Top, Math.Max(work.Top, work.Bottom - Height)));
    }
    public void RefreshIfVisible() { if (Visible) Rebuild(); }

    private void Rebuild()
    {
        string? selected = Items.Cast<ToolStripItem>().FirstOrDefault(i => i.Selected)?.Name;
        var display = state(); colors = MenuPalette.For(display.Settings.Appearance);
        BackColor = colors.Background; ForeColor = colors.Ink;
        float scale = DeviceDpi / 96f;
        var work = (Visible ? Screen.FromControl(this) : Screen.FromPoint(Cursor.Position)).WorkingArea;
        int width = (int)Math.Ceiling((ContentWidth + 24) * scale) + 2;
        MinimumSize = Size.Empty; MaximumSize = new(width, Math.Max(80, work.Height - 12)); MinimumSize = new(width, 0);
        SuspendLayout();
        var old = Items.Cast<ToolStripItem>().ToArray(); Items.Clear();
        foreach (var item in old) item.Dispose();

        Add("header", "QuotaBar", 62, (g, _, _) =>
        {
            g.TranslateTransform(0, 8);
            FillRound(g, colors.Ink, new(4, 8, 32, 32), 9);
            using (var mark = TrayIconRenderer.Render(new("", true, false, ""), 22, colors.Dark))
            {
                // The app tile has the inverse foreground of the menu surface.
                g.DrawImage(mark, new Rectangle(9, 13, 22, 22));
            }
            DrawText(g, "QuotaBar", 16, new(46, 5, 180, 23), colors.Ink, true);
            DrawText(g, "사용량 한눈에 보기", 11, new(47, 29, 200, 17), colors.Muted);
            DrawText(g, display.Demo ? "미리보기" : $"v{Brand.Version}", 10, new(244, 14, 86, 22), colors.Muted, right: true);
        });
        var accounts = display.Settings.Accounts.Where(a => a.Enabled && a.ShowInTray).ToArray();
        Add("caption", "남은 사용량", 32, (g, _, _) =>
        {
            DrawText(g, "남은 사용량", 11, new(4, 5, 120, 20), colors.Muted, true);
            DrawText(g, $"{accounts.Length}개 계정 · 기간별 잔여율", 10, new(130, 5, 202, 20), colors.Muted, right: true);
        });
        if (accounts.Length == 0)
        {
            bool none = display.Settings.Accounts.Count == 0;
            Add("empty", none ? "계정을 연결해 주세요" : "트레이에 표시할 계정이 없습니다", 94, (g, _, _) =>
            {
                FillRound(g, colors.Surface, new(0, 0, ContentWidth, 86), 12);
                DrawText(g, none ? "계정을 연결해 주세요" : "표시할 계정을 선택해 주세요", 13, new(14, 13, 308, 24), colors.Ink, true);
                DrawText(g, none ? "계정을 연결하면 남은 사용량을 여기서 볼 수 있어요." : "Dashboard에서 모니터링과 트레이 표시를 켜세요.",
                    11, new(14, 41, 308, 30), colors.Muted);
            }, () => navigate("dashboard"));
        }
        foreach (var account in accounts) AddAccount(display, account);
        Separator("accounts-divider");
        ActionRow("dashboard", "Dashboard 열기", () => navigate("dashboard"), primary: true);
        ActionRow("quit", "종료", async () => await quit());
        Add("bottom-padding", "", 8, (_, _, _) => { });
        ResumeLayout(true);
        if (selected != null && Items[selected] is { CanSelect: true } target) target.Select();
        RoundWindow(); Invalidate();
    }

    private void AddAccount(DisplayState display, Account account)
    {
        var snapshot = display.Snapshots.GetValueOrDefault(account.Id);
        var windows = snapshot?.Windows ?? [];
        bool uncertain = display.Errors.ContainsKey(account.Id) || snapshot?.State is "stale" or "reset" or "disconnected" or "api-routed";
        bool loading = display.Loading.Contains(account.Id);
        int rows = Math.Max(1, (windows.Count + 1) / 2);
        int height = 58 + rows * 52 + (uncertain ? 22 : 0);
        string description = $"{account.Provider} · {account.Name}. " + (windows.Count == 0 ? "데이터 대기 중" :
            string.Join(", ", windows.Select(w => $"{w.Title}: {Percent(w.Remaining)} 남음, {Reset(w.ResetAt)}")))
            + (uncertain ? ". 연결 확인 필요. 이전 값 포함." : "") + ". Dashboard에서 보기";
        var item = Add("account-" + account.Id, description, height + 8, (g, selected, enabled) =>
        {
            Color ink = colors.HighContrast && selected ? SystemColors.HighlightText : colors.Ink;
            FillRound(g, selected ? colors.Hover : colors.Surface, new(0, 0, ContentWidth, height), 12);
            StrokeRound(g, selected ? colors.Muted : colors.Border, new(.5f, .5f, ContentWidth - 1, height - 1), 12);
            FillRound(g, Color.White, new(12, 13, 30, 30), 8);
            int inset = account.Provider == Provider.Claude ? 4 : 2;
            g.DrawImage(logos[account.Provider], new Rectangle(12 + inset, 13 + inset, 30 - inset * 2, 30 - inset * 2));
            DrawText(g, account.Provider.ToString(), 12, new(52, 9, 150, 20), ink, true);
            DrawText(g, account.Name, 10, new(52, 30, 263, 17), colors.HighContrast ? ink : colors.Muted);
            DrawText(g, loading ? "확인 중…" : uncertain ? "확인 필요" : "연결됨", 10, new(212, 10, 110, 20),
                uncertain ? colors.Warning : colors.Muted, right: true);
            if (windows.Count == 0)
            {
                DrawText(g, uncertain ? "사용량을 확인할 수 없습니다" : "첫 사용량을 기다리고 있어요", 12, new(14, 58, 308, 20),
                    uncertain ? colors.Warning : colors.Muted);
                DrawText(g, "계정 카드를 눌러 자세히 보기", 10, new(14, 80, 308, 16), colors.Muted);
            }
            for (int i = 0; i < windows.Count; i++)
            {
                var window = windows[i];
                float x = 14 + i % 2 * 162, y = 56 + i / 2 * 52;
                // A lone quota period (for example weekly-only accounts) spans the full row.
                float width = i % 2 == 0 && i == windows.Count - 1 ? 308 : 146;
                Color color = double.IsFinite(window.Remaining) && window.Remaining <= 20 ? colors.Warning : colors.Provider(account.Provider);
                DrawText(g, window.Title, 10, new(x, y, width - 64, 18), colors.HighContrast ? ink : colors.Muted);
                DrawText(g, Percent(window.Remaining), 12, new(x + width - 63, y - 1, 63, 20), ink, true, right: true);
                FillRound(g, colors.Track, new(x, y + 23, width, 5), 2.5f);
                if (double.IsFinite(window.Remaining) && window.Remaining > 0)
                    FillRound(g, color, new(x, y + 23, Math.Max(3, (float)Math.Clamp(window.Remaining, 0, 100) * width / 100), 5), 2.5f);
                DrawText(g, Reset(window.ResetAt), 9, new(x, y + 32, width + 2, 15), colors.HighContrast ? ink : colors.Muted);
            }
            if (uncertain) DrawText(g, snapshot?.Windows.Count > 0 ? "이전 값 포함 · Dashboard에서 연결 확인" : "Dashboard에서 연결 상태를 확인하세요",
                10, new(14, height - 24, 308, 18), colors.Warning);
        }, () => navigate("dashboard"));
        item.ToolTipText = description;
    }

    private void ActionRow(string id, string label, Action action, bool primary = false)
    {
        Add(id, label, primary ? 38 : 32, (g, selected, active) =>
        {
            Color ink = !active ? colors.Muted : primary ? colors.Background : colors.Ink;
            if (primary || selected) FillRound(g, primary ? colors.Ink : colors.Hover, new(0, 1, ContentWidth, primary ? 34 : 30), 8);
            if (!primary && selected && colors.HighContrast) ink = SystemColors.HighlightText;
            DrawGlyph(g, id, new(12, primary ? 10 : 8, 16, 16), ink);
            DrawText(g, label, 12, new(39, primary ? 7 : 4, 264, 24), ink, primary);
            if (primary) DrawText(g, "↗", 15, new(304, 6, 20, 24), ink, right: true);
        }, action);
    }

    private MenuEntry Add(string id, string label, int height, Action<Graphics, bool, bool> draw, Action? action = null)
    {
        var item = new MenuEntry(id, label, ContentWidth + 24, height, DeviceDpi / 96f, draw, action);
        Items.Add(item); return item;
    }
    private void Separator(string id) => Add(id, "", 12, (g, _, _) =>
    { using var pen = new Pen(colors.Border); g.DrawLine(pen, 4, 6, ContentWidth - 4, 6); });

    protected override void OnSizeChanged(EventArgs e) { base.OnSizeChanged(e); RoundWindow(); }
    private void RoundWindow()
    {
        if (Width < 2 || Height < 2) return;
        using var path = Rounded(new(0, 0, Width, Height), 12 * DeviceDpi / 96f);
        var previous = Region; Region = new Region(path); previous?.Dispose();
    }
    protected override void Dispose(bool disposing)
    {
        if (disposing) { foreach (var logo in logos.Values) logo.Dispose(); logos.Clear(); }
        base.Dispose(disposing);
    }

    private static string Percent(double value) => double.IsFinite(value) ? $"{Math.Clamp(value, 0, 100):0}%" : "—";
    private static string Reset(DateTimeOffset? at) => at == null ? "리셋 시간 미제공" : at <= DateTimeOffset.UtcNow
        ? "리셋 경과 · 갱신 필요" : $"{at.Value.ToLocalTime():MM/dd HH:mm} 리셋";
    private static void DrawText(Graphics g, string value, float size, RectangleF bounds, Color color, bool bold = false, bool right = false)
    {
        using var font = new Font("Segoe UI", size, bold ? FontStyle.Bold : FontStyle.Regular, GraphicsUnit.Pixel);
        using var brush = new SolidBrush(color);
        using var format = new StringFormat { Alignment = right ? StringAlignment.Far : StringAlignment.Near,
            LineAlignment = StringAlignment.Center, Trimming = StringTrimming.EllipsisCharacter, FormatFlags = StringFormatFlags.NoWrap };
        g.DrawString(value, font, brush, bounds, format);
    }
    internal static GraphicsPath Rounded(RectangleF box, float radius)
    {
        float d = Math.Min(radius * 2, Math.Min(box.Width, box.Height));
        var path = new GraphicsPath();
        path.AddArc(box.X, box.Y, d, d, 180, 90); path.AddArc(box.Right - d, box.Y, d, d, 270, 90);
        path.AddArc(box.Right - d, box.Bottom - d, d, d, 0, 90); path.AddArc(box.X, box.Bottom - d, d, d, 90, 90);
        path.CloseFigure(); return path;
    }
    private static void FillRound(Graphics g, Color color, RectangleF bounds, float radius)
    { using var path = Rounded(bounds, radius); using var brush = new SolidBrush(color); g.FillPath(brush, path); }
    private static void StrokeRound(Graphics g, Color color, RectangleF bounds, float radius)
    { using var path = Rounded(bounds, radius); using var pen = new Pen(color); g.DrawPath(pen, path); }
    private static void DrawGlyph(Graphics g, string id, RectangleF box, Color color)
    {
        var saved = g.Save(); g.TranslateTransform(box.X, box.Y); g.ScaleTransform(box.Width / 20, box.Height / 20);
        using var pen = new Pen(color, 1.5f) { StartCap = LineCap.Round, EndCap = LineCap.Round, LineJoin = LineJoin.Round };
        switch (id)
        {
            case "dashboard":
                foreach (int x in new[] { 2, 12 }) foreach (int y in new[] { 2, 12 }) g.DrawRectangle(pen, x, y, 6, 6); break;
            case "quit": g.DrawArc(pen, 3, 3, 14, 14, -50, 280); g.DrawLine(pen, 10, 1, 10, 10); break;
        }
        g.Restore(saved);
    }

    private sealed class MenuEntry : ToolStripMenuItem
    {
        private readonly Action<Graphics, bool, bool> draw;
        private readonly bool actionable;
        private readonly float scale;
        private readonly Size preferred;
        public MenuEntry(string id, string label, int width, int height, float scale, Action<Graphics, bool, bool> draw, Action? action)
        {
            Name = id; Text = label; AccessibleName = label; AccessibleRole = action == null ? AccessibleRole.StaticText : AccessibleRole.MenuItem;
            this.draw = draw; this.scale = scale; actionable = action != null;
            AutoSize = false; Size = preferred = new((int)Math.Ceiling(width * scale), (int)Math.Ceiling(height * scale));
            Margin = Padding.Empty; Padding = Padding.Empty;
            if (action != null) Click += (_, _) => action();
        }
        public override bool CanSelect => actionable && base.CanSelect;
        public override Size GetPreferredSize(Size constrainingSize) => preferred;
        protected override void OnPaint(PaintEventArgs e)
        {
            var saved = e.Graphics.Save(); e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
            e.Graphics.TextRenderingHint = TextRenderingHint.ClearTypeGridFit;
            e.Graphics.InterpolationMode = InterpolationMode.HighQualityBicubic;
            e.Graphics.ScaleTransform(scale, scale); e.Graphics.TranslateTransform(12, 0);
            draw(e.Graphics, Selected, Enabled); e.Graphics.Restore(saved);
        }
    }
    private sealed class MenuRenderer(Func<MenuPalette> palette) : ToolStripRenderer
    {
        protected override void OnRenderToolStripBackground(ToolStripRenderEventArgs e) => e.Graphics.Clear(palette().Background);
        protected override void OnRenderToolStripBorder(ToolStripRenderEventArgs e)
        {
            e.Graphics.SmoothingMode = SmoothingMode.AntiAlias;
            StrokeRound(e.Graphics, palette().Border, new(.5f, .5f, e.ToolStrip.Width - 1, e.ToolStrip.Height - 1), 12 * e.ToolStrip.DeviceDpi / 96f);
        }
        protected override void OnRenderArrow(ToolStripArrowRenderEventArgs e) { e.ArrowColor = palette().Ink; base.OnRenderArrow(e); }
    }
    private sealed record MenuPalette(bool Dark, bool HighContrast, Color Background, Color Surface, Color Ink, Color Muted, Color Border, Color Track, Color Hover, Color Warning)
    {
        public Color Provider(Provider provider) => HighContrast ? SystemColors.Highlight : ColorTranslator.FromHtml(provider switch
        { QuotaBar.Core.Provider.Codex => Dark ? "#8995FF" : "#4F5CC8", QuotaBar.Core.Provider.Claude => Dark ? "#E6824A" : "#A84F26", _ => Dark ? "#8AB4F8" : "#1A73E8" });
        public static MenuPalette For(string appearance)
        {
            bool systemDark = false;
            try { systemDark = Registry.GetValue(@"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "AppsUseLightTheme", 1) is int n && n == 0; }
            catch (System.Security.SecurityException) { }
            bool dark = appearance == "dark" || appearance == "system" && systemDark;
            if (SystemInformation.HighContrast) return new(dark, true, SystemColors.Window, SystemColors.Window, SystemColors.WindowText,
                SystemColors.WindowText, SystemColors.WindowText, SystemColors.GrayText, SystemColors.Highlight, SystemColors.WindowText);
            Color C(string light, string night) => ColorTranslator.FromHtml(dark ? night : light);
            return new(dark, false, C("#FFFFFF", "#131313"), C("#F7F7F7", "#1C1C1C"), C("#171717", "#F2F2F2"),
                C("#757575", "#A3A3A3"), C("#EAEAEA", "#2D2D2D"), C("#E5E5E5", "#353535"), C("#EEEEEE", "#292929"), C("#B86708", "#F5A34F"));
        }
    }
}
