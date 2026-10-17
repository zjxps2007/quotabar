using System.Globalization;
using System.Xml.Linq;

namespace QuotaBar.Core;
public sealed record BrandBar(double X, double Y, double Width, double Height, double Radius);
public static class Brand
{
    public static string Version => typeof(Brand).Assembly.GetName().Version?.ToString(3) ?? "dev";
    public static IReadOnlyList<BrandBar> Bars { get; } = Load();
    private static List<BrandBar> Load()
    {
        using var resource = typeof(Brand).Assembly.GetManifestResourceStream("QuotaBar.AppMark")!;
        var svg = XDocument.Load(resource);
        return svg.Descendants().Where(e => e.Name.LocalName == "rect").Select(e =>
        {
            double Value(string name) => double.Parse(e.Attribute(name)!.Value, CultureInfo.InvariantCulture);
            return new BrandBar(Value("x"), Value("y"), Value("width"), Value("height"), Value("rx"));
        }).ToList();
    }
}
