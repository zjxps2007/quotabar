using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;

namespace QuotaBar.Core;

internal static class NativeRelease
{
    private const string Library = "quotabar_native";
    [DllImport(Library, EntryPoint = "qb_release_version")]
    private static extern int Version(byte[] data, nuint length, [Out] int[] components);
    [DllImport(Library, EntryPoint = "qb_windows_release_parse")]
    private static extern int Feed(ReleasePlatform platform, byte[] data, nuint length, out IntPtr result);
    [DllImport(Library, EntryPoint = "qb_string_free")]
    private static extern void Free(IntPtr value);
    [DllImport(Library, EntryPoint = "qb_release_verify")]
    private static extern int Verify(byte[] metadata, nuint metadataLength, byte[] manifest, nuint manifestLength,
        byte[] signature, nuint signatureLength);
    [DllImport(Library, EntryPoint = "qb_release_file", CharSet = CharSet.Ansi)]
    private static extern int VerifyFile([MarshalAs(UnmanagedType.LPUTF8Str)] string path, byte[] asset, nuint length, NativeCancellation cancellation);

    internal static ReleaseVersion? ParseVersion(string value)
    {
        if (value.Length > 33) return null;
        var bytes = Encoding.UTF8.GetBytes(value);
        var parts = new int[3];
        return Version(bytes, (nuint)bytes.Length, parts) == 0 ? new(parts[0], parts[1], parts[2]) : null;
    }
    internal static AppRelease? ParseFeed(ReadOnlyMemory<byte> data, ReleasePlatform platform)
    {
        if (data.Length > ReleaseFeed.MaximumResponseBytes) throw InvalidFeed();
        int status = Feed(platform, data.ToArray(), (nuint)data.Length, out var result);
        if (status != 0) throw ReleaseTransfer.Failure(status, 0, default);
        try { return JsonSerializer.Deserialize<AppRelease>(Marshal.PtrToStringUTF8(result)!, Json.Options); }
        finally { Free(result); }
    }
    internal static VerifiedUpdate VerifyManifest(byte[] manifest, byte[] signature, string publicKey, AppRelease release, string currentVersion)
    {
        var metadata = Json.Bytes(new { release, currentVersion, publicKey });
        int build = Verify(metadata, (nuint)metadata.Length, manifest, (nuint)manifest.Length, signature, (nuint)signature.Length);
        if (build < 1 || release.Asset == null) throw InvalidSignature();
        return new(release, build, release.Asset);
    }
    internal static async Task VerifyPayload(string path, ReleaseAsset asset, CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        if (path.Contains('\0')) throw new ArgumentException("Invalid update path", nameof(path));
        var data = Json.Bytes(asset);
        using var cancellation = NativeCancellation.Create();
        using var registration = token.Register(cancellation.Cancel);
        int status = await Task.Run(() => VerifyFile(path, data, (nuint)data.Length, cancellation), token).ConfigureAwait(false);
        token.ThrowIfCancellationRequested();
        if (status > 1) Marshal.ThrowExceptionForHR(unchecked((int)0x80070000) | (status - 1));
        if (status != 0) throw InvalidSignature();
    }
    private static UsageException InvalidFeed() => new("invalid_release", "업데이트 정보를 확인할 수 없습니다. 잠시 후 다시 확인하세요.");
    private static UsageException InvalidSignature() => new("update_signature", "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다.");
}
