using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text.Json;
using QuotaBar.Core;

namespace QuotaBar.Windows;

public static class Executables
{
    [DllImport("quotabar_native", EntryPoint = "qb_find_executable")]
    private static extern IntPtr FindNative([MarshalAs(UnmanagedType.LPUTF8Str)] string name,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string path, [MarshalAs(UnmanagedType.LPUTF8Str)] string userHome);
    [DllImport("quotabar_native", EntryPoint = "qb_string_free")] private static extern void Free(IntPtr result);

    public static string? Find(string name)
    {
        var found = FindNative(name, Environment.GetEnvironmentVariable("PATH") ?? "", AppPaths.UserHome);
        if (found == IntPtr.Zero) return null;
        try { return Marshal.PtrToStringUTF8(found); }
        finally { Free(found); }
    }
}

public sealed class CodexLogin(AppPaths paths)
{
    private int active;
    [DllImport("quotabar_native", EntryPoint = "qb_windows_login")]
    private static extern int Run([MarshalAs(UnmanagedType.LPUTF8Str)] string request, NativeCancellation cancel,
        out IntPtr bytes, out nuint length, out int osError);
    [DllImport("quotabar_native", EntryPoint = "qb_secret_free")] private static extern void Release(IntPtr bytes, nuint length);

    public async Task<byte[]> LoginAsync(CancellationToken token, string? executable = null, TimeSpan? timeout = null, IReadOnlyList<string>? prefix = null)
    {
        if (Interlocked.Exchange(ref active, 1) == 1) throw new UsageException("login_running", "이미 로그인이 진행 중입니다.");
        try
        {
            executable ??= Executables.Find("codex");
            if (executable == null) throw new UsageException("codex_missing", "Windows용 공식 Codex CLI를 설치하고 앱을 다시 실행하세요.");
            var duration = timeout ?? TimeSpan.FromMinutes(5);
            long? timeoutMs = (long)duration.TotalMilliseconds;
            if (timeoutMs == -1) timeoutMs = null;
            if (timeoutMs is < 0 or > 4294967294) throw new ArgumentOutOfRangeException(nameof(timeout));
            var request = JsonSerializer.Serialize(new { executable, directory = paths.Login, prefix = prefix ?? [], timeoutMs }, Json.Options);
            using var cancel = NativeCancellation.Create();
            using var registration = token.Register(cancel.Cancel);
            return await Task.Run(() =>
            {
                var status = Run(request, cancel, out var bytes, out var length, out var error);
                if (status != 0)
                    throw status switch {
                        1 => new OperationCanceledException(token.IsCancellationRequested ? token : new CancellationToken(true)),
                        2 => new UsageException("login_failed", "로그인이 완료되지 않았습니다. 다른 로그인 창을 닫고 다시 시도하세요."),
                        3 => new Win32Exception(error),
                        _ => (Exception)new IOException("로그인 폴더를 사용할 수 없습니다.", new Win32Exception(error))
                    };
                try
                {
                    token.ThrowIfCancellationRequested();
                    var result = new byte[checked((int)length)];
                    Marshal.Copy(bytes, result, 0, result.Length);
                    return result;
                }
                finally { Release(bytes, length); }
            });
        }
        finally { Interlocked.Exchange(ref active, 0); }
    }
}
