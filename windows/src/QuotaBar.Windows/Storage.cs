using System.ComponentModel;
using System.Runtime.ExceptionServices;
using System.Runtime.InteropServices;
using System.Security.Principal;
using System.Text.Json;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public sealed class AppPaths(string? root = null)
{
    public string Root { get; } = Path.GetFullPath(root ?? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "QuotaBar"));
    public string Credentials => Path.Combine(Root, "Credentials");
    public string Profiles => Path.Combine(Root, "ClaudeProfiles");
    public string Login => Path.Combine(Root, "Login");
    public static string UserHome => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
    public static string CodexHome => Environment.GetEnvironmentVariable("CODEX_HOME") ?? Path.Combine(UserHome, ".codex");
    public static string ClaudeHome => Environment.GetEnvironmentVariable("CLAUDE_CONFIG_DIR") ?? Path.Combine(UserHome, ".claude");
    public static string DefaultPath(Provider provider) => provider switch {
        Provider.Codex => Path.Combine(CodexHome, "auth.json"), Provider.Claude => ClaudeHome,
        _ => Path.Combine(UserHome, ".gemini", "oauth_creds.json") };
}
public static class PrivateFiles
{
    public static SecurityIdentifier User => WindowsIdentity.GetCurrent().User!;
    [DllImport("quotabar_native", EntryPoint = "qb_private_directory")]
    private static extern int CreateDirectory([MarshalAs(UnmanagedType.LPUTF8Str)] string path);
    [DllImport("quotabar_native", EntryPoint = "qb_reject_links")]
    private static extern int CheckLinks([MarshalAs(UnmanagedType.LPUTF8Str)] string path);
    [DllImport("quotabar_native", EntryPoint = "qb_private_write")]
    private static extern int WriteFile([MarshalAs(UnmanagedType.LPUTF8Str)] string path, byte[] data, nuint length);
    [DllImport("quotabar_native", EntryPoint = "qb_file_read")]
    private static extern int ReadFile([MarshalAs(UnmanagedType.LPUTF8Str)] string path, nuint limit, out IntPtr data, out nuint length);
    [DllImport("quotabar_native", EntryPoint = "qb_secret_free")]
    private static extern void ReleaseBytes(IntPtr data, nuint length);

    private static void Check(int code, string message)
    {
        if (code == 0) return;
        var cause = code > 0 ? new Win32Exception(code) : null;
        throw code switch {
            2 => new FileNotFoundException(message, cause),
            3 => new DirectoryNotFoundException(message, cause),
            5 => new UnauthorizedAccessException(message, cause),
            _ => new IOException(message, cause)
        };
    }
    public static void RejectLinks(string path)
    {
        Check(CheckLinks(Path.GetFullPath(path)), "앱 데이터 경로에 링크를 사용할 수 없습니다.");
    }
    public static void EnsureDirectory(string path)
    {
        Check(CreateDirectory(Path.GetFullPath(path)), "앱 데이터 폴더의 소유자와 권한을 확인하세요.");
    }
    public static byte[] ReadBounded(string path, int limit = 1_048_576)
    {
        ArgumentOutOfRangeException.ThrowIfNegative(limit);
        Check(ReadFile(Path.GetFullPath(path), (nuint)limit, out var data, out var size), "파일을 읽을 수 없거나 크기 제한을 초과했습니다.");
        try
        {
            var result = new byte[checked((int)size)];
            if (result.Length > 0) Marshal.Copy(data, result, 0, result.Length);
            return result;
        }
        finally { ReleaseBytes(data, size); }
    }
    public static byte[] ReadBounded(Stream stream, int limit)
    {
        using var result = new MemoryStream(); var buffer = new byte[8192]; int count;
        while ((count = stream.Read(buffer)) != 0)
        { if (result.Length + count > limit) throw new IOException("파일 크기 제한을 초과했습니다."); result.Write(buffer, 0, count); }
        return result.ToArray();
    }
    public static void Write(string path, byte[] data)
    {
        Check(WriteFile(Path.GetFullPath(path), data, (nuint)data.Length), "파일을 저장하지 못했습니다.");
    }
}
public sealed class SettingsStore(AppPaths paths)
{
    [DllImport("quotabar_native", EntryPoint = "qb_windows_settings")]
    private static extern int Operation([MarshalAs(UnmanagedType.LPUTF8Str)] string root, byte[] request, nuint requestLength,
        byte[]? current, nuint currentLength, out IntPtr output);
    [DllImport("quotabar_native", EntryPoint = "qb_string_free")]
    private static extern void Release(IntPtr value);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate int Revoke(IntPtr id);
    [DllImport("quotabar_native", EntryPoint = "qb_windows_connect")]
    private static extern int ConnectNative([MarshalAs(UnmanagedType.LPUTF8Str)] string root, byte[] request, nuint requestLength,
        byte[] current, nuint currentLength, byte[]? credential, nuint credentialLength, Revoke revoke, out IntPtr output);
    [DllImport("quotabar_native", EntryPoint = "qb_windows_imported")]
    private static extern int ReadImported([MarshalAs(UnmanagedType.LPUTF8Str)] string root, byte[] request, nuint requestLength,
        out IntPtr secret, out nuint length, out IntPtr output);
    [DllImport("quotabar_native", EntryPoint = "qb_secret_free")] private static extern void ReleaseSecret(IntPtr bytes, nuint length);

    private T Call<T>(object request, object? current = null)
    {
        var args = Json.Bytes(request);
        var state = current == null ? null : Json.Bytes(current);
        int status = Operation(paths.Root, args, (nuint)args.Length, state, (nuint)(state?.Length ?? 0), out var output);
        try { return JsonSerializer.Deserialize<T>(Reply(status, output), Json.Options)!; }
        finally { Release(output); }
    }
    private static string Reply(int status, IntPtr output)
    {
        string result = Marshal.PtrToStringUTF8(output) ?? "설정을 읽거나 저장하지 못했습니다.";
        if (status == 0) return result;
        if (status == -2)
        {
            var error = JsonSerializer.Deserialize<UsageError>(result, Json.Options)!;
            throw new UsageException(error.Code, error.Message);
        }
        throw status switch {
            -3 => new JsonException(result), 2 => new FileNotFoundException(result),
            3 => new DirectoryNotFoundException(result), 5 => new UnauthorizedAccessException(result),
            _ => new IOException(result)
        };
    }
    internal Settings ConnectCredential(Settings settings, Provider provider, string path, byte[]? login, Action<Guid> invalidate)
    {
        var args = Json.Bytes(new { provider = (int)provider, path, managed = login != null });
        var current = Json.Bytes(settings);
        ExceptionDispatchInfo? failure = null;
        int RevokeAccount(IntPtr id)
        {
            try { invalidate(Guid.Parse(Marshal.PtrToStringUTF8(id)!)); return 1; }
            catch (Exception error) { failure = ExceptionDispatchInfo.Capture(error); return 0; }
        }
        int status = ConnectNative(paths.Root, args, (nuint)args.Length, current, (nuint)current.Length, login,
            (nuint)(login?.Length ?? 0), RevokeAccount, out var output);
        try { failure?.Throw(); return JsonSerializer.Deserialize<Settings>(Reply(status, output), Json.Options)!; }
        finally { Release(output); }
    }
    internal byte[] Imported(Account account)
    {
        var request = Json.Bytes(new { account.Id, provider = (int)account.Provider, path = account.SourcePath, account.Identity });
        int status = ReadImported(paths.Root, request, (nuint)request.Length, out var secret, out var length, out var output);
        try
        {
            Reply(status, output);
            var bytes = new byte[checked((int)length)];
            if (bytes.Length != 0) Marshal.Copy(secret, bytes, 0, bytes.Length);
            return bytes;
        }
        finally { ReleaseSecret(secret, length); Release(output); }
    }
    private sealed record UsageError(string Code, string Message);
    internal sealed record Connection(Settings Settings, bool Existing)
    { internal Account Account => Settings.Accounts[^1]; }

    public Settings Load() => Call<Settings>(new { op = "load" });
    internal Dictionary<Guid, Snapshot> LoadSnapshots(Settings settings) =>
        Call<Dictionary<Guid, Snapshot>>(new { op = "loadSnapshots", accounts = settings.Accounts.Select(a => a.Id) });
    internal void SaveSnapshots(Dictionary<Guid, Snapshot> snapshots) =>
        Call<bool>(new { op = "saveSnapshots" }, snapshots);
    public void Save(Settings settings) => PrivateFiles.Write(Path.Combine(paths.Root, "settings.json"), Json.Bytes(settings));
    internal Settings Commit(Connection connection) => Call<Settings>(new { op = "save" }, connection.Settings);
    internal Settings Preferences(Settings current, int seconds, string theme, bool scan, bool taskbar, string? trayDisplay, bool? autoRefresh, bool? automaticUpdates) =>
        Call<Settings>(new { op = "preferences", options = new { seconds, theme, scan, taskbar, trayDisplay, autoRefresh, automaticUpdates } }, current);
    internal Settings Edit(Settings current, Guid id, string name, bool enabled, bool tray) =>
        Call<Settings>(new { op = "edit", id, name, enabled, tray }, current);
    internal Connection Prepare(Settings current, Provider provider, string name, string identity, string path, bool managed = false) =>
        Call<Connection>(new { op = "prepare", provider, name, identity, path, managed }, current);
    internal Settings Remove(Settings current, Guid id) => Call<Settings>(new { op = "remove", id }, current);
}
