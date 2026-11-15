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
public sealed class ReleaseClient(HttpMessageHandler? handler = null) : IDisposable
{
    private readonly ReleaseTransfer transfer = new(handler);
    public Task<AppRelease?> Latest(ReleasePlatform platform, string currentVersion, CancellationToken token = default) =>
        transfer.Latest(platform, currentVersion, token);
    public Task<VerifiedUpdate> Download(AppRelease release, string currentVersion, string publicKeyPem, string directory,
        Action<int> progress, CancellationToken token) =>
        transfer.Stage(release, currentVersion, publicKeyPem, directory, progress, token);
    public void Dispose() => transfer.Dispose();
}
