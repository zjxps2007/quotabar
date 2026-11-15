using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed record ClaudeProfile(string Id, string Directory, string Generation, string Command, JsonObject? Original, string Shell);
public sealed record ClaudeSample(string ProfileId, string Generation, string SessionId, DateTimeOffset ReceivedAt, DateTimeOffset ObservedAt, List<QuotaWindow> Windows);
public sealed class ClaudeConnection(AppPaths paths, string? statusLineExecutable = null)
{
    internal sealed record Location(string Directory, string Id);
    [DllImport("quotabar_native", EntryPoint = "qb_windows_claude")]
    private static extern int Operation([MarshalAs(UnmanagedType.LPUTF8Str)] string root, byte[] request, nuint requestLength,
        byte[]? input, nuint inputLength, out IntPtr output);
    [DllImport("quotabar_native", EntryPoint = "qb_string_free")]
    private static extern void Release(IntPtr value);

    private static T Call<T>(object request, string root = "", byte[]? input = null)
    {
        var bytes = Json.Bytes(request);
        int code = Operation(root, bytes, (nuint)bytes.Length, input, (nuint)(input?.Length ?? 0), out var result);
        try
        {
            string value = Marshal.PtrToStringUTF8(result) ?? "Claude 연결 정보를 처리하지 못했습니다.";
            if (code != 0) throw code switch {
                -2 => new UsageException("invalid_directory", value),
                -3 => new JsonException(value),
                2 => new FileNotFoundException(value),
                3 => new DirectoryNotFoundException(value),
                5 => new UnauthorizedAccessException(value),
                _ => new IOException(value)
            };
            return JsonSerializer.Deserialize<T>(value, Json.Options)!;
        }
        finally { Release(result); }
    }
    public static readonly string[] ApiKeys = Call<string[]>(new { op = "keys" });
    internal static Location Locate(string path) => Call<Location>(new { op = "locate", path });
    public static string Canonical(string path) => Locate(path).Directory;
    public static string Id(string path) => Locate(path).Id;
    public static string? Bash() => Call<string?>(new { op = "bash" });
    public static string Quote(string text, bool bash) => Call<string>(new { op = "quote", text, bash });
    public ClaudeProfile Load(string id) => Call<ClaudeProfile>(new { op = "load", id }, paths.Root);
    public ClaudeProfile Install(string directory, string helper, string? selectedShell = null) =>
        Call<ClaudeProfile>(new { op = "install", directory, helper, shell = selectedShell }, paths.Root);
    public bool Uninstall(string id) => Call<bool>(new { op = "uninstall", id }, paths.Root);
    public Snapshot Fetch(string directory) => Call<Snapshot>(new { op = "fetch", directory }, paths.Root);
    public ClaudeSample? Latest(ClaudeProfile profile) =>
        Call<ClaudeSample?>(new { op = "latest", profile.Id, profile.Generation }, paths.Root);
    public void Capture(ClaudeProfile profile, byte[] input) =>
        Call<bool>(new { op = "capture", profile.Id, profile.Generation }, paths.Root, input);

    // Stream callers use the same native process and generation checks as installed CLI callbacks.
    public async Task<int> RunAsync(string id, string generation, Stream input, Stream output, Stream error)
    {
        Load(id);
        var bytes = PrivateFiles.ReadBounded(input, 2 * 1024 * 1024);
        var helper = statusLineExecutable ?? Path.Combine(AppContext.BaseDirectory, "StatusLine", "QuotaBar.StatusLine.exe");
        var start = new ProcessStartInfo(helper) { UseShellExecute = false, CreateNoWindow = true,
            RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true };
        foreach (var arg in new[] { "--claude-statusline", id, generation, paths.Root }) start.ArgumentList.Add(arg);
        using var process = Process.Start(start)!;
        var stdout = process.StandardOutput.BaseStream.CopyToAsync(output);
        var stderr = process.StandardError.BaseStream.CopyToAsync(error);
        try { await process.StandardInput.BaseStream.WriteAsync(bytes); } catch (IOException) { }
        process.StandardInput.Close();
        await Task.WhenAll(stdout, stderr, process.WaitForExitAsync());
        return process.ExitCode;
    }
}
