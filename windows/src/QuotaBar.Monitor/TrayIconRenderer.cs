using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using QuotaBar.Core;

namespace QuotaBar.Monitor;

internal static class TrayIconRenderer
{
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
        try
        {
            var data = bitmap.LockBits(new Rectangle(0, 0, size, size), ImageLockMode.WriteOnly, PixelFormat.Format32bppArgb);
            try
            {
                if (data.Stride != size * 4 || RenderNative(summary.Text, size, summary.UseLogo ? 1 : 0,
                    summary.Warning ? 1 : 0, light ? 1 : 0, data.Scan0, (nuint)(size * data.Stride)) != 0)
                    throw new InvalidOperationException("Tray icon rendering failed.");
            }
            finally { bitmap.UnlockBits(data); }
            return bitmap;
        }
        catch { bitmap.Dispose(); throw; }
    }

    [DllImport("quotabar_native", EntryPoint = "qb_tray_render")]
    private static extern int RenderNative([MarshalAs(UnmanagedType.LPUTF8Str)] string label, int size, int logo, int warning,
        int light, IntPtr pixels, nuint length);

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

    [DllImport("user32.dll")] private static extern bool DestroyIcon(IntPtr handle);
}
