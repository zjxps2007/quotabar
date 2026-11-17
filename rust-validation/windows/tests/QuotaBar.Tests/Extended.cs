using System.Buffers.Binary;
using System.Diagnostics;
using System.IO.Pipes;
using System.Net;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using QuotaBar.Core;
using QuotaBar.Protocol;
using QuotaBar.Windows;

public static class Extended
{
    private static string FixturesPath => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../../../shared/fixtures"));
    private static JsonElement Fixture(string path) => Json.Read<JsonElement>(File.ReadAllBytes(Path.Combine(FixturesPath, path)));
    private static byte[] Expired()
    { var json = JsonNode.Parse(Fixtures.Auth())!; json["expires_at"] = DateTimeOffset.UtcNow.AddHours(-1).ToUnixTimeSeconds(); return Encoding.UTF8.GetBytes(json.ToJsonString()); }
    public static async Task Run(Suite suite)
    {
        await suite.Run("real process restart preserves settings DPAPI and last snapshot", async () =>
        {
            string root = Path.Combine(suite.Paths.Root, "process-restart");
            foreach (string mode in new[] { "--persist-write", "--persist-read" })
            {
                var info = new ProcessStartInfo(Environment.ProcessPath!) { UseShellExecute = false, CreateNoWindow = true };
                info.ArgumentList.Add(mode); info.ArgumentList.Add(root);
                using var child = Process.Start(info)!; using var timeout = new CancellationTokenSource(10000);
                await child.WaitForExitAsync(timeout.Token); Suite.Check(child.ExitCode == 0);
            }
        });
        await suite.Run("secret vault rejects hardlinked credential files", () =>
        {
            var id = Guid.NewGuid(); var vault = new SecretVault(suite.Paths); vault.Save(id, Fixtures.Auth());
            string file = Path.Combine(suite.Paths.Credentials, id + ".bin"), alias = Path.Combine(suite.Paths.Credentials, "link.bin");
            Suite.Check(CreateHardLink(alias, file, IntPtr.Zero)); Suite.Throws<IOException>(() => vault.Load(id));
            File.Delete(alias); Suite.Check(vault.Load(id) != null); vault.Delete(id); return Task.CompletedTask;
        });
        await suite.Run("shared provider quota contract and unknown values", () =>
        {
            var codex = ProviderClient.ParseCodex(Fixture("codex-quota.json")); Suite.Check(codex.Windows.Count == 3 && codex.Lowest == 47 && codex.Plan == "plus");
            var gemini = ProviderClient.ParseGemini(Fixture("gemini-quota.json")); Suite.Check(gemini.Windows.Select(w => w.Remaining).SequenceEqual(new double[] { 40, 90, 100 }));
            Suite.Check(ProviderClient.ParseGemini(Json.Element(new { })).Lowest == null); return Task.CompletedTask;
        });
        await suite.Run("HTTP accounts are isolated and cache disabled", async () =>
        {
            int calls = 0; var handler = new Handler(async (request, token) =>
            {
                string workspace = request.Headers.GetValues("ChatGPT-Account-Id").Single(); Suite.Check(request.Headers.Authorization?.Parameter == "access-" + workspace);
                Suite.Check(request.Headers.CacheControl is { NoCache: true, NoStore: true }); Interlocked.Increment(ref calls); await Task.Delay(10, token);
                return Reply(Fixture("codex-quota.json"));
            });
            using var client = new ProviderClient(handler);
            await Task.WhenAll(client.FetchCodex(Fixtures.Auth("a", access: "access-a"), default), client.FetchCodex(Fixtures.Auth("b", access: "access-b"), default)); Suite.Check(calls == 2);
        });
        await suite.Run("Gemini project discovery sends the selected project only to the quota endpoint", async () =>
        {
            var calls = new List<string>();
            using var client = new ProviderClient(new Handler(async (request, token) =>
            {
                var endpoint = request.RequestUri!.AbsolutePath;
                calls.Add(endpoint);
                Suite.Check(request.Headers.Authorization?.Parameter == "fixture-gemini" && !request.Headers.Contains("ChatGPT-Account-Id"));
                if (endpoint.EndsWith(":loadCodeAssist"))
                {
                    var body = Json.Read<JsonElement>(await request.Content!.ReadAsByteArrayAsync(token));
                    Suite.Check(body.Get("metadata").Get("ideType").Text() == "GEMINI_CLI");
                    return Reply(new { currentTier = new { id = "Standard" } });
                }
                if (endpoint == "/v1/projects")
                {
                    Suite.Check(request.Method == HttpMethod.Get && request.Content == null);
                    return Reply(new { projects = new[] { new { projectId = "ordinary" }, new { projectId = "gen-lang-client-example" } } });
                }
                Suite.Check(endpoint.EndsWith(":retrieveUserQuota"));
                var quota = Json.Read<JsonElement>(await request.Content!.ReadAsByteArrayAsync(token));
                Suite.Check(quota.Get("project").Text() == "gen-lang-client-example");
                return Reply(Fixture("gemini-quota.json"));
            }));
            var auth = Json.Bytes(new { access_token = "fixture-gemini", email = "gemini@example.invalid" });
            var result = await client.FetchGemini(auth, default);
            Suite.Check(calls.Count == 3 && result.Plan == "Standard" && result.Email == "gemini@example.invalid" && result.Lowest == 40);
        });
        await suite.Run("OAuth preserves omitted refresh token and rejects identity changes", async () =>
        {
            using var client = new ProviderClient(new Handler((request, _) =>
            { Suite.Check(request.Headers.Authorization == null); return Task.FromResult(Reply(new { access_token = "rotated-access", expires_in = 3600 })); }));
            var data = await client.Refresh(Expired()); var creds = Credentials.Parse(data, Provider.Codex);
            Suite.Check(creds.RefreshToken == "fixture-refresh" && creds.AccessToken == "rotated-access" && !creds.NeedsRefresh(DateTimeOffset.UtcNow));
            string jwt = "e30." + Convert.ToBase64String(Json.Bytes(new { sub = "different-user", chatgpt_account_id = "workspace" })).TrimEnd('=').Replace('+','-').Replace('/','_') + ".sig";
            using var bad = new ProviderClient(new Handler((_, _) => Task.FromResult(Reply(new { access_token = jwt }))));
            await Suite.ThrowsAsync<UsageException>(() => bad.Refresh(Expired()));
        });
        await suite.Run("concurrent OAuth refresh uses one rotation", async () =>
        {
            var store = new MemoryVault { Data = Expired() }; var id = Guid.NewGuid(); var account = new Account(id, Provider.Codex, "test", Credentials.Parse(store.Data, Provider.Codex).Identity, "", AppManagedLogin: true);
            int calls = 0; var sessions = new CodexSessions(store, async _ => { Interlocked.Increment(ref calls); await Task.Delay(30); return Fixtures.Auth(access: "new"); });
            var result = await Task.WhenAll(Enumerable.Range(0, 12).Select(_ => sessions.Credential(account))); Suite.Check(calls == 1 && result.All(r => Credentials.Parse(r, Provider.Codex).AccessToken == "new"));
        });
        await suite.Run("rotated token is retried after storage failure without reissuing refresh", async () =>
        {
            var store = new MemoryVault { Data = Expired(), FailSave = true }; var account = new Account(Guid.NewGuid(), Provider.Codex, "test", Credentials.Parse(store.Data, Provider.Codex).Identity, "");
            int calls = 0; var sessions = new CodexSessions(store, _ => { calls++; return Task.FromResult(Fixtures.Auth(access: "new")); });
            await Suite.ThrowsAsync<IOException>(() => sessions.Credential(account)); store.FailSave = false;
            var saved = await sessions.Credential(account); Suite.Check(calls == 1 && Credentials.Parse(saved, Provider.Codex).AccessToken == "new");
        });
        await suite.Run("automatic restart blocks until pending rotated credentials are saved", async () =>
        {
            var store = new MemoryVault { Data = Expired(), FailSave = true };
            var account = new Account(Guid.NewGuid(), Provider.Codex, "test", Credentials.Parse(store.Data, Provider.Codex).Identity, "");
            int calls = 0; var sessions = new CodexSessions(store, _ => { calls++; return Task.FromResult(Fixtures.Auth(access: "new")); });
            await Suite.ThrowsAsync<IOException>(() => sessions.Credential(account));
            Suite.Throws<IOException>(sessions.FlushPendingWrites);
            store.FailSave = false; sessions.FlushPendingWrites();
            Suite.Check(calls == 1 && Credentials.Parse(store.Data!, Provider.Codex).AccessToken == "new");
        });
        await suite.Run("account deletion prevents late OAuth resurrection", async () =>
        {
            var store = new MemoryVault { Data = Expired() }; var account = new Account(Guid.NewGuid(), Provider.Codex, "test", Credentials.Parse(store.Data, Provider.Codex).Identity, "");
            var gate = new TaskCompletionSource<byte[]>(); var sessions = new CodexSessions(store, _ => gate.Task);
            var pending = sessions.Credential(account); sessions.Invalidate(account.Id); store.Delete(account.Id); gate.SetResult(Fixtures.Auth());
            await Suite.ThrowsAsync<OperationCanceledException>(() => pending); Suite.Check(store.Data == null);
        });
        await suite.Run("revoked OAuth waits for explicit reconnect", async () =>
        {
            var store = new MemoryVault { Data = Expired() }; var account = new Account(Guid.NewGuid(), Provider.Codex, "test", Credentials.Parse(store.Data, Provider.Codex).Identity, "");
            int calls = 0; var sessions = new CodexSessions(store, _ => { calls++; throw new UsageException("reauth_required", "test"); });
            await Suite.ThrowsAsync<UsageException>(() => sessions.Credential(account)); await Suite.ThrowsAsync<UsageException>(() => sessions.Credential(account)); Suite.Check(calls == 1);
        });
        await suite.Run("queued Windows callers retry a failed rotation using the latest DPAPI credential", async () =>
        {
            var vault = new SecretVault(suite.Paths);
            var old = Expired();
            var replacement = Fixtures.Auth(access: "rotation");
            var account = new Account(Guid.NewGuid(), Provider.Codex, "test", Credentials.Parse(old, Provider.Codex).Identity, "");
            vault.Save(account.Id, old);
            var firstReply = new TaskCompletionSource<byte[]>(TaskCreationOptions.RunContinuationsAsynchronously);
            int calls = 0;
            var sessions = new CodexSessions(suite.Paths.Credentials, _ => ++calls == 1 ? firstReply.Task : Task.FromResult(replacement));
            var first = sessions.Credential(account);
            var retry = sessions.Credential(account, old);
            var follow = sessions.Credential(account, old);
            Suite.Check(!retry.IsCompleted && !follow.IsCompleted && calls == 1);
            firstReply.SetException(new HttpRequestException("fixture offline"));
            await Suite.ThrowsAsync<HttpRequestException>(() => first);
            Suite.Check((await retry).SequenceEqual(replacement) && (await follow).SequenceEqual(replacement));
            var reopened = new CodexSessions(suite.Paths.Credentials, _ => throw new Exception("Unexpected repeat rotation"));
            Suite.Check((await reopened.Credential(account)).SequenceEqual(replacement) && calls == 2);
            Suite.Check(vault.Load(account.Id)!.SequenceEqual(replacement));
        });
        await suite.Run("old Windows refresh cannot revoke a queued reconnected login", async () =>
        {
            var vault = new SecretVault(suite.Paths);
            var old = Expired();
            var account = new Account(Guid.NewGuid(), Provider.Codex, "test", Credentials.Parse(old, Provider.Codex).Identity, "");
            var replacement = Fixtures.Auth(access: "login-after-reconnect");
            vault.Save(account.Id, old);
            var delayed = new TaskCompletionSource<byte[]>(TaskCreationOptions.RunContinuationsAsynchronously);
            var sessions = new CodexSessions(suite.Paths.Credentials, _ => delayed.Task);
            var first = sessions.Credential(account);
            sessions.Invalidate(account.Id);
            vault.Save(account.Id, replacement);
            var second = sessions.Credential(account);
            Suite.Check(!second.IsCompleted);
            delayed.SetException(new UsageException("reauth_required", "old login"));
            await Suite.ThrowsAsync<UsageException>(() => first);
            Suite.Check((await second).SequenceEqual(replacement) && vault.Load(account.Id)!.SequenceEqual(replacement));
        });
        await suite.Run("monitor saves a rotated login before retrying the quota request after HTTP 401", async () =>
        {
            var paths = new AppPaths(Path.Combine(suite.Paths.Root, "managed-retry"));
            var vault = new SecretVault(paths);
            var original = Fixtures.Auth();
            var account = new Account(Guid.NewGuid(), Provider.Codex, "fixture", Credentials.Parse(original, Provider.Codex).Identity, "", AppManagedLogin: true);
            vault.Save(account.Id, original);
            new SettingsStore(paths).Save(new Settings { Accounts = [account] });
            var calls = new List<string>();
            using var monitor = new MonitorService(paths, new ProviderClient(new Handler((request, _) =>
            {
                if (request.Method == HttpMethod.Post)
                {
                    calls.Add("rotate");
                    Suite.Check(request.RequestUri!.AbsolutePath == "/oauth/token" && request.Headers.Authorization == null);
                    return Task.FromResult(Reply(new { access_token = "retried", expires_in = 3600 }));
                }
                var token = request.Headers.Authorization!.Parameter!;
                calls.Add(token);
                if (token == "fixture-access") return Task.FromResult(new HttpResponseMessage(HttpStatusCode.Unauthorized));
                Suite.Check(token == Credentials.Parse(vault.Load(account.Id)!, Provider.Codex).AccessToken);
                return Task.FromResult(Reply(Fixture("codex-quota.json")));
            })));
            await monitor.Refresh(account.Id);
            Suite.Check(calls.SequenceEqual(["fixture-access", "rotate", "retried"]));
            Suite.Check(monitor.State().Snapshots[account.Id].Lowest == 47 && monitor.State().Errors.Count == 0);
            await monitor.PrepareUpdateExit();
            await monitor.Shutdown();
        });
        await suite.Run("shared local usage contract duplicates forks cache and Claude streaming", () =>
        {
            var expected = Fixture("local-usage/expected.json"); var scanner = new LocalUsageScanner();
            for (int pass = 0; pass < 2; pass++)
            {
                var report = scanner.Scan([Path.Combine(FixturesPath, "local-usage/codex")], [Path.Combine(FixturesPath, "local-usage/claude")], default, DateTimeOffset.Parse(expected.Get("now").GetString()!), TimeZoneInfo.Utc);
                foreach (var provider in new[] { Provider.Codex, Provider.Claude })
                {
                    var counts = report.Days.Where(d => d.Provider == provider).Select(d => d.Counts).Aggregate(new TokenCounts(), (a, b) => a.Add(b));
                    var target = expected.Get(provider.ToString().ToLowerInvariant()); Suite.Check(counts.Input == target.Get("input").GetInt64() && counts.Cached == target.Get("cached").GetInt64() && counts.Output == target.Get("output").GetInt64());
                }
                Suite.Check(report.Warnings.Count == expected.Get("warningCount").GetInt32()); Suite.Check(!Encoding.UTF8.GetString(Json.Bytes(report)).Contains("SYNTHETIC_BODY"));
            }
            return Task.CompletedTask;
        });
        await suite.Run("large conversation streaming and partial rows", () =>
        {
            string dir = Path.Combine(suite.Paths.Root, "logs"); Directory.CreateDirectory(dir); string file = Path.Combine(dir, "log.jsonl");
            using (var writer = new StreamWriter(file))
            {
                writer.Write("{\"type\":\"assistant\",\"timestamp\":\"2026-10-16T02:00:00Z\",\"message\":{\"id\":\"large\",\"model\":\"claude-test\",\"content\":\"");
                for (int i = 0; i < 4000; i++) writer.Write(new string('X', 8192));
                writer.WriteLine("\",\"usage\":{\"input_tokens\":100,\"output_tokens\":30}}}"); writer.Write("{\"unfinished\":");
            }
            var scanner = new LocalUsageScanner(); var report = scanner.Scan([], [dir], default, DateTimeOffset.Parse("2026-10-16T12:00:00Z"), TimeZoneInfo.Utc);
            Suite.Check(report.Days.Single().Counts.Total == 130 && report.Warnings.Count == 0);
            File.AppendAllText(file, "\n{broken}\n"); report = scanner.Scan([], [dir], default, DateTimeOffset.Parse("2026-10-16T12:00:00Z"), TimeZoneInfo.Utc);
            Suite.Check(report.Warnings.Count > 0 && report.Days.Single().Counts.Total == 130);
            File.Delete(file); Suite.Check(scanner.Scan([], [dir], default).Days.Count == 0); return Task.CompletedTask;
        });
        await suite.Run("local usage respects timezone and cancellation", () =>
        {
            var result = new LocalUsageScanner().Scan([Path.Combine(FixturesPath, "local-usage/codex")], [], default, DateTimeOffset.Parse("2026-10-16T12:00:00Z"), TimeZoneInfo.CreateCustomTimeZone("west", TimeSpan.FromHours(-8), "west", "west"));
            Suite.Check(result.Days.All(d => d.Day == "2026-10-15")); using var cancel = new CancellationTokenSource(); cancel.Cancel();
            Suite.Throws<OperationCanceledException>(() => new LocalUsageScanner().Scan([FixturesPath], [], cancel.Token)); return Task.CompletedTask;
        });
        await suite.Run("settings and provider snapshot survive a fresh service owner", async () =>
        {
            var paths = new AppPaths(Path.Combine(suite.Paths.Root, "restart")); var source = Path.Combine(suite.Paths.Root, "cli-auth.json"); File.WriteAllBytes(source, Fixtures.Auth());
            Guid id;
            using (var service = new MonitorService(paths, new ProviderClient(new Handler((_, _) => Task.FromResult(Reply(Fixture("codex-quota.json")))))))
            {
                await service.Import(Provider.Codex, source); id = service.Settings.Accounts.Single().Id;
                service.Edit(id, "한글 계정", true, false); service.Preferences(900, "dark", false, false);
                File.WriteAllBytes(source, Fixtures.Auth("different-workspace")); await service.Refresh();
                Suite.Check(service.State().Errors[id].Contains("다른 계정") && service.State().Snapshots[id].Lowest == 47);
                // A mismatched CLI login must not replace the last usable DPAPI copy.
                File.Delete(source); await service.Refresh();
                Suite.Check(!service.State().Errors.ContainsKey(id) && service.State().Snapshots[id].Lowest == 47);
                var vault = new SecretVault(paths);
                var saved = vault.Load(id)!;
                vault.Save(id, Fixtures.Auth("other-vault-account")); await service.Refresh();
                Suite.Check(service.State().Errors[id] == "인증 계정이 일치하지 않습니다.");
                vault.Delete(id); await service.Refresh();
                Suite.Check(service.State().Errors[id] == "로그인 파일을 다시 연결하세요.");
                vault.Save(id, saved);
                await service.Shutdown();
            }
            using (var service = new MonitorService(paths))
            {
                Suite.Check(service.Settings is { Appearance: "dark", RefreshSeconds: 900, ScanLocalLogs: false, ShowTaskbarIcon: false });
                Suite.Check(service.Settings.Accounts.Single().Name == "한글 계정" && service.State().Snapshots[id].Lowest == 47);
                service.Remove(id); Suite.Check(new SecretVault(paths).Load(id) == null); await service.Shutdown();
            }
        });
        await suite.Run("one monitor owner and same-user activation", async () =>
        {
            var paths = new AppPaths(Path.Combine(suite.Paths.Root, "instance")); using var first = new SingleInstance(paths); using var second = new SingleInstance(paths);
            Suite.Check(first.IsOwner && !second.IsOwner); var activated = new TaskCompletionSource();
            var server = first.Listen(() => activated.TrySetResult()); await second.Activate(); await activated.Task.WaitAsync(TimeSpan.FromSeconds(3)); first.Dispose(); await server;
        });
        await suite.Run("original and Rust monitors share the lease and activate each other", async () =>
        {
            var paths = new AppPaths(Path.Combine(suite.Paths.Root, "기존 ßıᾀµſ𐐨"));
            string lockPath = Path.Combine(paths.Root, "monitor.lock");
            var digest = SHA256.HashData(Encoding.UTF8.GetBytes(paths.Root.ToUpperInvariant() + PrivateFiles.User.Value));
            string name = "QuotaBar-" + Convert.ToHexString(digest)[..24];
            const PipeOptions options = PipeOptions.Asynchronous | PipeOptions.CurrentUserOnly;
            using var timeout = new CancellationTokenSource(TimeSpan.FromSeconds(10));
            using (var owner = new SingleInstance(paths))
            {
                Suite.Check(owner.IsOwner);
                Suite.Throws<IOException>(() => { using var duplicate = new FileStream(lockPath, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None); });
                int activations = 0;
                var received = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
                var server = owner.Listen(() => { Interlocked.Increment(ref activations); received.TrySetResult(); });
                try
                {
                    // A previous client can disappear without writing, or send a different byte.
                    using var stalled = new NamedPipeClientStream(".", name, PipeDirection.Out, options);
                    await stalled.ConnectAsync(timeout.Token);
                    foreach (byte value in new byte[] { 7, 1 })
                    {
                        using var client = new NamedPipeClientStream(".", name, PipeDirection.Out, options);
                        await client.ConnectAsync(timeout.Token);
                        await client.WriteAsync(new byte[] { value }, timeout.Token);
                    }
                    await received.Task.WaitAsync(timeout.Token);
                    Suite.Check(Volatile.Read(ref activations) == 1);
                }
                finally { owner.Dispose(); await server.WaitAsync(timeout.Token); }
            }
            using (var legacyLease = new FileStream(lockPath, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None))
            using (var server = new NamedPipeServerStream(name, PipeDirection.In, 1, PipeTransmissionMode.Byte, options))
            using (var second = new SingleInstance(paths))
            {
                Suite.Check(!second.IsOwner);
                async Task Receive()
                {
                    await server.WaitForConnectionAsync(timeout.Token);
                    byte[] command = new byte[1];
                    Suite.Check(await server.ReadAsync(command, timeout.Token) == 1 && command[0] == 1);
                }
                await Task.WhenAll(Receive(), second.Activate()).WaitAsync(timeout.Token);
            }
            using var restarted = new SingleInstance(paths);
            Suite.Check(restarted.IsOwner);
        });
        await suite.Run("IPC fragmented frames unicode version and truncated EOF", async () =>
        {
            using var output = new MemoryStream(); using (var channel = new QuotaBar.Protocol.Channel(Stream.Null, output))
            {
                await channel.SendAsync(Message.Create("test", "한글 문자열")); var data = output.ToArray();
                using var reader = new QuotaBar.Protocol.Channel(new FragmentedStream(data), Stream.Null); string? text = null;
                await reader.ReadAsync(m => { text = m.Body<string>(); return Task.CompletedTask; }); Suite.Check(text == "한글 문자열");
                using var broken = new QuotaBar.Protocol.Channel(new MemoryStream(data[..^1]), Stream.Null); await Suite.ThrowsAsync<EndOfStreamException>(() => broken.ReadAsync(_ => Task.CompletedTask));
            }
            var payload = Json.Bytes(new Message(999, "test", null, Json.Element(true))); var frame = new byte[payload.Length + 4]; BinaryPrimitives.WriteInt32BigEndian(frame, payload.Length); payload.CopyTo(frame, 4);
            using var version = new QuotaBar.Protocol.Channel(new MemoryStream(frame), Stream.Null); await Suite.ThrowsAsync<InvalidDataException>(() => version.ReadAsync(_ => Task.CompletedTask));
        });
        await suite.Run("Claude stale generation cannot write snapshots", async () =>
        {
            var paths = new AppPaths(Path.Combine(suite.Paths.Root, "generation")); var service = new ClaudeConnection(paths, Fixtures.Helper);
            var dir = Path.Combine(suite.Paths.Root, "generation-claude"); Directory.CreateDirectory(dir); var first = service.Install(dir, Environment.ProcessPath!); var second = service.Install(dir, Environment.ProcessPath!);
            var bytes = Json.Bytes(new { session_id = Guid.NewGuid(), rate_limits = new { five_hour = new { used_percentage = 25, resets_at = DateTimeOffset.UtcNow.AddHours(1).ToUnixTimeSeconds() } } });
            using var output = new MemoryStream(); await service.RunAsync(first.Id, first.Generation, new MemoryStream(bytes), output, Stream.Null); Suite.Check(service.Latest(second) == null);
            service.Capture(second, bytes); Suite.Check(service.Latest(second)!.Windows.Single().Remaining == 75); service.Uninstall(second.Id);
        });
    }
    public static HttpResponseMessage Reply<T>(T data) => new(HttpStatusCode.OK) { Content = new ByteArrayContent(Json.Bytes(data)) };
    [System.Runtime.InteropServices.DllImport("kernel32.dll", CharSet = System.Runtime.InteropServices.CharSet.Unicode, SetLastError = true)]
    private static extern bool CreateHardLink(string alias, string file, IntPtr security);
}
public sealed class Handler(Func<HttpRequestMessage, CancellationToken, Task<HttpResponseMessage>> response) : HttpMessageHandler
{ protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken token) => response(request, token); }
public sealed class MemoryVault : ICredentialStore
{
    public byte[]? Data; public bool FailSave;
    public byte[]? Load(Guid id) => Data;
    public void Save(Guid id, byte[] data) { if (FailSave) throw new IOException("fixture write failure"); Data = data; }
    public void Delete(Guid id) => Data = null;
}
public sealed class FragmentedStream(byte[] data) : MemoryStream(data)
{ public override ValueTask<int> ReadAsync(Memory<byte> buffer, CancellationToken token = default) => base.ReadAsync(buffer[..Math.Min(1, buffer.Length)], token); }
