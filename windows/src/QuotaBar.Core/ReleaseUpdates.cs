using System.Net;

namespace QuotaBar.Core;

public readonly record struct ReleaseVersion(int Major, int Minor, int Patch) : IComparable<ReleaseVersion>
{
    public static ReleaseVersion? Parse(string text) => NativeRelease.ParseVersion(text);
    public int CompareTo(ReleaseVersion other)
    {
        int major = Major.CompareTo(other.Major), minor = Minor.CompareTo(other.Minor);
        return major != 0 ? major : minor != 0 ? minor : Patch.CompareTo(other.Patch);
    }
    public override string ToString() => FormattableString.Invariant($"{Major}.{Minor}.{Patch}");
}

public enum ReleasePlatform { MacOSArm64, MacOSX64, WindowsInstaller, WindowsPortable }
public sealed record ReleaseAsset(string Name, Uri Url, long Bytes, string Sha256);
public sealed record AppRelease(string Tag, string Version, Uri Page, ReleaseAsset? Asset)
{
    public bool IsNewer(string current) => ReleaseVersion.Parse(current) is { } installed
        && ReleaseVersion.Parse(Version) is { } offered && offered.CompareTo(installed) > 0;
}

public static class ReleaseFeed
{
    public const string Repository = "https://github.com/zjxps2007/quotabar";
    public const string Endpoint = "https://api.github.com/repos/zjxps2007/quotabar/releases/latest";
    public const int MaximumResponseBytes = 512 * 1024;
    public const long MaximumAssetBytes = 512 * 1024 * 1024;

    public static AppRelease? Parse(ReadOnlyMemory<byte> data, ReleasePlatform platform)
        => NativeRelease.ParseFeed(data, platform);
}

public sealed record VerifiedUpdate(AppRelease Release, int Build, ReleaseAsset Asset);
public enum UpdatePhase { Idle, Checking, Current, Available, Downloading, Ready, Installing, Failed }
public sealed record AppUpdateState(UpdatePhase Phase = UpdatePhase.Idle, string? Version = null, int? Progress = null,
    string? Message = null, DateTimeOffset? CheckedAt = null);
public static class UpdateSignature
{
    public static VerifiedUpdate Verify(byte[] manifest, byte[] signature, string publicKeyPem, AppRelease release, string currentVersion)
        => NativeRelease.VerifyManifest(manifest, signature, publicKeyPem, release, currentVersion);

    public static Task VerifyFile(string path, ReleaseAsset asset, CancellationToken token = default)
        => NativeRelease.VerifyPayload(path, asset, token);
}

/// Public release checks use their own HTTP client without provider credentials.
public sealed class ReleaseClient : IDisposable
{
    private readonly HttpClient http;
    public ReleaseClient(HttpMessageHandler? handler = null)
    {
        http = new(handler ?? new HttpClientHandler { AllowAutoRedirect = false, UseCookies = false })
        { Timeout = TimeSpan.FromSeconds(30) };
    }
    public async Task<AppRelease?> Latest(ReleasePlatform platform, string currentVersion, CancellationToken token = default)
    {
        if (ReleaseVersion.Parse(currentVersion) == null) throw new UsageException("development_build", "배포 앱에서 업데이트를 확인하세요.");
        using var request = new HttpRequestMessage(HttpMethod.Get, ReleaseFeed.Endpoint);
        request.Headers.Accept.ParseAdd("application/vnd.github+json");
        request.Headers.UserAgent.ParseAdd("QuotaBar/" + currentVersion);
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
        timeout.CancelAfter(TimeSpan.FromSeconds(30));
        using var response = await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token);
        if (response.StatusCode == HttpStatusCode.NotFound) return null;
        if (response.StatusCode is HttpStatusCode.Forbidden or HttpStatusCode.TooManyRequests)
            throw new UsageException("update_rate_limit", "업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요.");
        if (response.StatusCode != HttpStatusCode.OK || response.Content.Headers.ContentLength > ReleaseFeed.MaximumResponseBytes)
            throw new HttpRequestException("Update feed unavailable.");
        using var input = await response.Content.ReadAsStreamAsync(timeout.Token);
        using var output = new MemoryStream(); byte[] buffer = new byte[16 * 1024];
        int count;
        while ((count = await input.ReadAsync(buffer, timeout.Token)) != 0)
        {
            if (output.Length + count > ReleaseFeed.MaximumResponseBytes) throw new HttpRequestException("Update feed too large.");
            output.Write(buffer, 0, count);
        }
        token.ThrowIfCancellationRequested();
        return ReleaseFeed.Parse(output.ToArray(), platform);
    }
    public void Dispose() => http.Dispose();
}
