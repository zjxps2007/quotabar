using System.Runtime.ExceptionServices;
using System.Runtime.InteropServices;
using System.Text.Json;

namespace QuotaBar.Core;

// The installed Windows monitor passes no HTTP callback. Explicit handlers
// supply only headers and body chunks to the same Rust release transaction.
internal sealed class ReleaseTransfer : IDisposable
{
    private const string Library = "quotabar_native";
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate int Send([MarshalAs(UnmanagedType.LPUTF8Str)] string url,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string agent, int feed, IntPtr receiver);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] private delegate void Progress(int percent);
    private delegate int Operation(NativeCancellation cancel, Send? send, Progress progress, out IntPtr json, out int error);

    [DllImport(Library, EntryPoint = "qb_windows_release_latest")]
    private static extern int LatestNative(ReleasePlatform platform, [MarshalAs(UnmanagedType.LPUTF8Str)] string current,
        NativeCancellation cancel, Send? send, out IntPtr json, out int error);
    [DllImport(Library, EntryPoint = "qb_windows_release_stage")]
    private static extern int StageNative(byte[] metadata, nuint length, [MarshalAs(UnmanagedType.LPUTF8Str)] string directory,
        NativeCancellation cancel, Send? send, Progress progress, out IntPtr json, out int error);
    [DllImport(Library, EntryPoint = "qb_windows_release_wait_ms")] private static extern int Remaining(IntPtr receiver);
    [DllImport(Library, EntryPoint = "qb_windows_release_headers")]
    private static extern int Headers(IntPtr receiver, ushort status, long length, [MarshalAs(UnmanagedType.LPUTF8Str)] string? location);
    [DllImport(Library, EntryPoint = "qb_windows_release_chunk")] private static extern int Chunk(IntPtr receiver, byte[] data, nuint length);
    [DllImport(Library, EntryPoint = "qb_string_free")] private static extern void Free(IntPtr output);

    private readonly CancellationTokenSource lifetime = new();
    private readonly HttpClient? http;
    private int disposed;

    internal ReleaseTransfer(HttpMessageHandler? handler)
    {
        if (handler != null || !OperatingSystem.IsWindows())
            http = new(handler ?? new HttpClientHandler { AllowAutoRedirect = false, UseCookies = false })
                { Timeout = Timeout.InfiniteTimeSpan };
    }

    internal Task<AppRelease?> Latest(ReleasePlatform platform, string current, CancellationToken token) =>
        Run<AppRelease?>((NativeCancellation cancel, Send? send, Progress _, out IntPtr json, out int error) =>
            LatestNative(platform, current.Contains('\0') ? "" : current, cancel, send, out json, out error), token);

    internal Task<VerifiedUpdate> Stage(AppRelease release, string current, string key, string directory,
        Action<int> progress, CancellationToken token)
    {
        var bytes = Json.Bytes(new { release, currentVersion = current, publicKey = key });
        var path = Path.GetFullPath(directory);
        return Run<VerifiedUpdate>((NativeCancellation cancel, Send? send, Progress report, out IntPtr json, out int error) =>
            StageNative(bytes, (nuint)bytes.Length, path, cancel, send, report, out json, out error), token, progress);
    }

    private Task<T> Run<T>(Operation operation, CancellationToken token, Action<int>? progress = null)
    {
        var owner = SynchronizationContext.Current;
        var linked = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token, token);
        return Task.Run(() =>
        {
            using var scope = linked;
            using var cancel = NativeCancellation.Create();
            using var registration = linked.Token.Register(cancel.Cancel);
            ExceptionDispatchInfo? failure = null;
            int Exchange(string url, string agent, int feed, IntPtr receiver)
            {
                try { return Read(url, agent, feed != 0, receiver, linked.Token); }
                catch (Exception e) { failure = ExceptionDispatchInfo.Capture(e); return -1; }
            }
            void Report(int percent)
            {
                if (progress == null || failure != null) return;
                try
                {
                    if (owner == null) progress(percent);
                    else owner.Send(_ => progress(percent), null);
                }
                catch (Exception e) { failure = ExceptionDispatchInfo.Capture(e); cancel.Cancel(); }
            }
            linked.Token.ThrowIfCancellationRequested();
            int status = operation(cancel, http == null ? null : Exchange, Report, out var json, out var error);
            try
            {
                failure?.Throw();
                if (status != 0) throw Failure(status, error, linked.Token);
                return JsonSerializer.Deserialize<T>(Marshal.PtrToStringUTF8(json)!, Json.Options)!;
            }
            finally { Free(json); }
        });
    }

    private int Read(string url, string agent, bool feed, IntPtr receiver, CancellationToken token)
    {
        using var request = new HttpRequestMessage(HttpMethod.Get, url);
        request.Headers.UserAgent.ParseAdd(agent);
        if (feed) request.Headers.Accept.ParseAdd("application/vnd.github+json");
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token);
        void Deadline() => timeout.CancelAfter(Remaining(receiver));
        Deadline();
        using var reply = http!.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, timeout.Token).GetAwaiter().GetResult();
        int action = Headers(receiver, (ushort)reply.StatusCode, reply.Content.Headers.ContentLength ?? -1,
            reply.Headers.Location?.OriginalString);
        if (action <= 0) return action;
        Deadline();
        using var input = reply.Content.ReadAsStreamAsync(timeout.Token).GetAwaiter().GetResult();
        var buffer = new byte[65536];
        while (true)
        {
            Deadline();
            int count = input.ReadAsync(buffer.AsMemory(), timeout.Token).AsTask().GetAwaiter().GetResult();
            if (count == 0) return 0;
            if (Chunk(receiver, buffer, (nuint)count) != 0) return -1;
        }
    }

    internal static Exception Failure(int status, int error, CancellationToken token) => status switch
    {
        1 => new UsageException("development_build", "배포 앱에서 업데이트를 확인하세요."),
        2 => new UsageException("update_rate_limit", "업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요."),
        3 => new UsageException("invalid_release", "업데이트 정보를 확인할 수 없습니다. 잠시 후 다시 확인하세요."),
        4 => new UsageException("update_download", "서명된 업데이트 파일을 내려받지 못했습니다. 현재 버전을 유지합니다."),
        5 => new UsageException("update_signature", "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다."),
        6 => new OperationCanceledException(token),
        7 => new TaskCanceledException("업데이트 서버 응답 시간이 초과되었습니다."),
        8 => new HttpRequestException("Update feed or asset unavailable.", Marshal.GetExceptionForHR(error)),
        9 when error == 5 => new UnauthorizedAccessException("Update directory access denied."),
        10 => new JsonException("Invalid release JSON."),
        11 => new KeyNotFoundException("Missing release property."),
        12 => new InvalidOperationException("Unexpected release property type."),
        _ => new IOException("Could not read or stage the update.", error == 0 ? null : new System.ComponentModel.Win32Exception(error))
    };

    public void Dispose()
    {
        if (Interlocked.Exchange(ref disposed, 1) != 0) return;
        lifetime.Cancel();
        http?.Dispose();
        lifetime.Dispose();
    }
}
