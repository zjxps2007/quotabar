// Ported from QuotaBar's Apache-2.0 Claude subscription adapters; see NOTICE.
using System.Diagnostics;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;
using System.Security.AccessControl;
using QuotaBar.Core;

namespace QuotaBar.Windows;
public sealed record ClaudeProfile(string Id, string Directory, string Generation, string Command, JsonObject? Original, string Shell);
public sealed record ClaudeSample(string ProfileId, string Generation, string SessionId, DateTimeOffset ReceivedAt, DateTimeOffset ObservedAt, List<QuotaWindow> Windows);
public sealed class ClaudeConnection(AppPaths paths)
{
    public static readonly string[] ApiKeys = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX", "CLAUDE_CODE_USE_FOUNDRY"];
    public static string Canonical(string path)
    {
        if (!Path.IsPathFullyQualified(path) || path.StartsWith(@"\\")) throw new UsageException("invalid_directory", "Windows 로컬 절대경로를 선택하세요. WSL/네트워크 경로는 아직 지원하지 않습니다.");
        path = Path.TrimEndingDirectorySeparator(Path.GetFullPath(path));
        if (path == Path.TrimEndingDirectorySeparator(Path.GetPathRoot(path)!)) throw new UsageException("invalid_directory", "드라이브 루트는 설정 폴더로 사용할 수 없습니다.");
        PrivateFiles.RejectLinks(path); return path;
    }
    public static string Id(string path) => Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(Canonical(path).ToUpperInvariant())));
    private string Folder(string id)
    {
        if (id.Length != 64 || !id.All(Uri.IsHexDigit)) throw new IOException("잘못된 프로필입니다.");
        return Path.Combine(paths.Profiles, id);
    }
    public ClaudeProfile Load(string id)
    {
        var profile = Json.Read<ClaudeProfile>(PrivateFiles.ReadBounded(Path.Combine(Folder(id), "profile.json")));
        if (profile.Id != id || Id(profile.Directory) != id) throw new IOException("Claude 프로필 경로가 변경되었습니다.");
        return profile;
    }
    public static string? Bash()
    {
        string? custom = Environment.GetEnvironmentVariable("CLAUDE_CODE_GIT_BASH_PATH");
        return new[] { custom, Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "Git", "bin", "bash.exe"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "Git", "bin", "bash.exe") }.FirstOrDefault(p => p != null && File.Exists(p));
    }
    public static string Quote(string text, bool bash) => "'" + text.Replace("'", bash ? "'\"'\"'" : "''") + "'";
    public ClaudeProfile Install(string directory, string helper, string? selectedShell = null)
    {
        directory = Canonical(directory); string id = Id(directory);
        Directory.CreateDirectory(directory);
        var file = Path.Combine(directory, "settings.json"); var settings = ReadSettings(file);
        var current = settings["statusLine"] as JsonObject;
        string? command = current?["command"]?.GetValue<string>();
        string profileFile = Path.Combine(Folder(id), "profile.json");
        ClaudeProfile? previous = File.Exists(profileFile) ? Load(id) : null;
        bool wrapper = command?.Contains("--claude-statusline") == true;
        if (wrapper && (previous == null || command != previous.Command)) throw new IOException("이전 status line 백업을 복구한 뒤 다시 연결하세요.");
        if (settings["statusLine"] != null && (current?["type"]?.GetValue<string>() != "command" || command == null)) throw new IOException("기존 status line 형식을 지원하지 않습니다. 설정은 보존했습니다.");
        var original = wrapper ? previous!.Original : current?.DeepClone() as JsonObject;
        if (original?["command"]?.GetValue<string>().Contains("--claude-statusline") == true) throw new IOException("중복 callback은 연결할 수 없습니다.");
        string generation = Guid.NewGuid().ToString();
        string shell = selectedShell ?? Bash() ?? "powershell";
        var arguments = new[] { "--claude-statusline", id, generation, paths.Root.Replace('\\','/') };
        string executable = Path.GetFullPath(helper).Replace('\\','/');
        string installed;
        if (shell != "powershell") installed = "exec " + string.Join(" ", new[] { executable }.Concat(arguments).Select(s => Quote(s, true)));
        else
        {
            // Windows PowerShell's native pipeline transcodes text and changes exit
            // codes. Copy the raw standard handles instead; no second shell is used.
            string nativeArgs = string.Join(" ", arguments.Select(s => "\"" + s.Replace("\"", "\\\"") + "\""));
            installed = "$quotaStart = New-Object Diagnostics.ProcessStartInfo; $quotaStart.FileName = " + Quote(executable, false)
                + "; $quotaStart.Arguments = " + Quote(nativeArgs, false)
                + "; $quotaStart.UseShellExecute = $false; $quotaStart.CreateNoWindow = $true; $quotaStart.RedirectStandardInput = $true; $quotaStart.RedirectStandardOutput = $true; $quotaStart.RedirectStandardError = $true; "
                + "$quotaChild = [Diagnostics.Process]::Start($quotaStart); $quotaOut = $quotaChild.StandardOutput.BaseStream.CopyToAsync([Console]::OpenStandardOutput()); $quotaErr = $quotaChild.StandardError.BaseStream.CopyToAsync([Console]::OpenStandardError()); "
                + "try { [Console]::OpenStandardInput().CopyTo($quotaChild.StandardInput.BaseStream) } catch [IO.IOException] {} finally { $quotaChild.StandardInput.Close() }; "
                + "$quotaChild.WaitForExit(); $quotaOut.Wait(); $quotaErr.Wait(); exit $quotaChild.ExitCode";
        }
        var profile = new ClaudeProfile(id, directory, generation, installed, original, wrapper ? previous!.Shell : shell);
        var updated = current?.DeepClone() as JsonObject ?? new JsonObject(); updated["type"] = "command"; updated["command"] = installed;
        settings["statusLine"] = updated;
        PrivateFiles.Write(profileFile, Json.Bytes(profile));
        try { WriteSettings(file, settings); }
        catch { if (previous != null) PrivateFiles.Write(profileFile, Json.Bytes(previous)); else File.Delete(profileFile); throw; }
        return profile;
    }
    public bool Uninstall(string id)
    {
        var profile = Load(id); string file = Path.Combine(profile.Directory, "settings.json"); var settings = ReadSettings(file);
        var current = settings["statusLine"] as JsonObject;
        if (current?["command"]?.GetValue<string>() != profile.Command) return false;
        if (profile.Original != null)
        {
            current!["command"] = profile.Original["command"]?.DeepClone(); current["type"] = profile.Original["type"]?.DeepClone();
        }
        else if (current!.All(p => p.Key is "type" or "command")) settings.Remove("statusLine");
        else return false;
        WriteSettings(file, settings);
        PrivateFiles.Write(Path.Combine(Folder(id), "profile.json"), Json.Bytes(profile with { Generation = Guid.NewGuid().ToString(), Command = "" }));
        return true;
    }
    public Snapshot Fetch(string directory)
    {
        var profile = Load(Id(directory)); var settings = ReadSettings(Path.Combine(directory, "settings.json"));
        if (settings["statusLine"]?["command"]?.GetValue<string>() != profile.Command || profile.Command.Length == 0)
            return new(DateTimeOffset.UtcNow, [], State: "disconnected");
        if (settings["apiKeyHelper"] != null || ApiKeys.Any(k => Routed(k, settings["env"]?[k]?.ToString()) || Routed(k, Environment.GetEnvironmentVariable(k))))
            return new(DateTimeOffset.UtcNow, [], State: "api-routed");
        var sample = Latest(profile);
        if (sample == null) return new(DateTimeOffset.UtcNow, [], State: "waiting");
        return new(DateTimeOffset.UtcNow, sample.Windows.Select(w => w.ResetAt <= DateTimeOffset.UtcNow ? w with { Remaining = 100, ResetAt = null } : w).ToList(),
            ObservedAt: sample.ObservedAt, State: sample.Windows.All(w => w.ResetAt <= DateTimeOffset.UtcNow) ? "reset" : sample.ObservedAt < DateTimeOffset.UtcNow.AddMinutes(-5) ? "stale" : "snapshot");
    }
    public ClaudeSample? Latest(ClaudeProfile profile) => Directory.EnumerateFiles(Folder(profile.Id), "snapshot-*.json")
        .Select(p => { try { return Json.Read<ClaudeSample>(PrivateFiles.ReadBounded(p, 8192)); } catch { return null; } })
        .Where(s => s != null && s.ProfileId == profile.Id && s.Generation == profile.Generation && s.ObservedAt <= DateTimeOffset.UtcNow.AddMinutes(1))
        .OrderByDescending(s => s!.ObservedAt).FirstOrDefault();
    public void Capture(ClaudeProfile profile, byte[] input)
    {
        var json = Json.Read<JsonElement>(input); var limits = json.Get("rate_limits"); string? session = json.Get("session_id").Text();
        if (!Guid.TryParse(session, out _) || limits.ValueKind != JsonValueKind.Object) return;
        List<QuotaWindow> windows = [];
        foreach (var (key,label) in new[] { ("five_hour","5시간"), ("seven_day","주간") })
        {
            var item = limits.Get(key); if (item.ValueKind is JsonValueKind.Undefined or JsonValueKind.Null) continue;
            if (item.Get("used_percentage").Number() is not { } used || used is < 0 or > 100 || item.Get("resets_at").Number() is not { } reset || reset <= 0 || reset >= 253402300799) return;
            windows.Add(new(label, 100 - used, DateTimeOffset.FromUnixTimeSeconds((long)reset)));
        }
        if (windows.Count == 0) return;
        var now = DateTimeOffset.UtcNow; string path = Path.Combine(Folder(profile.Id), "snapshot-" + session + ".json");
        var sample = new ClaudeSample(profile.Id, profile.Generation, session!, now, now, windows);
        if (File.Exists(path))
        {
            try { var old = Json.Read<ClaudeSample>(PrivateFiles.ReadBounded(path, 8192)); if (old.Generation == sample.Generation && old.Windows.SequenceEqual(windows)) sample = sample with { ObservedAt = old.ObservedAt }; }
            catch (Exception e) when (e is IOException or JsonException) { }
        }
        PrivateFiles.Write(path, Json.Bytes(sample));
        foreach (var old in Directory.EnumerateFiles(Folder(profile.Id), "snapshot-*.json").OrderByDescending(File.GetLastWriteTimeUtc).Skip(20)) File.Delete(old);
    }
    public async Task<int> RunAsync(string id, string generation, Stream input, Stream output, Stream error)
    {
        var profile = Load(id); var bytes = PrivateFiles.ReadBounded(input, 2 * 1024 * 1024);
        if (profile.Generation == generation && !ApiKeys.Any(k => Routed(k, Environment.GetEnvironmentVariable(k))))
            try { Capture(profile, bytes); } catch (Exception e) when (e is IOException or JsonException) { }
        if (profile.Original?["command"]?.GetValue<string>() is { } command)
        {
            if (command.Contains("--claude-statusline")) return 1;
            var start = profile.Shell == "powershell" ? new ProcessStartInfo("powershell.exe") : new ProcessStartInfo(profile.Shell);
            start.UseShellExecute = false; start.CreateNoWindow = true; start.RedirectStandardInput = start.RedirectStandardOutput = start.RedirectStandardError = true;
            if (profile.Shell == "powershell") foreach (var arg in new[] { "-NoProfile", "-NonInteractive", "-EncodedCommand", Convert.ToBase64String(Encoding.Unicode.GetBytes(command)) }) start.ArgumentList.Add(arg);
            else { start.ArgumentList.Add("-c"); start.ArgumentList.Add(command); }
            using var process = Process.Start(start)!;
            var stdout = process.StandardOutput.BaseStream.CopyToAsync(output); var stderr = process.StandardError.BaseStream.CopyToAsync(error);
            try { await process.StandardInput.BaseStream.WriteAsync(bytes); } catch (IOException) { }
            process.StandardInput.Close(); await process.WaitForExitAsync(); await Task.WhenAll(stdout, stderr); return process.ExitCode;
        }
        await output.WriteAsync(Encoding.UTF8.GetBytes("Claude · " + (profile.Generation == generation ? "quota connected" : "reconnect quota feedback")));
        return 0;
    }
    private static JsonObject ReadSettings(string file) => File.Exists(file) ? JsonNode.Parse(PrivateFiles.ReadBounded(file)) as JsonObject ?? throw new IOException("Claude 설정이 JSON 객체가 아닙니다.") : new();
    private static bool Routed(string key, string? value) => !string.IsNullOrWhiteSpace(value) && (!key.StartsWith("CLAUDE_CODE_USE_") || value is "1" || value.Equals("true", StringComparison.OrdinalIgnoreCase));
    private static void WriteSettings(string file, JsonObject json)
    {
        // External CLI directory retains its existing ACL. Replace only this file.
        string temporary = file + ".quotabar-" + Guid.NewGuid() + ".tmp";
        try
        {
            PrivateFiles.RejectLinks(file);
            var acl = File.Exists(file) ? new FileInfo(file).GetAccessControl() : null;
            using (var stream = new FileStream(temporary, FileMode.CreateNew, FileAccess.Write, FileShare.None))
            { stream.Write(Encoding.UTF8.GetBytes(json.ToJsonString(Json.Options))); stream.Flush(true); }
            if (acl != null) new FileInfo(temporary).SetAccessControl(acl);
            File.Move(temporary, file, true);
        }
        finally { if (File.Exists(temporary)) File.Delete(temporary); }
    }
}
