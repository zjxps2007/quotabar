using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed record StagedUpdate(VerifiedUpdate Update, string Directory)
{ public string Archive => Path.Combine(Directory, Update.Asset.Name); }

public sealed class UpdateDownloader(HttpMessageHandler? handler = null) : IDisposable
{
    private readonly ReleaseClient client = new(handler);
    public async Task<StagedUpdate> Prepare(AppRelease release, string currentVersion, string publicKeyPem, string directory,
        Action<int> progress, CancellationToken token) =>
        new(await client.Download(release, currentVersion, publicKeyPem, directory, progress, token), directory);
    public void Dispose() => client.Dispose();
}
