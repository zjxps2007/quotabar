using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using QuotaBar.Core;

namespace QuotaBar.Monitor;

internal static class TrayIconRenderer
{
    public static bool LightTaskbar()
    {
        try { return Registry.GetValue(@"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize", "SystemUsesLightTheme", 0) is int mode && mode == 1; }
        catch (System.Security.SecurityException) { return false; }
    }
    public static int PixelSize()
    {
        uint dpi = GetDpiForWindow(FindWindow("Shell_TrayWnd", null));
        int size = dpi == 0 ? SystemInformation.SmallIconSize.Width : GetSystemMetricsForDpi(49, dpi); // SM_CXSMICON
        return Math.Clamp(size == 0 ? 16 : size, 16, 64);
    }
    public static Icon Create(TraySummary summary, int size, bool light)
    {
        using var bitmap = Render(summary, size, light);
        var handle = bitmap.GetHicon();
        try { return (Icon)Icon.FromHandle(handle).Clone(); }
        finally { DestroyIcon(handle); }
    }
    public static Bitmap Render(TraySummary summary, int size, bool light)
    {
        var bitmap = new Bitmap(size, size, PixelFormat.Format32bppArgb);
        using var g = Graphics.FromImage(bitmap);
        g.SmoothingMode = SmoothingMode.AntiAlias;
        using var brush = new SolidBrush(summary.Warning ? light ? Color.FromArgb(166, 85, 0) : Color.FromArgb(255, 183, 77) : light ? Color.Black : Color.White);
        if (summary.UseLogo)
        {
            float scale = (size - 2f) / 282, left = (size - 266 * scale) / 2;
            foreach (var bar in Brand.Bars)
            {
                float x = left + (float)bar.X * scale, y = 1 + (float)bar.Y * scale,
                    width = (float)bar.Width * scale, height = (float)bar.Height * scale, diameter = (float)bar.Radius * scale * 2;
                using var path = new GraphicsPath();
                path.AddArc(x, y, diameter, diameter, 180, 90);
                path.AddArc(x + width - diameter, y, diameter, diameter, 270, 90);
                path.AddArc(x + width - diameter, y + height - diameter, diameter, diameter, 0, 90);
                path.AddArc(x, y + height - diameter, diameter, diameter, 90, 90);
                path.CloseFigure(); g.FillPath(brush, path);
            }
        }
        else
        {
            // Fit the actual glyph bounds, so 100 remains readable without clipping. No ClearType on transparent pixels.
            using var family = new FontFamily("Segoe UI");
            using var path = new GraphicsPath();
            path.AddString(summary.Text, family, (int)FontStyle.Bold, 32, PointF.Empty, StringFormat.GenericTypographic);
            var bounds = path.GetBounds();
            float scale = Math.Min((size - 1f) / bounds.Width, (size * .84f) / bounds.Height);
            // A dash or exclamation mark should not expand to a full-width glyph.
            scale = Math.Min(scale, size / 24f);
            g.TranslateTransform((size - bounds.Width * scale) / 2, (size - bounds.Height * scale) / 2);
            g.ScaleTransform(scale, scale); g.TranslateTransform(-bounds.X, -bounds.Y);
            g.FillPath(brush, path);
        }
        return bitmap;
    }
    public static void SaveSamples(string directory)
    {
        string[] labels = ["0", "7", "16", "38", "99", "100", "—", "!", "logo"];
        int[] sizes = [16, 20, 24, 32, 40, 48, 64];
        foreach (bool light in new[] { true, false })
        {
            using var sheet = new Bitmap(800, 630); using var g = Graphics.FromImage(sheet);
            g.Clear(light ? Color.FromArgb(245, 245, 245) : Color.FromArgb(25, 25, 25));
            using var font = new Font("Segoe UI", 11, FontStyle.Regular, GraphicsUnit.Pixel);
            var text = light ? Brushes.DimGray : Brushes.LightGray;
            for (int column = 0; column < labels.Length; column++) g.DrawString(labels[column], font, text, 90 + column * 78, 10);
            for (int row = 0; row < sizes.Length; row++)
            {
                int size = sizes[row], y = 42 + row * 82;
                g.DrawString($"{size}px", font, text, 10, y + 20);
                for (int column = 0; column < labels.Length; column++)
                {
                    string label = labels[column];
                    var summary = new TraySummary(label, label == "logo", label is "0" or "7" or "16" or "!", "");
                    using var bitmap = Render(summary, size, light);
                    g.DrawImageUnscaled(bitmap, 78 + column * 78 + (64 - size) / 2, y + (64 - size) / 2);
                    using var icon = Create(summary, size, light);
                    if (icon.Width != size || icon.Height != size) throw new InvalidOperationException("Tray icon dimensions changed.");
                }
            }
            sheet.Save(Path.Combine(directory, $"tray-{(light ? "light" : "dark")}.png"), ImageFormat.Png);
        }
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern IntPtr FindWindow(string name, string? title);
    [DllImport("user32.dll")] private static extern uint GetDpiForWindow(IntPtr window);
    [DllImport("user32.dll")] private static extern int GetSystemMetricsForDpi(int index, uint dpi);
    [DllImport("user32.dll")] private static extern bool DestroyIcon(IntPtr handle);
}
