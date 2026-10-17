using System.Diagnostics;
using QuotaBar.Core;

namespace QuotaBar.Windows;
public static class Executables
{
    public static string? Find(string name)
    {
        var paths = (Environment.GetEnvironmentVariable("PATH") ?? "").Split(';').Where(Path.IsPathFullyQualified).ToList();
        paths.Add(Path.Combine(AppPaths.UserHome, ".local", "bin"));
        foreach (var folder in paths)
        {
            var exe = Path.Combine(folder.Trim('"'), name + ".exe"); if (File.Exists(exe)) return exe;
            if (name == "codex")
            {
                var package = Path.Combine(folder.Trim('"'), "node_modules", "@openai", "codex");
                if (Directory.Exists(package))
                {
                    var native = Directory.EnumerateFiles(package, "codex.exe", SearchOption.AllDirectories)
                        .FirstOrDefault(p => p.Contains(Environment.Is64BitProcess ? "x86_64-pc-windows" : "i686-pc-windows"));
                    if (native != null) return native;
                }
            }
        }
        return null;
    }
}
public sealed class CodexLogin(AppPaths paths)
{
    private int active;
    public async Task<byte[]> LoginAsync(CancellationToken token, string? executable = null, TimeSpan? timeout = null, IReadOnlyList<string>? prefix = null)
    {
        if (Interlocked.Exchange(ref active, 1) == 1) throw new UsageException("login_running", "이미 로그인이 진행 중입니다.");
        string? directory = null;
        try
        {
            executable ??= Executables.Find("codex");
            if (executable == null) throw new UsageException("codex_missing", "Windows용 공식 Codex CLI를 설치하고 앱을 다시 실행하세요.");
            PrivateFiles.EnsureDirectory(paths.Login);
            directory = Path.Combine(paths.Login, Guid.NewGuid().ToString()); PrivateFiles.EnsureDirectory(directory);
            var start = new ProcessStartInfo(executable) { WorkingDirectory = directory, UseShellExecute = false, CreateNoWindow = true,
                RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true };
            foreach (var arg in prefix ?? []) start.ArgumentList.Add(arg);
            foreach (var arg in new[] { "-c", "cli_auth_credentials_store=\"file\"", "login" }) start.ArgumentList.Add(arg);
            start.Environment["CODEX_HOME"] = directory;
            foreach (var key in new[] { "OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN", "OPENAI_BASE_URL" }) start.Environment.Remove(key);
            using var process = Process.Start(start) ?? throw new IOException("로그인 프로세스를 시작하지 못했습니다.");
            process.StandardInput.Close();
            // Drain without retaining URLs, tokens, or CLI transcripts.
            Task stdout = process.StandardOutput.BaseStream.CopyToAsync(Stream.Null);
            Task stderr = process.StandardError.BaseStream.CopyToAsync(Stream.Null);
            using var deadline = CancellationTokenSource.CreateLinkedTokenSource(token); deadline.CancelAfter(timeout ?? TimeSpan.FromSeconds(300));
            try
            {
                while (true)
                {
                    deadline.Token.ThrowIfCancellationRequested();
                    string auth = Path.Combine(directory, "auth.json");
                    if (File.Exists(auth))
                    {
                        byte[]? data = null;
                        try { data = PrivateFiles.ReadBounded(auth); _ = Credentials.Parse(data, Provider.Codex); }
                        catch (Exception e) when (e is IOException or System.Text.Json.JsonException or UsageException) { data = null; }
                        if (data != null && Credentials.Parse(data, Provider.Codex).RefreshToken != null) return data;
                    }
                    if (process.HasExited) throw new UsageException("login_failed", "로그인이 완료되지 않았습니다. 다른 로그인 창을 닫고 다시 시도하세요.");
                    await Task.Delay(150, deadline.Token);
                }
            }
            finally
            {
                if (!process.HasExited) process.Kill(); // Only the native CLI, never the user's browser.
                await process.WaitForExitAsync(); await Task.WhenAll(stdout, stderr);
            }
        }
        finally
        {
            if (directory != null && Directory.Exists(directory)) Directory.Delete(directory, true);
            Interlocked.Exchange(ref active, 0);
        }
    }
}
