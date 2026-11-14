// Parsing and OAuth semantics ported from QuotaBar and sylearn/AIUsage; see NOTICE.
using System.Net;
using static QuotaBar.Core.NativeProviderRequest;
using System.Text.Json;

namespace QuotaBar.Core;
public sealed class ProviderClient : IDisposable
{
    private readonly HttpClient http;
    public ProviderClient(HttpMessageHandler? handler = null)
    { http = new(handler ?? new HttpClientHandler { AllowAutoRedirect = false, UseCookies = false }) { Timeout = TimeSpan.FromSeconds(20) }; }
    private async Task<byte[]> RequestBytes(Endpoint endpoint, string access, CancellationToken token, string? account = null, string? project = null)
    {
        using var request = NativeProviderRequest.Build(endpoint, access, account, project);
        using var response = await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, token);
        if (!response.IsSuccessStatusCode)
        {
            string code = response.StatusCode is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden ? "unauthorized" : "http";
            if (endpoint == Endpoint.Refresh && (int)response.StatusCode is 400 or 401 or 403) code = "reauth_required";
            throw new UsageException(code, code == "reauth_required" ? "로그인이 해제되었습니다. 다시 로그인하세요." : $"사용량 서버 응답: HTTP {(int)response.StatusCode}. 마지막 성공 값을 표시합니다.");
        }
        await using var stream = await response.Content.ReadAsStreamAsync(token); using var bytes = new MemoryStream();
        var buffer = new byte[8192]; int count;
        while ((count = await stream.ReadAsync(buffer, token)) > 0)
        { if (bytes.Length + count > 2 * 1024 * 1024) throw new UsageException("large_response", "응답이 너무 큽니다."); bytes.Write(buffer, 0, count); }
        return bytes.ToArray();
    }
    public async Task<Snapshot> FetchCodex(byte[] auth, CancellationToken token)
    {
        var credentials = Credentials.Parse(auth, Provider.Codex);
        var bytes = await RequestBytes(Endpoint.Codex, credentials.AccessToken, token, account: credentials.AccountId);
        return ParseCodex(Json.Read<JsonElement>(bytes), credentials.Email);
    }
    public static Snapshot ParseCodex(JsonElement json, string? email = null) => NativeDisplay.Quota("codex", json, email);
    public async Task<byte[]> Refresh(byte[] current)
    {
        var old = Credentials.Parse(current, Provider.Codex);
        if (old.RefreshToken == null) throw new UsageException("reauth_required", "앱에서 다시 로그인하세요.");
        var result = await RequestBytes(Endpoint.Refresh, old.RefreshToken, CancellationToken.None);
        try { return Credentials.Rotate(current, result, DateTimeOffset.UtcNow); }
        finally { System.Security.Cryptography.CryptographicOperations.ZeroMemory(result); }
    }
    public async Task<Snapshot> FetchGemini(byte[] data, CancellationToken token)
    {
        var credentials = Credentials.Parse(data, Provider.Gemini); string? project = null; string? tier = null;
        try
        {
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token); timeout.CancelAfter(5000);
            var assist = Discover(await RequestBytes(Endpoint.GeminiAssist, credentials.AccessToken, timeout.Token), false);
            project = assist.Project; tier = assist.Tier;
        }
        catch (Exception e) when (e is UsageException or HttpRequestException or OperationCanceledException) { token.ThrowIfCancellationRequested(); }
        if (project == null)
        {
            try
            {
                using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token); timeout.CancelAfter(5000);
                project = Discover(await RequestBytes(Endpoint.GeminiProjects, credentials.AccessToken, timeout.Token), true).Project;
            }
            catch (Exception e) when (e is UsageException or HttpRequestException or OperationCanceledException) { token.ThrowIfCancellationRequested(); }
        }
        var result = Json.Read<JsonElement>(await RequestBytes(Endpoint.GeminiQuota, credentials.AccessToken, token, project: project));
        return ParseGemini(result) with { Email = credentials.Email, Plan = tier ?? "Code Assist" };
    }
    public static Snapshot ParseGemini(JsonElement root) => NativeDisplay.Quota("gemini", root);
    public void Dispose() => http.Dispose();
}
