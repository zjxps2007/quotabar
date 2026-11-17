using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;

namespace QuotaBar.Core;

internal static class NativeProviderQuery
{
    internal enum Operation { Codex, Gemini, Refresh }
    private sealed record Failure(string Code, string Message);
    internal sealed record Request(string Url, string Method, Dictionary<string, string> Headers, JsonElement? Body, double Timeout)
    {
        internal HttpRequestMessage ToHttpRequest()
        {
            var request = new HttpRequestMessage(new HttpMethod(Method), Url);
            try
            {
                if (Body is JsonElement body) request.Content = new ByteArrayContent(Json.Bytes(body));
                foreach (var (name, value) in Headers)
                    if (name == "Content-Type") request.Content!.Headers.Add(name, value);
                    else request.Headers.Add(name, value);
                return request;
            }
            catch { request.Dispose(); throw; }
        }
    }

    [DllImport("quotabar_native", EntryPoint = "qb_provider_query_start")]
    private static extern int Start(Operation op, int mac, byte[] auth, nuint authLength, double timeout,
        out IntPtr query, out IntPtr output, out nuint count);
    [DllImport("quotabar_native", EntryPoint = "qb_provider_query_step")]
    private static extern int Step(IntPtr query, int httpStatus, byte[] body, nuint length, long nowMicros,
        out IntPtr output, out nuint count);
    [DllImport("quotabar_native", EntryPoint = "qb_provider_query_free")]
    private static extern void Free(IntPtr query);
    [DllImport("quotabar_native", EntryPoint = "qb_provider_query_run")]
    private static extern int Execute(Operation op, byte[] auth, nuint length, double timeout, long nowMicros,
        NativeCancellation cancel, out int osError, out IntPtr output, out nuint count);

    private static byte[] Result(int state, IntPtr output, nuint count)
    {
        if (state == -31) throw new JsonException("공급자 응답 형식을 확인하세요.");
        if (state < 0 && state != -30) throw Credentials.Failure(state);
        var bytes = Credentials.TakeBytes(0, output, count);
        if (state != -30) return bytes;
        try
        {
            var error = Json.Read<Failure>(bytes);
            throw new UsageException(error.Code, error.Message);
        }
        finally { CryptographicOperations.ZeroMemory(bytes); }
    }

    internal static Task<byte[]> RunNative(Operation op, byte[] auth, CancellationToken token) => Task.Run(() =>
    {
        using var cancel = NativeCancellation.Create();
        using var registration = token.Register(cancel.Cancel);
        int status = Execute(op, auth, (nuint)auth.Length, 20, long.MinValue, cancel, out int osError, out var bytes, out var count);
        return NativeResult(status, osError, bytes, count, token);
    }, token);

    internal static byte[] NativeResult(int status, int osError, IntPtr bytes, nuint count, CancellationToken token) =>
        status switch
        {
            -40 => throw new HttpRequestException("사용량 서버에 연결하지 못했습니다.", Marshal.GetExceptionForHR(osError)),
            -41 => throw new OperationCanceledException(token),
            -42 => throw new TaskCanceledException("사용량 서버 응답 시간이 초과되었습니다."),
            -43 => throw new UsageException("large_response", "응답이 너무 큽니다."),
            _ => Result(status, bytes, count)
        };

    internal static async Task<byte[]> Run(Operation op, byte[] auth, CancellationToken token,
        Func<Request, CancellationToken, Task<(int Status, byte[] Body)>> send)
    {
        int state = Start(op, 0, auth, (nuint)auth.Length, 20, out var query, out var output, out var count);
        try
        {
            while (true)
            {
                var bytes = Result(state, output, count);
                if (state == 0) return bytes;
                Request request;
                try { request = Json.Read<Request>(bytes); }
                finally { CryptographicOperations.ZeroMemory(bytes); }

                int status;
                byte[] body;
                try
                {
                    if (state == 1)
                    {
                        using var deadline = CancellationTokenSource.CreateLinkedTokenSource(token);
                        deadline.CancelAfter(TimeSpan.FromSeconds(request.Timeout));
                        (status, body) = await send(request, deadline.Token);
                    }
                    else (status, body) = await send(request, token);
                }
                catch (Exception e) when (state == 1 && e is (UsageException or HttpRequestException or OperationCanceledException))
                {
                    token.ThrowIfCancellationRequested();
                    status = -1;
                    body = [];
                }
                try
                {
                    long now = (DateTime.UtcNow.Ticks - DateTime.UnixEpoch.Ticks) / 10;
                    state = Step(query, status, body, (nuint)body.Length, now, out output, out count);
                }
                finally { CryptographicOperations.ZeroMemory(body); }
            }
        }
        finally { Free(query); }
    }
}
