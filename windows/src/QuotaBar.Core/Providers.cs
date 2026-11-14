// Parsing and OAuth semantics ported from QuotaBar and sylearn/AIUsage; see NOTICE.
using System.Net;
using System.Net.Http.Headers;
using System.Text.Json;

namespace QuotaBar.Core;
public sealed class ProviderClient : IDisposable
{
    private readonly HttpClient http;
    public ProviderClient(HttpMessageHandler? handler = null)
    { http = new(handler ?? new HttpClientHandler { AllowAutoRedirect = false, UseCookies = false }) { Timeout = TimeSpan.FromSeconds(20) }; }
    private async Task<byte[]> RequestBytes(string url, string? access, string? account, object? body, CancellationToken token)
    {
        using var request = new HttpRequestMessage(body == null ? HttpMethod.Get : HttpMethod.Post, url);
        request.Headers.CacheControl = new() { NoCache = true, NoStore = true }; request.Headers.Pragma.ParseAdd("no-cache");
        request.Headers.UserAgent.ParseAdd("QuotaBar/" + Brand.Version); request.Headers.Accept.ParseAdd("application/json");
        if (access != null) request.Headers.Authorization = new AuthenticationHeaderValue("Bearer", access);
        if (account != null) request.Headers.Add("ChatGPT-Account-Id", account);
        if (body != null) request.Content = new ByteArrayContent(Json.Bytes(body)) { Headers = { ContentType = new("application/json") } };
        using var response = await http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, token);
        if (!response.IsSuccessStatusCode)
        {
            string code = response.StatusCode is HttpStatusCode.Unauthorized or HttpStatusCode.Forbidden ? "unauthorized" : "http";
            if (url.Contains("oauth/token") && (int)response.StatusCode is 400 or 401 or 403) code = "reauth_required";
            throw new UsageException(code, code == "reauth_required" ? "로그인이 해제되었습니다. 다시 로그인하세요." : $"사용량 서버 응답: HTTP {(int)response.StatusCode}. 마지막 성공 값을 표시합니다.");
        }
        await using var stream = await response.Content.ReadAsStreamAsync(token); using var bytes = new MemoryStream();
        var buffer = new byte[8192]; int count;
        while ((count = await stream.ReadAsync(buffer, token)) > 0)
        { if (bytes.Length + count > 2 * 1024 * 1024) throw new UsageException("large_response", "응답이 너무 큽니다."); bytes.Write(buffer, 0, count); }
        return bytes.ToArray();
    }
    private async Task<JsonElement> Request(string url, string? access, string? account, object? body, CancellationToken token)
        => Json.Read<JsonElement>(await RequestBytes(url, access, account, body, token));
    public async Task<Snapshot> FetchCodex(byte[] auth, CancellationToken token)
    {
        var credentials = Credentials.Parse(auth, Provider.Codex);
        return ParseCodex(await Request("https://chatgpt.com/backend-api/wham/usage", credentials.AccessToken, credentials.AccountId, null, token), credentials.Email);
    }
    public static Snapshot ParseCodex(JsonElement json, string? email = null) => NativeDisplay.Quota("codex", json, email);
    public async Task<byte[]> Refresh(byte[] current)
    {
        var old = Credentials.Parse(current, Provider.Codex);
        if (old.RefreshToken == null) throw new UsageException("reauth_required", "앱에서 다시 로그인하세요.");
        var result = await RequestBytes("https://auth.openai.com/oauth/token", null, null,
            new { grant_type = "refresh_token", client_id = "app_EMoamEEZ73f0CkXaXp7hrann", refresh_token = old.RefreshToken }, CancellationToken.None);
        try { return Credentials.Rotate(current, result, DateTimeOffset.UtcNow); }
        finally { System.Security.Cryptography.CryptographicOperations.ZeroMemory(result); }
    }
    public async Task<Snapshot> FetchGemini(byte[] data, CancellationToken token)
    {
        var credentials = Credentials.Parse(data, Provider.Gemini); string? project = null; string? tier = null;
        try
        {
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token); timeout.CancelAfter(5000);
            var assist = await Request("https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist", credentials.AccessToken, null,
                new { metadata = new { ideType = "GEMINI_CLI", pluginType = "GEMINI" } }, timeout.Token);
            var p = assist.Get("cloudaicompanionProject"); project = p.Text() ?? p.Get("id").Text(); tier = assist.Get("currentTier").Get("id").Text();
        }
        catch (Exception e) when (e is UsageException or HttpRequestException or OperationCanceledException) { token.ThrowIfCancellationRequested(); }
        if (project == null)
        {
            try
            {
                using var timeout = CancellationTokenSource.CreateLinkedTokenSource(token); timeout.CancelAfter(5000);
                var list = await Request("https://cloudresourcemanager.googleapis.com/v1/projects", credentials.AccessToken, null, null, timeout.Token);
                if (list.Get("projects").ValueKind == JsonValueKind.Array) project = list.Get("projects").EnumerateArray()
                    .FirstOrDefault(p => p.Get("projectId").Text()?.StartsWith("gen-lang-client") == true || p.Get("labels").Get("generative-language").ValueKind != JsonValueKind.Undefined).Get("projectId").Text();
            }
            catch (Exception e) when (e is UsageException or HttpRequestException or OperationCanceledException) { token.ThrowIfCancellationRequested(); }
        }
        var result = await Request("https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota", credentials.AccessToken, null,
            project == null ? new Dictionary<string,string>() : new() { ["project"] = project }, token);
        return ParseGemini(result) with { Email = credentials.Email, Plan = tier ?? "Code Assist" };
    }
    public static Snapshot ParseGemini(JsonElement root) => NativeDisplay.Quota("gemini", root);
    public void Dispose() => http.Dispose();
}
