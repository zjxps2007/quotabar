namespace QuotaBar.Core;
public sealed record TokenCounts(long Input = 0, long Cached = 0, long Output = 0)
{
    public long Total => Input + Output;
    public TokenCounts Add(TokenCounts b) => new(Input + b.Input, Cached + b.Cached, Output + b.Output);
}
public sealed record DailyUsage(string Day, Provider Provider, string Model, TokenCounts Counts);
public sealed record LocalUsageReport(List<DailyUsage> Days, int ScannedFiles, List<string> Warnings, DateTimeOffset ScannedAt);
