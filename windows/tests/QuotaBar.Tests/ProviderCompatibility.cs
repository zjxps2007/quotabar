using System.Net;
using System.Reflection;
using System.Runtime.Loader;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using QuotaBar.Core;

internal static class ProviderCompatibility
{
    private sealed record Step(int Status, string Body = "{}");
    private sealed record Case(string Name, string Method, byte[] Auth, params Step[] Replies);
    private sealed record Observation(JsonObject? Value, string? Error, JsonArray Requests);

    private static async Task<Observation> Observe(Type clientType, Case example)
    {
        var requests = new JsonArray();
        using var client = (IDisposable)Activator.CreateInstance(clientType,
            new Handler(async (request, token) =>
            {
                int index = requests.Count;
                requests.Add(new JsonObject {
                    ["url"] = request.RequestUri!.AbsoluteUri, ["method"] = request.Method.Method,
                    ["headers"] = JsonSerializer.SerializeToNode(request.Headers.ToDictionary(
                        h => h.Key.ToLowerInvariant(), h => h.Value.ToArray())),
                    ["contentType"] = request.Content?.Headers.ContentType?.ToString(),
                    ["body"] = request.Content == null ? null : JsonNode.Parse(await request.Content.ReadAsByteArrayAsync(token))
                });
                if (index >= example.Replies.Length) throw new InvalidOperationException("Unexpected request");
                var reply = example.Replies[index];
                if (reply.Status == -1) throw new HttpRequestException("fixture connection");
                if (reply.Status == -2) throw new TaskCanceledException("fixture deadline");
                return new HttpResponseMessage((HttpStatusCode)reply.Status) { Content = new ByteArrayContent(Encoding.UTF8.GetBytes(reply.Body)) };
            }))!;
        try
        {
            object[] parameters = example.Method == "Refresh" ? [example.Auth] : [example.Auth, CancellationToken.None];
            var task = (Task)clientType.GetMethod(example.Method)!.Invoke(client, parameters)!;
            await task;
            var value = task.GetType().GetProperty("Result")!.GetValue(task)!;
            var obj = value is byte[] raw ? JsonNode.Parse(raw)!.AsObject() : JsonSerializer.SerializeToNode(value, value.GetType(), Json.Options)!.AsObject();
            foreach (string key in new[] { "fetchedAt", "last_refresh" })
                if (obj.Remove(key, out var timestamp))
                    Suite.Check(DateTimeOffset.TryParse(timestamp!.GetValue<string>(), out var date) && (date - DateTimeOffset.UtcNow).Duration() < TimeSpan.FromMinutes(1));
            return new(obj, null, requests);
        }
        catch (Exception e)
        {
            var error = e is TargetInvocationException { InnerException: { } inner } ? inner : e;
            var code = error.GetType().GetProperty("Code")?.GetValue(error);
            return new(null, code == null ? error.GetType().Name : $"{code} {error.Message}", requests);
        }
    }

    public static async Task Compare(string referenceDirectory)
    {
        var reference = new AssemblyLoadContext("original provider client", isCollectible: true);
        try
        {
            var original = reference.LoadFromAssemblyPath(Path.GetFullPath(Path.Combine(referenceDirectory, "QuotaBar.Core.dll")))
                .GetType("QuotaBar.Core.ProviderClient", true)!;
            byte[] codex = Fixtures.Auth(), gemini = Json.Bytes(new { access_token = "fixture-gemini", email = "fixture@example.invalid" });
            var assist = new Step(200, """{"currentTier":{"id":"standard"},"cloudaicompanionProject":{"id":"selected"}}""");
            var list = new Step(200, """{"projects":[{"projectId":"gen-lang-client-42"}]}""");
            var reply = new Step(200, """{"buckets":[{"modelId":"gemini-flash","remainingFraction":0.4}]}""");
            var cases = new List<Case> {
                new("Codex snapshot", "FetchCodex", codex, new Step(200, """{"plan_type":"team","rate_limit":{"primary_window":{"used_percent":22}}}""")),
                new("Codex invalid response", "FetchCodex", codex, new Step(200, "{")),
                new("Codex top-level null", "FetchCodex", codex, new Step(200, "null")),
                new("Codex maximum depth", "FetchCodex", codex, new Step(200, new string('[', 65) + new string(']', 65))),
                new("Gemini direct selection", "FetchGemini", gemini, assist, reply),
                new("Gemini HTTP discovery failure", "FetchGemini", gemini, assist with { Status = 403 }, list, reply),
                new("Gemini missing selection", "FetchGemini", gemini, new Step(200), list, reply),
                new("Gemini optional timeouts", "FetchGemini", gemini, new Step(-2), new Step(-1), reply),
                new("Gemini malformed discovery", "FetchGemini", gemini, new Step(200, "{")),
                new("Gemini malformed list", "FetchGemini", gemini, new Step(200), new Step(200, "[")),
                new("Gemini oversized discovery", "FetchGemini", gemini, new Step(200, new string(' ', 2097153)), list, reply),
                new("Gemini oversized result", "FetchGemini", gemini, assist, new Step(200, new string(' ', 2097153))),
                new("OAuth malformed JSON", "Refresh", codex, new Step(200, "{")),
                new("OAuth no replacement", "Refresh", codex, new Step(200)),
                new("OAuth replacement", "Refresh", codex, new Step(200, """{"access_token":"next","refresh_token":"rotated"}"""))
            };
            foreach (int status in new[] { 301, 400, 401, 403, 429, 500 })
            {
                cases.Add(new($"Codex HTTP {status}", "FetchCodex", codex, new Step(status)));
                cases.Add(new($"Gemini HTTP {status}", "FetchGemini", gemini, assist, new Step(status)));
                cases.Add(new($"OAuth HTTP {status}", "Refresh", codex, new Step(status)));
            }
            foreach (var example in cases)
            {
                var old = await Observe(original, example);
                var native = await Observe(typeof(ProviderClient), example);
                Suite.Check(old.Requests.Count == example.Replies.Length, $"{example.Name} did not reach its reference HTTP replies");
                bool value = JsonNode.DeepEquals(old.Value, native.Value), requests = JsonNode.DeepEquals(old.Requests, native.Requests);
                Suite.Check(value && old.Error == native.Error && requests,
                    $"{example.Name}, output equal {value}, requests equal {requests}, original '{old.Error}', Rust '{native.Error}'");
                Console.WriteLine($"PASS {example.Name}");
            }
            Console.WriteLine($"Compared {cases.Count} Windows provider scenarios against the original client.");
        }
        finally { reference.Unload(); }
    }
}
