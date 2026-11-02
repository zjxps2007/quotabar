using System.Globalization;
using System.Net;
using System.Text.Json;
using System.Security.Cryptography;

namespace QuotaBar.Core;

public readonly record struct ReleaseVersion(int Major, int Minor, int Patch) : IComparable<ReleaseVersion>
{
    public static ReleaseVersion? Parse(string text)
    {
        if (text.StartsWith('v')) text = text[1..];
        var parts = text.Split('.');
        if (parts.Length != 3) return null;
        int[] values = new int[3];
        for (int i = 0; i < parts.Length; i++)
        {
            string part = parts[i];
            if (part.Length is 0 or > 10 || (part.Length > 1 && part[0] == '0') || part.Any(c => c is < '0' or > '9')
                || !int.TryParse(part, NumberStyles.None, CultureInfo.InvariantCulture, out values[i])) return null;
        }
        return new(values[0], values[1], values[2]);
    }
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
    {
        if (data.Length > MaximumResponseBytes) throw Invalid();
        using var document = JsonDocument.Parse(data);
        var release = document.RootElement;
        if (release.GetProperty("draft").GetBoolean() || release.GetProperty("prerelease").GetBoolean()) return null;
        string? tag = release.Get("tag_name").Text();
        if (tag == null || ReleaseVersion.Parse(tag) is not { } version
            || release.Get("html_url").Text() != $"{Repository}/releases/tag/{tag}") throw Invalid();
        string suffix = platform switch {
            ReleasePlatform.MacOSArm64 => "macOS-arm64.zip", ReleasePlatform.MacOSX64 => "macOS-x86_64.zip",
            ReleasePlatform.WindowsInstaller => "Windows-x64-Setup.exe", ReleasePlatform.WindowsPortable => "Windows-x64.zip",
            _ => throw Invalid()
        };
        string name = $"QuotaBar-{version}-{suffix}";
        var matches = release.GetProperty("assets").EnumerateArray().Where(a => a.Get("name").Text() == name).ToArray();
        if (matches.Length > 1) throw Invalid();
        ReleaseAsset? asset = null;
        if (matches.Length == 1)
        {
            var candidate = matches[0];
            string url = $"{Repository}/releases/download/{tag}/{name}";
            string? digest = candidate.Get("digest").Text();
            if (candidate.Get("state").Text() != "uploaded" || candidate.Get("browser_download_url").Text() != url
                || !candidate.Get("size").TryGetInt64(out long size) || size is <= 0 or > MaximumAssetBytes
                || digest == null || !digest.StartsWith("sha256:", StringComparison.Ordinal)) throw Invalid();
            string hash = digest[7..].ToLowerInvariant();
            if (hash.Length != 64 || hash.Any(c => !(c is >= '0' and <= '9' or >= 'a' and <= 'f'))) throw Invalid();
            asset = new(name, new Uri(url), size, hash);
        }
        return new(tag, version.ToString(), new Uri($"{Repository}/releases/tag/{tag}"), asset);
    }
    private static UsageException Invalid() => new("invalid_release", "업데이트 정보를 확인할 수 없습니다. 잠시 후 다시 확인하세요.");
}

public sealed record VerifiedUpdate(AppRelease Release, int Build, ReleaseAsset Asset);
public enum UpdatePhase { Idle, Checking, Current, Available, Downloading, Ready, Installing, Failed }
public sealed record AppUpdateState(UpdatePhase Phase = UpdatePhase.Idle, string? Version = null, int? Progress = null,
    string? Message = null, DateTimeOffset? CheckedAt = null);
public static class UpdateSignature
{
    private sealed record Manifest(int SchemaVersion, string Version, int Build, List<SignedAsset> Assets);
    private sealed record SignedAsset(string Name, long Bytes, string Sha256);

    public static VerifiedUpdate Verify(byte[] manifest, byte[] signature, string publicKeyPem, AppRelease release, string currentVersion)
    {
        if (manifest.Length > 128 * 1024 || signature.Length is < 8 or > 128 || !release.IsNewer(currentVersion) || release.Asset is not { } asset)
            throw Invalid();
        try
        {
            using var key = ECDsa.Create(); key.ImportFromPem(publicKeyPem);
            if (!key.VerifyData(manifest, signature, HashAlgorithmName.SHA256, DSASignatureFormat.Rfc3279DerSequence)) throw Invalid();
            var value = Json.Read<Manifest>(manifest);
            var matches = value.Assets?.Where(a => a.Name == asset.Name).ToArray();
            if (value.SchemaVersion != 1 || value.Version != release.Version || value.Build <= 0 || matches?.Length != 1
                || matches[0].Bytes != asset.Bytes || matches[0].Sha256 != asset.Sha256) throw Invalid();
            return new(release, value.Build, asset);
        }
        catch (Exception e) when (e is CryptographicException or ArgumentException or JsonException or UsageException) { throw Invalid(); }
    }

    public static async Task VerifyFile(string path, ReleaseAsset asset, CancellationToken token = default)
    {
        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        if (stream.Length != asset.Bytes) throw Invalid();
        var hash = await SHA256.HashDataAsync(stream, token);
        if (Convert.ToHexString(hash).ToLowerInvariant() != asset.Sha256) throw Invalid();
    }
    private static UsageException Invalid() => new("update_signature", "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다.");
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
