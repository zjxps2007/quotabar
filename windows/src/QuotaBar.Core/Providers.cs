// Parsing and OAuth semantics ported from QuotaBar and sylearn/AIUsage; see NOTICE.
using System.Net;
using System.Net.Http.Headers;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace QuotaBar.Core;
public sealed class ProviderClient : IDisposable
{
    private readonly HttpClient http;
    public ProviderClient(HttpMessageHandler? handler = null)
    { http = new(handler ?? new HttpClientHandler { AllowAutoRedirect = false, UseCookies = false }) { Timeout = TimeSpan.FromSeconds(20) }; }
    private async Task<JsonElement> Request(string url, string? access, string? account, object? body, CancellationToken token)
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
        return Json.Read<JsonElement>(bytes.ToArray());
    }
    public async Task<Snapshot> FetchCodex(byte[] auth, CancellationToken token)
    {
        var credentials = Credentials.Parse(auth, Provider.Codex);
        return ParseCodex(await Request("https://chatgpt.com/backend-api/wham/usage", credentials.AccessToken, credentials.AccountId, null, token), credentials.Email);
    }
    public static Snapshot ParseCodex(JsonElement json, string? email = null)
    {
        List<QuotaWindow> windows = [];
        foreach (var (value, fallback) in new[] { (json.Get("rate_limit").Get("primary_window"), "5시간"),
            (json.Get("rate_limit").Get("secondary_window"), "주간"), (json.Get("code_review_rate_limit").Get("primary_window"), "Code review") })
        {
            if (value.Get("used_percent").Number() is not { } used) continue;
            double? duration = value.Get("limit_window_seconds").Number();
            string label = duration == 604800 ? "Weekly Window" : duration is > 0 && duration % 86400 == 0 ? $"{duration / 86400:0}d Window" : duration is > 0 && duration % 3600 == 0 ? $"{duration / 3600:0}h Window" : fallback;
            if (fallback == "Code review") label = "Code review";
            double? reset = value.Get("reset_at").Number();
            windows.Add(new(label, Math.Clamp(100 - used, 0, 100), reset is > 0 and < 253402300799 ? DateTimeOffset.FromUnixTimeSeconds((long)reset) : null));
        }
        return new(DateTimeOffset.UtcNow, windows, json.Get("plan_type").Text(), json.Get("email").Text() ?? email);
    }
    public async Task<byte[]> Refresh(byte[] current)
    {
        var old = Credentials.Parse(current, Provider.Codex);
        if (old.RefreshToken == null) throw new UsageException("reauth_required", "앱에서 다시 로그인하세요.");
        var result = await Request("https://auth.openai.com/oauth/token", null, null,
            new { grant_type = "refresh_token", client_id = "app_EMoamEEZ73f0CkXaXp7hrann", refresh_token = old.RefreshToken }, CancellationToken.None);
        string? access = result.Get("access_token").Text();
        if (string.IsNullOrWhiteSpace(access)) throw new UsageException("oauth_response", "갱신 응답에 인증 정보가 없습니다.");
        foreach (var token in new[] { access, result.Get("id_token").Text() }.Where(t => t != null))
        {
            var claims = Credentials.Claims(token); var auth = claims.Get("https://api.openai.com/auth");
            string? account = auth.Get("chatgpt_account_id").Text() ?? claims.Get("chatgpt_account_id").Text();
            string? user = auth.Get("chatgpt_user_id").Text() ?? claims.Get("chatgpt_user_id").Text() ?? auth.Get("user_id").Text() ?? claims.Get("user_id").Text() ?? claims.Get("sub").Text();
            if (account != null && !string.Equals(account, old.AccountId, StringComparison.OrdinalIgnoreCase) || user != null && old.UserId != null && !string.Equals(user, old.UserId, StringComparison.OrdinalIgnoreCase))
                throw new UsageException("account_mismatch", "갱신된 계정이 다릅니다. 다시 연결하세요.");
        }
        var root = JsonNode.Parse(current)!.AsObject(); var target = root["tokens"] as JsonObject ?? root;
        target["access_token"] = access;
        foreach (var key in new[] { "id_token", "refresh_token" }) if (result.Get(key).Text() is { Length: > 0 } value) target[key] = value;
        root.Remove("expires_at"); root.Remove("expired");
        if (result.Get("expires_in").Number() is { } seconds && seconds is > 0 and < 315360000) root["expires_at"] = DateTimeOffset.UtcNow.AddSeconds(seconds).ToUnixTimeSeconds();
        root["last_refresh"] = DateTimeOffset.UtcNow.ToString("O");
        byte[] bytes = Encoding.UTF8.GetBytes(root.ToJsonString());
        if (Credentials.Parse(bytes, Provider.Codex).Identity != old.Identity) throw new UsageException("account_mismatch", "계정 정체성이 변경되었습니다.");
        return bytes;
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
    public static Snapshot ParseGemini(JsonElement root)
    {
        var buckets = root.Get("buckets"); List<QuotaWindow> windows = [];
        if (buckets.ValueKind == JsonValueKind.Array)
        {
            var groups = buckets.EnumerateArray().Where(b => b.Get("modelId").Text() != null && b.Get("remainingFraction").Number() != null)
                .GroupBy(b => b.Get("modelId").Text()!.Contains("flash-lite") ? "Flash Lite" : b.Get("modelId").Text()!.Contains("flash") ? "Flash" : b.Get("modelId").Text()!.Contains("pro") ? "Pro" : "Other");
            foreach (var group in groups.Where(g => g.Key != "Other").OrderBy(g => g.Key == "Pro" ? 0 : g.Key == "Flash" ? 1 : 2))
            { var lowest = group.MinBy(b => b.Get("remainingFraction").Number()); windows.Add(new(group.Key, Math.Clamp(lowest.Get("remainingFraction").Number()!.Value * 100, 0, 100), DateTimeOffset.TryParse(lowest.Get("resetTime").Text(), out var d) ? d : null)); }
        }
        return new(DateTimeOffset.UtcNow, windows);
    }
    public void Dispose() => http.Dispose();
}
