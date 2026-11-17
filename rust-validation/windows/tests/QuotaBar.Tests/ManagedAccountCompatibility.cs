using System.Net;
using System.Reflection;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using QuotaBar.Core;
using QuotaBar.Windows;

internal static class ManagedAccountCompatibility
{
    // The parent process rejects the real HTTP tunnel and checks its destination and headers.
    public static async Task DefaultTransport(Assembly assembly, bool expired)
    {
        var suite = new Suite();
        try
        {
            var document = JsonNode.Parse(Fixtures.Auth())!;
            if (expired) document["expires_at"] = 1;
            byte[] login = Encoding.UTF8.GetBytes(document.ToJsonString());
            var id = Guid.NewGuid();
            var account = new Account(id, Provider.Codex, "Proxy fixture", Credentials.Parse(login, Provider.Codex).Identity, "", AppManagedLogin: true);
            var vault = new SecretVault(suite.Paths);
            vault.Save(id, login);
            new SettingsStore(suite.Paths).Save(new Settings { Accounts = [account] });
            var host = assembly.GetType("QuotaBar.Windows.MonitorService", true)!;
            var paths = Activator.CreateInstance(assembly.GetType("QuotaBar.Windows.AppPaths", true)!, suite.Paths.Root)!;
            using var service = (IDisposable)Activator.CreateInstance(host, paths, null, null)!;
            await (Task)host.GetMethod("Refresh")!.Invoke(service, [id])!;
            var value = host.GetMethod("State")!.Invoke(service, null)!;
            var state = JsonSerializer.SerializeToNode(value, value.GetType(), Json.Options)!;
            Suite.Check((string?)state["errors"]![id.ToString("D")] == "서버에 연결하지 못했습니다. 네트워크를 확인하세요.");
            Suite.Check(vault.Load(id)!.SequenceEqual(login) && state["snapshots"]!.AsObject().Count == 0);
            await (Task)host.GetMethod("Shutdown")!.Invoke(service, null)!;
        }
        finally { suite.Cleanup(); }
    }

    public static async Task Compare(Assembly originalMonitor, Type originalClient)
    {
        var suite = new Suite();
        try
        {
            var old = await Exercise(originalMonitor, originalClient, Path.Combine(suite.Paths.Root, "original"));
            var native = await Exercise(typeof(MonitorService).Assembly, typeof(ProviderClient), Path.Combine(suite.Paths.Root, "rust"));
            Suite.Check(JsonNode.DeepEquals(old, native), $"Managed monitor observations differ: {old}, {native}");
            foreach (string name in old.Select(p => p.Key))
                Console.WriteLine($"PASS original and Rust managed Codex {name}");
        }
        finally { suite.Cleanup(); }
    }

    private static async Task<JsonObject> Exercise(Assembly monitorAssembly, Type clientType, string root)
    {
        var paths = new AppPaths(root);
        var vault = new SecretVault(paths);
        var id = Guid.NewGuid();
        var login = Fixtures.Auth();
        var account = new Account(id, Provider.Codex, "Managed fixture", Credentials.Parse(login, Provider.Codex).Identity, "", AppManagedLogin: true);
        new SettingsStore(paths).Save(new Settings { Accounts = [account] });
        vault.Save(id, login);
        byte[] quota = File.ReadAllBytes(Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../../../shared/fixtures/codex-quota.json")));
        int requests = 0;
        var handler = new Handler(async (request, token) =>
        {
            int index = ++requests;
            bool exchange = index is 2 or 5;
            Suite.Check(request.Method == (exchange ? HttpMethod.Post : HttpMethod.Get));
            if (exchange)
            {
                Suite.Check(request.RequestUri!.AbsolutePath == "/oauth/token" && request.Headers.Authorization == null);
                var body = JsonNode.Parse(await request.Content!.ReadAsByteArrayAsync(token))!;
                Suite.Check((string?)body["refresh_token"] == (index == 2 ? "fixture-refresh" : "fixture-rotated"));
            }
            else
            {
                Suite.Check(request.RequestUri!.AbsolutePath == "/backend-api/wham/usage");
                string? access = request.Headers.Authorization?.Parameter;
                Suite.Check(access == (index == 1 ? "fixture-access" : "fixture-second"));
                Suite.Check(Credentials.Parse(vault.Load(id)!, Provider.Codex).AccessToken == access, "Quota used an unsaved replacement");
            }
            return index switch {
                1 => new HttpResponseMessage(HttpStatusCode.Unauthorized),
                2 => new HttpResponseMessage(HttpStatusCode.OK) { Content = new StringContent(
                    """{"access_token":"fixture-second","refresh_token":"fixture-rotated","expires_in":7200}""", Encoding.UTF8, "application/json") },
                3 => new HttpResponseMessage(HttpStatusCode.OK) { Content = new ByteArrayContent(quota) },
                4 => new HttpResponseMessage(HttpStatusCode.Forbidden),
                5 => new HttpResponseMessage(HttpStatusCode.BadRequest),
                _ => throw new InvalidOperationException("Unexpected HTTP after revocation")
            };
        });
        var client = Activator.CreateInstance(clientType, handler)!;
        var serviceType = monitorAssembly.GetType("QuotaBar.Windows.MonitorService", true)!;
        var hostPaths = Activator.CreateInstance(monitorAssembly.GetType("QuotaBar.Windows.AppPaths", true)!, root)!;
        using var service = (IDisposable)Activator.CreateInstance(serviceType, hostPaths, client, null)!;
        var result = new JsonObject();
        JsonNode? cached = null;
        foreach (var (stage, expected) in new[] { ("persist before retry", 3), ("retain quota on revocation", 5), ("skip HTTP until reconnected", 5) })
        {
            await (Task)serviceType.GetMethod("Refresh")!.Invoke(service, [id])!;
            var state = serviceType.GetMethod("State")!.Invoke(service, null)!;
            var data = JsonSerializer.SerializeToNode(state, state.GetType(), Json.Options)!;
            var snapshot = data["snapshots"]![id.ToString("D")]!.AsObject();
            Suite.Check(requests == expected && snapshot["lowest"]!.GetValue<double>() == 47);
            if (cached != null) Suite.Check(JsonNode.DeepEquals(cached, snapshot), "A failed refresh changed the last snapshot");
            cached = snapshot.DeepClone();
            snapshot.Remove("fetchedAt");
            var error = (string?)data["errors"]![id.ToString("D")];
            Suite.Check((stage == "persist before retry") == (error == null));
            result[stage] = new JsonObject { ["requests"] = requests, ["snapshot"] = snapshot.DeepClone(), ["error"] = error };
        }
        await (Task)serviceType.GetMethod("PrepareUpdateExit")!.Invoke(service, null)!;
        await (Task)serviceType.GetMethod("Shutdown")!.Invoke(service, null)!;
        return result;
    }
}
