namespace QuotaBar.Windows;
public static class Arguments
{
    public static string? Value(string[] args, string name)
    { int i = Array.IndexOf(args, name); return i >= 0 && i + 1 < args.Length ? args[i + 1] : null; }
}
