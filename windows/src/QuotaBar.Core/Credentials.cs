using System.Text;
using System.Text.Json;
using System.Security.Cryptography;

namespace QuotaBar.Core;
public sealed record Credentials(string Identity, string Label, string AccessToken, string? RefreshToken,
    string? AccountId, string? UserId, string? Email, DateTimeOffset? ExpiresAt, DateTimeOffset? LastRefresh)
{
    public bool NeedsRefresh(DateTimeOffset now) => ExpiresAt is { } expiry ? expiry <= now.AddMinutes(5) : LastRefresh == null || LastRefresh <= now.AddHours(-1);
    public static JsonElement Claims(string? token)
    {
        try { var value = token!.Split('.')[1].Replace('-', '+').Replace('_', '/'); return Json.Read<JsonElement>(Convert.FromBase64String(value.PadRight((value.Length + 3) / 4 * 4, '='))); }
        catch (Exception e) when (e is NullReferenceException or IndexOutOfRangeException or FormatException or JsonException) { return default; }
    }
    public static Credentials Parse(byte[] data, Provider provider)
    {
        if (data.Length > 1_048_576) throw new UsageException("invalid_auth", "로그인 파일이 너무 큽니다.");
        var root = Json.Read<JsonElement>(data); var tokens = root.Get("tokens"); if (tokens.ValueKind != JsonValueKind.Object) tokens = root;
        string? First(params string?[] values) => values.FirstOrDefault(s => !string.IsNullOrWhiteSpace(s))?.Trim();
        string? Norm(string? value) => value?.Normalize().ToLowerInvariant();
        string? access = First(tokens.Get("access_token").Text(), tokens.Get("accessToken").Text());
        if (access == null || root.Get("OPENAI_API_KEY").Text() is { Length: > 0 }) throw new UsageException("missing_token", "API 키 대신 구독 계정의 로그인 파일을 연결하세요.");
        string? refresh = First(tokens.Get("refresh_token").Text(), tokens.Get("refreshToken").Text());
        var claims = new[] { Claims(First(tokens.Get("id_token").Text(), tokens.Get("idToken").Text())), Claims(access) };
        var auth = claims.Select(c => c.Get("https://api.openai.com/auth")).ToArray();
        string? account = Norm(First(tokens.Get("account_id").Text(), tokens.Get("accountId").Text(), root.Get("account_id").Text(), root.Get("chatgpt_account_id").Text(),
            auth.Select(a => a.Get("chatgpt_account_id").Text()).FirstOrDefault(s => s != null), claims.Select(c => c.Get("chatgpt_account_id").Text()).FirstOrDefault(s => s != null)));
        string? user = Norm(First(root.Get("chatgpt_user_id").Text(), auth.Select(a => a.Get("chatgpt_user_id").Text()).FirstOrDefault(s => s != null),
            claims.Select(c => c.Get("chatgpt_user_id").Text()).FirstOrDefault(s => s != null), auth.Select(a => a.Get("user_id").Text()).FirstOrDefault(s => s != null),
            root.Get("user_id").Text(), claims.Select(c => c.Get("sub").Text()).FirstOrDefault(s => s != null)));
        string? email = Norm(First(root.Get("email").Text(), claims.Select(c => c.Get("email").Text()).FirstOrDefault(s => s != null),
            claims.Select(c => c.Get("https://api.openai.com/profile").Get("email").Text()).FirstOrDefault(s => s != null)));
        if (email?.Contains('@') != true) email = null;
        string? identity = provider == Provider.Codex ? account == null ? null : user != null ? $"codex:account:{account}:user:{user}" : email == null ? null : $"codex:account:{account}:email:{email}"
            : claims[0].Get("sub").Text() is { } sub ? "gemini:subject:" + sub : email != null ? "gemini:email:" + email : refresh != null ? "gemini:credential:" + Convert.ToHexStringLower(SHA256.HashData(Encoding.UTF8.GetBytes(refresh))) : null;
        if (identity == null) throw new UsageException("missing_identity", "로그인 파일에서 계정과 워크스페이스를 확인할 수 없습니다.");
        double? expiry = root.Get("expires_at").Number() ?? Claims(access).Get("exp").Number();
        DateTimeOffset? exp = expiry is > 0 and < 253402300799 ? DateTimeOffset.FromUnixTimeSeconds((long)expiry) : DateTimeOffset.TryParse(root.Get("expired").Text(), out var d) ? d : null;
        DateTimeOffset? last = DateTimeOffset.TryParse(root.Get("last_refresh").Text(), out var r) ? r : null;
        return new(identity, email ?? provider + " · " + (account ?? identity)[..Math.Min(8, (account ?? identity).Length)], access, refresh, account, user, email, exp, last);
    }
}
