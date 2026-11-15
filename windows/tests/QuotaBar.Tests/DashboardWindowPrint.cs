using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

internal static partial class DashboardActions
{
    [StructLayout(LayoutKind.Sequential)]
    private struct ClientBounds { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll")] private static extern bool GetClientRect(nint window, out ClientBounds bounds);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetClassName(nint window, StringBuilder name, int capacity);
    [DllImport("user32.dll")] private static extern bool PrintWindow(nint window, nint dc, uint flags);

    private static bool PrintOwnedClient(nint window, int pid, bool confirmation, string destination)
    {
        var name = new StringBuilder(32);
        GetWindowThreadProcessId(window, out int owner);
        if (owner != pid || GetClassName(window, name, name.Capacity) == 0 || (confirmation && name.ToString() != "#32770") ||
            !GetClientRect(window, out var bounds))
            return false;
        int width = bounds.Right - bounds.Left, height = bounds.Bottom - bounds.Top;
        if (width is < 1 or > 4096 || height is < 1 or > 4096) return false;
        using var bitmap = new Bitmap(width, height, PixelFormat.Format24bppRgb);
        using (var graphics = Graphics.FromImage(bitmap))
        {
            var dc = graphics.GetHdc();
            try { if (!PrintWindow(window, dc, 3)) return false; } // PW_CLIENTONLY | PW_RENDERFULLCONTENT
            finally { graphics.ReleaseHdc(dc); }
        }
        // Some GPU windows return success with a blank client bitmap.
        // Sample inside the client, away from the non-client frame and its shadows.
        int light = 0, dark = 0;
        for (int y = height / 8; y < 3 * height / 4 && (light < 12 || dark < 12); y += Math.Max(2, height / 120))
            for (int x = width / 8; x < 7 * width / 8; x += Math.Max(2, width / 180))
            {
                var color = bitmap.GetPixel(x, y);
                int shade = (color.R + color.G + color.B) / 3;
                if (shade < 100) dark++;
                if (shade > 160) light++;
            }
        if (light < 12 || dark < 12) return false;
        Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
        bitmap.Save(destination, ImageFormat.Png);
        return true;
    }
}
