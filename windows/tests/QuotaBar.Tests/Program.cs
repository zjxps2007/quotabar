using System.Diagnostics;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using QuotaBar.Core;
using QuotaBar.Windows;
using QuotaBar.Protocol;

if (args.FirstOrDefault() == "--fake-login")
{
    var dir = Environment.GetEnvironmentVariable("CODEX_HOME")!;
    File.WriteAllText(args[2], dir);
    if (args[1] == "wait") { await Task.Delay(30000); return 0; }
    if (args[1] == "fail") return 1;
    File.WriteAllBytes(Path.Combine(dir, "auth.json"), Fixtures.Auth());
    await Task.Delay(2000); return 0;
}
if (args.FirstOrDefault() is "--persist-write" or "--persist-read")
{
    var paths = new AppPaths(args[1]); var id = Guid.Parse("ed8f82ac-58b8-408b-a07e-c54b91d99378");
    if (args[0] == "--persist-write")
    {
        var settings = new Settings { Appearance = "dark", RefreshSeconds = 1800, Accounts = [new(id, Provider.Codex, "재시작 검사", Credentials.Parse(Fixtures.Auth(), Provider.Codex).Identity, "fixture", Enabled: false)] };
        new SettingsStore(paths).Save(settings); new SecretVault(paths).Save(id, Fixtures.Auth());
        PrivateFiles.Write(Path.Combine(paths.Root, "snapshots.json"), Json.Bytes(new Dictionary<Guid, Snapshot> { [id] = new(DateTimeOffset.UtcNow, [new("fixture", 42)]) }));
        using var owner = new MonitorService(paths);
        owner.Preferences(1800, "dark", true, true, "icon");
    }
    else
    {
        using var service = new MonitorService(paths);
        if (service.Settings.Appearance != "dark" || service.Settings.RefreshSeconds != 1800 || service.Settings.TrayDisplayMode != "icon" || service.Settings.Accounts.Single().Name != "재시작 검사" || service.State().Snapshots[id].Lowest != 42 || Credentials.Parse(new SecretVault(paths).Load(id)!, Provider.Codex).AccessToken != "fixture-access") return 2;
    }
    return 0;
}
var suite = new Suite();
await suite.Run("credential identity separates workspaces", () =>
{
    Suite.Check(Credentials.Parse(Fixtures.Auth("a"), Provider.Codex).Identity != Credentials.Parse(Fixtures.Auth("b"), Provider.Codex).Identity);
    return Task.CompletedTask;
});
await suite.Run("unknown quota and refresh deadlines", () =>
{
    Suite.Check(new Snapshot(DateTimeOffset.UtcNow, []).Lowest == null);
    var now = DateTimeOffset.UtcNow; Suite.Check(RefreshSchedule.Next(false, true, false, 60, now) == now.AddMinutes(15));
    Suite.Check(RefreshSchedule.Next(true, true, true, 60, now) == null); return Task.CompletedTask;
});
await suite.Run("DPAPI persistence rotation deletion and size bounds", () =>
{
    var vault = new SecretVault(suite.Paths); var id = Guid.NewGuid(); var original = Fixtures.Auth();
    vault.Save(id, original); Suite.Check(vault.Load(id)!.SequenceEqual(original));
    Suite.Check(!Encoding.UTF8.GetString(File.ReadAllBytes(Path.Combine(suite.Paths.Credentials, id + ".bin"))).Contains("fixture-refresh"));
    Suite.Throws<IOException>(() => vault.Save(id, new byte[1_048_577])); Suite.Check(vault.Load(id)!.SequenceEqual(original));
    vault.Delete(id); Suite.Check(vault.Load(id) == null); return Task.CompletedTask;
});
await suite.Run("corrupt settings preserved", () =>
{
    var file = Path.Combine(suite.Paths.Root, "settings.json"); File.WriteAllText(file, "{broken");
    Suite.Throws<JsonException>(() => new SettingsStore(suite.Paths).Load()); Suite.Check(File.ReadAllText(file) == "{broken"); File.Delete(file); return Task.CompletedTask;
});
foreach (string mode in new[] { "success", "wait", "fail" }) await suite.Run("isolated login " + mode, async () =>
{
    var record = Path.Combine(suite.Paths.Root, "attempt.txt");
    using var cts = new CancellationTokenSource(); if (mode == "wait") cts.CancelAfter(1200);
    try
    {
        var bytes = await new CodexLogin(suite.Paths).LoginAsync(cts.Token, Environment.ProcessPath, TimeSpan.FromSeconds(6), ["--fake-login", mode, record]);
        Suite.Check(mode == "success" && Credentials.Parse(bytes, Provider.Codex).RefreshToken != null);
    }
    catch (OperationCanceledException) { Suite.Check(mode == "wait"); }
    catch (UsageException) { Suite.Check(mode == "fail"); }
    Suite.Check(File.Exists(record) && !Directory.Exists(File.ReadAllText(record))); File.Delete(record);
});
await suite.Run("missing login executable never imports CLI credentials", async () =>
{
    await Suite.ThrowsAsync<System.ComponentModel.Win32Exception>(() => new CodexLogin(suite.Paths).LoginAsync(CancellationToken.None, Path.Combine(suite.Paths.Root, "missing.exe")));
    Suite.Check(Directory.GetDirectories(suite.Paths.Login).Length == 0);
});
await suite.Run("Claude wrapping capture original output and field restoration", async () =>
{
    string folder = Path.Combine(suite.Paths.Root, "한글 user's profile"); Directory.CreateDirectory(folder);
    var settingsFile = Path.Combine(folder, "settings.json");
    string original = ClaudeConnection.Bash() != null ? "cat" : "[Console]::Out.Write([Console]::In.ReadToEnd())";
    File.WriteAllText(settingsFile, new JsonObject { ["theme"] = "dark", ["statusLine"] = new JsonObject { ["type"] = "command", ["command"] = original } }.ToJsonString());
    var service = new ClaudeConnection(suite.Paths);
    string helper = Fixtures.Helper;
    var profile = service.Install(folder, helper);
    byte[] payload = Json.Bytes(new { session_id = Guid.NewGuid().ToString(), rate_limits = new { five_hour = new { used_percentage = 25, resets_at = DateTimeOffset.UtcNow.AddHours(3).ToUnixTimeSeconds() } }, secret = "omit-this" });
    using var output = new MemoryStream();
    var code = await service.RunAsync(profile.Id, profile.Generation, new MemoryStream(payload), output, Stream.Null);
    Suite.Check(code == 0 && output.ToArray().SequenceEqual(payload));
    var first = service.Latest(profile)!; service.Capture(profile, payload); Suite.Check(service.Latest(profile)!.ObservedAt == first.ObservedAt);
    Suite.Check(first.Windows.Single().Remaining == 75 && !File.ReadAllText(Directory.GetFiles(Path.Combine(suite.Paths.Profiles, profile.Id), "snapshot-*.json")[0]).Contains("omit-this"));
    var settings = JsonNode.Parse(File.ReadAllText(settingsFile))!; settings["theme"] = "light"; File.WriteAllText(settingsFile, settings.ToJsonString());
    Suite.Check(service.Uninstall(profile.Id)); settings = JsonNode.Parse(File.ReadAllText(settingsFile))!;
    Suite.Check(settings["theme"]!.GetValue<string>() == "light" && settings["statusLine"]!["command"]!.GetValue<string>() == original);
    profile = service.Install(folder, helper); settings = JsonNode.Parse(File.ReadAllText(settingsFile))!; settings["statusLine"]!["command"] = "user-edited";
    File.WriteAllText(settingsFile, settings.ToJsonString()); Suite.Check(!service.Uninstall(profile.Id));
});
await suite.Run("Claude packaged callback through both shells", async () =>
{
    var service = new ClaudeConnection(suite.Paths);
    string helper = Fixtures.Helper;
    foreach (var shell in new[] { "powershell", ClaudeConnection.Bash() }.Where(s => s != null))
    {
        string folder = Path.Combine(suite.Paths.Root, "callback 공백 user's " + (shell == "powershell" ? "ps" : "bash")); Directory.CreateDirectory(folder);
        string original = shell == "powershell" ? "[Console]::OpenStandardInput().CopyTo([Console]::OpenStandardOutput()); exit 7" : "cat; exit 7";
        File.WriteAllText(Path.Combine(folder, "settings.json"), new JsonObject { ["statusLine"] = new JsonObject { ["type"] = "command", ["command"] = original } }.ToJsonString());
        var profile = service.Install(folder, helper, shell);
        var start = new ProcessStartInfo(shell == "powershell" ? "powershell.exe" : shell!) { UseShellExecute = false, CreateNoWindow = true, RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (var arg in shell == "powershell" ? new[] { "-NoProfile", "-NonInteractive", "-Command", profile.Command } : new[] { "-c", profile.Command }) start.ArgumentList.Add(arg);
        using var p = Process.Start(start)!;
        string payload = Encoding.UTF8.GetString(Json.Bytes(new { session_id = Guid.NewGuid().ToString(), rate_limits = new { five_hour = new { used_percentage = 42, resets_at = DateTimeOffset.UtcNow.AddHours(1).ToUnixTimeSeconds() } } }));
        payload = payload[..^1] + ",\"text\":\"한글😀\"}";
        byte[] expected = Encoding.UTF8.GetBytes(payload);
        await p.StandardInput.BaseStream.WriteAsync(expected);
        p.StandardInput.Close(); using var output = new MemoryStream(); var stdout = p.StandardOutput.BaseStream.CopyToAsync(output); var stderr = p.StandardError.ReadToEndAsync();
        await p.WaitForExitAsync(); Suite.Check(p.ExitCode == 7, await stderr); await stdout;
        Suite.Check(output.ToArray().SequenceEqual(expected), "Original UTF-8 bytes were changed.");
        Suite.Check(service.Latest(profile)!.Windows.Single().Remaining == 58);
    }
});
await suite.Run("IPC rejects oversized and truncated frames", async () =>
{
    using var input = new MemoryStream(new byte[] { 0x7f, 0xff, 0xff, 0xff }); using var output = new MemoryStream(); using var pipe = new QuotaBar.Protocol.Channel(input, output);
    await Suite.ThrowsAsync<InvalidDataException>(() => pipe.ReadAsync(_ => Task.CompletedTask));
});
await TrayTests.Run(suite);
await Extended.Run(suite);
Console.WriteLine($"{suite.Passed} passed, {suite.Failed} failed");
suite.Cleanup(); return suite.Failed == 0 ? 0 : 1;

public static class Fixtures
{
    public static string Helper => Environment.GetEnvironmentVariable("QUOTABAR_TEST_HELPER") ?? Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "../../../../../src/QuotaBar.StatusLine/bin/Release/net10.0-windows/QuotaBar.StatusLine.exe"));
    public static byte[] Auth(string account = "workspace", string user = "user", string access = "fixture-access") => Json.Bytes(new {
        tokens = new { access_token = access, refresh_token = "fixture-refresh", account_id = account }, chatgpt_user_id = user, expires_at = DateTimeOffset.UtcNow.AddHours(1).ToUnixTimeSeconds() });
}
public sealed class Suite
{
    public AppPaths Paths { get; } = new(Path.Combine(Environment.GetEnvironmentVariable("QUOTABAR_TEST_ROOT") ?? Path.GetTempPath(), "quotabar-tests-" + Guid.NewGuid()));
    public int Passed, Failed;
    public Suite() { PrivateFiles.EnsureDirectory(Paths.Root); }
    public async Task Run(string name, Func<Task> test)
    { try { await test(); Passed++; Console.WriteLine("PASS " + name); } catch (Exception e) { Failed++; Console.WriteLine("FAIL " + name + " :: " + e); } }
    public static void Check(bool condition, string? detail = null) { if (!condition) throw new Exception(detail ?? "Assertion failed"); }
    public static void Throws<T>(Action action) where T : Exception { try { action(); } catch (T) { return; } throw new Exception("Expected " + typeof(T).Name); }
    public static async Task ThrowsAsync<T>(Func<Task> action) where T : Exception { try { await action(); } catch (T) { return; } throw new Exception("Expected " + typeof(T).Name); }
    public void Cleanup() { if (Directory.Exists(Paths.Root)) Directory.Delete(Paths.Root, true); }
}
