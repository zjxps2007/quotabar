using System.Net;
using System.Security.Cryptography;
using System.Text.Json.Nodes;
using System.IO.Compression;
using QuotaBar.Core;
using QuotaBar.Windows;

internal static class UpdateTests
{
    private static string Root => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../../../shared/fixtures"));
    private static byte[] Fixture(string name) => File.ReadAllBytes(Path.Combine(Root, name));
    private static byte[] Changed(Action<JsonNode> mutation)
    { var root = JsonNode.Parse(Fixture("update-release.json"))!; mutation(root); return Json.Bytes(root); }
    public static async Task Contract()
    {
        Versions(); Feed(); Signatures(); Replacement(); await Files(); await Http();
    }
    public static async Task Run(Suite suite)
    {
        await suite.Run("updates compare stable numeric versions without downgrades", () => { Versions(); return Task.CompletedTask; });
        await suite.Run("update feed restricts repository architecture digest and size", () => { Feed(); return Task.CompletedTask; });
        await suite.Run("shared P256 update signature binds exact manifest and asset", () => { Signatures(); return Task.CompletedTask; });
        await suite.Run("downloaded update bytes match signed size and digest", Files);
        await suite.Run("public update requests are bounded unauthenticated and cancellable", Http);
        await suite.Run("update swap rolls back failures without losing either version", () => { Replacement(); return Task.CompletedTask; });
        await suite.Run("portable updates preserve user files and reject escaping archives", () => { Archives(); return Task.CompletedTask; });
        await suite.Run("signed downloads verify before fetching payload and clean incomplete stages", Downloads);
        await suite.Run("corrupt updater history cannot prevent monitor startup", () => {
            string path = Path.Combine(suite.Paths.Root, "update-history.json");
            foreach (string value in new[] { "null", "{broken", "[]" }) { PrivateFiles.Write(path, System.Text.Encoding.UTF8.GetBytes(value)); Suite.Check(UpdateInstaller.ReadResult(path) == null); }
            return Task.CompletedTask;
        });
    }
    private static void Versions()
    {
        Suite.Check(ReleaseVersion.Parse("0.1.10")!.Value.CompareTo(ReleaseVersion.Parse("v0.1.9")!.Value) > 0);
        foreach (string value in new[] { "", "0.1", "1.2.3.4", "01.2.3", "1.2.3-beta", "1.2.3+build", "v1.2.3/evil", "1.2.3?x", "１.2.3", "2147483648.0.0", " 1.2.3" })
            Suite.Check(ReleaseVersion.Parse(value) == null);
        var release = ReleaseFeed.Parse(Fixture("update-release.json"), ReleasePlatform.WindowsInstaller)!;
        Suite.Check(release.IsNewer("0.1.9") && !release.IsNewer("0.1.10") && !release.IsNewer("0.2.0") && !release.IsNewer("dev"));
    }
    private static void Feed()
    {
        Suite.Check(ReleaseFeed.Parse(Fixture("update-release.json"), ReleasePlatform.WindowsInstaller)!.Asset!.Name == "QuotaBar-0.1.10-Windows-x64-Setup.exe");
        Suite.Check(ReleaseFeed.Parse(Fixture("update-release.json"), ReleasePlatform.MacOSX64)!.Asset == null);
        foreach (string key in new[] { "draft", "prerelease" })
            Suite.Check(ReleaseFeed.Parse(Changed(root => root[key] = true), ReleasePlatform.WindowsInstaller) == null);
        Suite.Throws<UsageException>(() => ReleaseFeed.Parse(Changed(root => root["html_url"] = "https://example.com/evil"), ReleasePlatform.WindowsInstaller));
        foreach (string key in new[] { "browser_download_url", "digest", "state", "size", "duplicate" })
        {
            var data = Changed(root => {
                var assets = root["assets"]!.AsArray(); var asset = assets[1]!;
                switch (key) {
                    case "browser_download_url": asset[key] = "https://github.com/other/project/releases/download/v0.1.10/a.exe"; break;
                    case "digest": asset[key] = "sha256:invalid"; break;
                    case "state": asset[key] = "starter"; break;
                    case "size": asset[key] = ReleaseFeed.MaximumAssetBytes + 1; break;
                    default: assets.Add(asset.DeepClone()); break;
                }
            });
            Suite.Throws<UsageException>(() => ReleaseFeed.Parse(data, ReleasePlatform.WindowsInstaller));
        }
    }
    private static void Signatures()
    {
        var release = ReleaseFeed.Parse(Fixture("update-release.json"), ReleasePlatform.WindowsInstaller)!;
        byte[] manifest = Fixture("update-manifest.json"), signature = Fixture("update-manifest.sig");
        string key = File.ReadAllText(Path.Combine(Root, "update-test-public.pem"));
        Suite.Check(UpdateSignature.Verify(manifest, signature, key, release, "0.1.0").Build == 7);
        var changed = manifest.ToArray(); changed[0] ^= 1;
        Suite.Throws<UsageException>(() => UpdateSignature.Verify(changed, signature, key, release, "0.1.0"));
        using var other = ECDsa.Create(ECCurve.NamedCurves.nistP256);
        Suite.Throws<UsageException>(() => UpdateSignature.Verify(manifest, signature, other.ExportSubjectPublicKeyInfoPem(), release, "0.1.0"));
        Suite.Throws<UsageException>(() => UpdateSignature.Verify(manifest, signature, key, release, "0.2.0"));
        Suite.Throws<UsageException>(() => UpdateSignature.Verify(manifest, signature, key, release with { Asset = release.Asset! with { Bytes = 2048 } }, "0.1.0"));
    }
    private static async Task Files()
    {
        string path = Path.GetTempFileName();
        try {
            byte[] data = "synthetic update payload"u8.ToArray();
            var asset = new ReleaseAsset("fixture.zip", new Uri("https://example.invalid/fixture.zip"), data.Length, Convert.ToHexString(SHA256.HashData(data)).ToLowerInvariant());
            File.WriteAllBytes(path, data); await UpdateSignature.VerifyFile(path, asset);
            File.WriteAllBytes(path, new byte[data.Length]);
            await Suite.ThrowsAsync<UsageException>(() => UpdateSignature.VerifyFile(path, asset));
            File.WriteAllBytes(path, new byte[data.Length + 1]);
            await Suite.ThrowsAsync<UsageException>(() => UpdateSignature.VerifyFile(path, asset));
        } finally { File.Delete(path); }
    }
    private sealed class Handler(Func<HttpRequestMessage, HttpResponseMessage> respond) : HttpMessageHandler
    { protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token) { token.ThrowIfCancellationRequested(); return Task.FromResult(respond(request)); } }
    private static async Task Http()
    {
        using var client = new ReleaseClient(new Handler(request => {
            Suite.Check(request.RequestUri!.AbsoluteUri == ReleaseFeed.Endpoint);
            Suite.Check(request.Headers.Authorization == null && !request.Headers.Contains("Cookie"));
            Suite.Check(request.Headers.UserAgent.ToString() == "QuotaBar/0.1.0");
            return new(HttpStatusCode.OK) { Content = new ByteArrayContent(Fixture("update-release.json")) };
        }));
        Suite.Check((await client.Latest(ReleasePlatform.WindowsInstaller, "0.1.0"))!.IsNewer("0.1.0"));
        using var cancelled = new CancellationTokenSource(); cancelled.Cancel();
        await Suite.ThrowsAsync<OperationCanceledException>(() => client.Latest(ReleasePlatform.WindowsInstaller, "0.1.0", cancelled.Token));
        using var oversized = new ReleaseClient(new Handler(_ => new(HttpStatusCode.OK) { Content = new ByteArrayContent(new byte[ReleaseFeed.MaximumResponseBytes + 1]) }));
        await Suite.ThrowsAsync<HttpRequestException>(() => oversized.Latest(ReleasePlatform.WindowsInstaller, "0.1.0"));
        using var missing = new ReleaseClient(new Handler(_ => new(HttpStatusCode.NotFound)));
        Suite.Check(await missing.Latest(ReleasePlatform.WindowsInstaller, "0.1.0") == null);
    }

    private static void Replacement()
    {
        string root = Path.Combine(Path.GetTempPath(), "quotabar-swap-" + Guid.NewGuid()); Directory.CreateDirectory(root);
        try
        {
            var replacement = new UpdateReplacement(Path.Combine(root, "App"), Guid.NewGuid());
            Directory.CreateDirectory(replacement.Target); Directory.CreateDirectory(replacement.Incoming);
            File.WriteAllText(Path.Combine(replacement.Target, "version"), "old"); File.WriteAllText(Path.Combine(replacement.Incoming, "version"), "new");
            Suite.Throws<IOException>(() => replacement.Commit(() => throw new IOException("injected failure")));
            Suite.Check(File.ReadAllText(Path.Combine(replacement.Target, "version")) == "old" && !Directory.Exists(replacement.Backup));
            replacement.Commit();
            Suite.Check(File.ReadAllText(Path.Combine(replacement.Target, "version")) == "new" && File.ReadAllText(Path.Combine(replacement.Backup, "version")) == "old");
            replacement.Rollback();
            Suite.Check(File.ReadAllText(Path.Combine(replacement.Target, "version")) == "old" && !Directory.Exists(replacement.Backup));
        }
        finally { Directory.Delete(root, true); }
    }
    private static void Archives()
    {
        string root = Path.Combine(Path.GetTempPath(), "quotabar-archive-" + Guid.NewGuid()); Directory.CreateDirectory(root);
        try
        {
            string target = Path.Combine(root, "app"), incoming = Path.Combine(root, "incoming"), archive = Path.Combine(root, "update.zip");
            Directory.CreateDirectory(target); File.WriteAllText(Path.Combine(target, "user.txt"), "preserved"); File.WriteAllText(Path.Combine(target, "version.txt"), "old");
            void Zip(params string[] names)
            {
                File.Delete(archive); using var zip = ZipFile.Open(archive, ZipArchiveMode.Create);
                foreach (string name in names) { using var writer = new StreamWriter(zip.CreateEntry(name).Open()); writer.Write("new"); }
            }
            UpdateInstaller.CopyTree(target, incoming, CancellationToken.None);
            Zip("QuotaBar-Windows-x64/version.txt", "QuotaBar-Windows-x64/Dashboard/component.txt");
            UpdateInstaller.OverlayZip(archive, incoming, CancellationToken.None);
            Suite.Check(File.ReadAllText(Path.Combine(incoming, "user.txt")) == "preserved" && File.ReadAllText(Path.Combine(incoming, "version.txt")) == "new");
            foreach (string name in new[] { "../escape", "QuotaBar-Windows-x64/../../escape", "QuotaBar-Windows-x64/C:/escape", "QuotaBar-Windows-x64/dir./escape" })
            { Zip(name); Suite.Throws<IOException>(() => UpdateInstaller.OverlayZip(archive, incoming, CancellationToken.None)); }
            Zip("QuotaBar-Windows-x64/file", "QuotaBar-Windows-x64/FILE");
            Suite.Throws<IOException>(() => UpdateInstaller.OverlayZip(archive, incoming, CancellationToken.None));
            Suite.Check(!File.Exists(Path.Combine(root, "escape")));
        }
        finally { Directory.Delete(root, true); }
    }
    private static async Task Downloads()
    {
        using var key = ECDsa.Create(ECCurve.NamedCurves.nistP256);
        byte[] payload = "signed synthetic Windows payload"u8.ToArray();
        var original = ReleaseFeed.Parse(Fixture("update-release.json"), ReleasePlatform.WindowsInstaller)!;
        var asset = original.Asset! with { Bytes = payload.Length, Sha256 = Convert.ToHexString(SHA256.HashData(payload)).ToLowerInvariant() };
        var release = original with { Asset = asset };
        byte[] manifest = Json.Bytes(new { schemaVersion = 1, version = release.Version, build = 7, assets = new[] { new { name = asset.Name, bytes = asset.Bytes, sha256 = asset.Sha256 } } });
        byte[] signature = key.SignData(manifest, HashAlgorithmName.SHA256, DSASignatureFormat.Rfc3279DerSequence);
        string root = Path.Combine(Path.GetTempPath(), "quotabar-download-" + Guid.NewGuid()); PrivateFiles.EnsureDirectory(root);
        try
        {
            foreach (string mode in new[] { "success", "signature", "payload", "cancel", "oversized", "insecure" })
            {
                int requests = 0; using var cancel = new CancellationTokenSource();
                using var downloader = new UpdateDownloader(new Handler(request => {
                    requests++; Suite.Check(request.Headers.Authorization == null && !request.Headers.Contains("Cookie"));
                    string name = Path.GetFileName(request.RequestUri!.AbsolutePath);
                    byte[] data = name switch { "update-manifest.json" => manifest, "update-manifest.sig" => mode == "signature" ? new byte[72] : signature, _ => payload };
                    if (name == asset.Name)
                    {
                        if (mode == "cancel") cancel.Cancel();
                        if (mode == "payload") data = new byte[payload.Length];
                        if (mode == "oversized") data = new byte[payload.Length + 1];
                        if (mode == "insecure") { var redirect = new HttpResponseMessage(HttpStatusCode.Redirect); redirect.Headers.Location = new Uri("http://example.invalid/payload"); return redirect; }
                    }
                    return new(HttpStatusCode.OK) { Content = new ByteArrayContent(data) };
                }));
                string stage = Path.Combine(root, mode); bool succeeded = false;
                try { var result = await downloader.Prepare(release, "0.1.0", key.ExportSubjectPublicKeyInfoPem(), stage, _ => { }, cancel.Token); succeeded = true; Suite.Check(File.ReadAllBytes(result.Archive).SequenceEqual(payload)); }
                catch (Exception e) when (mode != "success" && e is UsageException or IOException or OperationCanceledException) { }
                Suite.Check(succeeded == (mode == "success") && Directory.Exists(stage) == succeeded);
                if (mode == "signature") Suite.Check(requests == 2, "Never request a payload before verifying its manifest.");
            }
        }
        finally { Directory.Delete(root, true); }
    }
}
