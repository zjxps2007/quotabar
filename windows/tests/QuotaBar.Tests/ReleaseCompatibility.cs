using System.Net;
using System.Reflection;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Nodes;
using QuotaBar.Core;
using QuotaBar.Windows;

internal static class ReleaseCompatibility
{
    // Run the same public entry points and arguments on the frozen and current assemblies.
    private sealed class Client(Assembly core, Assembly windows)
    {
        private readonly Type feed = core.GetType("QuotaBar.Core.ReleaseClient", true)!;
        private readonly Type downloader = windows.GetType("QuotaBar.Windows.UpdateDownloader", true)!;
        private readonly object platform = Enum.Parse(core.GetType("QuotaBar.Core.ReleasePlatform", true)!, "WindowsInstaller");
        internal string PublicKey => (string)windows.GetType("QuotaBar.Windows.UpdateInstaller", true)!
            .GetProperty("PublicKey")!.GetValue(null)!;

        private static async Task<object?> Run(Type type, string method, HttpMessageHandler? handler, params object[] args)
        {
            using var client = (IDisposable)Activator.CreateInstance(type, new object?[] { handler })!;
            var task = (Task)type.GetMethod(method)!.Invoke(client, args)!;
            await task;
            return task.GetType().GetProperty("Result")!.GetValue(task);
        }
        internal Task<object?> Latest(HttpMessageHandler? handler, CancellationToken token) =>
            Run(feed, "Latest", handler, platform, "0.0.0", token);
        internal Task<object?> Stage(object release, string key, string stage, HttpMessageHandler? handler, Action<int> progress, CancellationToken token) =>
            Run(downloader, "Prepare", handler, release, "0.0.0", key, stage, progress, token);
    }

    private sealed class Handler(Func<HttpRequestMessage, HttpResponseMessage> produce) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token)
        {
            token.ThrowIfCancellationRequested();
            return Task.FromResult(produce(request));
        }
    }

    private static JsonNode? Node(object? value) => value == null ? null : JsonSerializer.SerializeToNode(value, value.GetType(), Json.Options);
    private static string Hash(string path) { using var file = File.OpenRead(path); return Convert.ToHexStringLower(SHA256.HashData(file)); }

    public static async Task Compare(string directory, bool live)
    {
        var context = new AssemblyLoadContext("original Windows release client", isCollectible: true);
        string root = Directory.CreateTempSubdirectory("quotabar-release-compare-").FullName;
        try
        {
            directory = Path.GetFullPath(directory);
            var original = new Client(context.LoadFromAssemblyPath(Path.Combine(directory, "QuotaBar.Core.dll")),
                context.LoadFromAssemblyPath(Path.Combine(directory, "QuotaBar.Windows.dll")));
            var current = new Client(typeof(ReleaseClient).Assembly, typeof(UpdateDownloader).Assembly);
            if (live)
            {
                using var deadline = new CancellationTokenSource(TimeSpan.FromMinutes(10));
                async Task<JsonNode> Download(Client client, string name)
                {
                    var release = await client.Latest(null, deadline.Token) ?? throw new InvalidDataException("No public release.");
                    string stage = Path.Combine(root, name);
                    var progress = new List<int>();
                    var staged = (await client.Stage(release, client.PublicKey, stage, null, progress.Add, deadline.Token))!;
                    var update = Node(staged)!["update"]!;
                    var asset = update["asset"]!;
                    string file = Path.Combine(stage, (string)asset["name"]!);
                    string sha = Hash(file);
                    Suite.Check(new FileInfo(file).Length == (long)asset["bytes"]! && sha == (string)asset["sha256"]!);
                    Suite.Check(progress.Count > 1 && progress[^1] == 100 && progress.SequenceEqual(progress.Distinct().Order()));
                    Console.WriteLine($"{name}: {(string)asset["name"]!} {new FileInfo(file).Length} bytes sha256 {sha}, verified build {(int)update["build"]!}, progress 100%");
                    return update;
                }
                Suite.Check(JsonNode.DeepEquals(await Download(original, "original"), await Download(current, "Rust")));
                Console.WriteLine("PASS both installed release clients fetched the public feed and staged identical signed bytes");
                return;
            }

            using var key = ECDsa.Create(ECCurve.NamedCurves.nistP256);
            byte[] payload = new byte[160000]; RandomNumberGenerator.Fill(payload);
            string sha = Convert.ToHexStringLower(SHA256.HashData(payload));
            var feed = JsonNode.Parse(File.ReadAllBytes(Path.Combine(AppContext.BaseDirectory, "../../../../../../shared/fixtures/update-release.json")))!;
            var asset = feed["assets"]![1]!;
            asset["size"] = payload.Length; asset["digest"] = "sha256:" + sha;
            string assetName = (string)asset["name"]!;
            byte[] manifest = Json.Bytes(new { schemaVersion = 1, version = "0.1.10", build = 7,
                assets = new[] { new { name = assetName, bytes = payload.Length, sha256 = sha } } });
            byte[] signature = key.SignData(manifest, HashAlgorithmName.SHA256, DSASignatureFormat.Rfc3279DerSequence);
            string[] cases = ["success", "missing", "forbidden", "rateLimit", "feedRedirect", "invalidFeed", "missingFlag", "wrongFlag", "invalidTag", "badSignature",
                "badDigest", "oversize", "short", "fiveRedirects", "sixRedirects", "insecure", "seeOther", "cancel", "existing"];

            async Task<JsonNode> Observe(Client client, string name, string mode)
            {
                string stage = Path.Combine(root, name + "-" + mode);
                if (mode == "existing") { Directory.CreateDirectory(stage); File.WriteAllText(Path.Combine(stage, "keep"), "preserve"); }
                int hops = 0;
                var requests = new List<string>(); var progress = new List<int>();
                using var stop = new CancellationTokenSource();
                HttpResponseMessage Respond(HttpRequestMessage r)
                {
                    Uri uri = r.RequestUri!;
                    bool latest = uri.AbsoluteUri == ReleaseFeed.Endpoint;
                    Suite.Check(r.Headers.Authorization == null && !r.Headers.Contains("Cookie") && r.Method == HttpMethod.Get);
                    Suite.Check(r.Headers.UserAgent.ToString() == (latest ? "QuotaBar/0.0.0" : "QuotaBar-Updater"));
                    Suite.Check(r.Headers.Accept.ToString() == (latest ? "application/vnd.github+json" : ""));
                    string file = Path.GetFileName(uri.AbsolutePath);
                    requests.Add(uri.AbsoluteUri);
                    if (latest)
                    {
                        return mode switch {
                            "missing" => new(HttpStatusCode.NotFound), "forbidden" => new(HttpStatusCode.Forbidden),
                            "rateLimit" => new(HttpStatusCode.TooManyRequests), "feedRedirect" => Redirect("next"),
                            "invalidFeed" => Ok("{"u8.ToArray()), "missingFlag" => Ok("{}"u8.ToArray()),
                            "wrongFlag" => Ok("""{"draft":null}"""u8.ToArray()),
                            "invalidTag" => Ok("""{"draft":false,"prerelease":false}"""u8.ToArray()), _ => Ok(Json.Bytes(feed))
                        };
                    }
                    if (file == "update-manifest.json") return Ok(manifest);
                    if (file == "update-manifest.sig") return Ok(mode == "badSignature" ? new byte[72] : signature);
                    if ((mode == "fiveRedirects" && hops < 5) || (mode == "sixRedirects" && hops < 6))
                        return Redirect("redirect-" + ++hops);
                    return mode switch {
                        "insecure" => Redirect("http://example.invalid/file"), "seeOther" => new(HttpStatusCode.SeeOther),
                        "badDigest" => Ok(new byte[payload.Length]), "oversize" => Ok(new byte[payload.Length + 1]),
                        "short" => Ok(payload[..^1]), _ => Ok(payload)
                    };
                }
                string? error = null;
                try
                {
                    var release = await client.Latest(new Handler(Respond), stop.Token);
                    if (release != null)
                        await client.Stage(release, key.ExportSubjectPublicKeyInfoPem(), stage, new Handler(Respond), n => {
                            progress.Add(n);
                            if (mode == "cancel") stop.Cancel();
                        }, stop.Token);
                }
                catch (Exception e) when (e is IOException or HttpRequestException or OperationCanceledException or JsonException or KeyNotFoundException or InvalidOperationException || e.GetType().Name == "UsageException")
                {
                    error = e switch { OperationCanceledException => "cancelled", JsonException => "JsonException",
                        _ => e.GetType().GetProperty("Code")?.GetValue(e)?.ToString() ?? e.GetType().Name };
                }
                string[] files = Directory.Exists(stage) ? Directory.GetFiles(stage).Order().Select(f => Path.GetFileName(f) + "=" + Hash(f)).ToArray() : [];
                if (mode is "success" or "fiveRedirects")
                    Suite.Check(error == null && progress[^1] == 100 && Hash(Path.Combine(stage, assetName)) == sha);
                else if (mode == "existing") Suite.Check(error == "IOException" && File.ReadAllText(Path.Combine(stage, "keep")) == "preserve");
                else Suite.Check(!Directory.Exists(stage) && (mode == "missing" ? error == null : error != null), mode);
                if (mode == "badSignature") Suite.Check(requests.Count == 3);
                return Node(new { requests, progress, error, files })!;
            }
            foreach (string mode in cases)
                Suite.Check(JsonNode.DeepEquals(await Observe(original, "original", mode), await Observe(current, "rust", mode)), mode);
            Console.WriteLine($"PASS original/Rust Windows release discovery and staging: {cases.Length} cases, requests, progress, errors, files and cleanup");
        }
        finally { Directory.Delete(root, true); context.Unload(); }
    }

    private static HttpResponseMessage Ok(byte[] body) => new(HttpStatusCode.OK) { Content = new ByteArrayContent(body) };
    private static HttpResponseMessage Redirect(string uri)
    {
        var response = new HttpResponseMessage(HttpStatusCode.Redirect);
        response.Headers.Location = new Uri(uri, UriKind.RelativeOrAbsolute);
        return response;
    }
}
