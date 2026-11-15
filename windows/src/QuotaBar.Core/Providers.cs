// Provider compatibility with QuotaBar and sylearn/AIUsage; see NOTICE.
using System.Security.Cryptography;
using System.Text.Json;

namespace QuotaBar.Core;

public sealed class ProviderClient : IDisposable
{
    private readonly HttpClient? http;
    private readonly CancellationTokenSource lifetime = new();
    private int disposed;
    public ProviderClient(HttpMessageHandler? handler = null)
    {
        if (handler != null) http = new(handler) { Timeout = TimeSpan.FromSeconds(20) };
    }

    private async Task<(int, byte[])> Send(NativeProviderQuery.Request description, CancellationToken token)
    {
        using var request = description.ToHttpRequest();
        using var response = await http!.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, token);
        int status = (int)response.StatusCode;
        if (!response.IsSuccessStatusCode) return (status, []);
        await using var stream = await response.Content.ReadAsStreamAsync(token);
        using var bytes = new MemoryStream();
        var buffer = new byte[8192];
        try
        {
            int count;
            while ((count = await stream.ReadAsync(buffer, token)) > 0)
            {
                if (bytes.Length + count > 2 * 1024 * 1024) throw new UsageException("large_response", "응답이 너무 큽니다.");
                bytes.Write(buffer, 0, count);
            }
            return (status, bytes.ToArray());
        }
        finally
        {
            CryptographicOperations.ZeroMemory(buffer);
            CryptographicOperations.ZeroMemory(bytes.GetBuffer().AsSpan(0, (int)bytes.Length));
        }
    }

    public static Snapshot ParseCodex(JsonElement json, string? email = null) => NativeDisplay.Quota("codex", json, email);
    public static Snapshot ParseGemini(JsonElement json) => NativeDisplay.Quota("gemini", json);
    private async Task<byte[]> Query(NativeProviderQuery.Operation op, byte[] auth, CancellationToken token)
    {
        if (http != null) return await NativeProviderQuery.Run(op, auth, token, Send);
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(lifetime.Token, token);
        return await NativeProviderQuery.RunNative(op, auth, linked.Token);
    }
    public async Task<Snapshot> FetchCodex(byte[] auth, CancellationToken token) =>
        Json.Read<Snapshot>(await Query(NativeProviderQuery.Operation.Codex, auth, token));
    public async Task<Snapshot> FetchGemini(byte[] auth, CancellationToken token) =>
        Json.Read<Snapshot>(await Query(NativeProviderQuery.Operation.Gemini, auth, token));
    public Task<byte[]> Refresh(byte[] current) =>
        Query(NativeProviderQuery.Operation.Refresh, current, CancellationToken.None);
    public void Dispose()
    {
        if (Interlocked.Exchange(ref disposed, 1) != 0) return;
        lifetime.Cancel();
        lifetime.Dispose();
        http?.Dispose();
    }
}
