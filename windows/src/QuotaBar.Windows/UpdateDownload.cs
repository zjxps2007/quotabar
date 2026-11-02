using System.Net;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed record StagedUpdate(VerifiedUpdate Update, string Directory)
{ public string Archive => Path.Combine(Directory, Update.Asset.Name); }

public sealed class UpdateDownloader : IDisposable
{
    private readonly HttpClient http;
    public UpdateDownloader(HttpMessageHandler? handler = null) => http = new(handler ?? new HttpClientHandler { AllowAutoRedirect = false, UseCookies = false })
    { Timeout = Timeout.InfiniteTimeSpan };

    public async Task<StagedUpdate> Prepare(AppRelease release, string currentVersion, string publicKeyPem, string directory,
        Action<int> progress, CancellationToken token)
    {
        if (System.IO.Directory.Exists(directory)) throw new IOException("Update staging directory already exists.");
        PrivateFiles.EnsureDirectory(directory);
        bool completed = false;
        try
        {
            string root = $"{ReleaseFeed.Repository}/releases/download/{release.Tag}/";
            string manifest = Path.Combine(directory, "update-manifest.json"), signature = Path.Combine(directory, "update-manifest.sig");
            await Fetch(new Uri(root + "update-manifest.json"), manifest, 128 * 1024, null, token);
            await Fetch(new Uri(root + "update-manifest.sig"), signature, 128, null, token);
            var update = UpdateSignature.Verify(File.ReadAllBytes(manifest), File.ReadAllBytes(signature), publicKeyPem, release, currentVersion);
            var staged = new StagedUpdate(update, directory);
            await Fetch(update.Asset.Url, staged.Archive, update.Asset.Bytes, progress, token);
            await UpdateSignature.VerifyFile(staged.Archive, update.Asset, token);
            token.ThrowIfCancellationRequested(); completed = true; return staged;
        }
        finally { if (!completed && System.IO.Directory.Exists(directory)) System.IO.Directory.Delete(directory, true); }
    }

    private async Task Fetch(Uri uri, string destination, long limit, Action<int>? progress, CancellationToken token)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
        timeout.CancelAfter(TimeSpan.FromMinutes(20));
        for (int redirects = 0; redirects <= 5; redirects++)
        {
            if (uri.Scheme != "https") throw new IOException("Insecure update URL.");
            using var request = new HttpRequestMessage(HttpMethod.Get, uri);
            request.Headers.UserAgent.ParseAdd("QuotaBar-Updater");
            using var headersTimeout = CancellationTokenSource.CreateLinkedTokenSource(timeout.Token);
            headersTimeout.CancelAfter(TimeSpan.FromSeconds(30));
            using var response = await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, headersTimeout.Token);
            headersTimeout.CancelAfter(Timeout.InfiniteTimeSpan);
            if (response.StatusCode is HttpStatusCode.MovedPermanently or HttpStatusCode.Redirect or HttpStatusCode.TemporaryRedirect or HttpStatusCode.PermanentRedirect)
            {
                uri = response.Headers.Location is { } location ? new Uri(uri, location) : throw new IOException("Update redirect missing.");
                continue;
            }
            if (response.StatusCode != HttpStatusCode.OK || response.Content.Headers.ContentLength > limit)
                throw new UsageException("update_download", "서명된 업데이트 파일을 내려받지 못했습니다. 현재 버전을 유지합니다.");
            using var input = await response.Content.ReadAsStreamAsync(timeout.Token);
            using var output = new FileStream(destination, FileMode.CreateNew, FileAccess.Write, FileShare.None, 65536, FileOptions.Asynchronous);
            byte[] buffer = new byte[65536]; long total = 0; int last = -1;
            while (true)
            {
                using var readTimeout = CancellationTokenSource.CreateLinkedTokenSource(timeout.Token);
                readTimeout.CancelAfter(TimeSpan.FromSeconds(30));
                int count = await input.ReadAsync(buffer, readTimeout.Token); if (count == 0) break;
                total += count; if (total > limit) throw new IOException("Update exceeds declared size.");
                await output.WriteAsync(buffer.AsMemory(0, count), timeout.Token);
                int percent = (int)Math.Min(100, total * 100 / Math.Max(1, limit));
                if (last != percent) { last = percent; progress?.Invoke(percent); }
            }
            await output.FlushAsync(timeout.Token); return;
        }
        throw new IOException("Too many update redirects.");
    }
    public void Dispose() => http.Dispose();
}
